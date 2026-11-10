use crate::config;
use crate::runner;
use crate::types::*;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// 指数退避，封顶 1 小时
pub fn backoff(attempts: u32) -> i64 {
    let n = attempts.clamp(1, 8);
    (30i64 << (n - 1)).min(3600)
}

pub fn next_after(j: &Job, t: i64, tz: i64) -> i64 {
    if let Some(secs) = j.every_secs {
        if secs > 0 {
            return t + secs as i64;
        }
    }
    if let Some(d) = j.daily.as_deref() {
        if let Some(at) = next_daily(t, d, tz) {
            return at;
        }
    }
    t + 86400
}

/// 把失败后的重试语义收敛到这一个纯函数里，方便单测。
/// max_retries=0 表示"不额外重试"，按原周期继续；>0 才做退避重试并最终自停。
pub fn apply_failure(j: &mut Job, t: i64, tz: i64, reason: &str) {
    j.attempts += 1;
    j.last_error = Some(reason.to_string());
    if j.attempts <= j.max_retries {
        j.next_at = t + backoff(j.attempts);
    } else {
        if j.max_retries > 0 {
            j.enabled = false;
        }
        j.attempts = 0;
        j.next_at = next_after(j, t, tz);
    }
}

pub fn start_scheduler(app: Arc<App>) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(1));
        for id in collect_due(&app) {
            spawn_from_job(app.clone(), &id);
        }
        drain_queued(&app);
    });
}

/// 依赖判定的纯逻辑：None = 全部满足；Some(reason) = 某前置最近一次运行未成功。
/// 「已满足」＝该前置最新一条 run（created_at 最大）存在且 status == done。
pub fn dep_block(state: &State, j: &Job) -> Option<String> {
    for dep in &j.depends_on {
        match latest_job_run(state, dep) {
            Some(r) if r.status == RunStatus::Done => {}
            Some(r) => return Some(format!("waiting for {} ({})", dep, status_word(r.status))),
            None => return Some(format!("waiting for {} (no runs)", dep)),
        }
    }
    None
}

/// 某任务最新的一条 run（按 created_at 取最大），用于依赖判定。
pub fn latest_job_run<'a>(state: &'a State, job_id: &str) -> Option<&'a Run> {
    state
        .runs
        .iter()
        .filter(|r| r.job_id.as_deref() == Some(job_id))
        .max_by_key(|r| r.created_at)
}

/// RunStatus 的小写名（与 serde rename_all="lowercase" 保持一致），
/// 用于 last_skip 的原因文案。
pub fn status_word(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Queued => "queued",
        RunStatus::Running => "running",
        RunStatus::Done => "done",
        RunStatus::Failed => "failed",
        RunStatus::Timeout => "timeout",
        RunStatus::Killed => "killed",
        RunStatus::Interrupted => "interrupted",
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum DueAction {
    /// 依赖已满足、无并发冲突 -> 触发
    Trigger,
    /// 同 id 有未终结 run 且不允许重叠 -> 5 秒后再看
    Busy,
    /// 依赖未满足 -> 推进到下一周期并记录原因（不做 5s 重试，避免空转）
    Skip(String),
}

/// 单个 job 本轮该怎么处置（纯函数，collect_due 与单测共用）。
pub fn due_action(state: &State, j: &Job, busy: bool) -> DueAction {
    if let Some(reason) = dep_block(state, j) {
        return DueAction::Skip(reason);
    }
    if busy && !j.allow_overlap {
        return DueAction::Busy;
    }
    DueAction::Trigger
}

/// 纯逻辑：算出本轮要触发的 job id，并把 next_at / last_skip 写好。
/// 返回 (due, changed)：changed 表示 state 被改过（调用方据此决定是否落盘）。
pub fn plan_and_apply(state: &mut State, t: i64) -> (Vec<String>, bool) {
    let busy: Vec<String> = state
        .runs
        .iter()
        .filter(|r| !r.status.terminal())
        .filter_map(|r| r.job_id.clone())
        .collect();

    let mut due = Vec::new();
    let mut changed = false;
    let n = state.jobs.len();
    for i in 0..n {
        if !state.jobs[i].enabled || state.jobs[i].next_at > t {
            continue;
        }
        // 依赖判定要看全局 runs，先把 job 拷出来，避免同时持有 jobs 的可变借用
        let j = state.jobs[i].clone();
        match due_action(state, &j, busy.contains(&j.id)) {
            DueAction::Trigger => {
                state.jobs[i].next_at = next_after(&j, t, j.tz_offset_hours);
                state.jobs[i].last_skip = None;
                due.push(j.id.clone());
                changed = true;
            }
            DueAction::Busy => {
                state.jobs[i].next_at = t + 5;
                state.jobs[i].last_skip = None;
                changed = true;
            }
            DueAction::Skip(reason) => {
                state.jobs[i].next_at = next_after(&j, t, j.tz_offset_hours);
                state.jobs[i].last_skip = Some(reason);
                changed = true;
            }
        }
    }
    (due, changed)
}

fn collect_due(app: &Arc<App>) -> Vec<String> {
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let t = now();
    let (due, changed) = plan_and_apply(&mut st.state, t);
    if changed {
        st.save();
    }
    due
}

pub fn spawn_from_job(app: Arc<App>, job_id: &str) {
    {
        let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        let t = now();
        let j = match st.state.jobs.iter().find(|j| j.id == job_id).cloned() {
            Some(j) => j,
            None => return,
        };
        let id = st.next_id("run");
        st.state.runs.push(Run {
            id: id.clone(),
            title: if j.title.is_empty() {
                format!("job:{}", j.id)
            } else {
                j.title.clone()
            },
            agent: j.agent.clone(),
            model: j.model.clone(),
            workspace: j.workspace.clone(),
            prompt: j.prompt.clone(),
            status: RunStatus::Queued,
            created_at: t,
            started_at: None,
            ended_at: None,
            exit_code: None,
            error: None,
            resumed: false,
            resume_note: None,
            timeout_s: j.timeout_s,
            job_id: Some(j.id.clone()),
            pid: None,
            attempts: 0,
            agent_id: None,
            trigger_id: None,
            usage: None,
            labels: BTreeMap::new(),
            env: Vec::new(),
            cwd: None,
            options: BTreeMap::new(),
            images: Vec::new(),
            worktree: None,
        });
        if let Some(jj) = st.state.jobs.iter_mut().find(|x| x.id == job_id) {
            jj.last_run = Some(iso(t));
        }
        prune(&mut st, app.keep_runs);
        st.save();
    };
    drain_queued(&app);
}

/// 每轮从 Queued 里拾取，直到跑满 max_concurrent；手动 run 与 job run 一视同仁。
/// 纯逻辑：在 max_concurrent 约束下，本轮应从 Queued 中拾取哪些 run。
fn queued_to_start(state: &State, max_concurrent: usize) -> Vec<String> {
    let running = state
        .runs
        .iter()
        .filter(|r| r.status == RunStatus::Running)
        .count();
    let mut slots = max_concurrent.saturating_sub(running);
    let mut ids = Vec::new();
    for r in state.runs.iter() {
        if slots == 0 {
            break;
        }
        if r.status == RunStatus::Queued {
            ids.push(r.id.clone());
            slots -= 1;
        }
    }
    ids
}

pub fn drain_queued(app: &Arc<App>) {
    let ids = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        queued_to_start(&st.state, app.max_concurrent)
    };
    for id in ids {
        runner::start(app.clone(), id);
    }
}

/// 与启动期同一套阈值：keep_runs=0 表示不修剪。删除 terminal run 时同步删日志文件。
/// 返回"是否有删除"。
pub fn prune(st: &mut Store, keep_runs: usize) -> bool {
    // 只删 terminal：threshold 与"是否可删"的判定全部收敛到 config::trim_runs_plan，
    // 日志文件删除后再由 config::trim_runs 处理向量，不引入第二套阈值。
    let removed = config::trim_runs_plan(&st.state.runs, keep_runs);
    if removed.is_empty() {
        return false;
    }
    for id in &removed {
        let _ = std::fs::remove_file(st.log_path(id));
    }
    config::trim_runs(&mut st.state.runs, keep_runs)
}

/// 一次修剪动作的统计结果，供 `prune --dry-run` 与 `/v1/prune` 使用。
pub struct PruneStats {
    pub runs_removed: usize,
    pub logs_removed: usize,
    pub bytes_freed: usize,
}

/// 计算并（dry_run=false 时）执行修剪。dry_run=true 只统计，不删 run、不删日志、不 save。
pub fn prune_stats(st: &mut Store, keep_runs: usize, dry_run: bool) -> PruneStats {
    let removed = config::trim_runs_plan(&st.state.runs, keep_runs);
    let mut stats = PruneStats {
        runs_removed: removed.len(),
        logs_removed: 0,
        bytes_freed: 0,
    };
    if dry_run {
        for id in &removed {
            let path = st.log_path(id);
            if let Ok(meta) = std::fs::metadata(&path) {
                stats.bytes_freed += meta.len() as usize;
                stats.logs_removed += 1;
            }
        }
        return stats;
    }
    let removed_set: std::collections::HashSet<String> = removed.iter().cloned().collect();
    st.state.runs.retain(|r| !removed_set.contains(&r.id));
    for id in &removed {
        let path = st.log_path(id);
        if let Ok(meta) = std::fs::metadata(&path) {
            stats.bytes_freed += meta.len() as usize;
        }
        if std::fs::remove_file(&path).is_ok() {
            stats.logs_removed += 1;
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_exponential_and_capped() {
        assert_eq!(backoff(1), 30);
        assert_eq!(backoff(2), 60);
        assert_eq!(backoff(3), 120);
        assert_eq!(backoff(9), 3600);
    }

    fn sample_job(max_retries: u32) -> Job {
        Job {
            id: "job_000001".into(),
            title: String::new(),
            agent: "shell".into(),
            model: None,
            workspace: String::new(),
            prompt: "echo hi".into(),
            every_secs: Some(3600),
            daily: None,
            tz_offset_hours: 8,
            timeout_s: 60,
            max_retries,
            enabled: true,
            allow_overlap: false,
            next_at: 1000,
            last_run: None,
            last_error: None,
            attempts: 0,
            depends_on: Vec::new(),
            last_skip: None,
        }
    }

    fn job_with(id: &str, deps: &[&str]) -> Job {
        let mut j = sample_job(0);
        j.id = id.into();
        j.depends_on = deps.iter().map(|s| s.to_string()).collect();
        j
    }

    fn run_for(job_id: &str, status: RunStatus, created_at: i64) -> Run {
        Run {
            id: format!("run_{}", created_at),
            title: String::new(),
            agent: "shell".into(),
            model: None,
            workspace: String::new(),
            prompt: String::new(),
            status,
            created_at,
            started_at: None,
            ended_at: None,
            exit_code: None,
            error: None,
            resumed: false,
            resume_note: None,
            timeout_s: 60,
            job_id: Some(job_id.into()),
            pid: None,
            attempts: 0,
            agent_id: None,
            trigger_id: None,
            usage: None,
            labels: BTreeMap::new(),
            env: Vec::new(),
            cwd: None,
            options: BTreeMap::new(),
            images: Vec::new(),
            worktree: None,
        }
    }

    #[test]
    fn max_retries_zero_means_no_extra_retry() {
        let mut j = sample_job(0);
        apply_failure(&mut j, 1000, 8, "failed");
        assert_eq!(j.attempts, 0);
        assert!(j.enabled);
        assert_eq!(j.next_at, 1000 + 3600);
    }

    #[test]
    fn max_retries_exhaustion_disables_job() {
        let mut j = sample_job(2);
        apply_failure(&mut j, 1000, 8, "timeout");
        assert_eq!(j.attempts, 1);
        assert_eq!(j.next_at, 1000 + 30);
        assert!(j.enabled);
        apply_failure(&mut j, 1030, 8, "timeout");
        assert_eq!(j.attempts, 2);
        assert_eq!(j.next_at, 1030 + 60);
        assert!(j.enabled);
        apply_failure(&mut j, 1090, 8, "timeout");
        assert_eq!(j.attempts, 0);
        assert!(!j.enabled);
        assert_eq!(j.next_at, 1090 + 3600);
    }

    #[test]
    fn queued_to_start_respects_max_concurrent() {
        let mk = |id: &str, status: RunStatus| Run {
            id: id.into(),
            title: String::new(),
            agent: "shell".into(),
            model: None,
            workspace: String::new(),
            prompt: String::new(),
            status,
            created_at: 0,
            started_at: None,
            ended_at: None,
            exit_code: None,
            error: None,
            resumed: false,
            resume_note: None,
            timeout_s: 60,
            job_id: None,
            pid: None,
            attempts: 0,
            agent_id: None,
            trigger_id: None,
            usage: None,
            labels: BTreeMap::new(),
            env: Vec::new(),
            cwd: None,
            options: BTreeMap::new(),
            images: Vec::new(),
            worktree: None,
        };
        let state = State {
            runs: vec![
                mk("a", RunStatus::Running),
                mk("b", RunStatus::Queued),
                mk("c", RunStatus::Queued),
                mk("d", RunStatus::Queued),
                mk("e", RunStatus::Queued),
            ],
            jobs: vec![],
            agents: vec![],
            seq: 0,
        };
        let picked = queued_to_start(&state, 2);
        assert_eq!(picked, vec!["b".to_string()]);
        let picked = queued_to_start(&state, 5);
        assert_eq!(picked, vec!["b".to_string(), "c".to_string(), "d".to_string(), "e".to_string()]);
    }

    fn mk_run(id: &str, status: RunStatus, created_at: i64) -> Run {
        Run {
            id: id.to_string(),
            title: String::new(),
            agent: "shell".into(),
            model: None,
            workspace: String::new(),
            prompt: String::new(),
            status,
            created_at,
            started_at: None,
            ended_at: None,
            exit_code: None,
            error: None,
            resumed: false,
            resume_note: None,
            timeout_s: 60,
            job_id: None,
            pid: None,
            attempts: 0,
            agent_id: None,
            trigger_id: None,
            usage: None,
            labels: BTreeMap::new(),
            env: Vec::new(),
            cwd: None,
            options: BTreeMap::new(),
            images: Vec::new(),
            worktree: None,
        }
    }

    fn store_with(dir: std::path::PathBuf, runs: Vec<Run>) -> Store {
        Store {
            dir,
            state: State {
                runs,
                jobs: vec![],
                agents: vec![],
                seq: 0,
            },
            loaded_ok: true,
        }
    }

    #[test]
    fn prune_uses_keep_runs_not_hardcoded() {
        let dir = std::env::temp_dir().join(format!("roamway-prune-keep-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let runs = (0..900)
            .map(|i| mk_run(&format!("run_{:06}", i), RunStatus::Done, 0))
            .collect();
        let mut st = store_with(dir.clone(), runs);
        assert!(prune(&mut st, 100));
        assert_eq!(st.state.runs.len(), 100);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_zero_keeps_everything() {
        let dir = std::env::temp_dir().join(format!("roamway-prune-zero-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let runs = (0..900)
            .map(|i| mk_run(&format!("run_{:06}", i), RunStatus::Done, 0))
            .collect();
        let mut st = store_with(dir.clone(), runs);
        assert!(!prune(&mut st, 0));
        assert_eq!(st.state.runs.len(), 900);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_never_drops_running_or_queued() {
        let dir = std::env::temp_dir().join(format!("roamway-prune-active-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let mut runs: Vec<Run> = (0..900)
            .map(|i| mk_run(&format!("run_{:06}", i), RunStatus::Done, i as i64))
            .collect();
        runs.push(mk_run("run_running", RunStatus::Running, 0));
        runs.push(mk_run("run_queued", RunStatus::Queued, 0));
        let mut st = store_with(dir.clone(), runs);
        assert!(prune(&mut st, 10));
        assert_eq!(st.state.runs.len(), 10);
        assert!(st.state.runs.iter().any(|r| r.id == "run_running"));
        assert!(st.state.runs.iter().any(|r| r.id == "run_queued"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_removes_log_files_of_dropped_runs() {
        let dir = std::env::temp_dir().join(format!("roamway-prune-logs-{}", now()));
        let _ = std::fs::create_dir_all(dir.join("logs"));
        let runs = vec![
            mk_run("run_old_1", RunStatus::Done, 1),
            mk_run("run_old_2", RunStatus::Done, 2),
            mk_run("run_keep", RunStatus::Done, 3),
        ];
        let mut st = store_with(dir.clone(), runs);
        for id in ["run_old_1", "run_old_2", "run_keep"] {
            std::fs::write(st.log_path(id), b"x").unwrap();
        }
        assert!(prune(&mut st, 1));
        assert_eq!(st.state.runs.len(), 1);
        assert_eq!(st.state.runs[0].id, "run_keep");
        assert!(!st.log_path("run_old_1").exists());
        assert!(!st.log_path("run_old_2").exists());
        assert!(st.log_path("run_keep").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dep_without_runs_skips_and_advances() {
        let b = job_with("job_B", &["job_A"]);
        let mut state = State {
            runs: vec![],
            jobs: vec![b],
            agents: vec![],
            seq: 0,
        };
        let (due, changed) = plan_and_apply(&mut state, 1000);
        assert!(due.is_empty(), "dep 无 run 时不能触发");
        assert!(changed);
        assert_eq!(state.jobs[0].next_at, 1000 + 3600, "next_at 照常推进，不做 5s 空转");
        assert_eq!(
            state.jobs[0].last_skip.as_deref(),
            Some("waiting for job_A (no runs)")
        );
    }

    #[test]
    fn dep_last_done_triggers_and_clears_skip() {
        let mut b = job_with("job_B", &["job_A"]);
        b.last_skip = Some("waiting for job_A (failed)".into());
        let mut state = State {
            runs: vec![run_for("job_A", RunStatus::Done, 900)],
            jobs: vec![b],
            agents: vec![],
            seq: 0,
        };
        let (due, _) = plan_and_apply(&mut state, 1000);
        assert_eq!(due, vec!["job_B".to_string()]);
        assert!(state.jobs[0].last_skip.is_none(), "满足后必须清空 last_skip");
        assert_eq!(state.jobs[0].next_at, 1000 + 3600);
    }

    #[test]
    fn dep_last_failed_blocks_even_if_earlier_done() {
        let mut b = job_with("job_B", &["job_A"]);
        b.last_skip = None;
        let mut state = State {
            runs: vec![
                run_for("job_A", RunStatus::Done, 900),
                run_for("job_A", RunStatus::Failed, 950),
            ],
            jobs: vec![b],
            agents: vec![],
            seq: 0,
        };
        let (due, _) = plan_and_apply(&mut state, 1000);
        assert!(due.is_empty());
        assert_eq!(
            state.jobs[0].last_skip.as_deref(),
            Some("waiting for job_A (failed)")
        );
    }

    #[test]
    fn all_deps_must_be_done() {
        let b = job_with("job_B", &["job_A", "job_C"]);
        let mut state = State {
            runs: vec![run_for("job_A", RunStatus::Done, 900)],
            jobs: vec![b],
            agents: vec![],
            seq: 0,
        };
        let (due, _) = plan_and_apply(&mut state, 1000);
        assert!(due.is_empty(), "两条依赖只满足一条不能触发");
        assert_eq!(
            state.jobs[0].last_skip.as_deref(),
            Some("waiting for job_C (no runs)")
        );
    }

    #[test]
    fn dep_block_reports_reason_per_status() {
        let j = job_with("job_B", &["job_A"]);
        let mut state = State {
            runs: vec![run_for("job_A", RunStatus::Running, 900)],
            jobs: vec![j.clone()],
            agents: vec![],
            seq: 0,
        };
        assert_eq!(
            dep_block(&state, &j).as_deref(),
            Some("waiting for job_A (running)")
        );
        state.runs[0].status = RunStatus::Timeout;
        assert_eq!(
            dep_block(&state, &j).as_deref(),
            Some("waiting for job_A (timeout)")
        );
        state.runs[0].status = RunStatus::Done;
        assert!(dep_block(&state, &j).is_none());
        // 没有依赖的 job 永远放行
        let plain = job_with("job_Z", &[]);
        assert!(dep_block(&state, &plain).is_none());
    }

    #[test]
    fn old_state_without_depends_on_still_loads() {
        let raw = r#"{"id":"job_000009","title":"legacy","agent":"shell",
            "workspace":"","prompt":"echo hi","every_secs":60,"tz_offset_hours":8,
            "timeout_s":60,"next_at":1000,"enabled":true}"#;
        let j: Job = serde_json::from_str(raw).expect("旧 state.json 必须能反序列化");
        assert!(j.depends_on.is_empty());
        assert!(j.last_skip.is_none());
        assert_eq!(j.title, "legacy");
        assert_eq!(j.next_at, 1000);
    }
}
