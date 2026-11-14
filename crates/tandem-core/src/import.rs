use crate::*;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use similar::{DiffTag, TextDiff};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Edit {
    start: usize,
    end: usize,
    text: String,
}
fn edits(base: &str, branch: &str) -> Vec<Edit> {
    let diff = TextDiff::from_lines(base, branch);
    let old = diff.old_slices();
    let new = diff.new_slices();
    let mut offsets = vec![0];
    for line in old {
        offsets.push(offsets.last().unwrap() + line.len());
    }
    diff.ops()
        .iter()
        .filter(|op| op.tag() != DiffTag::Equal)
        .map(|op| {
            let a = op.old_range();
            let b = op.new_range();
            Edit {
                start: offsets[a.start],
                end: offsets[a.end],
                text: new[b].concat(),
            }
        })
        .collect()
}
fn overlap(a: &Edit, b: &Edit) -> bool {
    if a.start == a.end && b.start == b.end {
        return a.start == b.start;
    }
    if a.start == a.end {
        return b.start < a.start && a.start < b.end;
    }
    if b.start == b.end {
        return a.start < b.start && b.start < a.end;
    }
    a.start < b.end && b.start < a.end
}
pub fn merge_text(base: &str, agent: &str, current: &str) -> Result<String> {
    ensure!(
        base.len() <= MAX_FILE_BYTES as usize
            && agent.len() <= MAX_FILE_BYTES as usize
            && current.len() <= MAX_FILE_BYTES as usize,
        "Merge exceeds the supported file size"
    );
    if agent == base || agent == current {
        return Ok(current.into());
    }
    if current == base {
        return Ok(agent.into());
    }
    let mut changes = edits(base, current);
    for edit in edits(base, agent) {
        if changes.contains(&edit) {
            continue;
        }
        ensure!(
            !changes.iter().any(|other| overlap(&edit, other)),
            "Overlapping edits require manual resolution"
        );
        changes.push(edit);
    }
    changes.sort_by_key(|e| (e.start, e.end));
    let mut result = base.to_string();
    for edit in changes.into_iter().rev() {
        result.replace_range(edit.start..edit.end, &edit.text);
    }
    Ok(result)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ImportStatus {
    Applied,
    Undone,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportedFile {
    pub path: String,
    pub before: Option<Document>,
    pub after: Option<Document>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportTransaction {
    pub id: String,
    pub project: String,
    pub agent_run_id: String,
    pub files: Vec<ImportedFile>,
    pub status: ImportStatus,
    pub created_at_ms: i64,
    pub undone_at_ms: Option<i64>,
    pub sequence: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportConflict {
    pub path: String,
    pub reason: String,
    pub baseline: Option<String>,
    pub proposed: Option<String>,
    pub current: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportPreview {
    pub transaction: Option<ImportTransaction>,
    pub conflicts: Vec<ImportConflict>,
}

fn current_document(project: &Project, path: &str, live: &[Document]) -> Result<Option<Document>> {
    validate_new_name(path)?;
    if let Some(doc) = live.iter().find(|d| d.path == path) {
        if doc.base_hash.is_none() && project.root.join(path).symlink_metadata().is_err() {
            return Ok(Some(doc.clone()));
        }
        match project.disk_change(doc) {
            DiskChange::Unchanged => return Ok(Some(doc.clone())),
            DiskChange::MetadataChanged { read_only } => {
                let mut current = doc.clone();
                current.read_only = read_only;
                return Ok(Some(current));
            }
            DiskChange::Changed(disk) if !doc.dirty() => return Ok(Some(disk)),
            DiskChange::Missing if doc.base_hash.is_none() => return Ok(Some(doc.clone())),
            _ => anyhow::bail!(
                "Disk and the unsaved buffer diverged; compare with disk before importing"
            ),
        }
    }
    if project.root.join(path).symlink_metadata().is_ok() {
        Ok(Some(project.open_document(path, false)?))
    } else {
        validate_import_path(project, path)?;
        Ok(None)
    }
}
pub fn validate_import_path(project: &Project, path: &str) -> Result<()> {
    normalized_relative(std::path::Path::new(path))?;
    let mut current = project.root.clone();
    for part in std::path::Path::new(path).components() {
        current.push(part);
        if let Ok(meta) = current.symlink_metadata() {
            ensure!(
                !meta.file_type().is_symlink(),
                "Import paths cannot contain symlinks"
            );
            #[cfg(unix)]
            if meta.is_file() {
                use std::os::unix::fs::MetadataExt;
                ensure!(
                    meta.nlink() == 1,
                    "Import paths cannot contain hard-linked files"
                );
            }
        }
    }
    Ok(())
}
pub fn preview_import(
    run: &AgentRun,
    project: &Project,
    live: &[Document],
) -> Result<ImportPreview> {
    ensure!(
        run.project == project.key() && !run.status.active() && run.stage_verified,
        "Inspect a terminal, verified delegation before importing"
    );
    let changes = run.changed_paths();
    ensure!(!changes.is_empty(), "There are no staged changes to import");
    let mut files = vec![];
    let mut conflicts = vec![];
    for path in changes {
        let baseline = run
            .baseline
            .get(&path)
            .context("Missing delegation baseline")?
            .as_ref();
        let staged = run
            .staged
            .get(&path)
            .and_then(Option::as_ref)
            .context("File deletion is outside this grant")?;
        let result = (|| -> Result<ImportedFile> {
            validate_import_path(project, &path)?;
            let before = current_document(project, &path, live)?;
            let mut after = match (baseline, &before) {
                (Some(base), Some(current)) => {
                    let mut after = current.clone();
                    after.snapshot(merge_text(&base.text, &staged.text, &current.text)?);
                    after
                }
                (None, None) => Document {
                    path: path.clone(),
                    text: staged.text.clone(),
                    base_text: String::new(),
                    base_hash: None,
                    format: staged.format.clone(),
                    revision: 1,
                    recovered: false,
                    conflict: false,
                    read_only: false,
                },
                (None, Some(_)) => {
                    anyhow::bail!("A file or retained draft appeared after delegation")
                }
                (Some(_), None) => {
                    anyhow::bail!("The original working file was removed after delegation")
                }
            };
            after.recovered = false;
            Ok(ImportedFile {
                path: path.clone(),
                before,
                after: Some(after),
            })
        })();
        match result {
            Ok(file) => files.push(file),
            Err(error) => conflicts.push(ImportConflict {
                path: path.clone(),
                reason: error.to_string(),
                baseline: baseline.map(|d| d.text.clone()),
                proposed: Some(staged.text.clone()),
                current: live.iter().find(|d| d.path == path).map(|d| d.text.clone()),
            }),
        }
    }
    let transaction = conflicts.is_empty().then(|| ImportTransaction {
        id: new_id(),
        project: project.key().into(),
        agent_run_id: run.id.clone(),
        files,
        status: ImportStatus::Applied,
        created_at_ms: now_ms(),
        undone_at_ms: None,
        sequence: 0,
    });
    Ok(ImportPreview {
        transaction,
        conflicts,
    })
}
pub fn preview_undo(
    transaction: &ImportTransaction,
    project: &Project,
    live: &[Document],
) -> Result<ImportPreview> {
    ensure!(
        transaction.project == project.key() && transaction.status == ImportStatus::Applied,
        "This import is not available to undo"
    );
    let mut files = vec![];
    let mut conflicts = vec![];
    for imported in &transaction.files {
        let result = (|| -> Result<ImportedFile> {
            validate_import_path(project, &imported.path)?;
            let current = current_document(project, &imported.path, live)?
                .context("Imported file is no longer available")?;
            let after = imported.after.as_ref().context("Invalid import journal")?;
            let target = if let Some(before) = &imported.before {
                let mut restored = current.clone();
                restored.snapshot(merge_text(&after.text, &before.text, &current.text)?);
                Some(restored)
            } else {
                ensure!(
                    current.text == after.text,
                    "The created file has later edits; undo would discard them"
                );
                ensure!(
                    current.base_hash.is_none() && !project.root.join(&imported.path).exists(),
                    "The created file was saved after import; keep or remove it explicitly"
                );
                None
            };
            Ok(ImportedFile {
                path: imported.path.clone(),
                before: Some(current),
                after: target,
            })
        })();
        match result {
            Ok(file) => files.push(file),
            Err(error) => conflicts.push(ImportConflict {
                path: imported.path.clone(),
                reason: error.to_string(),
                baseline: imported.after.as_ref().map(|d| d.text.clone()),
                proposed: imported.before.as_ref().map(|d| d.text.clone()),
                current: live
                    .iter()
                    .find(|d| d.path == imported.path)
                    .map(|d| d.text.clone()),
            }),
        }
    }
    let result = conflicts.is_empty().then(|| ImportTransaction {
        id: transaction.id.clone(),
        project: transaction.project.clone(),
        agent_run_id: transaction.agent_run_id.clone(),
        files,
        status: ImportStatus::Undone,
        created_at_ms: transaction.created_at_ms,
        undone_at_ms: Some(now_ms()),
        sequence: transaction.sequence + 1,
    });
    Ok(ImportPreview {
        transaction: result,
        conflicts,
    })
}
pub fn import_diff(files: &[ImportedFile]) -> String {
    files
        .iter()
        .map(|f| {
            TextDiff::from_lines(
                f.before.as_ref().map_or("", |d| d.text.as_str()),
                f.after.as_ref().map_or("", |d| d.text.as_str()),
            )
            .unified_diff()
            .context_radius(3)
            .header(&format!("before/{}", f.path), &format!("after/{}", f.path))
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
pub fn apply_to_working_set(live: &[Document], transaction: &ImportTransaction) -> Vec<Document> {
    let mut result = live
        .iter()
        .cloned()
        .map(|d| (d.path.clone(), d))
        .collect::<BTreeMap<_, _>>();
    for file in &transaction.files {
        if let Some(after) = &file.after {
            result.insert(file.path.clone(), after.clone());
        } else {
            result.remove(&file.path);
        }
    }
    result.into_values().collect()
}
