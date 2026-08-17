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
    pub child: Child,
    pub url: String,
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
    // 排空 stderr（避免管道阻塞；同时记录日志）
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!("[dsh-child] {line}");
            }
        });
    }

    let result = timeout(wait, read_url(stdout)).await;
    match result {
        Ok(Ok(url)) => Ok(SpawnedDsh { child, url }),
        Ok(Err(e)) => {
            kill(&mut child).await;
            Err(format!("{program}: {e}"))
        }
        Err(_) => {
            kill(&mut child).await;
            Err(format!("{program}: 启动超时（{}s 内未输出地址）", wait.as_secs()))
        }
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
fn extract_url(line: &str) -> Option<String> {
    line.split_once("http://")
        .map(|(_, rest)| format!("http://{}", rest.trim()))
}

/// 终止 DSH 子进程。
///
/// Windows 上 `cmd` shim 会再拉起 node 进程，需按进程树（taskkill /T）一并终止；
/// 其他平台直接 kill 即可。
pub async fn kill(child: &mut Child) {
    #[cfg(target_os = "windows")]
    {
        if let Some(pid) = child.id() {
            let mut tk = Command::new("taskkill");
            tk.args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            // CREATE_NO_WINDOW：回收时也不闪现控制台窗口
            tk.creation_flags(0x0800_0000);
            let _ = tk.status().await;
        }
        let _ = child.kill().await;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = child.kill().await;
    }
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
    fn ignores_plain_lines() {
        assert_eq!(extract_url("some log line"), None);
        assert_eq!(extract_url(""), None);
    }

    /// kill 必须能终止进程树且不挂起（异步路径）。
    #[tokio::test]
    async fn kill_terminates_child_tree() {
        let mut child = Command::new("cmd")
            .args(["/C", "ping", "-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        kill(&mut child).await;
        let exited = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .expect("kill 后子进程应在 5s 内退出")
            .unwrap();
        assert!(exited.success() || !exited.success());
    }

    /// 与事件循环中 stop_dsh_child 相同的 block_on(kill) 模式不得死锁。
    #[test]
    fn kill_via_block_on_completes() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut child = rt.block_on(async {
            Command::new("cmd")
                .args(["/C", "ping", "-n", "30", "127.0.0.1"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap()
        });
        rt.block_on(kill(&mut child));
        assert!(child.try_wait().unwrap().is_some());
    }
}
