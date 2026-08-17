//! DSH 子进程管理：由桌宠拉起 `dsh --profile web` 并负责回收。
//!
//! 启动方式按优先级自动尝试（哪个能跑起来用哪个）：
//! 1. `dsh` —— PATH 中的真实可执行文件（Unix 主要路径）；
//! 2. `cmd /C dsh ...` —— Windows 上解析 PATH 中的 `.cmd` shim（npm 全局安装）；
//! 3. `npx --yes @deepseek-ai/dsh ...` —— npm on-demand，无需全局安装。
//!
//! 统一使用 `--port 0` 让系统分配空闲端口，实际地址由子进程 stdout
//! 输出一行 `dsh web: http://127.0.0.1:<port>`，本模块负责解析。

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};
use tokio::time::timeout;

/// 单个候选启动方式的等待超时
const DIRECT_TIMEOUT: Duration = Duration::from_secs(8);
const NPX_TIMEOUT: Duration = Duration::from_secs(60);

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

/// 按优先级依次尝试所有候选启动方式，返回第一个解析出地址的。
pub async fn start() -> Result<SpawnedDsh, String> {
    let mut errors: Vec<String> = Vec::new();
    let args = ["--profile", "web", "--host", "127.0.0.1", "--port", "0"];

    // 1) PATH 中的 dsh（Unix 上的可执行脚本；Windows 上若存在 dsh.exe 也可用）
    match try_launch("dsh", &args, DIRECT_TIMEOUT).await {
        Ok(s) => return Ok(s),
        Err(e) => errors.push(e),
    }

    #[cfg(target_os = "windows")]
    {
        // 2) cmd /C dsh（解析 npm 全局安装产生的 dsh.cmd shim）
        let mut cmd_args = vec!["/C", "dsh"];
        cmd_args.extend_from_slice(&args);
        match try_launch("cmd", &cmd_args, DIRECT_TIMEOUT).await {
            Ok(s) => return Ok(s),
            Err(e) => errors.push(e),
        }

        // 3) npx on-demand（用户常见用法：npx @deepseek-ai/dsh web）
        let mut npx_args = vec!["/C", "npx", "--yes", "@deepseek-ai/dsh"];
        npx_args.extend_from_slice(&args);
        match try_launch("cmd", &npx_args, NPX_TIMEOUT).await {
            Ok(s) => return Ok(s),
            Err(e) => errors.push(e),
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        // 2) npx on-demand（Unix 直接执行 npx）
        let mut npx_args = vec!["--yes", "@deepseek-ai/dsh"];
        npx_args.extend_from_slice(&args);
        match try_launch("npx", &npx_args, NPX_TIMEOUT).await {
            Ok(s) => return Ok(s),
            Err(e) => errors.push(e),
        }
    }

    Err(format!(
        "无法启动 DSH：{}。可执行 npm install -g @deepseek-ai/dsh 安装，或确保 dsh 命令在 PATH 中",
        errors.join("；")
    ))
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
        Err(_) => Err(format!("{program}: 启动超时（{}s 内未输出地址）", wait.as_secs())),
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
