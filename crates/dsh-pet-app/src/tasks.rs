//! 异步任务：轮询 session.list + 双 SSE + 定时心跳。
//!
//! 对应 main.js L77-177 的 pollSessions / connectMux / connectHost。

use std::sync::Arc;
use std::time::Duration;

use dsh_pet_core::{Config, RpcClient, SseConnector, StateEvent};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// 轮询间隔
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// 轮询 session.list（2s 间隔）
pub async fn poll_task(
    client: Arc<RpcClient>,
    tx: mpsc::Sender<StateEvent>,
    cancel: CancellationToken,
) {
    loop {
        match client.session_list().await {
            Ok(sessions) => {
                let _ = tx
                    .send(StateEvent::Poll {
                        sessions,
                        ok: true,
                        error: None,
                    })
                    .await;
            }
            Err(e) => {
                let _ = tx
                    .send(StateEvent::Poll {
                        sessions: vec![],
                        ok: false,
                        error: Some(e.to_string()),
                    })
                    .await;
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(POLL_INTERVAL) => {}
            _ = cancel.cancelled() => return,
        }
    }
}

/// SSE /api/events.mux（审批/提问/队列推送）
pub async fn sse_mux_task(
    sse: Arc<SseConnector>,
    tx: mpsc::Sender<StateEvent>,
    cancel: CancellationToken,
) {
    let tx_clone = tx.clone();
    sse.run(
        move |frame| {
            let _ = tx_clone.try_send(StateEvent::MuxFrame(frame));
        },
        None::<fn()>,
        cancel,
    )
    .await;
}

/// SSE /api/events.host（running 翻转即时推送；重连后补一次轮询）
pub async fn sse_host_task(
    sse: Arc<SseConnector>,
    client: Arc<RpcClient>,
    tx: mpsc::Sender<StateEvent>,
    cancel: CancellationToken,
) {
    let tx_for_frame = tx.clone();
    let tx_for_reconnect = tx.clone();
    let client_for_reconnect = client.clone();
    sse.run(
        move |frame| {
            let _ = tx_for_frame.try_send(StateEvent::HostFrame(frame));
        },
        Some(move || {
            // 重连后立即轮询补基线（防止漏掉连接期间的翻转）
            let client = client_for_reconnect.clone();
            let tx = tx_for_reconnect.clone();
            tokio::spawn(async move {
                if let Ok(sessions) = client.session_list().await {
                    let _ = tx
                        .send(StateEvent::Poll {
                            sessions,
                            ok: true,
                            error: None,
                        })
                        .await;
                }
            });
        }),
        cancel,
    )
    .await;
}

/// 定时心跳（30s），用于 TTL 过期清理
pub async fn tick_task(tx: mpsc::Sender<StateEvent>, cancel: CancellationToken) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(30)) => {
                let _ = tx.send(StateEvent::Tick).await;
            }
            _ = cancel.cancelled() => return,
        }
    }
}

/// 创建 RPC + 双 SSE 客户端并 spawn 三个网络任务，返回用于取消的 CancellationToken。
///
/// endpoint 变更时调用此函数重建网络任务（热切换）。
pub fn spawn_network_tasks(
    rt: &tokio::runtime::Runtime,
    endpoint: &str,
    http: &reqwest::Client,
    event_tx: mpsc::Sender<StateEvent>,
) -> CancellationToken {
    let cancel = CancellationToken::new();
    let rpc_client = Arc::new(RpcClient::new(endpoint.to_string(), http.clone()));
    let sse_mux = Arc::new(SseConnector::new(
        format!("{endpoint}/api/events.mux"),
        http.clone(),
    ));
    let sse_host = Arc::new(SseConnector::new(
        format!("{endpoint}/api/events.host"),
        http.clone(),
    ));
    rt.spawn(poll_task(
        rpc_client.clone(),
        event_tx.clone(),
        cancel.clone(),
    ));
    rt.spawn(sse_mux_task(sse_mux, event_tx.clone(), cancel.clone()));
    rt.spawn(sse_host_task(
        sse_host,
        rpc_client,
        event_tx,
        cancel.clone(),
    ));
    cancel
}

/// 校验 endpoint 字符串是否为合法的 http/https URL。
pub fn is_valid_endpoint(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty() && (s.starts_with("http://") || s.starts_with("https://"))
}

/// 确认 `Config::ENDPOINT_DEFAULT` 非空（编译期检查）。
const _: () = {
    assert!(!Config::ENDPOINT_DEFAULT.is_empty());
};
