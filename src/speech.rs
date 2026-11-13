use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SpeechConfig {
    #[serde(default)]
    pub stt: Vec<String>,
    #[serde(default)]
    pub tts: Vec<String>,
}

pub fn load(dir: &Path) -> SpeechConfig {
    let _ = fs::create_dir_all(dir);
    let path = dir.join("speech.json");
    match fs::read_to_string(&path) {
        Ok(raw) if raw.trim().is_empty() => SpeechConfig::default(),
        Ok(raw) => match serde_json::from_str::<SpeechConfig>(&raw) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("roamway: speech.json parse failed: {}", e);
                SpeechConfig::default()
            }
        },
        Err(_) => SpeechConfig::default(),
    }
}

// Config writer kept for `speech` config commands (P10 web control plane); no CLI path yet.
#[allow(dead_code)]
pub fn save(dir: &Path, cfg: &SpeechConfig) -> std::io::Result<()> {
    let _ = fs::create_dir_all(dir);
    let tmp = dir.join("speech.json.tmp");
    let data = serde_json::to_vec_pretty(cfg).map_err(std::io::Error::other)?;
    fs::write(&tmp, data)?;
    fs::rename(&tmp, dir.join("speech.json"))?;
    Ok(())
}

fn contains_placeholder(args: &[String], ph: &str) -> bool {
    args.iter().any(|a| a.contains(ph))
}

pub fn expand(template: &[String], input: &str, output: &str) -> Vec<String> {
    let mut args: Vec<String> = template
        .iter()
        .map(|a| a.replace("{in}", input).replace("{out}", output))
        .collect();
    if !contains_placeholder(template, "{in}") && !input.is_empty() {
        args.push(input.to_string());
    }
    if !contains_placeholder(template, "{out}") && !output.is_empty() {
        args.push(output.to_string());
    }
    args
}

pub fn binary_available(cmd: &[String]) -> bool {
    let first = match cmd.first() {
        Some(f) if !f.is_empty() => f,
        _ => return false,
    };
    if first.contains('/') || first.contains('\\') {
        return Path::new(first).is_file();
    }
    crate::providers::which_in_path(first)
}

fn run_command(args: &[String]) -> Result<String, String> {
    if args.is_empty() {
        return Err("empty command template".into());
    }
    let mut cmd = Command::new(&args[0]);
    cmd.args(&args[1..]);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // 超时保护：whisper/edge-tts 可能因网络/资源挂起，不能无限阻塞调用线程。
    // kill_process_group 需要进程组，spawn 时按平台配置（与 plugins.rs 一致）。
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn {}: {}", args[0], e))?;
    // Drain both pipes concurrently: reading stdout to EOF before touching
    // stderr deadlocks any child that writes >64KB to stderr (pipe buffer
    // full -> child blocks on write -> parent blocks on stdout read).
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let out_handle = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut p) = stdout_pipe {
            let _ = p.read_to_string(&mut s);
        }
        s
    });
    let err_handle = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut p) = stderr_pipe {
            let _ = p.read_to_string(&mut s);
        }
        s
    });
    // 120s 超时：drain 线程已在后台读管道，这里只等退出；超时杀整个进程组。
    // 必须先等退出再 join drain 线程——反过来会阻塞到 EOF，超时形同虚设。
    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if Instant::now() >= deadline {
                    crate::runner::kill_process_group(child.id());
                    let _ = child.wait();
                    let _ = out_handle.join();
                    let _ = err_handle.join();
                    return Err(format!("{}: timeout after 120s", args[0]));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                crate::runner::kill_process_group(child.id());
                let _ = child.wait();
                let _ = out_handle.join();
                let _ = err_handle.join();
                return Err(e.to_string());
            }
        }
    };
    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();
    if status.success() {
        Ok(stdout.trim().to_string())
    } else {
        let msg = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        Err(if msg.is_empty() {
            format!("command exited with {}", status)
        } else {
            msg
        })
    }
}

pub fn default_tts_path(dir: &Path) -> PathBuf {
    dir.join("tts").join(format!("{}.mp3", crate::types::now()))
}

pub fn transcribe(dir: &Path, file: &str) -> Result<String, String> {
    let cfg = load(dir);
    if cfg.stt.is_empty() {
        return Err("speech: stt not configured".into());
    }
    let args = expand(&cfg.stt, file, "");
    run_command(&args)
}

pub fn say(dir: &Path, text: &str, out: Option<&str>) -> Result<String, String> {
    let cfg = load(dir);
    if cfg.tts.is_empty() {
        return Err("speech: tts not configured".into());
    }
    let out_path = match out {
        Some(o) if !o.trim().is_empty() => PathBuf::from(o),
        _ => default_tts_path(dir),
    };
    if let Some(parent) = out_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let args = expand(&cfg.tts, text, &out_path.to_string_lossy());
    run_command(&args)?;
    Ok(out_path.to_string_lossy().to_string())
}

pub fn status_lines(dir: &Path) -> Vec<String> {
    let cfg = load(dir);
    let mut out = Vec::new();
    if cfg.stt.is_empty() {
        out.push("stt: (not configured)".into());
    } else {
        out.push(format!(
            "stt: {} [{}]",
            cfg.stt.join(" "),
            if binary_available(&cfg.stt) {
                "available"
            } else {
                "missing binary"
            }
        ));
    }
    if cfg.tts.is_empty() {
        out.push("tts: (not configured)".into());
    } else {
        out.push(format!(
            "tts: {} [{}]",
            cfg.tts.join(" "),
            if binary_available(&cfg.tts) {
                "available"
            } else {
                "missing binary"
            }
        ));
    }
    out
}

pub fn example_config() -> String {
    r#"{"stt": ["whisper", "--model", "base", "{in}"], "tts": ["edge-tts", "--text", "{in}", "--write-media", "{out}"]}"#.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_replaces_placeholders() {
        let tpl = vec!["/bin/echo".into(), "in={in}".into(), "out={out}".into()];
        let args = expand(&tpl, "hello", "/tmp/a.mp3");
        assert_eq!(args, vec!["/bin/echo", "in=hello", "out=/tmp/a.mp3"]);
    }

    #[test]
    fn expand_appends_when_placeholder_missing() {
        let tpl = vec!["/bin/echo".into()];
        let args = expand(&tpl, "hello", "/tmp/a.mp3");
        assert_eq!(args, vec!["/bin/echo", "hello", "/tmp/a.mp3"]);
    }

    #[test]
    fn status_reports_unconfigured() {
        let dir = std::env::temp_dir().join(format!("roamway-speech-{}", crate::types::now()));
        let _ = fs::create_dir_all(&dir);
        let lines = status_lines(&dir);
        assert!(lines.iter().any(|l| l.starts_with("stt: (not configured)")));
        assert!(lines.iter().any(|l| l.starts_with("tts: (not configured)")));
        let _ = fs::remove_dir_all(&dir);
    }
}
