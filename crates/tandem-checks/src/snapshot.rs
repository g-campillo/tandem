use crate::ignore_policy::CapturedIgnorePolicy;
use anyhow::{Context, Result, ensure};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tandem_core::*;

pub const MAX_SNAPSHOT_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_INPUT_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_INPUT_FILES: usize = 20_000;

pub struct MaterializedSnapshot {
    pub manifest: SnapshotManifest,
    pub input: PathBuf,
    pub artifacts: PathBuf,
    ignore_policy: CapturedIgnorePolicy,
}
impl MaterializedSnapshot {
    /// Uses only the immutable policy loaded during materialization. Unknown
    /// directories cannot expand through history, even if they now exist.
    pub fn eligible_path(&self, path: &str) -> bool {
        self.ignore_policy.allows_captured_path(path)
    }
}

pub fn excluded(path: &str) -> Option<&'static str> {
    for part in Path::new(path).components() {
        let name = part.as_os_str().to_string_lossy().to_lowercase();
        if matches!(
            name.as_str(),
            ".git"
                | ".codex"
                | ".agents"
                | "target"
                | "node_modules"
                | ".tandem-local"
                | ".venv"
                | "__pycache__"
        ) {
            return Some("generated, executable-agent configuration, or private metadata");
        }
        if name == "agents.md" || name == "claude.md" {
            return Some("agent instructions are not validation inputs");
        }
        if name == ".env"
            || name.starts_with(".env.")
            || name == "auth.json"
            || name == "credentials"
            || name == "credentials.json"
            || name == "id_rsa"
            || name == "id_ed25519"
            || [".pem", ".key", ".p12", ".pfx"]
                .iter()
                .any(|suffix| name.ends_with(suffix))
        {
            return Some("credential-like input excluded");
        }
    }
    None
}

/// Open each component relative to an already-open directory descriptor. A
/// concurrent parent-directory symlink swap cannot redirect a read elsewhere.
pub fn secure_read(project: &Project, path: &str) -> Result<(Vec<u8>, bool)> {
    normalized_relative(Path::new(path))?;
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::OpenOptionsExt;
        let mut directory = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(&project.root)?;
        let parts: Vec<_> = Path::new(path).components().collect();
        for (index, part) in parts.iter().enumerate() {
            let name = CString::new(part.as_os_str().as_encoded_bytes())?;
            let flags = libc::O_RDONLY
                | libc::O_NOFOLLOW
                | libc::O_CLOEXEC
                | libc::O_NONBLOCK
                | if index + 1 < parts.len() {
                    libc::O_DIRECTORY
                } else {
                    0
                };
            let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
            ensure!(
                fd >= 0,
                "Input could not be opened without following symlinks: {path}"
            );
            let file = unsafe { File::from_raw_fd(fd) };
            if index + 1 < parts.len() {
                directory = file;
                continue;
            }
            let metadata = file.metadata()?;
            ensure!(
                metadata.is_file() && metadata.nlink() == 1,
                "Special or hard-linked input excluded: {path}"
            );
            ensure!(
                metadata.len() <= MAX_INPUT_BYTES,
                "Input exceeds the 8 MiB validation limit: {path}"
            );
            let mut bytes = Vec::new();
            file.take(MAX_INPUT_BYTES + 1).read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 <= MAX_INPUT_BYTES,
                "Input grew beyond its limit: {path}"
            );
            return Ok((bytes, metadata.mode() & 0o111 != 0));
        }
        anyhow::bail!("Empty input path")
    }
    #[cfg(not(unix))]
    {
        let _ = (project, path);
        anyhow::bail!("Secure validation snapshots are not supported on this platform")
    }
}

pub fn ignored_new_buffer(project: &Project, path: &str) -> Result<bool> {
    let parent = Path::new(path).parent().context("Input has no parent")?;
    let mut ancestors = parent.ancestors().collect::<Vec<_>>();
    ancestors.reverse();
    for directory in ancestors {
        let mut builder = ignore::gitignore::GitignoreBuilder::new(project.root.join(directory));
        for name in [".gitignore", ".ignore"] {
            let policy = directory.join(name);
            if project.root.join(&policy).exists() {
                let (bytes, _) = secure_read(
                    project,
                    policy.to_str().context("Invalid ignore-file path")?,
                )?;
                for line in std::str::from_utf8(&bytes)?.lines() {
                    builder.add_line(None, line)?;
                }
            }
        }
        if builder
            .build()?
            .matched_path_or_any_parents(project.root.join(path), false)
            .is_ignore()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn materialize(
    project: &Project,
    overlays: &[Document],
    destination: &Path,
    cancelled: &AtomicBool,
) -> Result<MaterializedSnapshot> {
    ensure!(
        !destination.starts_with(&project.root),
        "Validation artifacts must be outside the original project"
    );
    fs::create_dir_all(destination)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(destination, fs::Permissions::from_mode(0o700))?;
    }
    let input = destination.join("input");
    let artifacts = destination.join("artifacts");
    fs::create_dir(&input)?;
    fs::create_dir(&artifacts)?;
    let mut paths = std::collections::BTreeSet::new();
    let mut omitted = Vec::new();
    let mut scanned = 0;
    let policy = Arc::new(Mutex::new(CapturedIgnorePolicy::capture(project)?));
    let policy_filter = policy.clone();
    let policy_error = Arc::new(Mutex::new(None));
    let filter_error = policy_error.clone();
    let project_root = project.root.clone();
    let walker = ignore::WalkBuilder::new(&project.root)
        .standard_filters(false)
        .follow_links(false)
        .require_git(false)
        .max_depth(Some(64))
        .filter_entry(move |entry| {
            if matches!(
                entry.file_name().to_str(),
                Some(
                    ".git"
                        | ".codex"
                        | ".agents"
                        | "target"
                        | "node_modules"
                        | ".tandem-local"
                        | ".venv"
                        | "__pycache__"
                )
            ) {
                return false;
            }
            let Ok(relative) = entry.path().strip_prefix(&project_root) else {
                return false;
            };
            match policy_filter
                .lock()
                .unwrap()
                .allows_entry(relative, entry.file_type().is_some_and(|t| t.is_dir()))
            {
                Ok(allowed) => allowed,
                Err(error) => {
                    *filter_error.lock().unwrap() = Some(error);
                    false
                }
            }
        })
        .build();
    for entry in walker {
        ensure!(
            !cancelled.load(Ordering::Relaxed),
            "Snapshot capture cancelled"
        );
        scanned += 1;
        ensure!(
            scanned <= MAX_INPUT_FILES * 4,
            "Project exceeds the validation traversal limit"
        );
        let entry = entry.context("Cannot enumerate validation inputs")?;
        let Some(relative) = entry
            .path()
            .strip_prefix(&project.root)
            .ok()
            .and_then(Path::to_str)
        else {
            continue;
        };
        if relative.is_empty() {
            continue;
        }
        if let Some(reason) = excluded(relative) {
            omitted.push(OmittedInput {
                path: relative.into(),
                reason: reason.into(),
            });
            continue;
        }
        if entry.file_type().is_some_and(|kind| kind.is_symlink()) {
            omitted.push(OmittedInput {
                path: relative.into(),
                reason: "symlink not followed".into(),
            });
            continue;
        }
        if entry.file_type().is_some_and(|kind| kind.is_file()) {
            paths.insert(relative.to_string());
        }
    }
    if let Some(error) = policy_error.lock().unwrap().take() {
        return Err(error);
    }
    let mut ignore_policy = Arc::try_unwrap(policy).ok().unwrap().into_inner().unwrap();
    let overlays: BTreeMap<_, _> = overlays
        .iter()
        .map(|doc| (doc.path.as_str(), doc))
        .collect();
    for path in overlays.keys() {
        if let Some(reason) = excluded(path) {
            omitted.push(OmittedInput {
                path: (*path).into(),
                reason: reason.into(),
            });
        } else {
            normalized_relative(Path::new(path))?;
            if !paths.contains(*path)
                && (overlays[path].base_hash.is_some() || !ignore_policy.allows_buffer(path)?)
            {
                omitted.push(OmittedInput {
                    path: (*path).into(),
                    reason: "ignored open buffer excluded from validation".into(),
                });
                continue;
            }
            paths.insert((*path).into());
        }
    }
    ensure!(paths.len() <= MAX_INPUT_FILES, "Too many validation inputs");
    let mut files = Vec::new();
    let mut total_bytes = 0;
    for path in paths {
        ensure!(
            !cancelled.load(Ordering::Relaxed),
            "Snapshot capture cancelled"
        );
        let original = secure_read(project, &path);
        let overlay = overlays.get(path.as_str());
        let (bytes, executable, original_hash, revision) = match (original, overlay) {
            (Ok((bytes, executable)), Some(doc)) => (
                doc.format.encode(&doc.text),
                executable,
                Some(content_hash(&bytes)),
                Some(doc.revision),
            ),
            (Ok((bytes, executable)), None) => {
                let hash = content_hash(&bytes);
                (bytes, executable, Some(hash), None)
            }
            (Err(_), Some(doc))
                if !project.root.join(&path).exists() && doc.base_hash.is_none() =>
            {
                (
                    doc.format.encode(&doc.text),
                    false,
                    None,
                    Some(doc.revision),
                )
            }
            (Err(error), _) => {
                omitted.push(OmittedInput {
                    path,
                    reason: error.to_string(),
                });
                continue;
            }
        };
        total_bytes += bytes.len() as u64;
        ensure!(
            bytes.len() as u64 <= MAX_INPUT_BYTES && total_bytes <= MAX_SNAPSHOT_BYTES,
            "Validation snapshot exceeds its byte budget"
        );
        let target = input.join(&path);
        fs::create_dir_all(target.parent().context("Input path has no parent")?)?;
        let mut output = File::create(&target)?;
        output.write_all(&bytes)?;
        output.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                &target,
                fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
            )?;
        }
        files.push(SnapshotFile {
            path,
            hash: content_hash(&bytes),
            bytes: bytes.len() as u64,
            executable,
            buffer_revision: revision,
            original_hash,
        });
    }
    ignore_policy.verify_sources()?;
    let identity = files
        .iter()
        .map(|file| format!("{}\0{}\0{}", file.path, file.hash, file.executable))
        .collect::<Vec<_>>()
        .join("\n");
    let manifest = SnapshotManifest {
        id: content_hash(format!("{}:{}", identity, ignore_policy.identity()).as_bytes()),
        project: project.key().into(),
        files,
        omitted,
        total_bytes,
        captured_at_ms: now_ms(),
    };
    fs::write(
        destination.join("snapshot.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(MaterializedSnapshot {
        manifest,
        input,
        artifacts,
        ignore_policy,
    })
}

pub fn snapshot_unchanged(snapshot: &MaterializedSnapshot) -> bool {
    let Ok(project) = Project::open(&snapshot.input) else {
        return false;
    };
    snapshot.manifest.files.iter().all(|file| {
        secure_read(&project, &file.path).is_ok_and(|(bytes, executable)| {
            content_hash(&bytes) == file.hash && executable == file.executable
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn materialization_and_expansion_share_the_full_hierarchical_ignore_policy() {
        let fixture = tempfile::tempdir().unwrap();
        let directory = fixture.path().join("project");
        fs::create_dir_all(directory.join("src")).unwrap();
        fs::create_dir_all(directory.join("blocked")).unwrap();
        fs::create_dir_all(directory.join(".git/info")).unwrap();
        fs::write(
            fixture.path().join(".gitignore"),
            "project/parent-private.txt\n",
        )
        .unwrap();
        fs::write(fixture.path().join(".ignore"), "!project/parent-kept.rs\n").unwrap();
        for (name, text) in [
            (".gitignore", "*.rs\nblocked/\n"),
            (".ignore", "!public.rs\n"),
            (".git/info/exclude", "exclude-private.txt\n"),
            ("src/.gitignore", "!nested.rs\n"),
            ("blocked/.ignore", "!child.txt\n"),
            ("blocked/child.txt", "blocked"),
            ("ordinary.rs", "ignored"),
            ("public.rs", "public"),
            ("parent-private.txt", "ignored by parent"),
            ("parent-kept.rs", "kept by parent ignore precedence"),
            ("exclude-private.txt", "ignored by Git exclude"),
            ("src/nested.rs", "kept by nested policy"),
            ("visible.txt", "visible"),
        ] {
            fs::write(directory.join(name), text).unwrap();
        }
        let project = Project::open(&directory).unwrap();
        let expected = ignore::WalkBuilder::new(&project.root)
            .hidden(false)
            .follow_links(false)
            .require_git(false)
            .max_depth(Some(64))
            .build()
            .map(Result::unwrap)
            .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
            .filter_map(|entry| {
                let path = entry
                    .path()
                    .strip_prefix(&project.root)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string();
                excluded(&path).is_none().then_some(path)
            })
            .collect::<std::collections::BTreeSet<_>>();
        let snapshot = materialize(
            &project,
            &[],
            &fixture.path().join("snapshot"),
            &AtomicBool::new(false),
        )
        .unwrap();
        let actual = snapshot
            .manifest
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(actual, expected);
        for path in [
            "public.rs",
            "parent-kept.rs",
            "src/nested.rs",
            "visible.txt",
            "deleted.txt",
        ] {
            assert!(snapshot.eligible_path(path), "{path}");
        }
        for path in [
            "ordinary.rs",
            "parent-private.txt",
            "exclude-private.txt",
            "blocked/child.txt",
            "unknown/child.txt",
        ] {
            assert!(!snapshot.eligible_path(path), "{path}");
        }
        // Policies, directories and source leaves can change after capture;
        // historical eligibility still uses only the sealed compiled policy.
        fs::write(directory.join(".ignore"), "*\n").unwrap();
        fs::write(directory.join(".gitignore"), "").unwrap();
        fs::write(directory.join(".git/info/exclude"), "").unwrap();
        fs::write(fixture.path().join(".gitignore"), "").unwrap();
        fs::remove_dir_all(directory.join("src")).unwrap();
        assert!(snapshot.eligible_path("src/nested.rs"));
        assert!(snapshot.eligible_path("deleted.txt"));
        assert!(!snapshot.eligible_path("ordinary.rs"));
        assert!(!snapshot.eligible_path("parent-private.txt"));
        assert!(!snapshot.eligible_path("exclude-private.txt"));
    }

    #[test]
    fn incomplete_or_unsafe_ignore_policies_refuse_snapshot_capture() {
        let fixture = tempfile::tempdir().unwrap();
        for kind in ["symlink", "hardlink", "exclude-parent-symlink", "malformed"] {
            let directory = fixture.path().join(kind);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("visible.txt"), "visible").unwrap();
            let policy = fixture.path().join(format!("{kind}-policy"));
            fs::write(&policy, "private.txt\n").unwrap();
            match kind {
                "symlink" => {
                    std::os::unix::fs::symlink(&policy, directory.join(".ignore")).unwrap()
                }
                "hardlink" => fs::hard_link(&policy, directory.join(".ignore")).unwrap(),
                "exclude-parent-symlink" => {
                    let info = fixture.path().join("external-info");
                    fs::create_dir(&info).unwrap();
                    fs::write(info.join("exclude"), "private.txt\n").unwrap();
                    fs::create_dir(directory.join(".git")).unwrap();
                    std::os::unix::fs::symlink(&info, directory.join(".git/info")).unwrap();
                }
                "malformed" => fs::write(directory.join(".ignore"), "[z-a]\n").unwrap(),
                _ => unreachable!(),
            }
            assert!(
                materialize(
                    &Project::open(&directory).unwrap(),
                    &[],
                    &fixture.path().join(format!("{kind}-snapshot")),
                    &AtomicBool::new(false),
                )
                .is_err(),
                "{kind}"
            );
        }
    }

    #[test]
    fn ignored_buffers_stay_excluded_and_new_source_buffers_are_captured() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("project");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join(".gitignore"), "ignored*\n").unwrap();
        fs::write(directory.join("ignored.rs"), "private").unwrap();
        let project = Project::open(&directory).unwrap();
        let mut ignored = project.open_document("ignored.rs", false).unwrap();
        ignored.snapshot("unsaved private".into());
        let mut ignored_new = project.open_document("ignored-new.rs", true).unwrap();
        ignored_new.snapshot("new private".into());
        let mut new = project.open_document("new.rs", true).unwrap();
        new.snapshot("new source".into());
        let snapshot = materialize(
            &project,
            &[ignored, ignored_new, new],
            &root.path().join("run"),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(!snapshot.input.join("ignored.rs").exists());
        assert!(!snapshot.input.join("ignored-new.rs").exists());
        assert_eq!(
            fs::read_to_string(snapshot.input.join("new.rs")).unwrap(),
            "new source"
        );
    }
    #[test]
    fn unsaved_overlay_is_captured_without_secrets_symlinks_or_original_writes() {
        let root = tempfile::tempdir().unwrap();
        let project_path = root.path().join("project");
        fs::create_dir(&project_path).unwrap();
        fs::write(project_path.join("code.rs"), "disk").unwrap();
        fs::write(project_path.join(".env"), "secret").unwrap();
        fs::write(project_path.join("AGENTS.md"), "override").unwrap();
        let outside = root.path().join("outside");
        fs::write(&outside, "outside").unwrap();
        std::os::unix::fs::symlink(&outside, project_path.join("link.rs")).unwrap();
        let project = Project::open(&project_path).unwrap();
        let mut doc = project.open_document("code.rs", false).unwrap();
        doc.snapshot("unsaved 🦀".into());
        let snapshot = materialize(
            &project,
            &[doc],
            &root.path().join("run"),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(snapshot.input.join("code.rs")).unwrap(),
            "unsaved 🦀"
        );
        assert_eq!(
            fs::read_to_string(project_path.join("code.rs")).unwrap(),
            "disk"
        );
        assert!(!snapshot.input.join(".env").exists());
        assert!(!snapshot.input.join("AGENTS.md").exists());
        assert!(!snapshot.input.join("link.rs").exists());
        assert!(snapshot_unchanged(&snapshot));
        fs::write(snapshot.input.join("code.rs"), "mutated").unwrap();
        assert!(!snapshot_unchanged(&snapshot));
    }
    #[test]
    fn parent_symlinks_and_hardlinks_cannot_supply_inputs() {
        let root = tempfile::tempdir().unwrap();
        let project_path = root.path().join("project");
        fs::create_dir(&project_path).unwrap();
        let other = root.path().join("other");
        fs::create_dir(&other).unwrap();
        fs::write(other.join("key"), "private").unwrap();
        std::os::unix::fs::symlink(&other, project_path.join("parent")).unwrap();
        fs::hard_link(other.join("key"), project_path.join("hard")).unwrap();
        let project = Project::open(&project_path).unwrap();
        assert!(secure_read(&project, "parent/key").is_err());
        assert!(secure_read(&project, "hard").is_err());
    }
}
