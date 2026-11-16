//! Takeover undo. Before Claude edits the project, Tandem copies every
//! non-ignored file into a snapshot folder; afterwards a diff against the
//! project lists what changed, and each file can be kept or undone on its own.
//! Comparing the project itself, not Claude's tool calls, also covers files
//! changed by shell commands, and the snapshot survives a restart.

use crate::chat::ChangeKind;
use crate::{Project, content_hash, normalized_relative, validate_import_path};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, Metadata};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

/// Files up to this size are stored and can be restored.
pub const SNAPSHOT_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Larger files are not tracked at all.
pub const SNAPSHOT_MAX_TRACKED_BYTES: u64 = 64 * 1024 * 1024;
/// Total stored bytes per snapshot.
pub const SNAPSHOT_MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

const MANIFEST: &str = "manifest.json";
const BLOBS: &str = "blobs";

/// One file that differs from the snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotChange {
    pub path: String,
    pub kind: ChangeKind,
    /// The hash of the file now; `None` when it was deleted.
    pub after_hash: Option<String>,
    /// Whether `undo` can bring back the snapshot's version.
    pub undoable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    hash: String,
    /// The bytes are in `blobs/<hash>`.
    stored: bool,
    /// Unix permission bits.
    mode: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Manifest {
    /// The project key the snapshot was taken of.
    root: String,
    entries: BTreeMap<String, Entry>,
}

/// The baseline of one takeover, kept in its own folder.
#[derive(Debug)]
pub struct Snapshot {
    dir: PathBuf,
    manifest: Manifest,
    /// Bytes in `blobs/`, counted against `SNAPSHOT_MAX_TOTAL_BYTES`.
    stored_bytes: u64,
}

impl Snapshot {
    /// Records the project's non-ignored files in `dir`, which only this snapshot uses.
    pub fn capture(project: &Project, dir: &Path) -> Result<Snapshot> {
        let index = project.index();
        ensure!(
            !index.truncated,
            "This project has more than 20,000 files that are not ignored; a takeover cannot be undone safely here."
        );
        let created = !dir.exists();
        let result = Self::capture_files(project, dir, &index.files);
        if result.is_err() && created {
            let _ = fs::remove_dir_all(dir);
        }
        result
    }

    fn capture_files(project: &Project, dir: &Path, files: &[String]) -> Result<Snapshot> {
        create_private_dir(dir)?;
        create_private_dir(&dir.join(BLOBS))?;
        let mut snapshot = Snapshot {
            dir: dir.to_path_buf(),
            manifest: Manifest {
                root: project.key().to_string(),
                entries: BTreeMap::new(),
            },
            stored_bytes: 0,
        };
        for path in files {
            // A file that cannot be read has no baseline to compare with.
            let Ok(Some(file)) = CurrentFile::open(project, path) else {
                continue;
            };
            if file.len > SNAPSHOT_MAX_TRACKED_BYTES {
                continue;
            }
            let Ok(read) = file.read() else {
                continue;
            };
            let entry = snapshot.record(read)?;
            snapshot.manifest.entries.insert(path.clone(), entry);
        }
        snapshot.write_manifest()?;
        Ok(snapshot)
    }

    /// Reopens a snapshot that `capture` wrote, for example after a restart.
    pub fn open(dir: &Path) -> Result<Snapshot> {
        let bytes = fs::read(dir.join(MANIFEST))
            .with_context(|| format!("Cannot read the takeover snapshot in {}", dir.display()))?;
        let manifest: Manifest = serde_json::from_slice(&bytes)
            .with_context(|| format!("The takeover snapshot in {} is damaged", dir.display()))?;
        let mut stored_bytes = 0;
        if let Ok(blobs) = fs::read_dir(dir.join(BLOBS)) {
            for blob in blobs.flatten() {
                stored_bytes += blob.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
        Ok(Snapshot {
            dir: dir.to_path_buf(),
            manifest,
            stored_bytes,
        })
    }

    /// Every file that differs from the snapshot, sorted by path.
    pub fn diff(&self, project: &Project) -> Result<Vec<SnapshotChange>> {
        self.ensure_project(project)?;
        let mut changes = Vec::new();
        // Checked on disk, not in the index, so a file that merely became
        // ignored is not reported as deleted.
        for (path, entry) in &self.manifest.entries {
            let current = match CurrentFile::open(project, path) {
                Ok(Some(file)) => match file.hash() {
                    Ok(hash) => Some(hash),
                    Err(_) => continue,
                },
                Ok(None) => None,
                // A file that cannot be read now cannot be compared.
                Err(_) => continue,
            };
            match current {
                None => changes.push(SnapshotChange {
                    path: path.clone(),
                    kind: ChangeKind::Deleted,
                    after_hash: None,
                    undoable: entry.stored,
                }),
                Some(hash) if hash != entry.hash => changes.push(SnapshotChange {
                    path: path.clone(),
                    kind: ChangeKind::Modified,
                    after_hash: Some(hash),
                    undoable: entry.stored,
                }),
                Some(_) => {}
            }
        }
        for path in project.index().files {
            if self.manifest.entries.contains_key(&path) {
                continue;
            }
            let Ok(Some(file)) = CurrentFile::open(project, &path) else {
                continue;
            };
            if file.len > SNAPSHOT_MAX_TRACKED_BYTES {
                continue;
            }
            let Ok(hash) = file.hash() else {
                continue;
            };
            changes.push(SnapshotChange {
                path,
                kind: ChangeKind::Created,
                after_hash: Some(hash),
                undoable: true,
            });
        }
        changes.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(changes)
    }

    /// Accepts the current state of `path` as its baseline, so `diff` no longer reports it.
    pub fn keep(&mut self, project: &Project, path: &str) -> Result<()> {
        self.ensure_project(project)?;
        normalized_relative(Path::new(path))?;
        let entry = match CurrentFile::open(project, path)? {
            Some(file) if file.len <= SNAPSHOT_MAX_TRACKED_BYTES => {
                let read = file
                    .read()
                    .with_context(|| format!("Cannot read {path} to keep it"))?;
                Some(self.record(read)?)
            }
            _ => None,
        };
        let previous = match entry {
            Some(entry) => self.manifest.entries.insert(path.to_string(), entry),
            None => self.manifest.entries.remove(path),
        };
        if let Err(error) = self.write_manifest() {
            match previous {
                Some(entry) => self.manifest.entries.insert(path.to_string(), entry),
                None => self.manifest.entries.remove(path),
            };
            return Err(error);
        }
        Ok(())
    }

    /// Puts back the snapshot's version of `path`, or deletes it when the
    /// snapshot has none. `expected_after_hash` is what `diff` reported; when
    /// the file no longer matches it, nothing is changed.
    pub fn undo(
        &self,
        project: &Project,
        path: &str,
        expected_after_hash: Option<&str>,
    ) -> Result<()> {
        validate_import_path(project, path)?;
        self.ensure_project(project)?;
        let current = match CurrentFile::open(project, path)? {
            Some(file) => Some(file.hash().with_context(|| format!("Cannot read {path}"))?),
            None => None,
        };
        ensure!(
            current.as_deref() == expected_after_hash,
            "{path} changed after Claude's edit, so it was not restored."
        );
        let target = project.root.join(path);
        match self.manifest.entries.get(path) {
            None => match fs::remove_file(&target) {
                Err(error) if error.kind() != ErrorKind::NotFound => {
                    Err(error).with_context(|| format!("Cannot remove {path}"))
                }
                _ => Ok(()),
            },
            Some(entry) if !entry.stored => bail!("{path} is too large to restore."),
            Some(entry) => self.restore(project, path, entry),
        }
    }

    /// Removes the snapshot folder.
    pub fn discard(self) -> Result<()> {
        match fs::remove_dir_all(&self.dir) {
            Err(error) if error.kind() != ErrorKind::NotFound => Err(error).with_context(|| {
                format!("Cannot remove the takeover snapshot {}", self.dir.display())
            }),
            _ => Ok(()),
        }
    }

    fn ensure_project(&self, project: &Project) -> Result<()> {
        ensure!(
            self.manifest.root == project.key(),
            "This takeover snapshot belongs to another project."
        );
        Ok(())
    }

    /// The manifest entry for a file just read, storing its bytes when they fit.
    fn record(&mut self, read: ReadFile) -> Result<Entry> {
        let stored = match &read.bytes {
            Some(bytes) => self.store_blob(&read.hash, bytes)?,
            None => false,
        };
        Ok(Entry {
            hash: read.hash,
            stored,
            mode: read.mode,
        })
    }

    /// Writes `blobs/<hash>` unless it exists. `false` when the snapshot is full.
    fn store_blob(&mut self, hash: &str, bytes: &[u8]) -> Result<bool> {
        let blob = self.dir.join(BLOBS).join(hash);
        if blob.symlink_metadata().is_ok() {
            return Ok(true);
        }
        let len = bytes.len() as u64;
        if self.stored_bytes + len > SNAPSHOT_MAX_TOTAL_BYTES {
            return Ok(false);
        }
        let mut pending = tempfile::NamedTempFile::new_in(self.dir.join(BLOBS))
            .context("Cannot write the takeover snapshot")?;
        pending.write_all(bytes)?;
        pending
            .persist(&blob)
            .context("Cannot write the takeover snapshot")?;
        self.stored_bytes += len;
        Ok(true)
    }

    fn write_manifest(&self) -> Result<()> {
        let mut pending = tempfile::NamedTempFile::new_in(&self.dir)
            .context("Cannot write the takeover snapshot")?;
        serde_json::to_writer(&mut pending, &self.manifest)?;
        pending.as_file().sync_all()?;
        pending
            .persist(self.dir.join(MANIFEST))
            .context("Cannot write the takeover snapshot")?;
        Ok(())
    }

    fn restore(&self, project: &Project, path: &str, entry: &Entry) -> Result<()> {
        let bytes = fs::read(self.dir.join(BLOBS).join(&entry.hash)).with_context(|| {
            format!("The saved copy of {path} is missing, so it was not restored.")
        })?;
        // Blobs are not synced one by one, so a crash can leave one incomplete.
        ensure!(
            content_hash(&bytes) == entry.hash,
            "The saved copy of {path} is damaged, so it was not restored."
        );
        let target = project.root.join(path);
        let parent = target.parent().context("File has no parent folder")?;
        create_parents(project, path)?;
        let mut pending = tempfile::NamedTempFile::new_in(parent)
            .with_context(|| format!("Cannot restore {path}"))?;
        pending.write_all(&bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            pending
                .as_file()
                .set_permissions(fs::Permissions::from_mode(entry.mode))?;
        }
        pending.as_file().sync_all()?;
        pending
            .persist(&target)
            .with_context(|| format!("Cannot restore {path}"))?;
        Ok(())
    }
}

/// A regular file of the project, opened without following symlinks.
struct CurrentFile {
    file: File,
    len: u64,
    mode: u32,
}

/// A file's hash, and its bytes when it is small enough to store.
struct ReadFile {
    hash: String,
    bytes: Option<Vec<u8>>,
    mode: u32,
}

impl CurrentFile {
    /// `None` when there is no regular file at `path` that is reached through
    /// real folders: nothing is there, or a symlink, a folder or a path through
    /// a symlinked folder is. Such a path is not a project file, as in the index.
    fn open(project: &Project, path: &str) -> Result<Option<CurrentFile>> {
        normalized_relative(Path::new(path))?;
        let mut full = project.root.clone();
        let mut parts = Path::new(path).components().peekable();
        while let Some(part) = parts.next() {
            full.push(part);
            let metadata = match full.symlink_metadata() {
                Ok(metadata) => metadata,
                Err(error)
                    if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(error).with_context(|| format!("Cannot inspect {path}")),
            };
            let expected = if parts.peek().is_some() {
                metadata.is_dir()
            } else {
                metadata.is_file()
            };
            if metadata.file_type().is_symlink() || !expected {
                return Ok(None);
            }
        }
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = match options.open(&full) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).with_context(|| format!("Cannot read {path}")),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Ok(None);
        }
        Ok(Some(CurrentFile {
            file,
            len: metadata.len(),
            mode: permission_bits(&metadata),
        }))
    }

    fn hash(self) -> Result<String> {
        Ok(self.stream(false)?.hash)
    }

    /// Reads the whole file once, keeping the bytes when it fits in a blob.
    fn read(self) -> Result<ReadFile> {
        let small = self.len <= SNAPSHOT_MAX_FILE_BYTES;
        self.stream(small)
    }

    /// The same SHA-256 as `content_hash` over the whole file, streamed from
    /// the handle that was checked, so a large file is never held in memory.
    fn stream(mut self, keep_bytes: bool) -> Result<ReadFile> {
        let mut hasher = Sha256::new();
        let mut bytes = keep_bytes.then(Vec::new);
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let count = self.file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
            if let Some(kept) = &mut bytes {
                if kept.len() + count > SNAPSHOT_MAX_FILE_BYTES as usize {
                    // It grew while being read; it is hashed but not stored.
                    bytes = None;
                } else {
                    kept.extend_from_slice(&buffer[..count]);
                }
            }
        }
        Ok(ReadFile {
            hash: format!("{:x}", hasher.finalize()),
            bytes,
            mode: self.mode,
        })
    }
}

fn permission_bits(metadata: &Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        if metadata.permissions().readonly() {
            0o444
        } else {
            0o644
        }
    }
}

fn create_private_dir(dir: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(dir)
        .with_context(|| format!("Cannot create {}", dir.display()))?;
    // The umask can narrow the mode above, and an existing folder keeps its own.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Creates the missing folders above `path`, refusing anything that is not a real folder.
fn create_parents(project: &Project, path: &str) -> Result<()> {
    let Some(parent) = Path::new(path).parent() else {
        return Ok(());
    };
    let mut directory = project.root.clone();
    for part in parent.components() {
        directory.push(part);
        match directory.symlink_metadata() {
            Ok(metadata) => ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "{path} cannot be restored because a file is in the way of its folder."
            ),
            Err(error) if error.kind() == ErrorKind::NotFound => fs::create_dir(&directory)
                .with_context(|| format!("Cannot create the folder for {path}"))?,
            Err(error) => {
                return Err(error).with_context(|| format!("Cannot create the folder for {path}"));
            }
        }
    }
    Ok(())
}
