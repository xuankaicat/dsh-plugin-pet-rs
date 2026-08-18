//! DSH 子进程管理：由桌宠拉起 `dsh --profile web` 并负责回收。
//!
//! 启动方式：
//! 1. `dsh` —— PATH 中的真实可执行文件（Unix 主要路径）；
//! 2. `cmd /C dsh ...` —— Windows 上解析 PATH 中的 `.cmd` shim（npm 全局安装）。
//!
//! 如果环境中没有 dsh，则返回 `StartError::NotInstalled` 交给桌宠弹窗确认；
//! 用户选择安装源后由 `install` 执行
//! `npm install -g @deepseek-ai/dsh --verbose --registry=<源>`，
//! 并把下载进度逐行抛给桌宠气泡，安装完成后重试启动。
//!
//! 优先使用默认端口 3080（与 DSH 默认地址 http://127.0.0.1:3080 一致），
//! 被占用时回退 `--port 0` 由系统分配空闲端口；实际地址由子进程 stdout
//! 输出一行 `dsh web: http://127.0.0.1:<port>`，本模块负责解析。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};
use tokio::time::timeout;

/// 单个候选启动方式的等待超时
const DIRECT_TIMEOUT: Duration = Duration::from_secs(8);

/// 优先使用的端口：与 DSH 默认地址 http://127.0.0.1:3080 保持一致。
const PREFERRED_PORT: &str = "3080";
/// 端口被占用时的回退：`--port 0` 让系统分配空闲端口。
const FALLBACK_PORT: &str = "0";

/// npm 安装源
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Registry {
    /// 官方源
    Official,
    /// npmmirror（阿里）
    Npmmirror,
    /// 腾讯云镜像
    Tencent,
}

impl Registry {
    pub fn label(self) -> &'static str {
        match self {
            Registry::Official => "官方源",
            Registry::Npmmirror => "npmmirror",
            Registry::Tencent => "腾讯源",
        }
    }

    pub fn url(self) -> &'static str {
        match self {
            Registry::Official => "https://registry.npmjs.org/",
            Registry::Npmmirror => "https://registry.npmmirror.com",
            Registry::Tencent => "https://mirrors.cloud.tencent.com/npm/",
        }
    }
}

/// 启动 DSH 的失败类型
#[derive(Debug)]
pub enum StartError {
    /// 环境中没有 dsh，需要用户确认后安装
    NotInstalled { errors: Vec<String> },
}

/// 成功拉起并解析出地址的子进程
pub struct SpawnedDsh {
    pub child: ChildGuard,
    pub url: String,
}

/// 进程树回收 guard：任何路径（任务 abort / drop / 正常回收）下，
/// drop 都会同步终止整个子进程树（Windows 用 taskkill /T /F）。
pub struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    pub fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    /// 可变访问内部 Child（如取 stderr 管道）。
    pub fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("ChildGuard 已空")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            #[cfg(target_os = "windows")]
            {
                // cmd shim 会再拉起 node，必须按进程树终止。
                // 用 spawn 不等待（fire-and-forget）：drop 里同步等待 taskkill
                // 会阻塞事件循环导致桌宠卡死。
                if let Some(pid) = child.id() {
                    let _ = std::process::Command::new("taskkill")
                        .args(["/PID", &pid.to_string(), "/T", "/F"])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                }
            }
            #[cfg(not(target_os = "windows"))]
            {
                let mut child = child;
                let _ = child.start_kill();
            }
        }
    }
}

/// 启动 DSH：先找 PATH 中的 dsh，再找自定义安装目录里的 dsh；
/// 找不到则返回 `NotInstalled`，由上层弹窗让用户选择是否安装及安装源。
pub async fn start(custom_dir: Option<&Path>) -> Result<SpawnedDsh, StartError> {
    let mut errors: Vec<String> = Vec::new();

    for port in [PREFERRED_PORT, FALLBACK_PORT] {
        if let Some(spawned) = try_existing_candidates(port, custom_dir, &mut errors).await {
            return Ok(spawned);
        }
    }

    Err(StartError::NotInstalled { errors })
}

/// 尝试已安装的 dsh：PATH 中的 `dsh` / Windows `cmd /C dsh`，
/// 以及自定义安装目录（`--prefix` 目录）中的 dsh 可执行文件。
async fn try_existing_candidates(
    port: &str,
    custom_dir: Option<&Path>,
    errors: &mut Vec<String>,
) -> Option<SpawnedDsh> {
    let args = ["--profile", "web", "--host", "127.0.0.1", "--port", port];

    match try_launch("dsh", &args, DIRECT_TIMEOUT).await {
        Ok(s) => return Some(s),
        Err(e) => errors.push(e),
    }

    #[cfg(target_os = "windows")]
    {
        let mut cmd_args = vec!["/C", "dsh"];
        cmd_args.extend_from_slice(&args);
        match try_launch("cmd", &cmd_args, DIRECT_TIMEOUT).await {
            Ok(s) => return Some(s),
            Err(e) => errors.push(e),
        }
    }

    if let Some(dir) = custom_dir {
        for candidate in custom_dsh_candidates(dir) {
            let program = candidate.to_string_lossy().into_owned();
            match try_launch(&program, &args, DIRECT_TIMEOUT).await {
                Ok(s) => return Some(s),
                Err(e) => errors.push(e),
            }
        }

        #[cfg(target_os = "windows")]
        {
            // npm --prefix 在 Windows 生成的 dsh.cmd shim 需经 cmd /C 执行
            let cmd_script = dir.join("dsh.cmd");
            if cmd_script.exists() {
                let script = cmd_script.to_string_lossy().into_owned();
                let mut cmd_args: Vec<&str> = vec!["/C", &script];
                cmd_args.extend_from_slice(&args);
                match try_launch("cmd", &cmd_args, DIRECT_TIMEOUT).await {
                    Ok(s) => return Some(s),
                    Err(e) => errors.push(e),
                }
            }
        }
    }

    None
}

/// 自定义安装目录中的 dsh 可执行文件候选。
fn custom_dsh_candidates(dir: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(target_os = "windows")]
    {
        candidates.push(dir.join("dsh.exe"));
        candidates.push(dir.join("dsh"));
    }
    #[cfg(not(target_os = "windows"))]
    {
        candidates.push(dir.join("bin").join("dsh"));
        candidates.push(dir.join("dsh"));
    }
    candidates
}

/// npm 全局安装 @deepseek-ai/dsh（指定安装源与可选安装目录），并逐行回传下载进度。
pub async fn install<F>(
    registry: Registry,
    install_dir: Option<&Path>,
    on_line: F,
) -> Result<(), String>
where
    F: Fn(String) + Send + Sync + 'static,
{
    if let Some(dir) = install_dir {
        if let Err(e) = std::fs::create_dir_all(dir) {
            return Err(format!("创建安装目录失败: {e}"));
        }
    }
    let args = install_args(registry, install_dir);
    let status = npm_install_status(&args, &on_line).await?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "npm install 退出码 {}",
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "unknown".into())
        ))
    }
}

/// 构造 npm 安装参数，可选追加 `--prefix <目录>`。
pub fn install_args(registry: Registry, install_dir: Option<&Path>) -> Vec<String> {
    let mut args = vec![
        "install".to_string(),
        "-g".to_string(),
        "@deepseek-ai/dsh".to_string(),
        "--verbose".to_string(),
        "--registry".to_string(),
        registry.url().to_string(),
    ];
    if let Some(dir) = install_dir {
        args.push("--prefix".to_string());
        args.push(dir.to_string_lossy().into_owned());
    }
    args
}

async fn npm_install_status<F>(
    args: &[String],
    on_line: &F,
) -> Result<std::process::ExitStatus, String>
where
    F: Fn(String) + Send + Sync + 'static,
{
    match run_streaming("npm", args, on_line).await {
        Ok(status) => Ok(status),
        Err(_e) => {
            #[cfg(target_os = "windows")]
            {
                on_line("npm 不在 PATH，尝试 cmd /C npm…".to_string());
                let mut cmd_args = vec!["/C".to_string(), "npm".to_string()];
                cmd_args.extend(args.iter().cloned());
                run_streaming("cmd", &cmd_args, on_line).await
            }
            #[cfg(not(target_os = "windows"))]
            {
                Err(_e)
            }
        }
    }
}

/// 运行一个命令，合并 stdout/stderr 并逐行交给回调，返回进程退出状态。
async fn run_streaming<F>(
    program: &str,
    args: &[String],
    on_line: &F,
) -> Result<std::process::ExitStatus, String>
where
    F: Fn(String) + Send + Sync + 'static,
{
    let mut cmd = Command::new(program);
    cmd.args(args.iter().map(|s| s.as_str()))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(target_os = "windows")]
    {
        // CREATE_NO_WINDOW：后台静默启动，不弹出 CLI/控制台窗口
        cmd.creation_flags(0x0800_0000);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("无法执行 {program}: {e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{program}: 无法接管 stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("{program}: 无法接管 stderr"))?;

    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();
    let mut stdout_done = false;
    let mut stderr_done = false;

    while !(stdout_done && stderr_done) {
        tokio::select! {
            line = stdout_lines.next_line(), if !stdout_done => {
                match line {
                    Ok(Some(line)) => on_line(crate::term::clean_line(&line)),
                    Ok(None) => stdout_done = true,
                    Err(e) => {
                        on_line(format!("[stdout] {e}"));
                        stdout_done = true;
                    }
                }
            }
            line = stderr_lines.next_line(), if !stderr_done => {
                match line {
                    Ok(Some(line)) => on_line(crate::term::clean_line(&line)),
                    Ok(None) => stderr_done = true,
                    Err(e) => {
                        on_line(format!("[stderr] {e}"));
                        stderr_done = true;
                    }
                }
            }
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| format!("等待 {program} 退出失败: {e}"))?;
    Ok(status)
}

/// 以给定命令启动并等待地址输出；失败时回收子进程并返回错误说明。
async fn try_launch(program: &str, args: &[&str], wait: Duration) -> Result<SpawnedDsh, String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false);
    #[cfg(target_os = "windows")]
    {
        // CREATE_NO_WINDOW：后台静默启动，不弹出 CLI/控制台窗口
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Err(format!("无法执行 {program}: {e}")),
    };
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => return Err(format!("{program}: 无法接管 stdout")),
    };
    // 立即包上回收 guard：此后任何路径（含任务被 abort）drop 都会杀进程树
    let mut guard = ChildGuard::new(child);
    // 排空 stderr（避免管道阻塞；同时记录日志）
    if let Some(stderr) = guard.child_mut().stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!("[dsh-child] {line}");
            }
        });
    }

    let result = timeout(wait, read_url(stdout)).await;
    match result {
        Ok(Ok(url)) => Ok(SpawnedDsh { child: guard, url }),
        Ok(Err(e)) => Err(format!("{program}: {e}")),
        Err(_) => Err(format!(
            "{program}: 启动超时（{}s 内未输出地址）",
            wait.as_secs()
        )),
    }
}

/// 逐行读取 stdout，返回第一个含 "http://" 的地址。
async fn read_url(stdout: ChildStdout) -> Result<String, String> {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(url) = extract_url(&line) {
            return Ok(url);
        }
    }
    Err("子进程提前退出，未输出 DSH 地址".to_string())
}

/// 从 "dsh web: http://127.0.0.1:12345" 之类的行提取 URL。
/// 只取到首个空白字符为止，避免把行尾 "(LAN: http://...)" 之类的附加说明带进 URL。
fn extract_url(line: &str) -> Option<String> {
    for scheme in ["http://", "https://"] {
        if let Some(start) = line.find(scheme) {
            let rest = &line[start + scheme.len()..];
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            return Some(format!("{scheme}{}", &rest[..end]));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn install_args_without_dir() {
        let args = install_args(Registry::Official, None);
        assert!(!args.iter().any(|a| a == "--prefix"));
    }

    #[test]
    fn install_args_with_dir() {
        let args = install_args(Registry::Npmmirror, Some(Path::new("D:\\dsh")));
        let idx = args.iter().position(|a| a == "--prefix").unwrap();
        assert_eq!(args[idx + 1], "D:\\dsh");
    }

    #[test]
    fn extracts_url_from_banner() {
        assert_eq!(
            extract_url("dsh web: http://127.0.0.1:63198"),
            Some("http://127.0.0.1:63198".to_string())
        );
    }

    #[test]
    fn truncates_trailing_lan_annotation() {
        assert_eq!(
            extract_url("dsh web: http://127.0.0.1:3080 (LAN: http://172.29.32.1:3080)"),
            Some("http://127.0.0.1:3080".to_string())
        );
    }

    #[test]
    fn ignores_plain_lines() {
        assert_eq!(extract_url("some log line"), None);
        assert_eq!(extract_url(""), None);
    }

    /// ChildGuard drop 必须能终止整棵进程树且不挂起（同步路径）。
    #[test]
    fn guard_drop_kills_process_tree() {
        let child = Command::new("cmd")
            .args(["/C", "ping", "-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut guard = ChildGuard::new(child);
        let pid = guard.child_mut().id().unwrap();
        drop(guard);
        // 同步 taskkill 后，进程应在数秒内消失
        let gone = std::time::Instant::now();
        loop {
            let alive = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
                .unwrap_or(false);
            if !alive {
                break;
            }
            assert!(
                gone.elapsed() < Duration::from_secs(10),
                "guard drop 后进程 {pid} 应在 10s 内退出"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// 模拟任务被 abort：guard 在 async 块内 drop，子进程也必须被回收。
    #[tokio::test]
    async fn guard_drop_on_abort_kills_child() {
        let pid_slot = std::sync::Arc::new(std::sync::Mutex::new(None::<u32>));
        let task = tokio::spawn({
            let pid_slot = pid_slot.clone();
            async move {
                let child = Command::new("cmd")
                    .args(["/C", "ping", "-n", "30", "127.0.0.1"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                let mut g = ChildGuard::new(child);
                let pid = g.child_mut().id().unwrap();
                *pid_slot.lock().unwrap() = Some(pid);
                // 永远不返回：任务稍后被 abort，guard 随任务 drop
                std::future::pending::<()>().await;
            }
        });
        // 等任务启动并注册 pid
        let pid = loop {
            if let Some(pid) = *pid_slot.lock().unwrap() {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        task.abort();
        let _ = task.await;
        // abort 后 guard drop → 进程树被杀
        let gone = std::time::Instant::now();
        loop {
            let alive = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
                .unwrap_or(false);
            if !alive {
                break;
            }
            assert!(
                gone.elapsed() < Duration::from_secs(10),
                "abort 后进程 {pid} 应在 10s 内退出"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
