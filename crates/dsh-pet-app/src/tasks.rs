//! 异步任务：轮询 session.list + 双 SSE + 定时心跳。
//!
//! 对应 main.js L77-177 的 pollSessions / connectMux / connectHost。

use std::sync::Arc;
use std::time::Duration;

use dsh_pet_core::{RpcClient, SseConnector, StateEvent};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// 轮询间隔
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// 默认 DSH URL
pub const DSH_URL_DEFAULT: &str = "http://127.0.0.1:3080";

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
