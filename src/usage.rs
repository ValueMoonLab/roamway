use crate::types::*;
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct AggRow {
    pub day: String,
    pub provider: String,
    pub model: String,
    pub runs: u64,
    pub in_tokens: u64,
    pub out_tokens: u64,
    pub total_tokens: u64,
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn parse_iso_to_ts(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let date = s.split('T').next().unwrap_or(s);
    let mut it = date.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok()?;
    let d: u32 = it.next()?.parse().ok()?;
    let days = days_from_civil(y, m, d);
    let secs = if let Some(rest) = s.split('T').nth(1) {
        let rest = rest.trim_end_matches('Z');
        let mut parts = rest.split(':');
        let h: i64 = parts.next().unwrap_or("0").parse().unwrap_or(0);
        let min: i64 = parts.next().unwrap_or("0").parse().unwrap_or(0);
        let sec: i64 = parts.next().unwrap_or("0").parse().unwrap_or(0);
        h * 3600 + min * 60 + sec
    } else {
        0
    };
    Some(days * 86400 + secs)
}

pub fn read_state(dir: &std::path::Path) -> State {
    match std::fs::read(dir.join("state.json")) {
        Ok(bytes) if bytes.is_empty() => State::default(),
        Ok(bytes) => match serde_json::from_slice::<State>(&bytes) {
            Ok(state) => state,
            Err(e) => {
                eprintln!("roamway: state.json parse failed: {}", e);
                State::default()
            }
        },
        Err(_) => State::default(),
    }
}

pub fn since_ts(spec: Option<&str>) -> Option<i64> {
    match spec.map(|s| s.trim()) {
        None | Some("") => Some(0),
        Some("7d") => Some(now() - 7 * 86400),
        Some("24h") => Some(now() - 86400),
        Some(s) => parse_iso_to_ts(s),
    }
}

fn usage_data(run: &Run) -> Option<&Usage> {
    match run.usage.as_ref()? {
        RunUsage::Data(u) => Some(u),
        RunUsage::Note(_) => None,
    }
}

fn completed_with_usage(state: &State, since: i64) -> Vec<&Run> {
    state
        .runs
        .iter()
        .filter(|r| r.status == RunStatus::Done)
        .filter(|r| r.created_at >= since)
        .filter(|r| usage_data(r).is_some())
        .collect()
}

pub fn aggregate(state: &State, since: i64, provider_filter: Option<&str>) -> Vec<AggRow> {
    let mut groups: std::collections::HashMap<(String, String, String), AggRow> =
        std::collections::HashMap::new();
    for run in completed_with_usage(state, since) {
        let u = usage_data(run).unwrap();
        let provider = if u.provider.is_empty() {
            run.agent.clone()
        } else {
            u.provider.clone()
        };
        if let Some(want) = provider_filter {
            if provider != want {
                continue;
            }
        }
        let model = if u.model.is_empty() {
            run.model.clone().unwrap_or_else(|| "-".into())
        } else {
            u.model.clone()
        };
        let day = iso(run.created_at).chars().take(10).collect::<String>();
        let key = (day.clone(), provider.clone(), model.clone());
        let row = groups.entry(key).or_insert_with(|| AggRow {
            day,
            provider,
            model,
            runs: 0,
            in_tokens: 0,
            out_tokens: 0,
            total_tokens: 0,
        });
        row.runs += 1;
        row.in_tokens += u.prompt_tokens;
        row.out_tokens += u.completion_tokens;
        row.total_tokens += u.total_tokens;
    }
    let mut rows: Vec<AggRow> = groups.into_values().collect();
    rows.sort_by(|a, b| {
        (&a.day, &a.provider, &a.model).cmp(&(&b.day, &b.provider, &b.model))
    });
    rows
}

pub fn inspect_lines(state: &State, id: &str) -> Option<Vec<String>> {
    let run = state.runs.iter().find(|r| r.id == id)?;
    let mut out = Vec::new();
    out.push(format!("run: {}", run.id));
    out.push(format!("agent: {}", run.agent));
    out.push(format!("status: {:?}", run.status));
    let duration = match (run.started_at, run.ended_at) {
        (Some(s), Some(e)) if e >= s => format!("{}s", e - s),
        _ => "unknown".into(),
    };
    out.push(format!("duration: {}", duration));
    out.push(format!(
        "retried: {} (attempts={})",
        if run.attempts > 0 { "yes" } else { "no" },
        run.attempts
    ));
    match run.usage.as_ref() {
        Some(RunUsage::Data(u)) => {
            out.push(format!(
                "model: {}",
                if u.model.is_empty() {
                    run.model.clone().unwrap_or_else(|| "-".into())
                } else {
                    u.model.clone()
                }
            ));
            out.push(format!(
                "provider: {}",
                if u.provider.is_empty() {
                    run.agent.clone()
                } else {
                    u.provider.clone()
                }
            ));
            out.push(format!(
                "tokens: in={} out={} total={}",
                u.prompt_tokens, u.completion_tokens, u.total_tokens
            ));
        }
        Some(RunUsage::Note(n)) => {
            out.push(format!("usage: {}", n));
        }
        None => out.push("usage: (none)".into()),
    }
    Some(out)
}

pub fn report_json(state: &State) -> Value {
    let runs = completed_with_usage(state, 0);
    let mut overview_runs = 0u64;
    let mut prompt = 0u64;
    let mut completion = 0u64;
    let mut total = 0u64;
    let mut by_provider: std::collections::HashMap<String, (u64, u64, u64)> =
        std::collections::HashMap::new();
    let mut by_model: std::collections::HashMap<String, (u64, u64, u64)> =
        std::collections::HashMap::new();
    for run in &runs {
        let u = usage_data(run).unwrap();
        overview_runs += 1;
        prompt += u.prompt_tokens;
        completion += u.completion_tokens;
        total += u.total_tokens;
        let provider = if u.provider.is_empty() {
            run.agent.clone()
        } else {
            u.provider.clone()
        };
        let model = if u.model.is_empty() {
            run.model.clone().unwrap_or_else(|| "-".into())
        } else {
            u.model.clone()
        };
        let pe = by_provider.entry(provider).or_insert((0, 0, 0));
        pe.0 += u.prompt_tokens;
        pe.1 += u.completion_tokens;
        pe.2 += u.total_tokens;
        let me = by_model.entry(model).or_insert((0, 0, 0));
        me.0 += u.prompt_tokens;
        me.1 += u.completion_tokens;
        me.2 += u.total_tokens;
    }
    let mut providers: Vec<Value> = by_provider
        .into_iter()
        .map(|(name, (i, o, t))| {
            json!({ "provider": name, "prompt_tokens": i, "completion_tokens": o, "total_tokens": t })
        })
        .collect();
    providers.sort_by(|a, b| b["total_tokens"].as_u64().cmp(&a["total_tokens"].as_u64()));
    let mut models: Vec<Value> = by_model
        .into_iter()
        .map(|(name, (i, o, t))| {
            json!({ "model": name, "prompt_tokens": i, "completion_tokens": o, "total_tokens": t })
        })
        .collect();
    models.sort_by(|a, b| b["total_tokens"].as_u64().cmp(&a["total_tokens"].as_u64()));
    let mut top: Vec<&Run> = runs;
    top.sort_by(|a, b| {
        let ta = usage_data(a).map(|u| u.total_tokens).unwrap_or(0);
        let tb = usage_data(b).map(|u| u.total_tokens).unwrap_or(0);
        tb.cmp(&ta)
    });
    let top = top
        .into_iter()
        .take(20)
        .map(|r| {
            let u = usage_data(r).unwrap();
            json!({
                "run_id": r.id,
                "model": u.model,
                "provider": u.provider,
                "total_tokens": u.total_tokens,
                "created_at": r.created_at,
            })
        })
        .collect::<Vec<Value>>();
    json!({
        "overview": { "runs": overview_runs, "prompt_tokens": prompt, "completion_tokens": completion, "total_tokens": total },
        "by_provider": providers,
        "by_model": models,
        "top_expensive": top,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_run(id: &str, day: i64, prompt: u64, completion: u64, provider: &str, model: &str) -> Run {
        Run {
            id: id.into(),
            title: String::new(),
            agent: provider.into(),
            model: Some(model.into()),
            workspace: String::new(),
            prompt: "hi".into(),
            status: RunStatus::Done,
            created_at: day,
            started_at: Some(day),
            ended_at: Some(day + 5),
            exit_code: Some(0),
            error: None,
            timeout_s: 60,
            job_id: None,
            pid: None,
            attempts: 0,
            agent_id: None,
            usage: Some(RunUsage::Data(Usage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: prompt + completion,
                model: model.into(),
                provider: provider.into(),
            })),
        }
    }

    #[test]
    fn iso_roundtrip_parses_known_epoch() {
        let ts = parse_iso_to_ts("2026-11-09T00:00:00Z").unwrap();
        assert!(iso(ts).starts_with("2026-11-09"));
        assert_eq!(parse_iso_to_ts("2026-11-09").unwrap(), ts);
        assert_eq!(parse_iso_to_ts("nope"), None);
    }

    #[test]
    fn aggregate_groups_by_day_provider_model() {
        let base = parse_iso_to_ts("2026-11-09T00:00:00Z").unwrap();
        let mut state = State::default();
        state.runs.push(mk_run("a", base, 10, 20, "api:deepseek", "deepseek-chat"));
        state.runs.push(mk_run("b", base + 10, 5, 5, "api:deepseek", "deepseek-chat"));
        state.runs.push(mk_run("c", base, 100, 200, "api:qwen", "qwen-plus"));
        state.runs.push(mk_run("d", base, 1, 2, "api:qwen", "qwen-plus"));
        let rows = aggregate(&state, 0, None);
        assert_eq!(rows.len(), 2);
        let ds = rows.iter().find(|r| r.provider == "api:deepseek").unwrap();
        assert_eq!(ds.runs, 2);
        assert_eq!(ds.in_tokens, 15);
        assert_eq!(ds.out_tokens, 25);
        assert_eq!(ds.total_tokens, 40);
        let qw = rows.iter().find(|r| r.provider == "api:qwen").unwrap();
        assert_eq!(qw.in_tokens, 101);
        assert_eq!(qw.out_tokens, 202);
        assert_eq!(qw.total_tokens, 303);
        let filtered = aggregate(&state, 0, Some("api:qwen"));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].provider, "api:qwen");
    }

    #[test]
    fn inspect_reports_tokens_duration_and_retry() {
        let base = parse_iso_to_ts("2026-11-09T00:00:00Z").unwrap();
        let mut run = mk_run("run_000001", base, 7, 8, "api:deepseek", "deepseek-chat");
        run.attempts = 1;
        let state = State { runs: vec![run], jobs: vec![], agents: vec![], seq: 1 };
        let lines = inspect_lines(&state, "run_000001").unwrap();
        let joined = lines.join("\n");
        assert!(joined.contains("tokens: in=7 out=8 total=15"));
        assert!(joined.contains("retried: yes (attempts=1)"));
        assert!(joined.contains("duration: 5s"));
    }
}