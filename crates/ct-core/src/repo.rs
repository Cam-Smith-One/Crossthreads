//! Repository identity that is the same on every machine (SPEC §6.1).
//!
//! A session records the absolute path it ran in, and that path differs per
//! device and per worktree. To group work across them we derive a **repo key**:
//!
//! 1. the normalised `origin` remote URL (`github.com/owner/repo`), else
//! 2. the root commit (`root:<sha>`), else
//! 3. the absolute path (`path:<abs>`), which is only meaningful on this device.
//!
//! All worktrees of a repository share one key, because the lookup follows a
//! worktree's `.git` file back to the common git directory.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Prefix of a key derived from a root commit.
pub const ROOT_PREFIX: &str = "root:";
/// Prefix of a device-local key derived from a path.
pub const PATH_PREFIX: &str = "path:";

/// Normalise a git remote URL to `host/path`: no scheme, no credentials, no
/// port, no trailing `.git` or slash, lowercased. Returns `None` for input that
/// has no recognisable host and path (e.g. a bare local path).
pub fn normalize_remote(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    // Split off the scheme, or recognise scp-like `user@host:path`.
    let rest = if let Some((scheme, rest)) = url.split_once("://") {
        if scheme.eq_ignore_ascii_case("file") {
            return None;
        }
        rest.to_string()
    } else if let Some((left, path)) = url.split_once(':') {
        // scp-like syntax has no slash before the colon (`git@host:owner/repo`).
        // A Windows drive path (`C:\repo`) or a plain path is not a remote.
        if left.contains('/') || left.len() < 2 || path.starts_with('\\') {
            return None;
        }
        format!("{left}/{path}")
    } else {
        return None;
    };

    // Drop credentials (`user:token@`), which may precede the host.
    let (authority, path) = rest.split_once('/')?;
    let host = authority.rsplit('@').next().unwrap_or(authority);
    // Drop a port.
    let host = host.split(':').next().unwrap_or(host);
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).trim_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{host}/{path}").to_lowercase())
}

/// The repo key for a project directory, or `None` when `project` is empty.
/// Never fails: a directory that is missing or not a git repository gets a
/// device-local `path:` key.
pub fn repo_key_for(project: &str) -> Option<String> {
    let project = project.trim();
    if project.is_empty() {
        return None;
    }
    let path = Path::new(project);
    if let Some(common) = common_git_dir(path) {
        if let Some(remote) = origin_url(&common).and_then(|u| normalize_remote(&u)) {
            return Some(remote);
        }
        if let Some(root) = root_commit(path) {
            return Some(format!("{ROOT_PREFIX}{root}"));
        }
        // No remote and no commits: key on the repository's own directory so
        // its worktrees still group together.
        let base = common.parent().unwrap_or(&common);
        return Some(format!("{PATH_PREFIX}{}", base.display()));
    }
    Some(format!("{PATH_PREFIX}{project}"))
}

/// A short human label for a repo key: `owner/repo` for a remote, the last path
/// segment for a local path, a short hash for a root commit.
pub fn repo_label(key: &str) -> String {
    if let Some(root) = key.strip_prefix(ROOT_PREFIX) {
        return format!("repo {}", &root[..root.len().min(8)]);
    }
    let body = key.strip_prefix(PATH_PREFIX).unwrap_or(key);
    let parts: Vec<&str> = body.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    let keep = if key.starts_with(PATH_PREFIX) { 1 } else { 2 };
    let start = parts.len().saturating_sub(keep);
    let label = parts[start..].join("/");
    if label.is_empty() {
        body.to_string()
    } else {
        label
    }
}

/// Walk up from `start` to the enclosing repository and return its *common*
/// git directory (the main `.git`, even when `start` is inside a worktree).
fn common_git_dir(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let dot_git = d.join(".git");
        if dot_git.is_dir() {
            return Some(dot_git);
        }
        if dot_git.is_file() {
            // A worktree or submodule: `.git` is a file `gitdir: <path>`.
            let text = std::fs::read_to_string(&dot_git).ok()?;
            let target = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
            let git_dir = resolve(d, target);
            // A worktree's git dir names the shared one in `commondir`.
            if let Ok(common) = std::fs::read_to_string(git_dir.join("commondir")) {
                return Some(resolve(&git_dir, common.trim()));
            }
            return Some(git_dir);
        }
        dir = d.parent();
    }
    None
}

fn resolve(base: &Path, target: &str) -> PathBuf {
    let p = Path::new(target);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    };
    joined.canonicalize().unwrap_or(joined)
}

/// The URL of the `origin` remote (or, failing that, the first remote) from a
/// git directory's `config` file. Parsed directly so indexing never needs git.
fn origin_url(git_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(git_dir.join("config")).ok()?;
    let mut section: Option<String> = None;
    let mut first: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            // `[remote "origin"]` → `origin`.
            section = line
                .strip_prefix("[remote \"")
                .and_then(|r| r.split('"').next())
                .map(str::to_string);
            continue;
        }
        let Some(name) = &section else { continue };
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() != "url" {
            continue;
        }
        let url = v.trim().to_string();
        if name == "origin" {
            return Some(url);
        }
        first.get_or_insert(url);
    }
    first
}

/// The repository's root commit, via git. `None` when git is unavailable, the
/// repository has no commits, or history has several roots we can't order.
fn root_commit(path: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-list", "--max-parents=0", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Several roots (merged histories): take the smallest hash so the choice is
    // stable on every clone.
    text.lines()
        .map(str::trim)
        .filter(|l| l.len() >= 7 && l.chars().all(|c| c.is_ascii_hexdigit()))
        .min()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_common_remote_forms_to_one_key() {
        let want = Some("github.com/acme/api".to_string());
        for url in [
            "git@github.com:Acme/API.git",
            "https://github.com/acme/api.git",
            "https://github.com/acme/api/",
            "https://user:s3cret@github.com/acme/api",
            "ssh://git@github.com:22/acme/api.git",
            "  https://GitHub.com/acme/api  ",
        ] {
            assert_eq!(normalize_remote(url), want, "{url}");
        }
    }

    #[test]
    fn rejects_things_that_are_not_remotes() {
        for url in [
            "",
            "/home/dev/api",
            "file:///srv/git/api.git",
            "C:\\code\\api",
            "api",
        ] {
            assert_eq!(normalize_remote(url), None, "{url}");
        }
    }

    #[test]
    fn never_keeps_credentials() {
        let key =
            normalize_remote("https://x-access-token:ghp_abc123@github.com/acme/api").unwrap();
        assert!(!key.contains("ghp_abc123") && !key.contains('@'), "{key}");
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn reads_origin_from_a_checkout_and_from_a_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("api");
        write(
            &repo.join(".git/config"),
            "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = https://example.com/up/api\n\
             [remote \"origin\"]\n\turl = git@github.com:acme/api.git\n",
        );
        std::fs::create_dir_all(repo.join("src/deep")).unwrap();
        let want = Some("github.com/acme/api".to_string());
        assert_eq!(repo_key_for(repo.to_str().unwrap()), want);
        assert_eq!(repo_key_for(repo.join("src/deep").to_str().unwrap()), want);
    }

    #[test]
    fn a_worktree_shares_its_repository_key() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("api");
        write(
            &repo.join(".git/config"),
            "[remote \"origin\"]\n\turl = https://github.com/acme/api.git\n",
        );
        // A linked worktree: `.git` is a file pointing into the main git dir,
        // which names the shared dir in `commondir`.
        let wt_git = repo.join(".git/worktrees/feature");
        write(&wt_git.join("commondir"), "../..\n");
        let wt = tmp.path().join("api-feature");
        write(&wt.join(".git"), &format!("gitdir: {}\n", wt_git.display()));

        assert_eq!(
            repo_key_for(wt.to_str().unwrap()),
            Some("github.com/acme/api".to_string())
        );
    }

    #[test]
    fn falls_back_to_a_device_local_path_key() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("scratch");
        std::fs::create_dir_all(&dir).unwrap();
        let key = repo_key_for(dir.to_str().unwrap()).unwrap();
        assert!(key.starts_with(PATH_PREFIX), "{key}");
        // A project path that no longer exists still gets a key.
        assert_eq!(
            repo_key_for("/no/such/dir/anywhere"),
            Some("path:/no/such/dir/anywhere".to_string())
        );
        assert_eq!(repo_key_for("  "), None);
    }

    #[test]
    fn labels_are_short_and_readable() {
        assert_eq!(repo_label("github.com/acme/api"), "acme/api");
        assert_eq!(repo_label("path:/home/dev/scratch"), "scratch");
        assert_eq!(repo_label("root:0123456789abcdef"), "repo 01234567");
    }
}
