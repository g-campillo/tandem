//! Captures review inputs and deliberately attached output. This module never
//! executes a model command or reads arbitrary terminal buffers.
use crate::{context::context_from_snapshot, credential_like, matching_check_evidence};
use anyhow::{Context, Result, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::atomic::AtomicBool,
};
use tandem_core::*;

pub struct ActionContext {
    pub context: ContextSnapshot,
    pub targets: BTreeMap<String, ActionFileState>,
    pub snapshot: SnapshotManifest,
}

pub fn capture_action_context(
    session: &TaskSession,
    project: &Project,
    overlays: &[Document],
    surrounding: &[String],
    cache: &Path,
    cancel: &AtomicBool,
) -> Result<ActionContext> {
    ensure!(
        session.project == project.key(),
        "Action belongs to another project"
    );
    let (walk, action) = session.current_action()?;
    let mut required = BTreeSet::new();
    match &action.target {
        ActionTarget::File { path, .. } => {
            required.insert(path.clone());
        }
        ActionTarget::Validation { paths, .. } => required.extend(paths.iter().cloned()),
        _ => {}
    }
    required.extend(action.required_evidence.iter().filter_map(|e| match e {
        RequiredActionEvidence::FileSnapshot { path } => Some(path.clone()),
        _ => None,
    }));
    let step = session
        .plan
        .as_ref()
        .and_then(|p| p.task.steps.iter().find(|s| s.id == walk.step_id))
        .context("Missing active step")?;
    required.extend(step.files.iter().cloned());
    required.extend(surrounding.iter().cloned());
    ensure!(
        required.len() <= 128,
        "Review needs at most 128 relevant paths"
    );
    for path in &required {
        normalized_relative(Path::new(path))?;
    }
    let snapshot = tandem_checks::materialize(project, overlays, &cache.join(new_id()), cancel)?;
    // Include surrounding source interfaces within the existing text budget.
    // Required paths that cannot fit remain explicit unavailable evidence.
    let mut paths = required.clone();
    let mut bytes = snapshot
        .manifest
        .files
        .iter()
        .filter(|f| required.contains(&f.path))
        .map(|f| f.bytes)
        .sum::<u64>();
    for file in &snapshot.manifest.files {
        if paths.len() >= 128 {
            break;
        }
        if !paths.contains(&file.path) && bytes + file.bytes <= crate::CONTEXT_LIMIT as u64 {
            paths.insert(file.path.clone());
            bytes += file.bytes;
        }
    }
    let context =
        context_from_snapshot(&snapshot, overlays, &paths.into_iter().collect::<Vec<_>>())?;
    let mut targets = BTreeMap::new();
    for path in required {
        let state = if let Some(file) = context.files.iter().find(|f| f.path == path) {
            ActionFileState::Present {
                hash: file.hash.clone(),
            }
        } else if snapshot.manifest.files.iter().any(|f| f.path == path) {
            ActionFileState::Unavailable {
                reason: "Target is excluded from bounded text context".into(),
            }
        } else if tandem_checks::excluded(&path).is_some()
            || !snapshot.eligible_path(&path)
            || snapshot.manifest.omitted.iter().any(|o| o.path == path)
        {
            ActionFileState::Unavailable {
                reason: "Target excluded, ignored, or unreadable".into(),
            }
        } else if safely_absent(project, &path) {
            ActionFileState::Absent
        } else {
            ActionFileState::Unavailable {
                reason: "Target cannot be read safely".into(),
            }
        };
        targets.insert(path, state);
    }
    Ok(ActionContext {
        context,
        targets,
        snapshot: snapshot.manifest,
    })
}
fn safely_absent(project: &Project, path: &str) -> bool {
    let mut current = project.root.clone();
    for part in Path::new(path).components() {
        current.push(part);
        match current.symlink_metadata() {
            Ok(metadata) if metadata.file_type().is_symlink() => return false,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return true,
            Err(_) => return false,
        }
    }
    false
}

/// Checks are selected from explicit local receipts, with the existing snapshot,
/// unchanged-input, recipe-fingerprint and terminal-timestamp checks preserved.
/// Failed and incomplete matching receipts remain visible to the fail-closed gate.
pub fn collect_action_check_evidence(
    scope: ActionEvidenceScope,
    context: &ContextSnapshot,
    checks: &[CheckRecord],
    recipes: &[(String, String)],
) -> Vec<ActionCommandEvidence> {
    checks
        .iter()
        // The legacy helper caps display summaries at six. Apply its secure
        // matching predicate to each receipt so failures cannot disappear.
        .filter(|c| {
            !matching_check_evidence(context, std::slice::from_ref(*c), recipes).is_empty()
                // Preserve same-input incomplete runs as nonpassing evidence.
                // Otherwise an older pass could hide a current running check.
                || (c.snapshot.as_ref().is_some_and(|s| s.id == context.project_fingerprint)
                    && recipes.iter().any(|(id, fingerprint)| c.recipe_id == *id && c.recipe_fingerprint == *fingerprint))
                // Preparation and infrastructure failures can precede snapshot
                // capture and argv resolution. Retain uncertainty for a relevant
                // recipe, without inventing successful input provenance.
                || (c.snapshot.is_none()
                    && recipes.iter().any(|(id, fingerprint)| c.recipe_id == *id && c.recipe_fingerprint == *fingerprint))
        })
        .filter_map(|c| {
            let mut receipt = ActionCommandEvidence::from_local_check(scope.clone(), c).ok()?;
            if credential_like(&c.output) {
                receipt.output = CommandOutput::Withheld {
                    hash: content_hash(c.output.as_bytes()),
                    reason: "Credential-like output excluded".into(),
                };
                receipt.truncated = true;
            }
            Some(receipt)
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct UserCommandAttachment {
    pub argv: Vec<String>,
    pub cwd: String,
    pub exit_code: i64,
    pub stdout: String,
    pub stderr: String,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
}
/// Only an explicit UI attachment command should call this function. A new
/// attachment always receives a new identity, including a corrected transcript.
pub fn attach_action_output(
    scope: ActionEvidenceScope,
    capture: &ActionContext,
    attachment: UserCommandAttachment,
) -> Result<ActionCommandEvidence> {
    ensure!(
        !credential_like(&attachment.stdout) && !credential_like(&attachment.stderr),
        "Attached output contains credential-like data"
    );
    ensure!(
        attachment.started_at_ms >= 0
            && attachment.completed_at_ms >= attachment.started_at_ms
            && attachment.completed_at_ms <= now_ms(),
        "Attached command timestamps are invalid"
    );
    let receipt = ActionCommandEvidence {
        id: new_id(),
        scope,
        argv: attachment.argv,
        cwd: attachment.cwd,
        exit_code: Some(attachment.exit_code),
        output: CommandOutput::Split {
            stdout: attachment.stdout,
            stderr: attachment.stderr,
        },
        started_at_ms: attachment.started_at_ms,
        completed_at_ms: Some(attachment.completed_at_ms),
        snapshot: Some(capture.snapshot.clone()),
        snapshot_unchanged: true,
        // This is a user's report, not an authenticated local check verdict.
        status: CheckStatus::Unvalidated,
        truncated: false,
        recipe_id: None,
        recipe_fingerprint: None,
        provenance: CommandProvenance::UserAttachedOutput,
    };
    receipt.validate()?;
    Ok(receipt)
}
