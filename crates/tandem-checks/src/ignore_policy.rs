//! Retain the complete materialization policy for later immutable expansion.
use crate::{
    MAX_INPUT_FILES, MAX_SNAPSHOT_BYTES, inspection::git_metadata_directories, secure_read,
};
use anyhow::{Context, Result, ensure};
use ignore::{IncrementalIgnore, WalkBuilder};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use tandem_core::{Project, content_hash, normalized_relative};

pub(crate) struct CapturedIgnorePolicy {
    project: Project,
    matcher: IncrementalIgnore,
    directories: BTreeSet<PathBuf>,
    sources: BTreeMap<PathBuf, (Option<Project>, Option<String>)>,
    bytes: u64,
}

impl CapturedIgnorePolicy {
    pub(crate) fn capture(project: &Project) -> Result<Self> {
        let mut policy = Self {
            project: project.clone(),
            matcher: WalkBuilder::new(&project.root)
                .standard_filters(false)
                .build_matchers()
                .pop()
                .unwrap(),
            directories: BTreeSet::new(),
            sources: BTreeMap::new(),
            bytes: 0,
        };
        // These are all ambient sources read by ignore 0.4.34. Inspect them
        // before invoking its parser, which otherwise suppresses I/O errors.
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let xdg = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|p| p.join(".config")));
        for path in [
            std::env::var_os("GIT_CONFIG_GLOBAL")
                .filter(|p| !p.is_empty())
                .map(PathBuf::from),
            home.map(|p| p.join(".gitconfig")),
            xdg.map(|p| p.join("git/config")),
            Some(
                std::env::var_os("GIT_CONFIG_SYSTEM")
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| "/etc/gitconfig".into()),
            ),
        ]
        .into_iter()
        .flatten()
        {
            // This conventional explicit empty config has no policy bytes.
            if path != Path::new("/dev/null") {
                policy.record(&path)?;
            }
        }
        if let Some(path) = ignore::gitignore::gitconfig_excludes_path() {
            policy.record(&path)?;
        }
        // Unlike WalkBuilder, report malformed or unreadable global patterns.
        let (_, error) = ignore::gitignore::Gitignore::global();
        if let Some(error) = error {
            return Err(error).context("Cannot capture the global ignore policy");
        }
        for directory in project.root.ancestors() {
            policy.record_directory(directory)?;
        }
        // Keep the exact original options and library precedence. The recursive
        // walker disables its own filters and delegates to this one matcher.
        policy.matcher = WalkBuilder::new(&project.root)
            .hidden(false)
            .follow_links(false)
            .require_git(false)
            .max_depth(Some(64))
            .build_matchers()
            .pop()
            .unwrap();
        policy.load_directory(Path::new(""))?;
        policy.verify_sources()?;
        Ok(policy)
    }

    fn read(path: &Path, root: Option<&Project>) -> Result<Option<Vec<u8>>> {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
            Ok(_) => {
                if let Some(project) = root {
                    let relative = path
                        .strip_prefix(&project.root)?
                        .to_str()
                        .context("Invalid ignore policy path")?;
                    return Ok(Some(secure_read(project, relative)?.0));
                }
                let project = Project::open(path.parent().context("Ignore policy has no parent")?)?;
                let name = path
                    .file_name()
                    .and_then(|p| p.to_str())
                    .context("Invalid ignore policy path")?;
                Ok(Some(secure_read(&project, name)?.0))
            }
        }
    }

    fn record(&mut self, path: &Path) -> Result<()> {
        self.record_from(path, None)
    }

    fn record_from(&mut self, path: &Path, root: Option<&Project>) -> Result<()> {
        if self.sources.contains_key(path) {
            return Ok(());
        }
        let bytes =
            Self::read(path, root).context("Cannot safely capture the complete ignore policy")?;
        self.bytes += bytes.as_ref().map_or(0, |b| b.len() as u64);
        ensure!(
            self.bytes <= MAX_SNAPSHOT_BYTES && self.sources.len() < MAX_INPUT_FILES * 16,
            "Ignore policy exceeds its capture budget"
        );
        self.sources.insert(
            path.into(),
            (root.cloned(), bytes.map(|b| content_hash(&b))),
        );
        Ok(())
    }

    fn record_directory(&mut self, directory: &Path) -> Result<()> {
        let source_project = directory
            .starts_with(&self.project.root)
            .then(|| self.project.clone());
        for name in [".ignore", ".gitignore"] {
            self.record_from(&directory.join(name), source_project.as_ref())?;
        }
        if !directory.try_exists()? {
            return Ok(());
        }
        let project = Project::open(directory)?;
        if let Some((local, common)) = git_metadata_directories(&project)? {
            if local != common {
                self.record_from(&directory.join(".git"), Some(&project))?;
                let local_project = Project::open(&local)?;
                self.record_from(&local.join("commondir"), Some(&local_project))?;
                self.record_from(&local.join("gitdir"), Some(&local_project))?;
                // The library resolves relative Git directory pointers against
                // the process cwd; that policy cannot be captured equivalently.
                let (pointer, _) = secure_read(&project, ".git")?;
                ensure!(
                    Path::new(
                        std::str::from_utf8(&pointer)?
                            .trim()
                            .strip_prefix("gitdir: ")
                            .context("Invalid Git directory pointer")?
                    )
                    .is_absolute(),
                    "Relative Git ignore metadata is unavailable"
                );
            }
            self.record_from(&common.join("info/exclude"), Some(&Project::open(&common)?))?;
        }
        Ok(())
    }

    fn load_directory(&mut self, relative: &Path) -> Result<()> {
        if self.directories.contains(relative) {
            return Ok(());
        }
        self.record_directory(&self.matcher.root().join(relative))?;
        // Checking a child eagerly loads this directory's policies, even when
        // it is empty. No later history query may load an uncaptured directory.
        let (_, error) = self
            .matcher
            .matched_with_errors(relative.join("__tandem_policy_probe"), false);
        if let Some(error) = error {
            return Err(error).context("Cannot capture the complete directory ignore policy");
        }
        self.directories.insert(relative.into());
        Ok(())
    }

    pub(crate) fn allows_entry(&mut self, relative: &Path, is_dir: bool) -> Result<bool> {
        let (matched, error) = self.matcher.matched_with_errors(relative, is_dir);
        if let Some(error) = error {
            return Err(error).context("Cannot capture the complete ignore policy");
        }
        if matched.is_ignore() {
            return Ok(false);
        }
        if is_dir {
            self.load_directory(relative)?;
        }
        Ok(true)
    }

    pub(crate) fn allows_buffer(&mut self, path: &str) -> Result<bool> {
        normalized_relative(Path::new(path))?;
        let mut parents = Path::new(path)
            .parent()
            .unwrap()
            .ancestors()
            .collect::<Vec<_>>();
        parents.reverse();
        for parent in parents {
            if !self.allows_entry(parent, true)? {
                return Ok(false);
            }
        }
        self.allows_entry(Path::new(path), false)
    }

    pub(crate) fn allows_captured_path(&self, path: &str) -> bool {
        normalized_relative(Path::new(path)).is_ok()
            && Path::new(path)
                .parent()
                .is_some_and(|p| self.directories.contains(p))
            && {
                let (matched, error) = self.matcher.clone().matched_with_errors(path, false);
                error.is_none() && !matched.is_ignore()
            }
    }

    pub(crate) fn verify_sources(&self) -> Result<()> {
        for (path, (root, expected)) in &self.sources {
            let actual = Self::read(path, root.as_ref())?.map(|b| content_hash(&b));
            ensure!(actual == *expected, "Ignore policy changed during capture");
        }
        Ok(())
    }

    pub(crate) fn identity(&self) -> String {
        let hashes = self
            .sources
            .iter()
            .map(|(path, (_, hash))| (path, hash))
            .collect::<BTreeMap<_, _>>();
        content_hash(&serde_json::to_vec(&hashes).expect("ignore policy source hashes"))
    }
}
