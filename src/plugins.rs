use crate::types::{iso, now, App};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Plugin {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub entry: Vec<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub trusted: bool,
    #[serde(default = "default_timeout")]
    pub timeout_s: u64,
}

fn default_version() -> String {
    "0.1.0".into()
}

fn default_timeout() -> u64 {
    30
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Registry {
    #[serde(skip)]
    pub path: PathBuf,
    pub plugins: Vec<Plugin>,
}

pub fn valid_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 32 {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

pub fn plugins_dir(dir: &Path) -> PathBuf {
    dir.join("plugins")
}

pub fn plugin_dir(dir: &Path, name: &str) -> PathBuf {
    plugins_dir(dir).join(name)
}

pub fn log_path(dir: &Path, name: &str) -> PathBuf {
    plugin_dir(dir, name).join("plugin.log")
}

impl Registry {
    pub fn load(dir: &Path) -> Self {
        let _ = fs::create_dir_all(dir);
        let path = dir.join("plugins.json");
        match fs::read_to_string(&path) {
            Ok(raw) if raw.trim().is_empty() => Registry {
                path,
                plugins: Vec::new(),
            },
            Ok(raw) => match parse_registry(&raw) {
                Ok(plugins) => Registry { path, plugins },
                Err(e) => {
                    eprintln!("roamway: plugins.json parse failed: {}", e);
                    let ts = now();
                    let corrupt = dir.join(format!("plugins.json.corrupt-{}", ts));
                    let _ = fs::rename(&path, &corrupt);
                    Registry {
                        path,
                        plugins: Vec::new(),
                    }
                }
            },
            Err(_) => Registry {
                path,
                plugins: Vec::new(),
            },
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_file_name("plugins.json.tmp");
        let wrapped = json!({ "plugins": self.plugins.clone() });
        let data = serde_json::to_vec_pretty(&wrapped).map_err(std::io::Error::other)?;
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

    pub fn get(&self, name: &str) -> Option<&Plugin> {
        self.plugins.iter().find(|p| p.name == name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Plugin> {
        self.plugins.iter_mut().find(|p| p.name == name)
    }

    pub fn upsert(&mut self, plugin: Plugin) {
        match self.plugins.iter_mut().find(|p| p.name == plugin.name) {
            Some(existing) => *existing = plugin,
            None => self.plugins.push(plugin),
        }
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.plugins.len();
        self.plugins.retain(|p| p.name != name);
        self.plugins.len() != before
    }
}

fn parse_registry(raw: &str) -> Result<Vec<Plugin>, String> {
    let v: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    let arr = v
        .get("plugins")
        .and_then(|p| p.as_array())
        .cloned()
        .or_else(|| v.as_array().cloned())
        .unwrap_or_default();
    arr.into_iter()
        .map(|item| serde_json::from_value::<Plugin>(item).map_err(|e| e.to_string()))
        .collect()
}

pub fn read_plugin_file(dir: &Path, name: &str) -> Result<Plugin, String> {
    let path = plugin_dir(dir, name).join("plugin.json");
    let raw = fs::read_to_string(&path).map_err(|e| format!("read {}: {}", path.display(), e))?;
    serde_json::from_str::<Plugin>(&raw).map_err(|e| format!("bad plugin.json: {}", e))
}

fn write_plugin_file(dir: &Path, plugin: &Plugin) -> Result<(), String> {
    let pd = plugin_dir(dir, &plugin.name);
    fs::create_dir_all(&pd).map_err(|e| e.to_string())?;
    let data = serde_json::to_vec_pretty(plugin).map_err(|e| e.to_string())?;
    fs::write(pd.join("plugin.json"), data).map_err(|e| e.to_string())
}

pub fn init(dir: &Path, name: &str) -> Result<(), String> {
    if !valid_name(name) {
        return Err("name must match [a-z0-9-_]{1,32}".into());
    }
    let pd = plugin_dir(dir, name);
    fs::create_dir_all(&pd).map_err(|e| e.to_string())?;
    let plugin = Plugin {
        name: name.to_string(),
        version: default_version(),
        entry: vec!["python3".into(), "main.py".into()],
        enabled: false,
        trusted: false,
        timeout_s: 30,
    };
    write_plugin_file(dir, &plugin)?;
    let skeleton = r#"#!/usr/bin/env python3
import json
import sys

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        print(json.dumps({"ok": False, "message": "bad input"}))
        break
    print(json.dumps({"ok": True, "message": "hello from " + msg.get("hook", "")}))
    break
"#;
    fs::write(pd.join("main.py"), skeleton).map_err(|e| e.to_string())?;
    let mut reg = Registry::load(dir);
    reg.upsert(plugin);
    reg.save().map_err(|e| e.to_string())
}

pub fn enable(dir: &Path, name: &str, trust: bool) -> Result<(), String> {
    let mut reg = Registry::load(dir);
    let plugin = reg.get_mut(name).ok_or_else(|| {
        format!(
            "plugin '{}' not found; run `roamway plugin init {}` first",
            name, name
        )
    })?;
    if !plugin.trusted && !trust {
        return Err(format!(
            "plugin '{}' is not trusted; re-run with --trust",
            name
        ));
    }
    if trust {
        plugin.trusted = true;
    }
    plugin.enabled = true;
    let plugin = plugin.clone();
    reg.save().map_err(|e| e.to_string())?;
    write_plugin_file(dir, &plugin)
}

pub fn disable(dir: &Path, name: &str) -> Result<(), String> {
    let mut reg = Registry::load(dir);
    let plugin = reg
        .get_mut(name)
        .ok_or_else(|| format!("plugin '{}' not found", name))?;
    plugin.enabled = false;
    let plugin = plugin.clone();
    reg.save().map_err(|e| e.to_string())?;
    write_plugin_file(dir, &plugin)
}

pub fn reload(dir: &Path, name: &str) -> Result<(), String> {
    let plugin = read_plugin_file(dir, name)?;
    let mut reg = Registry::load(dir);
    reg.upsert(plugin);
    reg.save().map_err(|e| e.to_string())
}

pub fn remove(dir: &Path, name: &str, keep_files: bool) -> Result<(), String> {
    let mut reg = Registry::load(dir);
    if !reg.remove(name) {
        return Err(format!("plugin '{}' not found", name));
    }
    reg.save().map_err(|e| e.to_string())?;
    if !keep_files {
        let pd = plugin_dir(dir, name);
        if pd.is_dir() {
            fs::remove_dir_all(&pd).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn configure_process(c: &mut Command) {
    use std::os::unix::process::CommandExt;
    c.process_group(0);
}

#[cfg(windows)]
fn configure_process(c: &mut Command) {
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    use std::os::windows::process::CommandExt;
    c.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

fn kill_child(child: &mut Child) {
    crate::runner::kill_process_group(child.id());
    let _ = child.wait();
}

pub fn run_one(dir: &Path, plugin: &Plugin, hook: &str, payload: &Value) -> Result<String, String> {
    let pd = plugin_dir(dir, &plugin.name);
    if !pd.is_dir() {
        return Err("plugin directory missing".into());
    }
    if plugin.entry.is_empty() {
        return Err("empty entry".into());
    }
    let mut cmd = Command::new(&plugin.entry[0]);
    cmd.args(&plugin.entry[1..]);
    cmd.current_dir(&pd);
    configure_process(&mut cmd);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("spawn: {}", e))?;
    // Drain both pipes on background threads for the whole lifetime of the
    // child: the try_wait loop below never reads them, so a plugin writing
    // >64KB to stdout or stderr would fill the pipe buffer and block forever
    // (parent waits for exit, child waits for a reader) until the timeout.
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let out_handle = thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut p) = stdout_pipe {
            let _ = p.read_to_string(&mut s);
        }
        s
    });
    let err_handle = thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut p) = stderr_pipe {
            let _ = p.read_to_string(&mut s);
        }
        s
    });
    let line = serde_json::to_string(&json!({ "hook": hook, "payload": payload }))
        .map_err(|e| format!("json: {}", e))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(line.as_bytes());
        let _ = stdin.write_all(b"\n");
    }
    let timeout = plugin.timeout_s.max(1);
    let deadline = Instant::now() + Duration::from_secs(timeout);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_child(&mut child);
                    let _ = out_handle.join();
                    let _ = err_handle.join();
                    return Err(format!("timeout after {}s", timeout));
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                kill_child(&mut child);
                let _ = out_handle.join();
                let _ = err_handle.join();
                return Err(format!("wait: {}", e));
            }
        }
    }
    let out = out_handle.join().unwrap_or_default();
    let _ = err_handle.join();
    let first = out.lines().next().unwrap_or("").trim().to_string();
    if first.is_empty() {
        return Err("plugin produced no output".into());
    }
    let v: Value = serde_json::from_str(&first).map_err(|e| format!("bad plugin json: {}", e))?;
    if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
        Ok(v.get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string())
    } else {
        Err(v
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("ok=false")
            .to_string())
    }
}

pub fn append_log(dir: &Path, name: &str, line: &str) {
    let path = log_path(dir, name);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut lines: Vec<String> = fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .map(|s| s.to_string())
        .collect();
    lines.push(format!("{} {}", iso(now()), line));
    if lines.len() > 256 {
        let excess = lines.len() - 256;
        lines.drain(..excess);
    }
    let data = lines.join("\n") + "\n";
    let _ = fs::write(&path, data);
}

pub fn read_log(dir: &Path, name: &str) -> String {
    fs::read_to_string(log_path(dir, name)).unwrap_or_default()
}

pub fn fire_dir(dir: &Path, hook: &str, payload: Value) {
    let reg = Registry::load(dir);
    for plugin in reg.plugins.iter().filter(|p| p.enabled && p.trusted) {
        match run_one(dir, plugin, hook, &payload) {
            Ok(message) => append_log(dir, &plugin.name, &format!("hook={} ok: {}", hook, message)),
            Err(e) => append_log(dir, &plugin.name, &format!("hook={} failed: {}", hook, e)),
        }
    }
}

pub fn fire(app: &Arc<App>, hook: &str, payload: Value) {
    let dir = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        st.dir.clone()
    };
    fire_dir(&dir, hook, payload);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TMP_SEQ: AtomicUsize = AtomicUsize::new(0);

    fn tmp() -> PathBuf {
        std::env::temp_dir().join(format!(
            "roamway-plugin-{}-{}-{}",
            std::process::id(),
            TMP_SEQ.fetch_add(1, Ordering::Relaxed),
            now()
        ))
    }

    #[test]
    fn registry_roundtrip() {
        let dir = tmp();
        let _ = fs::create_dir_all(&dir);
        let mut reg = Registry::load(&dir);
        reg.upsert(Plugin {
            name: "demo".into(),
            version: "0.1.0".into(),
            entry: vec!["python3".into(), "main.py".into()],
            enabled: true,
            trusted: true,
            timeout_s: 30,
        });
        reg.save().unwrap();
        let loaded = Registry::load(&dir);
        assert_eq!(loaded.plugins.len(), 1);
        assert_eq!(loaded.plugins[0].name, "demo");
        assert!(loaded.plugins[0].trusted);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn untrusted_plugin_requires_trust() {
        let dir = tmp();
        let _ = fs::create_dir_all(&dir);
        let mut reg = Registry::load(&dir);
        reg.upsert(Plugin {
            name: "demo".into(),
            version: "0.1.0".into(),
            entry: vec!["python3".into(), "main.py".into()],
            enabled: false,
            trusted: false,
            timeout_s: 30,
        });
        reg.save().unwrap();
        assert!(enable(&dir, "demo", false).is_err());
        assert!(enable(&dir, "demo", true).is_ok());
        let reg = Registry::load(&dir);
        assert!(reg.get("demo").unwrap().enabled);
        assert!(reg.get("demo").unwrap().trusted);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn fire_dir_runs_trusted_plugin_and_logs_receipt() {
        let dir = tmp();
        let _ = fs::create_dir_all(&dir);
        let pd = plugin_dir(&dir, "demo");
        let _ = fs::create_dir_all(&pd);
        fs::write(
            pd.join("run.sh"),
            "read line\necho '{\"ok\":true,\"message\":\"ran\"}'\n",
        )
        .unwrap();
        let mut reg = Registry::load(&dir);
        reg.upsert(Plugin {
            name: "demo".into(),
            version: "0.1.0".into(),
            entry: vec!["sh".into(), "run.sh".into()],
            enabled: true,
            trusted: true,
            timeout_s: 5,
        });
        reg.save().unwrap();
        fire_dir(&dir, "agent.finished", json!({ "run_id": "r1" }));
        let text = read_log(&dir, "demo");
        assert!(text.contains("hook=agent.finished ok: ran"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_slow_plugin() {
        let dir = tmp();
        let _ = fs::create_dir_all(&dir);
        let _ = fs::create_dir_all(plugin_dir(&dir, "slow"));
        let plugin = Plugin {
            name: "slow".into(),
            version: "0.1.0".into(),
            entry: vec!["sh".into(), "-c".into(), "sleep 10".into()],
            enabled: true,
            trusted: true,
            timeout_s: 1,
        };
        let start = Instant::now();
        let result = run_one(&dir, &plugin, "agent.finished", &json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("timeout"));
        assert!(start.elapsed().as_secs() < 8);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_ring_truncates_to_256() {
        let dir = tmp();
        let _ = fs::create_dir_all(&dir);
        for i in 0..300 {
            append_log(&dir, "demo", &format!("line {}", i));
        }
        let lines = read_log(&dir, "demo").lines().count();
        assert_eq!(lines, 256);
        let text = read_log(&dir, "demo");
        assert!(text.contains("line 299"));
        assert!(!text.contains("line 0"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_plugin_json_errors_without_panic() {
        let dir = tmp();
        let pd = plugin_dir(&dir, "demo");
        let _ = fs::create_dir_all(&pd);
        fs::write(pd.join("plugin.json"), "{ bad json").unwrap();
        let err = read_plugin_file(&dir, "demo").unwrap_err();
        assert!(err.contains("bad plugin.json"));
        let _ = fs::remove_dir_all(&dir);
    }
}
