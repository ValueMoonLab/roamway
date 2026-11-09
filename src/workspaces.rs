use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use std::hash::{BuildHasher, Hash, Hasher};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub path: String,
    pub remote: String,
    pub host: String,
    pub created_at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Registry {
    #[serde(skip)]
    pub path: PathBuf,
    pub workspaces: Vec<Workspace>,
}

static WS_SEQ: AtomicU64 = AtomicU64::new(0);

pub fn gen_workspace_id() -> String {
    let n = WS_SEQ.fetch_add(1, Ordering::SeqCst);
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    n.hash(&mut hasher);
    t.hash(&mut hasher);
    let h = hasher.finish();
    let seed = (n.wrapping_add(t).wrapping_add(h)) as u32;
    format!("ws_{:08x}", seed)
}

impl Registry {
    pub fn load(dir: &Path) -> Self {
        let _ = fs::create_dir_all(dir);
        let path = dir.join("workspaces.json");
        match fs::read(&path) {
            Ok(bytes) if bytes.is_empty() => Registry {
                path,
                workspaces: Vec::new(),
            },
            Ok(bytes) => match serde_json::from_slice::<Vec<Workspace>>(&bytes) {
                Ok(workspaces) => Registry { path, workspaces },
                Err(e) => {
                    eprintln!("roamway: workspaces.json parse failed: {}", e);
                    let ts = crate::types::now();
                    let corrupt = dir.join(format!("workspaces.json.corrupt-{}", ts));
                    let _ = fs::rename(&path, &corrupt);
                    Registry {
                        path,
                        workspaces: Vec::new(),
                    }
                }
            },
            Err(_) => Registry {
                path,
                workspaces: Vec::new(),
            },
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_file_name("workspaces.json.tmp");
        let data = serde_json::to_vec_pretty(&self.workspaces)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&data)?;
        f.sync_all()?;
        drop(f);
        #[cfg(windows)]
        if self.path.exists() {
            fs::remove_file(&self.path)?;
        }
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn add(&mut self, path: &Path, remote: &str) -> Result<Workspace, String> {
        let abs = fs::canonicalize(path).map_err(|e| format!("workspace not found: {}", e))?;
        let name = abs
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace")
            .to_string();
        self.add_named(&name, path, remote)
    }

    pub fn add_named(&mut self, name: &str, path: &Path, remote: &str) -> Result<Workspace, String> {
        let abs = fs::canonicalize(path).map_err(|e| format!("workspace not found: {}", e))?;
        let abs_path = abs.to_string_lossy().to_string();
        if name.trim().is_empty() {
            return Err("name is required".to_string());
        }
        if self.workspaces.iter().any(|w| w.name == name) {
            return Err(format!("workspace '{}' already registered", name));
        }
        if self.workspaces.iter().any(|w| w.path == abs_path) {
            return Err("workspace path already registered".to_string());
        }
        let ws = Workspace {
            id: gen_workspace_id(),
            name: name.to_string(),
            path: abs_path,
            remote: remote.to_string(),
            host: "local".to_string(),
            created_at: crate::types::now(),
        };
        self.workspaces.push(ws.clone());
        Ok(ws)
    }

    pub fn get(&self, name_or_path: &str) -> Option<&Workspace> {
        if let Some(w) = self.workspaces.iter().find(|w| w.name == name_or_path) {
            return Some(w);
        }
        let canon = fs::canonicalize(name_or_path).ok();
        self.workspaces.iter().find(|w| {
            w.path == name_or_path
                || canon
                    .as_ref()
                    .map(|c| c.to_string_lossy().to_string() == w.path)
                    .unwrap_or(false)
        })
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.workspaces.len();
        self.workspaces.retain(|w| w.name != name && w.path != name);
        self.workspaces.len() != before
    }
}

/// `owner/repo` shorthand or a full git url.
pub fn build_clone_url(input: &str, protocol: &str) -> Result<String, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("repo is required".into());
    }
    if input.contains("://") || input.starts_with("git@") {
        return Ok(input.to_string());
    }
    let parts: Vec<&str> = input.split('/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err("expected owner/repo or a git url".into());
    }
    let owner = parts[0];
    let repo = parts[1].trim_end_matches(".git");
    if repo.is_empty() {
        return Err("expected owner/repo or a git url".into());
    }
    match protocol {
        "https" => Ok(format!("https://github.com/{}/{}.git", owner, repo)),
        "ssh" => Ok(format!("git@github.com:{}/{}.git", owner, repo)),
        other => Err(format!("unknown protocol: {}", other)),
    }
}

/// Extract the repo basename (without trailing `.git`) from a clone url.
pub fn repo_name_from_url(url: &str) -> Option<String> {
    let s = url.trim().trim_end_matches('/');
    let last = s.rsplit('/').next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

pub fn cleanup_partial_dir(target: &Path) {
    if target.exists() {
        let _ = fs::remove_dir_all(target);
    }
}

fn git_clone(url: &str, target: &Path) -> Result<std::process::Output, String> {
    Command::new("git")
        .arg("clone")
        .args(["--depth", "0"])
        .arg(url)
        .arg(target)
        .output()
        .map_err(|e| format!("git clone failed: {}", e))
}

fn stderr_text(out: &std::process::Output) -> String {
    let s = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if s.is_empty() {
        "unknown error".to_string()
    } else {
        s
    }
}

/// Full clone into `<dir>/<repo basename>` (or `<data>/workspaces`), register on success.
pub fn clone_repo(
    data_dir: &Path,
    input: &str,
    dir: Option<&str>,
    protocol: &str,
) -> Result<Workspace, String> {
    let url = build_clone_url(input, protocol)?;
    let name = repo_name_from_url(&url).ok_or_else(|| "could not derive repo name".to_string())?;
    let parent = match dir.filter(|d| !d.trim().is_empty()) {
        Some(d) => PathBuf::from(d),
        None => data_dir.join("workspaces"),
    };
    let target = parent.join(&name);
    if target.exists() {
        return Err(format!("target already exists: {}", target.display()));
    }
    if let Err(e) = fs::create_dir_all(&parent) {
        return Err(format!("cannot create {}: {}", parent.display(), e));
    }
    match git_clone(&url, &target) {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            cleanup_partial_dir(&target);
            return Err(format!("git clone failed: {}", stderr_text(&out)));
        }
        Err(e) => {
            cleanup_partial_dir(&target);
            return Err(e);
        }
    }
    let mut reg = Registry::load(data_dir);
    let ws = reg.add(&target, &url)?;
    reg.save()
        .map_err(|e| format!("save workspaces.json failed: {}", e))?;
    Ok(ws)
}

pub fn worktrees_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("worktrees")
}

pub fn worktree_path(data_dir: &Path, agent_id: &str) -> PathBuf {
    worktrees_dir(data_dir).join(agent_id)
}

pub fn worktree_branch(agent_id: &str) -> String {
    format!("rw/{}", agent_id)
}

pub fn git_repo_root(workspace: &str) -> Result<String, String> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(workspace)
        .output()
        .map_err(|_| "workspace is not a git repo; drop --worktree".to_string())?;
    if !out.status.success() {
        return Err("workspace is not a git repo; drop --worktree".into());
    }
    let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if root.is_empty() {
        return Err("workspace is not a git repo; drop --worktree".into());
    }
    Ok(root)
}

pub fn worktree_add(repo_root: &str, path: &Path, branch: &str) -> Result<(), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["worktree", "add"])
        .arg(path)
        .args(["-b", branch])
        .output()
        .map_err(|e| format!("git worktree add failed: {}", e))?;
    if !out.status.success() {
        return Err(format!("git worktree add failed: {}", stderr_text(&out)));
    }
    Ok(())
}

fn branch_exists(repo_root: &str, branch: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["show-ref", "--verify", "--quiet"])
        .arg(format!("refs/heads/{}", branch))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Remove a per-agent worktree and its branch. Prints a dry-run line before each
/// destructive command and propagates failures (never silently swallowed).
pub fn worktree_remove(repo_root: &str, path: &Path, branch: &str) -> Result<(), String> {
    println!(
        "[worktree] dry-run: git -C {} worktree remove --force {}",
        repo_root,
        path.display()
    );
    println!(
        "[worktree] dry-run: git -C {} branch -D {}",
        repo_root, branch
    );
    if path.exists() {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args(["worktree", "remove", "--force"])
            .arg(path)
            .output()
            .map_err(|e| format!("git worktree remove failed: {}", e))?;
        if !out.status.success() {
            return Err(format!("git worktree remove failed: {}", stderr_text(&out)));
        }
    }
    if branch_exists(repo_root, branch) {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args(["branch", "-D", branch])
            .output()
            .map_err(|e| format!("git branch -D failed: {}", e))?;
        if !out.status.success() {
            return Err(format!("git branch -D failed: {}", stderr_text(&out)));
        }
    }
    Ok(())
}


pub fn worktree_add_existing(repo_root: &str, path: &Path, branch: &str) -> Result<(), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["worktree", "add"])
        .arg(path)
        .arg(branch)
        .output()
        .map_err(|e| format!("git worktree add failed: {}", e))?;
    if !out.status.success() {
        return Err(format!("git worktree add failed: {}", stderr_text(&out)));
    }
    Ok(())
}

pub fn worktree_remove_keep_branch(repo_root: &str, path: &Path) -> Result<(), String> {
    println!(
        "[worktree] dry-run: git -C {} worktree remove --force {}",
        repo_root,
        path.display()
    );
    if path.exists() {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args(["worktree", "remove", "--force"])
            .arg(path)
            .output()
            .map_err(|e| format!("git worktree remove failed: {}", e))?;
        if !out.status.success() {
            return Err(format!("git worktree remove failed: {}", stderr_text(&out)));
        }
    }
    Ok(())
}

pub fn git_current_branch(repo_root: &str) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .map_err(|e| format!("git rev-parse failed: {}", e))?;
    if !out.status.success() {
        return Err(format!("git rev-parse failed: {}", stderr_text(&out)));
    }
    let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if branch.is_empty() {
        return Err("cannot resolve current branch".into());
    }
    Ok(branch)
}

pub fn gh_pr_checkout(workspace: &str, pr: &str) -> Result<(), String> {
    let out = Command::new("gh")
        .args(["pr", "checkout", pr])
        .current_dir(workspace)
        .output()
        .map_err(|_| "checkout-pr requires gh in PATH".to_string())?;
    if !out.status.success() {
        return Err(format!("gh pr checkout failed: {}", stderr_text(&out)));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("roamway-ws-{}-{}", tag, crate::types::now()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn clone_url_build_https_and_ssh() {
        assert_eq!(
            build_clone_url("valuemoonlab/roamway", "https").unwrap(),
            "https://github.com/valuemoonlab/roamway.git"
        );
        assert_eq!(
            build_clone_url("valuemoonlab/roamway", "ssh").unwrap(),
            "git@github.com:valuemoonlab/roamway.git"
        );
        assert_eq!(
            build_clone_url("owner/repo.git", "https").unwrap(),
            "https://github.com/owner/repo.git"
        );
        assert_eq!(
            build_clone_url("https://example.com/org/repo.git", "https").unwrap(),
            "https://example.com/org/repo.git"
        );
        assert_eq!(
            build_clone_url("git@github.com:org/repo.git", "https").unwrap(),
            "git@github.com:org/repo.git"
        );
        assert!(build_clone_url("nope", "https").is_err());
        assert!(build_clone_url("a/b/c", "https").is_err());
        assert!(build_clone_url("owner/repo", "ftp").is_err());
    }

    #[test]
    fn clone_rejects_partial_dir_on_failure() {
        let dir = temp_dir("clone-fail");
        // invalid shorthand is rejected before any directory is materialized
        let res = clone_repo(&dir, "not-a-repo", None, "https");
        assert!(res.is_err());
        assert!(!dir.join("workspaces").join("not-a-repo").exists());
        // partial directory cleanup removes whatever git left behind
        let partial = dir.join("partial");
        fs::create_dir_all(&partial).unwrap();
        fs::write(partial.join("junk"), b"x").unwrap();
        cleanup_partial_dir(&partial);
        assert!(!partial.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_registry_roundtrip() {
        let dir = temp_dir("roundtrip");
        let sub = dir.join("repo");
        fs::create_dir_all(&sub).unwrap();
        let mut reg = Registry::load(&dir);
        let ws = reg.add(&sub, "https://github.com/o/r.git").unwrap();
        assert_eq!(ws.name, "repo");
        assert_eq!(ws.host, "local");
        assert!(ws.id.starts_with("ws_"));
        reg.save().unwrap();
        let reg2 = Registry::load(&dir);
        assert_eq!(reg2.workspaces.len(), 1);
        assert_eq!(reg2.workspaces[0].id, ws.id);
        assert_eq!(reg2.workspaces[0].path, ws.path);
        assert_eq!(reg2.workspaces[0].remote, ws.remote);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_lookup_by_name_and_path() {
        let dir = temp_dir("lookup");
        let sub = dir.join("repo");
        fs::create_dir_all(&sub).unwrap();
        let mut reg = Registry::load(&dir);
        let ws = reg.add(&sub, "git@github.com:o/r.git").unwrap();
        assert_eq!(reg.get("repo").map(|w| w.id.clone()), Some(ws.id.clone()));
        assert_eq!(
            reg.get(&ws.path).map(|w| w.id.clone()),
            Some(ws.id.clone())
        );
        assert!(reg.get("missing").is_none());
        assert!(reg.remove("repo"));
        assert!(reg.get("repo").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn worktree_path_is_under_data_dir() {
        let dir = temp_dir("wt-path");
        let p = worktree_path(&dir, "ag_1234");
        assert!(p.starts_with(worktrees_dir(&dir)));
        assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("ag_1234"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn worktree_branch_name_unique_per_agent() {
        let a = worktree_branch("ag_00000001");
        let b = worktree_branch("ag_00000002");
        assert_ne!(a, b);
        assert!(a.starts_with("rw/"));
        assert!(b.starts_with("rw/"));
    }
}