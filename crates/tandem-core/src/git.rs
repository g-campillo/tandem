//! The name of the git branch a project is on, read from the repository's `HEAD` file.
//! Nothing here runs git.
use std::fs;
use std::path::{Path, PathBuf};

/// What `HEAD` points at: the branch name of `ref: refs/heads/<name>`, or the first seven
/// characters of a detached commit id. `None` for anything else.
pub fn parse_head(head: &str) -> Option<String> {
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref:") {
        let reference = reference.trim();
        let name = reference.strip_prefix("refs/heads/").unwrap_or(reference);
        return (!name.is_empty()).then(|| name.to_owned());
    }
    (head.len() >= 7 && head.chars().all(|c| c.is_ascii_hexdigit())).then(|| head[..7].to_owned())
}

/// The folder that holds `HEAD` for the repository `root` is in: the `.git` folder of
/// `root` or of its nearest ancestor, or the folder a `.git` file (a worktree or a
/// submodule) names with `gitdir:`.
fn git_dir(root: &Path) -> Option<PathBuf> {
    root.ancestors().find_map(|folder| {
        let dot_git = folder.join(".git");
        if dot_git.is_dir() {
            return Some(dot_git);
        }
        let text = fs::read_to_string(&dot_git).ok()?;
        let target = text.trim().strip_prefix("gitdir:")?.trim();
        Some(folder.join(target))
    })
}

/// The branch the project at `root` is on, or `None` when it is not in a git repository
/// or `HEAD` cannot be read.
pub fn head_branch(root: &Path) -> Option<String> {
    parse_head(&fs::read_to_string(git_dir(root)?.join("HEAD")).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_head_reads_a_branch() {
        assert_eq!(
            parse_head("ref: refs/heads/main\n").as_deref(),
            Some("main")
        );
        assert_eq!(
            parse_head("ref: refs/heads/build/ui-polish").as_deref(),
            Some("build/ui-polish")
        );
        assert_eq!(
            parse_head("ref: refs/remotes/origin/main").as_deref(),
            Some("refs/remotes/origin/main")
        );
    }

    #[test]
    fn parse_head_shortens_a_detached_commit() {
        assert_eq!(
            parse_head("a0e9c95d2f0b4c1e8a7d6b5c4f3e2d1c0b9a8f7e\n").as_deref(),
            Some("a0e9c95")
        );
    }

    #[test]
    fn parse_head_rejects_anything_else() {
        for head in ["", "\n", "ref:", "ref: refs/heads/", "not a head", "abc12"] {
            assert_eq!(parse_head(head), None, "{head:?}");
        }
    }

    #[test]
    fn head_branch_finds_the_repository_of_a_subfolder() {
        let repo = tempfile::tempdir().unwrap();
        fs::create_dir_all(repo.path().join(".git")).unwrap();
        fs::write(repo.path().join(".git/HEAD"), "ref: refs/heads/feature\n").unwrap();
        let nested = repo.path().join("crates/app");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(head_branch(repo.path()).as_deref(), Some("feature"));
        assert_eq!(head_branch(&nested).as_deref(), Some("feature"));
    }

    #[test]
    fn head_branch_follows_a_worktrees_git_file() {
        let folder = tempfile::tempdir().unwrap();
        let real = folder.path().join("main/.git/worktrees/polish");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("HEAD"), "ref: refs/heads/build/ui-polish\n").unwrap();
        let worktree = folder.path().join("polish");
        fs::create_dir_all(&worktree).unwrap();
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", real.display()),
        )
        .unwrap();
        assert_eq!(head_branch(&worktree).as_deref(), Some("build/ui-polish"));
        // A relative gitdir is relative to the folder of the `.git` file.
        fs::write(
            worktree.join(".git"),
            "gitdir: ../main/.git/worktrees/polish\n",
        )
        .unwrap();
        assert_eq!(head_branch(&worktree).as_deref(), Some("build/ui-polish"));
    }

    #[test]
    fn head_branch_is_none_outside_a_repository() {
        let folder = tempfile::tempdir().unwrap();
        // A `.git` folder without a HEAD file: nothing to read.
        fs::create_dir_all(folder.path().join(".git")).unwrap();
        assert_eq!(head_branch(folder.path()), None);
    }
}
