//! Which project a directory belongs to: the repository around it, and the
//! branch checked out there. Read from the files git keeps, without
//! running git.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The project an agent works in.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Project {
    /// The repository's folder name, or the directory's own outside one.
    pub name: String,
    /// The repository's own folder: the same for every worktree of it, so
    /// agents in different worktrees gather under one project.
    pub root: PathBuf,
    pub branch: Option<String>,
    /// Whether the directory is in a linked worktree of the repository.
    pub worktree: bool,
}

impl Project {
    #[must_use]
    pub fn of(directory: &Path) -> Self {
        let found = directory.ancestors().find_map(|ancestor| {
            let dot_git = ancestor.join(".git");
            if dot_git.is_dir() {
                Some((ancestor.to_path_buf(), dot_git, false))
            } else if dot_git.is_file() {
                // A linked worktree: `gitdir: <repository>/.git/worktrees/<name>`.
                let text = std::fs::read_to_string(&dot_git).ok()?;
                let git_dir = text.lines().find_map(|line| line.strip_prefix("gitdir:"))?;
                let git_dir = ancestor.join(git_dir.trim());
                let root = git_dir
                    .ancestors()
                    .find(|path| path.file_name().is_some_and(|name| name == ".git"))
                    .and_then(Path::parent)
                    .map(Path::to_path_buf);
                // A submodule's git directory is under its parent's, and
                // is no worktree of it.
                let linked = git_dir.parent().is_some_and(|parent| {
                    parent.file_name().is_some_and(|name| name == "worktrees")
                });
                match root {
                    Some(root) if linked => Some((root, git_dir, true)),
                    _ => Some((ancestor.to_path_buf(), git_dir, false)),
                }
            } else {
                None
            }
        });
        let Some((root, git_dir, worktree)) = found else {
            return Self {
                name: folder_name(directory),
                root: directory.to_path_buf(),
                branch: None,
                worktree: false,
            };
        };
        Self {
            name: folder_name(&root),
            root,
            branch: branch(&git_dir),
            worktree,
        }
    }
}

fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The branch `HEAD` names, or the start of the commit it is detached at.
fn branch(git_dir: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref:") {
        let reference = reference.trim();
        return Some(
            reference
                .strip_prefix("refs/heads/")
                .unwrap_or(reference)
                .to_owned(),
        );
    }
    (head.len() >= 7 && head.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| head[..7].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_is_named_for_the_repository_around_it() {
        let root = tempfile::tempdir().expect("temporary directory");
        let repository = root.path().join("mux");
        std::fs::create_dir_all(repository.join(".git")).expect("git directory");
        std::fs::write(repository.join(".git/HEAD"), "ref: refs/heads/fix/flaky\n").expect("HEAD");
        let inside = repository.join("crates/mux-fleet");
        std::fs::create_dir_all(&inside).expect("directory");

        let project = Project::of(&inside);
        assert_eq!(project.name, "mux");
        assert_eq!(project.root, repository);
        assert_eq!(project.branch.as_deref(), Some("fix/flaky"));
        assert!(!project.worktree);
    }

    #[test]
    fn a_linked_worktree_belongs_to_its_repository() {
        let root = tempfile::tempdir().expect("temporary directory");
        let repository = root.path().join("mux");
        let git_dir = repository.join(".git/worktrees/agent-2");
        std::fs::create_dir_all(&git_dir).expect("git directory");
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/agent-2\n").expect("HEAD");
        let worktree = root.path().join("worktrees/agent-2");
        std::fs::create_dir_all(&worktree).expect("worktree");
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .expect("git file");

        let project = Project::of(&worktree);
        assert_eq!(project.name, "mux");
        assert_eq!(project.root, repository);
        assert_eq!(project.branch.as_deref(), Some("agent-2"));
        assert!(project.worktree);
    }

    #[test]
    fn a_detached_head_and_a_plain_folder_are_named_too() {
        let root = tempfile::tempdir().expect("temporary directory");
        let repository = root.path().join("site");
        std::fs::create_dir_all(repository.join(".git")).expect("git directory");
        std::fs::write(
            repository.join(".git/HEAD"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .expect("HEAD");
        assert_eq!(Project::of(&repository).branch.as_deref(), Some("0123456"));

        let plain = root.path().join("notes");
        std::fs::create_dir_all(&plain).expect("directory");
        let project = Project::of(&plain);
        assert_eq!(project.name, "notes");
        assert_eq!(project.branch, None);
    }
}
