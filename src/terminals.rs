//! P7：受管长驻进程（terminal / script）。
//!
//! 用「管道 + 滚动缓冲」实现，不引入 PTY：
//! - 有持久 stdin，可连续 `send-keys`
//! - stdout/stderr 合并进 256 KB 环形缓冲，支持 `capture --since N` / `--tail N`
//! - 可 `kill` 整个进程树
//!
//! 因此这是**非交互式终端（无 TTY）**：依赖 isatty 的程序（vim/htop 等 TUI、
//! 需要回显提示符的交互式安装器）不支持。需要 TUI 时请在本机终端里跑。

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::types::{now, App};

/// 每个终端的输出缓冲上限（KB），对应用户可见的 256 KB。
pub const DEFAULT_BUFFER_KB: usize = 256;
/// ls 时认为「刚退出」的宽限期，让 reaper 有机会把 state 落盘。
const REAP_INTERVAL_MS: u64 = 700;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Terminal {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub workspace: String,
    #[serde(default)]
    pub shell: String,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub bytes_out: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Script {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub cmd: String,
    #[serde(default)]
    pub workspace: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub last_exit: Option<i32>,
    #[serde(default)]
    pub started_at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct TerminalStore {
    #[serde(skip)]
    pub dir: PathBuf,
    #[serde(default)]
    pub terminals: Vec<Terminal>,
    #[serde(default)]
    pub scripts: Vec<Script>,
}

impl TerminalStore {
    pub fn load(dir: &Path) -> Self {
        let _ = fs::create_dir_all(dir);
        let mut terminals: Vec<Terminal> =
            read_json(&dir.join("terminals.json")).unwrap_or_default();
        let mut scripts: Vec<Script> = read_json(&dir.join("scripts.json")).unwrap_or_default();
        // 重启后不复活子进程：内存里的管道已经没了，标 exited/stopped 才是事实。
        // 但子进程本身还活着（daemon 被 kill / 部署重启时它已自成进程组），
        // 不收掉就是永久孤儿进程，且用户再也管不到它 —— 这里按记录的 pid 整组收掉。
        // pid_is_our_run 用 /proc 启动时间兜底，PID 复用不会误杀别人的进程。
        let mut dirty = false;
        for t in terminals.iter_mut() {
            if t.state == "running" {
                if let Some(pid) = t.pid {
                    if crate::runner::pid_is_our_run(pid, Some(t.created_at)) {
                        crate::runner::kill_process_group(pid);
                    }
                }
                t.state = "exited".into();
                t.pid = None;
                t.exit_code = None;
                dirty = true;
            }
        }
        for s in scripts.iter_mut() {
            if s.state == "running" {
                if let Some(pid) = s.pid {
                    if crate::runner::pid_is_our_run(pid, Some(s.started_at)) {
                        crate::runner::kill_process_group(pid);
                    }
                }
                s.state = "stopped".into();
                s.pid = None;
                dirty = true;
            }
        }
        let store = TerminalStore {
            dir: dir.to_path_buf(),
            terminals,
            scripts,
        };
        if dirty {
            store.save();
        }
        store
    }

    pub fn save(&self) {
        let _ = fs::create_dir_all(&self.dir);
        if let Err(e) = write_json(&self.dir.join("terminals.json"), &self.terminals) {
            eprintln!("roamway: save terminals.json failed: {}", e);
        }
        if let Err(e) = write_json(&self.dir.join("scripts.json"), &self.scripts) {
            eprintln!("roamway: save scripts.json failed: {}", e);
        }
    }

    pub fn find(&self, id: &str) -> Option<&Terminal> {
        // 允许用 id 前缀（term_xxx）或名字定位，CLI 更顺手
        self.terminals
            .iter()
            .find(|t| t.id == id || t.name == id)
            .or_else(|| {
                let hits: Vec<&Terminal> = self
                    .terminals
                    .iter()
                    .filter(|t| !id.is_empty() && t.id.starts_with(id))
                    .collect();
                if hits.len() == 1 {
                    Some(hits[0])
                } else {
                    None
                }
            })
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut Terminal> {
        let real = self.find(id).map(|t| t.id.clone())?;
        self.terminals.iter_mut().find(|t| t.id == real)
    }

    pub fn script(&self, name: &str) -> Option<&Script> {
        self.scripts.iter().find(|s| s.name == name || s.id == name)
    }

    pub fn script_mut(&mut self, name: &str) -> Option<&mut Script> {
        let real = self.script(name).map(|s| s.id.clone())?;
        self.scripts.iter_mut().find(|s| s.id == real)
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    if bytes.is_empty() {
        return None;
    }
    match serde_json::from_slice::<T>(&bytes) {
        Ok(v) => Some(v),
        Err(e) => {
            eprintln!("roamway: {} parse failed: {}", path.display(), e);
            let _ = fs::rename(path, path.with_extension("json.corrupt"));
            None
        }
    }
}

fn write_json<T: Serialize>(path: &Path, v: &T) -> std::io::Result<()> {
    let s = serde_json::to_string_pretty(v).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    let mut f = fs::File::create(&tmp)?;
    f.write_all(s.as_bytes())?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, path)?;
    Ok(())
}

// ---------------------------------------------------------------- ring buffer

/// 环形输出缓冲：超上限丢头部，同时记录丢弃字节数，保证 `--since` 的绝对偏移稳定。
#[derive(Debug)]
pub struct Ring {
    data: VecDeque<u8>,
    cap: usize,
    dropped: u64,
}

impl Ring {
    pub fn new(cap: usize) -> Self {
        Ring {
            data: VecDeque::with_capacity(cap.min(1 << 20)),
            cap: cap.max(1),
            dropped: 0,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.data.extend(bytes.iter().copied());
        while self.data.len() > self.cap {
            self.data.pop_front();
            self.dropped += 1;
        }
    }

    /// 已写入的总字节数（含已被丢弃的部分）——`--since` 的坐标原点。
    pub fn total(&self) -> u64 {
        self.dropped + self.data.len() as u64
    }

    /// 从绝对偏移 `since` 取到末尾；`since` 落在已丢弃区间时从缓冲起点开始。
    pub fn slice_from(&self, since: u64) -> Vec<u8> {
        let start = since.saturating_sub(self.dropped) as usize;
        if start >= self.data.len() {
            return Vec::new();
        }
        self.data.iter().skip(start).copied().collect()
    }

    pub fn tail(&self, n: usize) -> Vec<u8> {
        if n == 0 || n >= self.data.len() {
            return self.data.iter().copied().collect();
        }
        self.data
            .iter()
            .skip(self.data.len() - n)
            .copied()
            .collect()
    }
}

// ------------------------------------------------------------------- runtime

pub struct Runtime {
    pub child: Arc<Mutex<Child>>,
    pub stdin: Arc<Mutex<Option<ChildStdin>>>,
    pub ring: Arc<Mutex<Ring>>,
    pub exited: Arc<AtomicBool>,
    pub exit_code: Arc<Mutex<Option<i32>>>,
    pub finalized: AtomicBool,
}

impl Runtime {
    fn alive(&self) -> bool {
        !self.exited.load(Ordering::Relaxed)
    }
}

fn gen_id(prefix: &str) -> String {
    format!("{}_{}", prefix, crate::auth::random_hex(8))
}

fn default_shell() -> String {
    if cfg!(windows) {
        "cmd".into()
    } else {
        "bash".into()
    }
}

/// shell 名 -> (可执行文件, 参数)。长驻语义：Unix 用 -i，cmd 用 /K。
fn shell_cmd(shell: &str) -> Result<(String, Vec<String>), String> {
    let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match shell {
        "bash" => Ok(("/bin/bash".into(), a(&["-i"]))),
        "sh" => Ok(("/bin/sh".into(), a(&["-i"]))),
        "pwsh" => Ok(("pwsh".into(), a(&["-NoLogo"]))),
        "powershell" => Ok(("powershell".into(), a(&["-NoLogo"]))),
        "cmd" => Ok(("cmd.exe".into(), a(&["/Q", "/K"]))),
        other => Err(format!("unsupported shell: {}", other)),
    }
}

fn child_command(cmd: &mut Command) {
    #[cfg(windows)]
    {
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            // SAFETY: pre_exec 回调运行在 fork 之后、exec 之前的子进程里，
            // 单线程上下文，只调用 async-signal-safe 的 setpgid(2)。
            cmd.pre_exec(|| {
                // 自成进程组，kill 时能整树收掉；失败则回落父进程组（仍可 kill 单 pid）
                let _ = libc::setpgid(0, 0);
                Ok(())
            });
        }
    }
}

fn pump<R: Read + Send + 'static>(mut r: R, ring: Arc<Mutex<Ring>>) {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match r.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if let Ok(mut g) = ring.lock() {
                        g.push(&buf[..n]);
                    }
                }
                Err(_) => break,
            }
        }
    });
}

fn spawn_runtime(mut cmd: Command, buffer_kb: usize) -> Result<Runtime, String> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    child_command(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {}", e))?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let ring = Arc::new(Mutex::new(Ring::new(buffer_kb.max(1) * 1024)));
    if let Some(o) = stdout {
        pump(o, ring.clone());
    }
    if let Some(e) = stderr {
        pump(e, ring.clone());
    }
    Ok(Runtime {
        child: Arc::new(Mutex::new(child)),
        stdin: Arc::new(Mutex::new(stdin)),
        ring,
        exited: Arc::new(AtomicBool::new(false)),
        exit_code: Arc::new(Mutex::new(None)),
        finalized: AtomicBool::new(false),
    })
}

// ---------------------------------------------------------------- operations

fn workspace_dir(workspace: &str) -> Result<PathBuf, String> {
    if workspace.is_empty() {
        return std::env::current_dir().map_err(|e| format!("cwd unavailable: {}", e));
    }
    let p = PathBuf::from(workspace);
    if !p.is_dir() {
        return Err(format!("workspace not found: {}", workspace));
    }
    Ok(p)
}

pub fn create(
    app: &Arc<App>,
    name: &str,
    workspace: &str,
    shell: &str,
    buffer_kb: usize,
) -> Result<Terminal, String> {
    let shell = if shell.trim().is_empty() {
        default_shell()
    } else {
        shell.trim().to_string()
    };
    let (prog, args) = shell_cmd(&shell)?;
    let dir = workspace_dir(workspace)?;
    let mut cmd = Command::new(prog);
    cmd.args(args)
        .current_dir(&dir)
        .env("PS1", "")
        .env("TERM", "dumb")
        .env("PAGER", "cat");
    let rt = spawn_runtime(cmd, buffer_kb)?;
    let pid = rt.child.lock().map(|c| c.id()).ok();

    let mut store = app.terms.lock().unwrap_or_else(|e| e.into_inner());
    let n = store.terminals.len() + 1;
    let name = if name.trim().is_empty() {
        format!("shell-{}", n)
    } else {
        name.trim().to_string()
    };
    let term = Terminal {
        id: gen_id("term"),
        name,
        workspace: dir.display().to_string(),
        shell,
        pid,
        state: "running".into(),
        created_at: now(),
        exit_code: None,
        bytes_out: 0,
    };
    store.terminals.push(term.clone());
    store.save();
    let key = format!("term:{}", term.id);
    drop(store);
    app.term_rt
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, rt);
    Ok(term)
}

pub fn list(app: &Arc<App>) -> Vec<Terminal> {
    let store = app.terms.lock().unwrap_or_else(|e| e.into_inner());
    let rts = app.term_rt.lock().unwrap_or_else(|e| e.into_inner());
    store
        .terminals
        .iter()
        .map(|t| {
            let mut t = t.clone();
            if let Some(rt) = rts.get(&format!("term:{}", t.id)) {
                if let Ok(r) = rt.ring.lock() {
                    t.bytes_out = r.total();
                }
            }
            t
        })
        .collect()
}

pub fn get(app: &Arc<App>, id: &str) -> Result<Terminal, String> {
    list(app)
        .into_iter()
        .find(|t| t.id == id || t.name == id)
        .ok_or_else(|| format!("no such terminal: {}", id))
}

pub fn capture(
    app: &Arc<App>,
    id: &str,
    since: Option<u64>,
    tail: Option<usize>,
) -> Result<(Vec<u8>, u64), String> {
    let term = get(app, id)?;
    let rts = app.term_rt.lock().unwrap_or_else(|e| e.into_inner());
    let rt = rts
        .get(&format!("term:{}", term.id))
        .ok_or_else(|| format!("terminal {} has no live buffer (daemon restarted)", term.id))?;
    let ring = rt.ring.lock().unwrap_or_else(|e| e.into_inner());
    let total = ring.total();
    let bytes = match tail {
        Some(n) => ring.tail(n),
        None => ring.slice_from(since.unwrap_or(0)),
    };
    Ok((bytes, total))
}

pub fn send_keys(app: &Arc<App>, id: &str, keys: &str, enter: bool) -> Result<(), String> {
    let term = get(app, id)?;
    if term.state != "running" {
        return Err(format!("terminal {} is {}", term.id, term.state));
    }
    let rts = app.term_rt.lock().unwrap_or_else(|e| e.into_inner());
    let rt = rts
        .get(&format!("term:{}", term.id))
        .ok_or_else(|| format!("terminal {} has no live stdin", term.id))?;
    let mut guard = rt.stdin.lock().unwrap_or_else(|e| e.into_inner());
    let stdin = guard
        .as_mut()
        .ok_or_else(|| format!("terminal {} stdin closed", term.id))?;
    let payload = payload(keys, enter);
    stdin
        .write_all(&payload)
        .and_then(|_| stdin.flush())
        .map_err(|e| format!("write failed: {}", e))
}

pub fn kill(app: &Arc<App>, id: &str) -> Result<Terminal, String> {
    let term = get(app, id)?;
    let mut seen: Option<u64> = None;
    {
        let rts = app.term_rt.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(rt) = rts.get(&format!("term:{}", term.id)) {
            if let Ok(r) = rt.ring.lock() {
                // 落盘前把实际字节数带回元数据，否则重启后 ls 会显示 0
                seen = Some(r.total());
            }
            if rt.alive() {
                let pid = rt.child.lock().map(|c| c.id()).unwrap_or(0);
                if pid > 1 {
                    crate::runner::kill_process_group(pid);
                }
                let mut c = rt.child.lock().unwrap_or_else(|e| e.into_inner());
                let _ = c.try_wait();
                rt.exited.store(true, Ordering::Relaxed);
            }
        }
    }
    // kill 幂等：已经 exited 时只更新元数据并成功返回
    let mut store = app.terms.lock().unwrap_or_else(|e| e.into_inner());
    let out = match store.find_mut(&term.id) {
        Some(t) => {
            t.state = "exited".into();
            t.pid = None;
            if let Some(n) = seen {
                t.bytes_out = n;
            }
            t.clone()
        }
        None => return Err(format!("no such terminal: {}", id)),
    };
    store.save();
    Ok(out)
}

pub fn scripts(app: &Arc<App>) -> Vec<Script> {
    let store = app.terms.lock().unwrap_or_else(|e| e.into_inner());
    store.scripts.clone()
}

pub fn script_add(
    app: &Arc<App>,
    name: &str,
    cmd: &str,
    workspace: &str,
) -> Result<Script, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("name required".into());
    }
    if cmd.trim().is_empty() {
        return Err("cmd required".into());
    }
    let mut store = app.terms.lock().unwrap_or_else(|e| e.into_inner());
    if store.script(name).is_some() {
        return Err(format!("script already exists: {}", name));
    }
    if !workspace.is_empty() && !Path::new(workspace).is_dir() {
        return Err(format!("workspace not found: {}", workspace));
    }
    let s = Script {
        id: gen_id("scr"),
        name: name.to_string(),
        cmd: cmd.to_string(),
        workspace: workspace.to_string(),
        state: "stopped".into(),
        pid: None,
        last_exit: None,
        started_at: 0,
    };
    store.scripts.push(s.clone());
    store.save();
    Ok(s)
}

pub fn script_rm(app: &Arc<App>, name: &str) -> Result<usize, String> {
    let mut store = app.terms.lock().unwrap_or_else(|e| e.into_inner());
    let before = store.scripts.len();
    let id = store.script(name).map(|s| s.id.clone());
    let Some(id) = id else {
        return Err(format!("no such script: {}", name));
    };
    if store
        .script(name)
        .map(|s| s.state == "running")
        .unwrap_or(false)
    {
        return Err(format!("script {} is running; stop it first", name));
    }
    store.scripts.retain(|s| s.id != id);
    let removed = before - store.scripts.len();
    store.save();
    Ok(removed)
}

pub fn script_start(app: &Arc<App>, name: &str) -> Result<Script, String> {
    let mut store = app.terms.lock().unwrap_or_else(|e| e.into_inner());
    let s = store
        .script(name)
        .cloned()
        .ok_or_else(|| format!("no such script: {}", name))?;
    let dir = workspace_dir(&s.workspace)?;
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd.exe");
        c.args(["/Q", "/C", &s.cmd]);
        c
    } else {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", &s.cmd]);
        c
    };
    cmd.current_dir(&dir);
    // TOCTOU 修复：检查+spawn+insert 全程持 term_rt 锁，否则并发 script_start
    // 都能通过 alive 检查，各自 spawn 后互相覆盖 map 项，泄漏子进程。
    // 锁序统一 terms → term_rt（terms 锁上面已持有，Mutex 不可重入，不能再取）。
    let mut rts = app.term_rt.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(rt) = rts.get(&format!("script:{}", s.name)) {
        if rt.alive() {
            return Err(format!("script {} is already running", s.name));
        }
    }
    let rt = spawn_runtime(cmd, DEFAULT_BUFFER_KB)?;
    let pid = rt.child.lock().map(|c| c.id()).ok();
    rts.insert(format!("script:{}", s.name), rt);
    drop(rts);
    let out = if let Some(x) = store.script_mut(&s.name) {
        x.state = "running".into();
        x.pid = pid;
        x.started_at = now();
        x.last_exit = None;
        x.clone()
    } else {
        s
    };
    store.save();
    Ok(out)
}

pub fn script_stop(app: &Arc<App>, name: &str) -> Result<Script, String> {
    let store = app.terms.lock().unwrap_or_else(|e| e.into_inner());
    let s = store
        .script(name)
        .cloned()
        .ok_or_else(|| format!("no such script: {}", name))?;
    drop(store);
    let mut code = None;
    {
        let rts = app.term_rt.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(rt) = rts.get(&format!("script:{}", s.name)) {
            let pid = rt.child.lock().map(|c| c.id()).unwrap_or(0);
            if rt.alive() && pid > 1 {
                // kill 内部已含 SIGTERM→等待→SIGKILL 升级，不再持锁 sleep 阻塞其他终端操作
                crate::runner::kill_process_group(pid);
            }
            let mut c = rt.child.lock().unwrap_or_else(|e| e.into_inner());
            if let Ok(Some(st)) = c.try_wait() {
                code = st.code();
            }
            rt.exited.store(true, Ordering::Relaxed);
        }
    }
    let mut store = app.terms.lock().unwrap_or_else(|e| e.into_inner());
    let out = if let Some(x) = store.script_mut(&s.name) {
        x.state = "stopped".into();
        x.pid = None;
        if code.is_some() {
            x.last_exit = code;
        }
        x.clone()
    } else {
        s
    };
    store.save();
    Ok(out)
}

/// 后台回收线程：子进程退出后把 state/exit_code 落盘（含脚本）。
/// 没有它的话 metadata 会一直显示 running，直到下一次重启。
pub fn start_reaper(app: Arc<App>) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(REAP_INTERVAL_MS));
        let mut changed_terms: Vec<(String, Option<i32>, u64)> = Vec::new();
        let mut changed_scripts: Vec<(String, Option<i32>)> = Vec::new();
        {
            let rts = app.term_rt.lock().unwrap_or_else(|e| e.into_inner());
            for (key, rt) in rts.iter() {
                if rt.exited.load(Ordering::Relaxed) || rt.finalized.load(Ordering::Relaxed) {
                    continue;
                }
                let done = {
                    let mut c = match rt.child.lock() {
                        Ok(c) => c,
                        Err(e) => e.into_inner(),
                    };
                    match c.try_wait() {
                        Ok(Some(st)) => Some(st.code()),
                        Ok(None) => None,
                        Err(_) => Some(None),
                    }
                };
                if let Some(code) = done {
                    rt.exited.store(true, Ordering::Relaxed);
                    rt.finalized.store(true, Ordering::Relaxed);
                    if let Ok(mut g) = rt.exit_code.lock() {
                        *g = code;
                    }
                    let total = rt.ring.lock().map(|r| r.total()).unwrap_or(0);
                    if let Some(id) = key.strip_prefix("term:") {
                        changed_terms.push((id.to_string(), code, total));
                    } else if let Some(n) = key.strip_prefix("script:") {
                        changed_scripts.push((n.to_string(), code));
                    }
                }
            }
        }
        if changed_terms.is_empty() && changed_scripts.is_empty() {
            continue;
        }
        let mut store = app.terms.lock().unwrap_or_else(|e| e.into_inner());
        for (id, code, total) in changed_terms {
            if let Some(t) = store.find_mut(&id) {
                if t.state == "running" {
                    t.state = "exited".into();
                    t.pid = None;
                    t.exit_code = code;
                    t.bytes_out = total;
                }
            }
        }
        for (name, code) in changed_scripts {
            if let Some(s) = store.script_mut(&name) {
                if s.state == "running" {
                    s.state = "stopped".into();
                    s.pid = None;
                    s.last_exit = code;
                }
            }
        }
        store.save();
    });
}

fn payload(keys: &str, enter: bool) -> Vec<u8> {
    let mut p = keys.as_bytes().to_vec();
    if enter && !p.ends_with(b"\n") {
        p.push(b'\n');
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_truncates_oldest() {
        let mut r = Ring::new(8);
        r.push(b"abcdefghij");
        assert_eq!(r.total(), 10);
        assert_eq!(r.slice_from(0), b"cdefghij".to_vec());
    }

    #[test]
    fn ring_since_offset_is_absolute() {
        let mut r = Ring::new(8);
        r.push(b"abcdefgh"); // dropped=0, total=8
        r.push(b"ij"); // dropped=2, total=10
                       // since=2 表示从最初的第 2 个字节开始，但该字节已被丢弃 -> 从缓冲起点
        assert_eq!(r.slice_from(2), b"cdefghij".to_vec());
        assert_eq!(r.slice_from(9), b"j".to_vec());
        assert_eq!(r.slice_from(10), b"".to_vec());
    }

    #[test]
    fn ring_tail_and_total_accumulate() {
        let mut r = Ring::new(1024);
        r.push(b"one\n");
        r.push(b"two\n");
        assert_eq!(r.total(), 8);
        assert_eq!(r.tail(4), b"two\n".to_vec());
        assert_eq!(r.tail(0), b"one\ntwo\n".to_vec());
    }

    #[test]
    fn enter_appends_newline_once() {
        assert_eq!(payload("echo hi", true), b"echo hi\n".to_vec());
        assert_eq!(payload("echo hi\n", true), b"echo hi\n".to_vec());
        assert_eq!(payload("echo hi", false), b"echo hi".to_vec());
    }

    #[test]
    fn unsupported_shell_rejected() {
        let e = shell_cmd("zsh").unwrap_err();
        assert!(e.contains("unsupported shell"));
    }

    #[test]
    fn store_marks_running_as_exited_after_restart() {
        let dir = std::env::temp_dir().join(format!("rw-term-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let terms = vec![Terminal {
            id: "term_0000abcd".into(),
            name: "shell-1".into(),
            state: "running".into(),
            pid: Some(999),
            ..Default::default()
        }];
        write_json(&dir.join("terminals.json"), &terms).unwrap();
        let scripts = vec![Script {
            id: "scr_0000abcd".into(),
            name: "build".into(),
            state: "running".into(),
            pid: Some(998),
            ..Default::default()
        }];
        write_json(&dir.join("scripts.json"), &scripts).unwrap();
        let st = TerminalStore::load(&dir);
        assert_eq!(st.terminals[0].state, "exited");
        assert_eq!(st.terminals[0].pid, None);
        assert_eq!(st.scripts[0].state, "stopped");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn restart_reaps_orphan_children() {
        let dir = std::env::temp_dir().join(format!("rw-orphan-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut cmd = Command::new(default_shell());
        cmd.arg("-c").arg("sleep 60");
        child_command(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let pid = child.id();
        let terms = vec![Terminal {
            id: "term_0000beef".into(),
            name: "orphan".into(),
            state: "running".into(),
            pid: Some(pid),
            created_at: now(),
            ..Default::default()
        }];
        write_json(&dir.join("terminals.json"), &terms).unwrap();
        let st = TerminalStore::load(&dir);
        assert_eq!(st.terminals[0].state, "exited");
        assert_eq!(st.terminals[0].pid, None);
        // 进程组必须被收掉：wait 能返回且不是正常退出（被信号杀掉）
        let status = child.wait().unwrap();
        assert!(
            !status.success(),
            "orphan child survived restart: {:?}",
            status
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_lookup_by_id_prefix_and_name() {
        let st = TerminalStore {
            dir: PathBuf::from("."),
            terminals: vec![Terminal {
                id: "term_12345678".into(),
                name: "shell-1".into(),
                ..Default::default()
            }],
            scripts: Vec::new(),
        };
        assert!(st.find("term_1234").is_some());
        assert!(st.find("shell-1").is_some());
        assert!(st.find("nope").is_none());
    }

    #[test]
    fn script_duplicate_is_rejected_by_store_rule() {
        let mut st = TerminalStore::default();
        st.scripts.push(Script {
            name: "build".into(),
            ..Default::default()
        });
        assert!(st.script("build").is_some());
        assert!(st.script("other").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn live_terminal_roundtrip() {
        // 真跑一个 sh：create -> send-keys -> capture 应看到输出，kill 后 state=exited
        let dir = std::env::temp_dir().join(format!("rw-term-live-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let app = crate::test_support::test_app(&dir);
        let t = create(&app, "t1", dir.to_str().unwrap(), "sh", 64).unwrap();
        assert_eq!(t.state, "running");
        send_keys(&app, &t.id, "echo hi", true).unwrap();
        let mut seen = String::new();
        for _ in 0..40 {
            thread::sleep(Duration::from_millis(100));
            let (bytes, _) = capture(&app, &t.id, Some(0), None).unwrap();
            seen = String::from_utf8_lossy(&bytes).to_string();
            if seen.contains("hi") {
                break;
            }
        }
        assert!(seen.contains("hi"), "capture missed output: {:?}", seen);
        let _ = kill(&app, &t.id).unwrap();
        let _ = kill(&app, &t.id).unwrap(); // 幂等
        let live = get(&app, &t.id).unwrap();
        assert_eq!(live.state, "exited");
        let _ = fs::remove_dir_all(&dir);
    }
}
