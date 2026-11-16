//! Local workspace IO and immutable recovery snapshots. No model or shell access.
mod storage;
pub use storage::*;
pub mod appearance;
pub mod breakdown;
pub mod chat;
mod chat_store;
pub mod edit;
pub mod layout;
pub mod takeover;
mod tools;
pub use tools::*;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_INDEX_FILES: usize = 20_000;
pub const MAX_SEARCH_RESULTS: usize = 200;

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn content_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineEnding {
    Lf,
    CrLf,
    Mixed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextFormat {
    pub bom: bool,
    pub line_ending: LineEnding,
}

impl Default for TextFormat {
    fn default() -> Self {
        Self {
            bom: false,
            line_ending: LineEnding::Lf,
        }
    }
}

impl TextFormat {
    pub fn decode(bytes: &[u8]) -> Result<(Self, String)> {
        ensure!(!bytes.contains(&0), "Binary files cannot be edited yet");
        let bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
        let text = std::str::from_utf8(if bom { &bytes[3..] } else { bytes })
            .context("Only UTF-8 text is supported; the file was not changed")?;
        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count();
        let line_ending = if crlf == 0 {
            LineEnding::Lf
        } else if crlf == lf {
            LineEnding::CrLf
        } else {
            LineEnding::Mixed
        };
        let text = if line_ending == LineEnding::CrLf {
            text.replace("\r\n", "\n")
        } else {
            text.to_string()
        };
        Ok((Self { bom, line_ending }, text))
    }

    pub fn encode(&self, text: &str) -> Vec<u8> {
        let text = match self.line_ending {
            LineEnding::CrLf => text.replace("\r\n", "\n").replace('\n', "\r\n"),
            _ => text.to_string(),
        };
        let mut bytes = if self.bom {
            vec![0xef, 0xbb, 0xbf]
        } else {
            Vec::new()
        };
        bytes.extend_from_slice(text.as_bytes());
        bytes
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    pub path: String,
    pub text: String,
    pub base_text: String,
    pub base_hash: Option<String>,
    pub format: TextFormat,
    pub revision: u64,
    pub recovered: bool,
    pub conflict: bool,
    pub read_only: bool,
}

impl Document {
    pub fn dirty(&self) -> bool {
        self.base_hash.is_none() || self.text != self.base_text
    }
    pub fn snapshot(&mut self, text: String) {
        if self.text != text {
            self.text = text;
            self.revision = self.revision.saturating_add(1);
        }
    }
    pub fn language(&self) -> &'static str {
        match Path::new(&self.path).extension().and_then(|s| s.to_str()) {
            Some("rs") => "rust",
            Some("py" | "pyi") => "python",
            Some("json") => "json",
            Some("toml") => "toml",
            Some("md") => "markdown",
            _ => "plain_text",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct FileIndex {
    pub files: Vec<String>,
    pub truncated: bool,
    pub unreadable_entries: usize,
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub path: String,
    pub line: usize,
    pub preview: String,
}

#[derive(Clone, Debug)]
pub enum DiskChange {
    Unchanged,
    MetadataChanged { read_only: bool },
    Changed(Document),
    Missing,
    Unavailable(String),
}

impl Project {
    pub fn open(root: &Path) -> Result<Self> {
        let root = fs::canonicalize(root).context("Cannot open project folder")?;
        ensure!(root.is_dir(), "Choose a folder to open as a project");
        ensure!(root.to_str().is_some(), "Project paths must be valid UTF-8");
        Ok(Self { root })
    }
    pub fn key(&self) -> &str {
        self.root.to_str().expect("validated project path")
    }

    /// Resolve through canonical parents so symlinks cannot escape the selected project.
    pub fn resolve(&self, relative: &str) -> Result<PathBuf> {
        let relative = Path::new(relative);
        ensure!(
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
            "Use a relative file path inside this project"
        );
        let candidate = self.root.join(relative);
        let resolved = if candidate.exists() || candidate.symlink_metadata().is_ok() {
            fs::canonicalize(&candidate).context("File path is missing or has a broken symlink")?
        } else {
            let parent = fs::canonicalize(candidate.parent().context("File has no parent")?)
                .context("Create the parent directory before creating this file")?;
            parent.join(candidate.file_name().context("File name is missing")?)
        };
        ensure!(
            resolved.starts_with(&self.root),
            "This path points outside the selected project"
        );
        Ok(resolved)
    }

    /// An absent nested path is a draft, not permission to create directories.
    /// Existing ancestors are inspected without following links; Save repeats
    /// these checks and creates parents only after the explicit save action.
    fn missing_new_path(&self, relative: &str) -> Result<Option<PathBuf>> {
        validate_new_name(relative)?;
        normalized_relative(Path::new(relative))?;
        let relative: PathBuf = Path::new(relative).components().collect();
        let path = self.root.join(&relative);
        match path.symlink_metadata() {
            Ok(_) => return Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        validate_import_path(self, relative.to_str().context("Paths must be UTF-8")?)?;
        let mut parent = path.parent();
        while let Some(directory) = parent {
            if directory == self.root {
                break;
            }
            match directory.symlink_metadata() {
                Ok(metadata) => {
                    ensure!(
                        metadata.is_dir() && !metadata.file_type().is_symlink(),
                        "New file parents must be real project directories"
                    );
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => parent = directory.parent(),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(Some(path))
    }
    pub fn open_document(&self, relative: &str, allow_new: bool) -> Result<Document> {
        let path = match if allow_new {
            self.missing_new_path(relative)?
        } else {
            None
        } {
            Some(path) => path,
            None => self.resolve(relative)?,
        };
        let relative = path
            .strip_prefix(&self.root)?
            .to_str()
            .context("File paths must be valid UTF-8")?
            .to_string();
        let bytes = match read_bounded(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if allow_new && !path.exists() => {
                let _ = error;
                None
            }
            Err(error) => return Err(error),
        };
        let (format, text) = match &bytes {
            Some(bytes) => TextFormat::decode(bytes)?,
            None => (TextFormat::default(), String::new()),
        };
        Ok(Document {
            path: relative,
            base_text: text.clone(),
            text,
            base_hash: bytes.as_deref().map(content_hash),
            format,
            revision: 0,
            recovered: false,
            conflict: false,
            read_only: path.metadata().is_ok_and(|m| m.permissions().readonly()),
        })
    }

    pub fn save(&self, snapshot: &Document) -> Result<Document> {
        if snapshot.base_hash.is_none() {
            validate_import_path(self, &snapshot.path)?;
            let parent = Path::new(&snapshot.path)
                .parent()
                .context("New file has no parent")?;
            let mut directory = self.root.clone();
            for component in parent.components() {
                directory.push(component);
                if !directory.exists() {
                    fs::create_dir(&directory)
                        .context("Cannot create the new file's parent directory")?;
                }
                let metadata = directory.symlink_metadata()?;
                ensure!(
                    metadata.is_dir() && !metadata.file_type().is_symlink(),
                    "New file parents must be real project directories"
                );
            }
        }
        let path = self.resolve(&snapshot.path)?;
        let current = if path.exists() {
            Some(read_bounded(&path)?)
        } else {
            None
        };
        ensure!(
            current.as_deref().map(content_hash) == snapshot.base_hash,
            "File changed or was deleted on disk. Your draft is preserved; compare it with disk or use Save Copy"
        );
        let permissions = path.metadata().ok().map(|m| m.permissions());
        ensure!(
            !permissions.as_ref().is_some_and(|p| p.readonly()),
            "File became read-only; your draft is preserved"
        );
        if path.exists() {
            let mut access = fs::OpenOptions::new();
            access.write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                access.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
            }
            access
                .open(&path)
                .context("File is not writable; use Save Copy to keep your draft")?;
        }
        let bytes = snapshot.format.encode(&snapshot.text);
        ensure!(
            bytes.len() as u64 <= MAX_FILE_BYTES,
            "This editor currently supports files up to 2 MiB; your draft is preserved"
        );
        let parent = path.parent().context("File has no parent directory")?;
        let mut pending = tempfile::NamedTempFile::new_in(parent)
            .context("Cannot prepare an atomic save in this folder")?;
        pending.write_all(&bytes)?;
        if let Some(permissions) = permissions {
            pending.as_file().set_permissions(permissions)?;
        }
        pending.as_file().sync_all()?;
        // Recheck immediately before replacement. Uncooperative external writers
        // still have a small race window; this is not filesystem-wide CAS.
        let latest = if path.exists() {
            Some(read_bounded(&path)?)
        } else {
            None
        };
        ensure!(
            latest.as_deref().map(content_hash) == snapshot.base_hash,
            "File changed during save; your draft is preserved"
        );
        if snapshot.base_hash.is_none() {
            pending
                .persist_noclobber(&path)
                .context("Another file appeared at this path; your draft is preserved")?;
        } else {
            pending
                .persist(&path)
                .context("Cannot replace the file; your draft is preserved")?;
        }
        // Directory fsync is not uniformly supported. File content was fsynced above.
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        let mut saved = snapshot.clone();
        saved.base_hash = Some(content_hash(&bytes));
        saved.base_text = saved.text.clone();
        saved.recovered = false;
        saved.conflict = false;
        saved.read_only = false;
        Ok(saved)
    }

    pub fn disk_change(&self, snapshot: &Document) -> DiskChange {
        if snapshot.base_hash.is_none() {
            match self.missing_new_path(&snapshot.path) {
                Ok(Some(_)) => return DiskChange::Unchanged,
                Ok(None) => {}
                Err(e) => return DiskChange::Unavailable(e.to_string()),
            }
        }
        let path = match self.resolve(&snapshot.path) {
            Ok(path) => path,
            Err(e) => return DiskChange::Unavailable(e.to_string()),
        };
        if !path.exists() {
            return if snapshot.base_hash.is_none() {
                DiskChange::Unchanged
            } else {
                DiskChange::Missing
            };
        }
        match self.open_document(&snapshot.path, false) {
            Ok(current)
                if current.base_hash == snapshot.base_hash
                    && current.read_only != snapshot.read_only =>
            {
                DiskChange::MetadataChanged {
                    read_only: current.read_only,
                }
            }
            Ok(current) if current.base_hash == snapshot.base_hash => DiskChange::Unchanged,
            Ok(current) => DiskChange::Changed(current),
            Err(error) => DiskChange::Unavailable(error.to_string()),
        }
    }

    pub fn index(&self) -> FileIndex {
        let mut index = FileIndex::default();
        let walker = ignore::WalkBuilder::new(&self.root)
            .hidden(false)
            .follow_links(false)
            .require_git(false)
            .max_depth(Some(64))
            .filter_entry(|entry| {
                !matches!(
                    entry.file_name().to_str(),
                    Some(
                        ".git"
                            | "target"
                            | "node_modules"
                            | ".tandem-local"
                            | ".venv"
                            | "__pycache__"
                    )
                )
            })
            .build();
        for (scanned, entry) in walker.enumerate() {
            if scanned >= MAX_INDEX_FILES * 4 {
                index.truncated = true;
                break;
            }
            let entry = match entry {
                Ok(e) => e,
                Err(_) => {
                    index.unreadable_entries += 1;
                    continue;
                }
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            if let Ok(path) = entry.path().strip_prefix(&self.root)
                && let Some(path) = path.to_str()
            {
                index.files.push(path.to_string());
            }
            if index.files.len() >= MAX_INDEX_FILES {
                index.truncated = true;
                break;
            }
        }
        index.files.sort();
        index
    }

    pub fn search(&self, files: &[String], query: &str, cancelled: &AtomicBool) -> Vec<SearchHit> {
        self.search_with_overlays(files, query, &[], cancelled)
    }
    pub fn search_with_overlays(
        &self,
        files: &[String],
        query: &str,
        overlays: &[Document],
        cancelled: &AtomicBool,
    ) -> Vec<SearchHit> {
        if query.is_empty() {
            return Vec::new();
        }
        let mut hits = Vec::new();
        let mut paths = files.to_vec();
        for overlay in overlays {
            if !paths.contains(&overlay.path) {
                paths.push(overlay.path.clone());
            }
        }
        for path in &paths {
            if cancelled.load(Ordering::Relaxed) {
                break;
            }
            let document = if let Some(overlay) = overlays.iter().find(|doc| doc.path == *path) {
                overlay.clone()
            } else {
                let Ok(document) = self.open_document(path, false) else {
                    continue;
                };
                document
            };
            for (line, text) in document.text.lines().enumerate() {
                if text.contains(query) {
                    hits.push(SearchHit {
                        path: path.clone(),
                        line: line + 1,
                        preview: text.chars().take(180).collect(),
                    });
                }
                if hits.len() >= MAX_SEARCH_RESULTS {
                    return hits;
                }
            }
        }
        hits
    }

    pub fn diff(&self, snapshot: &Document) -> Result<String> {
        let disk = match self.open_document(&snapshot.path, false) {
            Ok(document) => document.text,
            Err(_) if !self.resolve(&snapshot.path)?.exists() => String::new(),
            Err(error) => return Err(error),
        };
        Ok(similar::TextDiff::from_lines(&disk, &snapshot.text)
            .unified_diff()
            .context_radius(3)
            .header("Disk", "Your buffer")
            .to_string())
    }
}

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    ensure!(fs::metadata(path)?.is_file(), "Choose a regular text file");
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("Cannot read {}", path.display()))?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "Choose a regular text file");
    ensure!(
        metadata.len() <= MAX_FILE_BYTES,
        "File is larger than the 2 MiB editing limit"
    );
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_FILE_BYTES,
        "File grew beyond the editing limit while reading"
    );
    Ok(bytes)
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

pub fn validate_new_name(name: &str) -> Result<()> {
    ensure!(!name.trim().is_empty(), "Enter a file name");
    if name.contains(['\n', '\r', '\0']) {
        bail!("File name contains unsupported characters");
    }
    Ok(())
}
