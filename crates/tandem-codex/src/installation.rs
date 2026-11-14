//! Locate only the tested runtime. A broken app bundle never falls back to PATH.
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};
pub const RUNTIME_SHA256: &str = "739cda86e15d30a6e2b40cf8dcbbfb988aaf48b305b27065573bb8e9fd41d002";
pub const EXEC_SERVER_SHA256: &str =
    "0532760c03ec71af4ce4d7a8f745399dabdae2b9ee0283ec849c4997facdb918";
pub const DEVELOPMENT_RUNTIME: &str = "/opt/homebrew/Caskroom/codex/0.169.0/bin/codex";
#[derive(Clone, Debug)]
pub struct RuntimeInstallation {
    pub binary: PathBuf,
    pub bundled: bool,
    pub resources: Option<PathBuf>,
}
fn bundle_resources(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && contents
            .parent()?
            .extension()?
            .to_str()?
            .eq_ignore_ascii_case("app"))
    .then(|| contents.join("Resources"))
}
pub fn discover_runtime() -> Result<RuntimeInstallation> {
    let executable = std::env::current_exe().context("Cannot locate the Tandem application")?;
    let installation = locate_runtime(&executable, Path::new(DEVELOPMENT_RUNTIME));
    verify_runtime(&installation.binary).context(if installation.bundled{"Bundled Codex is missing or damaged. Reinstall a verified Tandem release; editing remains available"}else{"The development Codex runtime is unavailable; use a packaged Tandem release"})?;
    Ok(installation)
}
fn locate_runtime(executable: &Path, development: &Path) -> RuntimeInstallation {
    match bundle_resources(executable) {
        Some(resources) => RuntimeInstallation {
            binary: resources.join("codex/bin/codex"),
            bundled: true,
            resources: Some(resources),
        },
        None => RuntimeInstallation {
            binary: development.into(),
            bundled: false,
            resources: None,
        },
    }
}
pub fn verify_runtime(binary: &Path) -> Result<()> {
    verify_hash(binary, RUNTIME_SHA256)?;
    verify_hash(
        &binary
            .parent()
            .context("Missing runtime directory")?
            .join("codex-exec-server"),
        EXEC_SERVER_SHA256,
    )?;
    Ok(())
}
fn verify_hash(path: &Path, expected: &str) -> Result<()> {
    let metadata = path
        .symlink_metadata()
        .with_context(|| format!("Missing runtime component: {}", path.display()))?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Runtime components must be regular files"
    );
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    ensure!(
        format!("{:x}", digest.finalize()) == expected,
        "Codex 0.169.0 component fingerprint differs: {}",
        path.display()
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relocation_never_selects_an_inherited_or_fallback_binary() {
        for executable in [
            "/Applications/Tandem.app/Contents/MacOS/tandem",
            "/Applications/Tandem.APP/Contents/MacOS/tandem",
            "/Users/example/Applications/My Tools/Tandem.app/Contents/MacOS/tandem",
        ] {
            let found = locate_runtime(Path::new(executable), Path::new("/fallback/codex"));
            assert!(found.bundled);
            assert_eq!(
                found.binary,
                Path::new(executable)
                    .parent()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join("Resources/codex/bin/codex")
            );
            assert!(verify_runtime(&found.binary).is_err());
        }
        assert!(
            !locate_runtime(
                Path::new("/code/target/debug/tandem"),
                Path::new("/fallback/codex")
            )
            .bundled
        );
    }
    #[test]
    fn fingerprints_refuse_corruption_and_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("runtime");
        std::fs::write(&path, b"fixture").unwrap();
        let hash = format!("{:x}", Sha256::digest(b"fixture"));
        verify_hash(&path, &hash).unwrap();
        std::fs::write(&path, b"changed").unwrap();
        assert!(verify_hash(&path, &hash).is_err());
        #[cfg(unix)]
        {
            let link = temp.path().join("link");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(verify_hash(&link, &format!("{:x}", Sha256::digest(b"changed"))).is_err());
        }
    }
}
