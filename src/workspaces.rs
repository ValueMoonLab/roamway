use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use std::hash::{BuildHasher, Hash, Hasher};

/// 描述一次 worktree 创建请求，对应 `--worktree-slug/--new-branch/--base/--branch`。
/// `branch` 兼作分支名；`existing=true` 表示检出已有分支（对应 `--branch`）。
/// `existing=false` 时 `branch` 为 `--new-branch` 指定值，未指定时回落 `rw/<agent_id>`。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct WorktreeSpec {
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub existing: bool,
}

/// run 上记录的 worktree 创建结果，供清理与 inspect 反查。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

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

/// worktree 目录：给定 `--worktree-slug` 时用 slug，否则回落到 agent_id。
pub fn worktree_path_named(data_dir: &Path, agent_id: &str, slug: Option<&str>) -> PathBuf {
    match slug {
        Some(s) if !s.is_empty() => worktrees_dir(data_dir).join(s),
        _ => worktree_path(data_dir, agent_id),
    }
}

pub fn worktree_branch(agent_id: &str) -> String {
    format!("rw/{}", agent_id)
}

/// `--worktree-slug` 必须匹配 `[a-z0-9][a-z0-9-]{0,39}`（1..=40 字符），且首字符不能是 `-`。
pub fn validate_worktree_slug(slug: &str) -> Result<(), String> {
    let ok = !slug.is_empty()
        && slug.len() <= 40
        && slug.bytes().enumerate().all(|(i, b)| {
            let alnum = b.is_ascii_lowercase() || b.is_ascii_digit();
            if i == 0 {
                alnum
            } else {
                alnum || b == b'-'
            }
        });
    if ok {
        Ok(())
    } else {
        Err("run --worktree-slug: expected [a-z0-9-], max 40".to_string())
    }
}

/// `--new-branch` 必须匹配 `[A-Za-z0-9._/-]{1,100}`，且不以 `-`/`/` 开头或结尾（对齐 git check-ref-format 语义）。
pub fn validate_new_branch_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 100
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'))
        && !name.starts_with('-')
        && !name.starts_with('/')
        && !name.ends_with('-')
        && !name.ends_with('/');
    if ok {
        Ok(())
    } else {
        Err(format!("run --new-branch: invalid branch name: {}", name))
    }
}

/// 把 4 个 flag 收敛成 WorktreeSpec；`--branch` 与 `--new-branch`/`--base` 互斥。
/// `--new-branch` 走新建分支（existing=false），`--branch` 走已有分支（existing=true）。
pub fn spec_from_parts(
    slug: Option<String>,
    new_branch: Option<String>,
    base: Option<String>,
    branch: Option<String>,
) -> Result<WorktreeSpec, String> {
    if branch.is_some() && (new_branch.is_some() || base.is_some()) {
        return Err("run --branch conflicts with --new-branch/--base".to_string());
    }
    if let Some(s) = slug.as_deref() {
        validate_worktree_slug(s)?;
    }
    if let Some(n) = new_branch.as_deref() {
        validate_new_branch_name(n)?;
    }
    let (branch, existing) = if let Some(b) = branch {
        (Some(b), true)
    } else {
        (new_branch, false)
    };
    Ok(WorktreeSpec {
        slug,
        branch,
        base,
        existing,
    })
}

/// 在创建 worktree 之前校验 slug / new-branch 的形态，git 侧校验在 `validate_worktree_git`。
pub fn validate_worktree_spec(spec: &WorktreeSpec) -> Result<(), String> {
    if let Some(s) = spec.slug.as_deref() {
        validate_worktree_slug(s)?;
    }
    if !spec.existing {
        if let Some(n) = spec.branch.as_deref() {
            validate_new_branch_name(n)?;
        }
    }
    Ok(())
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

/// 构造 `git worktree add <path> -b <branch> [<base>]` 的 argv，后续由 `git -C <root>` 前缀执行。
pub fn worktree_add_argv(path: &Path, branch: &str, base: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        "worktree".to_string(),
        "add".to_string(),
        path.to_string_lossy().to_string(),
        "-b".to_string(),
        branch.to_string(),
    ];
    if let Some(b) = base {
        argv.push(b.to_string());
    }
    argv
}

/// 创建 worktree：`git -C <root> worktree add <path> -b <branch> [<base>]`，缺省 base 为 HEAD。
pub fn worktree_add_from(
    repo_root: &str,
    path: &Path,
    branch: &str,
    base: Option<&str>,
) -> Result<(), String> {
    let argv = worktree_add_argv(path, branch, base);
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(&argv)
        .output()
        .map_err(|e| format!("git worktree add failed: {}", e))?;
    if !out.status.success() {
        return Err(format!("git worktree add failed: {}", stderr_text(&out)));
    }
    Ok(())
}

pub fn branch_exists(repo_root: &str, branch: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["show-ref", "--verify", "--quiet"])
        .arg(format!("refs/heads/{}", branch))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// `git rev-parse --verify <ref>^{commit}` 解析 commit，失败返回 None。
pub fn resolve_ref_commit(repo_root: &str, r: &str) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["rev-parse", "--verify"])
        .arg(format!("{}^{{commit}}", r))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if hash.is_empty() {
        None
    } else {
        Some(hash)
    }
}

/// slug 是否已被占用：分支同名，或 git worktree 列表中已有同名目录。
pub fn worktree_slug_taken(repo_root: &str, slug: &str) -> bool {
    if branch_exists(repo_root, slug) {
        return true;
    }
    let out = match Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["worktree", "list", "--porcelain"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return false,
    };
    String::from_utf8_lossy(&out.stdout).lines().any(|line| {
        line.strip_prefix("worktree ")
            .and_then(|p| std::path::Path::new(p).file_name())
            .and_then(|n| n.to_str())
            == Some(slug)
    })
}

/// 创建 worktree 前的 git 侧校验：slug 未占用 / 已有分支存在 / base 可解析。
pub fn validate_worktree_git(repo_root: &str, spec: &WorktreeSpec) -> Result<(), String> {
    if let Some(slug) = spec.slug.as_deref() {
        if worktree_slug_taken(repo_root, slug) {
            return Err(format!("run --worktree-slug: already exists: {}", slug));
        }
    }
    if spec.existing {
        if let Some(branch) = spec.branch.as_deref() {
            if !branch_exists(repo_root, branch) {
                return Err(format!("run --branch: no such branch: {}", branch));
            }
        }
    }
    if let Some(base) = spec.base.as_deref() {
        if resolve_ref_commit(repo_root, base).is_none() {
            return Err(format!("run --base: unknown ref: {}", base));
        }
    }
    Ok(())
}

/// 生成删除前的 dry-run 行，打印删除 worktree 与分支的 git 命令。
pub fn worktree_remove_plan(repo_root: &str, path: &Path, branch: &str) -> Vec<String> {
    vec![
        format!(
            "[worktree] dry-run: git -C {} worktree remove --force {}",
            repo_root,
            path.display()
        ),
        format!(
            "[worktree] dry-run: git -C {} branch -D {}",
            repo_root, branch
        ),
    ]
}

/// Remove a per-agent worktree and its branch. Prints a dry-run line before each
/// destructive command and propagates failures (never silently swallowed).
pub fn worktree_remove(repo_root: &str, path: &Path, branch: &str) -> Result<(), String> {
    for line in worktree_remove_plan(repo_root, path, branch) {
        println!("{}", line);
    }
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

    #[test]
    fn worktree_spec_parses_slug_branch_base() {
        // 第 1 组：slug
        let spec = spec_from_parts(Some("p16-a".into()), None, None, None).unwrap();
        assert_eq!(spec.slug.as_deref(), Some("p16-a"));
        assert_eq!(spec.branch, None);
        assert_eq!(spec.base, None);
        assert!(!spec.existing);

        // 第 2 组：new-branch + base
        let spec = spec_from_parts(None, Some("feat/p16".into()), Some("base1".into()), None).unwrap();
        assert_eq!(spec.branch.as_deref(), Some("feat/p16"));
        assert_eq!(spec.base.as_deref(), Some("base1"));
        assert!(!spec.existing);

        // 第 3 组：已有分支
        let spec = spec_from_parts(None, None, None, Some("main".into())).unwrap();
        assert_eq!(spec.branch.as_deref(), Some("main"));
        assert!(spec.existing);

        // 第 4 组：冲突与非法值
        let conflict = spec_from_parts(None, Some("feat".into()), Some("base1".into()), Some("main".into()))
            .unwrap_err();
        assert!(conflict.contains("conflicts"), "{}", conflict);
        let bad_slug = spec_from_parts(Some("Bad_Slug".into()), None, None, None).unwrap_err();
        assert!(bad_slug.contains("expected [a-z0-9-]"), "{}", bad_slug);
        let bad_branch = spec_from_parts(None, Some("-bad".into()), None, None).unwrap_err();
        assert!(bad_branch.contains("invalid branch name"), "{}", bad_branch);
    }

    #[test]
    fn worktree_add_from_argv_includes_base() {
        let argv = worktree_add_argv(Path::new("/tmp/wt"), "feat/p16", Some("base1"));
        assert_eq!(argv, vec!["worktree", "add", "/tmp/wt", "-b", "feat/p16", "base1"]);
        let no_base = worktree_add_argv(Path::new("/tmp/wt"), "feat/p16", None);
        assert_eq!(no_base, vec!["worktree", "add", "/tmp/wt", "-b", "feat/p16"]);
    }

    #[test]
    fn worktree_path_named_uses_slug_and_falls_back_to_agent_id() {
        let dir = temp_dir("wt-named");
        let with_slug = worktree_path_named(&dir, "ag_1234", Some("p16-a"));
        assert!(with_slug.starts_with(worktrees_dir(&dir)));
        assert_eq!(with_slug.file_name().and_then(|n| n.to_str()), Some("p16-a"));
        let fallback = worktree_path_named(&dir, "ag_1234", None);
        assert_eq!(fallback, worktree_path(&dir, "ag_1234"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn worktree_remove_uses_recorded_path() {
        let info = WorktreeInfo {
            path: "/data/worktrees/p16-a".into(),
            branch: "feat/p16".into(),
            base: Some("base1".into()),
        };
        let lines = worktree_remove_plan("/tmp/repo", Path::new(&info.path), &info.branch);
        assert!(lines[0].contains("/data/worktrees/p16-a"), "{}", lines[0]);
        assert!(lines[1].contains("feat/p16"), "{}", lines[1]);
    }
}