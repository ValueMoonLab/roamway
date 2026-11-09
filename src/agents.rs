use crate::types::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::hash::{BuildHasher, Hash, Hasher};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Msg {
    pub role: String,
    pub content: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Agent {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub model: String,
    pub workspace: String,
    pub session_id: String,
    pub state: String,
    pub parent: String,
    pub history: Vec<Msg>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_run_id: String,
    pub last_error: String,
    #[serde(default)]
    pub worktree: bool,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub base_path: Option<String>,
}

static AGENT_SEQ: AtomicU64 = AtomicU64::new(0);

pub fn gen_agent_id() -> String {
    let n = AGENT_SEQ.fetch_add(1, Ordering::SeqCst);
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    n.hash(&mut hasher);
    t.hash(&mut hasher);
    let h = hasher.finish();
    let seed = (n.wrapping_add(t).wrapping_add(h)) as u32;
    format!("ag_{:08x}", seed)
}

pub fn gen_session_id() -> String {
    let t = now() as u64;
    let n = AGENT_SEQ.fetch_add(1, Ordering::SeqCst);
    let v = (t.rotate_left(17) ^ n.wrapping_mul(0x9e37_79b9)) & 0xffff_ffff_ffff_ffff;
    format!("claude_{:016x}", v)
}

pub fn title_from_prompt(prompt: &str) -> String {
    let mut t: String = prompt.trim().chars().take(60).collect();
    if t.is_empty() {
        t = "agent".into();
    }
    t
}

/// codex / claude 有 provider 侧会话；shell / api:* 没有可 resume 的会话 id
pub fn is_resumable_kind(kind: &str) -> bool {
    matches!(kind, "codex" | "claude")
}

pub fn cap_history(agent: &mut Agent) {
    let max = if agent.kind.starts_with("api:") { 40 } else { 20 };
    if agent.history.len() > max {
        let drop = agent.history.len() - max;
        agent.history.drain(..drop);
    }
}

pub fn push_msg(agent: &mut Agent, role: &str, content: &str) {
    agent
        .history
        .push(Msg { role: role.to_string(), content: content.to_string() });
    cap_history(agent);
}

pub fn transition(agent: &mut Agent, next: &str) {
    agent.state = next.to_string();
    agent.updated_at = now();
}

pub fn detach(agent: &mut Agent) -> bool {
    let had_parent = !agent.parent.is_empty();
    agent.parent.clear();
    if had_parent {
        agent.updated_at = now();
    }
    had_parent
}

pub fn record_session_id(agent: &mut Agent, sid: &str) -> bool {
    if is_resumable_kind(&agent.kind) && !sid.trim().is_empty() {
        agent.session_id = sid.trim().to_string();
        agent.updated_at = now();
        true
    } else {
        false
    }
}

pub fn apply_run_status_to_agent(agent: &mut Agent, run_id: &str, ok: bool, err: Option<&str>) {
    agent.last_run_id = run_id.to_string();
    if ok {
        transition(agent, "idle");
        agent.last_error.clear();
    } else {
        transition(agent, "error");
        if let Some(e) = err {
            agent.last_error = e.to_string();
        }
    }
}

pub fn filter_visible(agents: &[Agent], all: bool) -> Vec<&Agent> {
    agents
        .iter()
        .filter(|a| all || a.state != "archived")
        .collect()
}

fn canonicalize_dir(path: &str) -> Option<String> {
    if !Path::new(path).is_dir() {
        return None;
    }
    fs::canonicalize(path).ok().map(|p| p.to_string_lossy().to_string())
}

fn default_workspace(workspace: Option<String>) -> Result<String, String> {
    match workspace.filter(|w| !w.trim().is_empty()) {
        Some(w) => canonicalize_dir(&w).ok_or_else(|| "workspace not found".to_string()),
        None => Ok(std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".into())),
    }
}

fn push_run_locked(
    st: &mut Store,
    agent: &mut Agent,
    prompt: &str,
    timeout_s: u64,
) -> Result<Run, String> {
    let id = st.next_id("run");
    let run = Run {
        id: id.clone(),
        title: agent.title.clone(),
        agent: agent.kind.clone(),
        model: if agent.model.is_empty() {
            None
        } else {
            Some(agent.model.clone())
        },
        workspace: agent.workspace.clone(),
        prompt: prompt.to_string(),
        status: RunStatus::Queued,
        created_at: now(),
        started_at: None,
        ended_at: None,
        exit_code: None,
        error: None,
        timeout_s,
        job_id: None,
        pid: None,
        attempts: 0,
        agent_id: Some(agent.id.clone()),
    };
    st.state.runs.push(run.clone());
    agent.last_run_id = id;
    agent.state = "running".into();
    agent.updated_at = now();
    agent.last_error.clear();
    push_msg(agent, "user", prompt);
    Ok(run)
}

pub fn create_agent_and_run(
    app: &Arc<App>,
    kind: &str,
    model: Option<String>,
    workspace: Option<String>,
    title: Option<String>,
    prompt: &str,
    timeout_s: u64,
    use_worktree: bool,
) -> Result<(Agent, Run), String> {
    if !is_known_agent(kind) {
        return Err("unknown agent".into());
    }
    if prompt.trim().is_empty() {
        return Err("prompt is required".into());
    }
    let ws = default_workspace(workspace)?;
    let data_dir = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        st.dir.clone()
    };
    let id = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        let mut id = gen_agent_id();
        while st.state.agents.iter().any(|a| a.id == id) {
            id = gen_agent_id();
        }
        id
    };
    let (agent_workspace, worktree, branch, base_path) = if use_worktree {
        let root = crate::workspaces::git_repo_root(&ws)?;
        let path = crate::workspaces::worktree_path(&data_dir, &id);
        let branch = crate::workspaces::worktree_branch(&id);
        crate::workspaces::worktree_add(&root, &path, &branch)?;
        (
            path.to_string_lossy().to_string(),
            true,
            Some(branch),
            Some(root),
        )
    } else {
        (ws, false, None, None)
    };
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let mut agent = Agent {
        id: id.clone(),
        title: title.unwrap_or_else(|| title_from_prompt(prompt)),
        kind: kind.to_string(),
        model: model.unwrap_or_default(),
        workspace: agent_workspace,
        session_id: String::new(),
        state: "running".into(),
        parent: String::new(),
        history: Vec::new(),
        created_at: now(),
        updated_at: now(),
        last_run_id: String::new(),
        last_error: String::new(),
        worktree,
        branch,
        base_path,
    };
    let run = push_run_locked(&mut st, &mut agent, prompt, timeout_s)?;
    st.state.agents.push(agent.clone());
    st.save();
    drop(st);
    crate::jobs::drain_queued(app);
    Ok((agent, run))
}

pub fn queue_agent_run(
    app: &Arc<App>,
    agent_id: &str,
    prompt: &str,
    timeout_s: u64,
) -> Result<Run, String> {
    if prompt.trim().is_empty() {
        return Err("prompt is required".into());
    }
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let mut agent = st
        .state
        .agents
        .iter()
        .find(|a| a.id == agent_id)
        .cloned()
        .ok_or_else(|| "no such agent".to_string())?;
    if agent.state == "archived" {
        return Err("agent is archived".into());
    }
    let run = push_run_locked(&mut st, &mut agent, prompt, timeout_s)?;
    if let Some(a) = st.state.agents.iter_mut().find(|a| a.id == agent_id) {
        *a = agent.clone();
    }
    st.save();
    drop(st);
    crate::jobs::drain_queued(app);
    Ok(run)
}

pub fn reload_agent(app: &Arc<App>, agent_id: &str, timeout_s: u64) -> Result<Run, String> {
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let mut agent = st
        .state
        .agents
        .iter()
        .find(|a| a.id == agent_id)
        .cloned()
        .ok_or_else(|| "no such agent".to_string())?;
    if agent.state == "archived" {
        return Err("agent is archived".into());
    }
    let first_prompt = agent
        .history
        .iter()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone())
        .ok_or_else(|| "no prompt to reload".to_string())?;
    agent.session_id.clear();
    agent.history.clear();
    agent.last_error.clear();
    let run = push_run_locked(&mut st, &mut agent, &first_prompt, timeout_s)?;
    if let Some(a) = st.state.agents.iter_mut().find(|a| a.id == agent_id) {
        *a = agent.clone();
    }
    st.save();
    drop(st);
    crate::jobs::drain_queued(app);
    Ok(run)
}

pub fn update_agent_meta(
    app: &Arc<App>,
    agent_id: &str,
    title: Option<String>,
    model: Option<String>,
    workspace: Option<String>,
) -> Result<Agent, String> {
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let a = st
        .state
        .agents
        .iter_mut()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| "no such agent".to_string())?;
    if let Some(t) = title {
        a.title = t;
    }
    if let Some(m) = model {
        a.model = m;
    }
    if let Some(w) = workspace {
        a.workspace = default_workspace(Some(w))?;
    }
    a.updated_at = now();
    let out = a.clone();
    st.save();
    Ok(out)
}

pub fn archive_agent(app: &Arc<App>, agent_id: &str, keep_worktree: bool) -> Result<(), String> {
    let (cleanup, data_dir) = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        let a = st
            .state
            .agents
            .iter()
            .find(|a| a.id == agent_id)
            .ok_or_else(|| "no such agent".to_string())?;
        (
            !keep_worktree
                && a.worktree
                && a.branch.is_some()
                && a.base_path.is_some()
                && !a.workspace.is_empty(),
            st.dir.clone(),
        )
    };
    if cleanup {
        let (branch, base_path, workspace) = {
            let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
            let a = st.state.agents.iter().find(|a| a.id == agent_id).unwrap();
            (
                a.branch.clone().unwrap_or_default(),
                a.base_path.clone().unwrap_or_default(),
                a.workspace.clone(),
            )
        };
        let wt = crate::workspaces::worktree_path(&data_dir, agent_id);
        let path = std::path::Path::new(&workspace);
        let path = if path.exists() { path.to_path_buf() } else { wt };
        if let Err(e) = crate::workspaces::worktree_remove(&base_path, &path, &branch) {
            return Err(format!("archive failed: {}", e));
        }
    }
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let a = st
        .state
        .agents
        .iter_mut()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| "no such agent".to_string())?;
    a.state = "archived".into();
    a.updated_at = now();
    st.save();
    Ok(())
}

pub fn detach_agent(app: &Arc<App>, agent_id: &str) -> Result<bool, String> {
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let a = st
        .state
        .agents
        .iter_mut()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| "no such agent".to_string())?;
    let had = detach(a);
    st.save();
    Ok(had)
}

pub fn import_agent(
    app: &Arc<App>,
    kind: &str,
    session: &str,
    title: Option<String>,
    model: Option<String>,
    workspace: Option<String>,
) -> Result<Agent, String> {
    if !is_known_agent(kind) {
        return Err("unknown agent".into());
    }
    if session.trim().is_empty() {
        return Err("session is required".into());
    }
    if !is_resumable_kind(kind) {
        return Err("session import requires codex or claude".into());
    }
    let ws = default_workspace(workspace)?;
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let mut id = gen_agent_id();
    while st.state.agents.iter().any(|a| a.id == id) {
        id = gen_agent_id();
    }
    let agent = Agent {
        id: id.clone(),
        title: title.unwrap_or_else(|| "imported session".into()),
        kind: kind.to_string(),
        model: model.unwrap_or_default(),
        workspace: ws,
        session_id: session.trim().to_string(),
        state: "idle".into(),
        parent: String::new(),
        history: Vec::new(),
        created_at: now(),
        updated_at: now(),
        last_run_id: String::new(),
        last_error: String::new(),
        worktree: false,
        branch: None,
        base_path: None,
    };
    st.state.agents.push(agent.clone());
    st.save();
    Ok(agent)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(kind: &str) -> Agent {
        Agent {
            id: "ag_00000001".into(),
            title: "t".into(),
            kind: kind.into(),
            model: String::new(),
            workspace: String::new(),
            session_id: String::new(),
            state: "idle".into(),
            parent: String::new(),
            history: Vec::new(),
            created_at: 0,
            updated_at: 0,
            last_run_id: String::new(),
            last_error: String::new(),
            worktree: false,
            branch: None,
            base_path: None,
        }
    }

    #[test]
    fn agent_id_unique() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            seen.insert(gen_agent_id());
        }
        assert_eq!(seen.len(), 1000);
    }

    #[test]
    fn title_from_prompt_truncates_60() {
        let long = "长".repeat(100);
        let t = title_from_prompt(&long);
        assert_eq!(t.chars().count(), 60);
        assert_eq!(title_from_prompt("hi"), "hi");
        assert_eq!(title_from_prompt("   "), "agent");
    }

    #[test]
    fn state_transitions_idle_running_idle() {
        let mut a = sample("shell");
        transition(&mut a, "running");
        assert_eq!(a.state, "running");
        transition(&mut a, "idle");
        assert_eq!(a.state, "idle");
    }

    #[test]
    fn archive_hides_from_default_list() {
        let mut a = sample("shell");
        a.state = "archived".into();
        let b = sample("codex");
        let agents = vec![a, b];
        assert_eq!(filter_visible(&agents, false).len(), 1);
        assert_eq!(filter_visible(&agents, true).len(), 2);
    }

    #[test]
    fn history_capped_at_20() {
        let mut a = sample("shell");
        for i in 0..25 {
            push_msg(&mut a, "user", &format!("m{}", i));
        }
        assert_eq!(a.history.len(), 20);
        assert_eq!(a.history.first().unwrap().content, "m5");
        assert_eq!(a.history.last().unwrap().content, "m24");
    }

    #[test]
    fn session_id_recorded_only_for_resumable_kinds() {
        let mut codex = sample("codex");
        assert!(record_session_id(&mut codex, "sid-1"));
        assert_eq!(codex.session_id, "sid-1");
        let mut api = sample("api:deepseek");
        assert!(!record_session_id(&mut api, "sid-2"));
        assert!(api.session_id.is_empty());
        let mut shell = sample("shell");
        assert!(!record_session_id(&mut shell, "sid-3"));
        assert!(shell.session_id.is_empty());
    }

    #[test]
    fn agent_roundtrip_in_state_json() {
        let mut a = sample("api:deepseek");
        push_msg(&mut a, "user", "hi");
        push_msg(&mut a, "assistant", "hello");
        let state = State {
            runs: vec![],
            jobs: vec![],
            agents: vec![a],
            seq: 3,
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: State = serde_json::from_str(&json).unwrap();
        assert_eq!(back.agents.len(), 1);
        assert_eq!(back.agents[0].history.len(), 2);

        // 旧 state.json（无 agents 字段）必须可加载，且已有 run 不丢
        let old = r#"{"runs":[{"id":"run_000001","title":"","agent":"shell","model":null,"workspace":"","prompt":"echo hi","status":"done","created_at":1,"started_at":null,"ended_at":null,"exit_code":0,"error":null,"timeout_s":60,"job_id":null,"pid":null,"attempts":0}],"jobs":[],"seq":1}"#;
        let legacy: State = serde_json::from_str(old).unwrap();
        assert!(legacy.agents.is_empty());
        assert_eq!(legacy.runs.len(), 1);
        assert_eq!(legacy.runs[0].agent_id, None);
    }

    #[test]
    fn detach_clears_parent() {
        let mut a = sample("shell");
        a.parent = "ag_parent".into();
        assert!(detach(&mut a));
        assert!(a.parent.is_empty());
        assert!(!detach(&mut a));
    }
}
