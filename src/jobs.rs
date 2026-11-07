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

pub fn start_scheduler(app: Arc<App>) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(1));
        for id in collect_due(&app) {
            spawn_from_job(app.clone(), &id);
        }
    });
}

fn collect_due(app: &Arc<App>) -> Vec<String> {
    let mut st = app.store.lock().unwrap();
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
    let run_id = {
        let mut st = app.store.lock().unwrap();
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
        });
        if let Some(jj) = st.state.jobs.iter_mut().find(|x| x.id == job_id) {
            jj.last_run = Some(iso(t));
        }
        prune(&mut st);
        st.save();
        id
    };
    runner::start(app, run_id);
}

/// 只保留最近 800 条运行记录，日志同步清理，避免长跑无限膨胀
fn prune(st: &mut Store) {
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
}
