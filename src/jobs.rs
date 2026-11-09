use crate::runner;
use crate::types::*;
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

fn collect_due(app: &Arc<App>) -> Vec<String> {
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let t = now();
    let busy: Vec<String> = st
        .state
        .runs
        .iter()
        .filter(|r| !r.status.terminal())
        .filter_map(|r| r.job_id.clone())
        .collect();

    let mut due = Vec::new();
    let mut updates: Vec<(usize, i64)> = Vec::new();
    for (i, j) in st.state.jobs.iter().enumerate() {
        if !j.enabled || j.next_at > t {
            continue;
        }
        if busy.contains(&j.id) && !j.allow_overlap {
            updates.push((i, t + 5));
            continue;
        }
        updates.push((i, next_after(j, t, j.tz_offset_hours)));
        due.push(j.id.clone());
    }
    if !due.is_empty() {
        for (i, na) in updates {
            st.state.jobs[i].next_at = na;
        }
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
            timeout_s: j.timeout_s,
            job_id: Some(j.id.clone()),
            pid: None,
            attempts: 0,
            agent_id: None,
        });
        if let Some(jj) = st.state.jobs.iter_mut().find(|x| x.id == job_id) {
            jj.last_run = Some(iso(t));
        }
        prune(&mut st);
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

/// 只保留最近 800 条运行记录，日志同步清理，避免长跑无限膨胀
pub fn prune(st: &mut Store) {
    const MAX_RUNS: usize = 800;
    if st.state.runs.len() <= MAX_RUNS {
        return;
    }
    let mut terminal_idx: Vec<usize> = st
        .state
        .runs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.status.terminal())
        .map(|(i, _)| i)
        .collect();
    let excess = st.state.runs.len() - MAX_RUNS;
    terminal_idx.truncate(excess.min(terminal_idx.len()));
    for i in terminal_idx.into_iter().rev() {
        let r = st.state.runs.remove(i);
        let _ = std::fs::remove_file(st.log_path(&r.id));
    }
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
            timeout_s: 60,
            job_id: None,
            pid: None,
            attempts: 0,
            agent_id: None,
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

    #[test]
    fn prune_trims_terminal_runs() {
        let mk = |i: usize| Run {
            id: format!("run_{:06}", i),
            title: String::new(),
            agent: "shell".into(),
            model: None,
            workspace: String::new(),
            prompt: String::new(),
            status: RunStatus::Done,
            created_at: 0,
            started_at: None,
            ended_at: None,
            exit_code: None,
            error: None,
            timeout_s: 60,
            job_id: None,
            pid: None,
            attempts: 0,
            agent_id: None,
        };
        let dir = std::env::temp_dir().join(format!("roamway-prune-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let mut store = Store {
            dir,
            state: State {
                runs: (0..805).map(mk).collect(),
                jobs: vec![],
                agents: vec![],
                seq: 0,
            },
            loaded_ok: true,
        };
        prune(&mut store);
        assert_eq!(store.state.runs.len(), 800);
    }
}