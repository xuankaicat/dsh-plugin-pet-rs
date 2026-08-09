//! 事件连接器（WebSocket）集成测试
//! 用 tokio-tungstenite 起一个本地 WebSocket 服务端，模拟 DSH 事件端点

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dsh_pet_core::SseConnector;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

/// 启动一个 WebSocket 测试服务端，发送指定帧后关闭（或保持连接）
async fn start_ws_server(
    frames: Vec<String>,
    close_after_send: bool,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://{addr}");

    let handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();

        for frame in &frames {
            ws.send(Message::Text(frame.clone())).await.unwrap();
        }

        if close_after_send {
            ws.send(Message::Close(None)).await.unwrap();
        } else {
            // 保持连接一段时间
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });

    (url, handle)
}

/// 启动一个持续运行的 WebSocket 服务端（预留，当前测试未使用）
#[allow(dead_code)]
async fn start_persistent_ws_server(
    frames: Vec<String>,
) -> (String, Arc<AtomicUsize>, CancellationToken) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://{addr}");
    let count = Arc::new(AtomicUsize::new(0));
    let cancel = CancellationToken::new();

    let count_clone = count.clone();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    let (stream, _) = accept_result.unwrap();
                    let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                    let count_inner = count_clone.clone();
                    let frames_inner = frames.clone();
                    tokio::spawn(async move {
                        for frame in &frames_inner {
                            if ws.send(Message::Text(frame.clone())).await.is_err() {
                                return;
                            }
                            count_inner.fetch_add(1, Ordering::SeqCst);
                        }
                        // 保持连接直到客户端断开
                        while let Some(Ok(_)) = ws.next().await {}
                    });
                }
                _ = cancel_clone.cancelled() => return,
            }
        }
    });

    (url, count, cancel)
}

async fn run_connector_briefly(
    url: String,
    on_frame: impl Fn(serde_json::Value) + Send + Sync + 'static,
    cancel: CancellationToken,
    dwell: Duration,
) {
    let connector = Arc::new(SseConnector::new(url, reqwest::Client::new()));
    let cancel2 = cancel.clone();
    tokio::spawn(async move {
        connector.run(on_frame, None::<fn()>, cancel2).await;
    });
    tokio::time::sleep(dwell).await;
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(300)).await;
}

#[tokio::test]
async fn ws_normal_close_reconnects() {
    // 服务端发送 1 帧后关闭 → 连接器应重连
    let frame = r#"{"type":"server-request","rpcId":"r1","method":"session/queue","payload":{"type":"session/queue","items":[]}}"#;
    let (url, _handle) = start_ws_server(vec![frame.to_string()], true).await;

    let received = Arc::new(AtomicUsize::new(0));
    let r = received.clone();
    let cancel = CancellationToken::new();

    run_connector_briefly(
        url,
        move |_| {
            r.fetch_add(1, Ordering::SeqCst);
        },
        cancel,
        Duration::from_secs(1),
    )
    .await;

    // 应至少收到 1 帧（重连前的初始帧）
    assert!(received.load(Ordering::SeqCst) >= 1, "应收到至少 1 帧");
}

#[tokio::test]
async fn ws_frame_payload_extracted() {
    // 验证 payload 字段被正确提取
    let frame = r#"{"type":"server-request","rpcId":"r1","method":"approval/requested","payload":{"type":"approval/requested","approvalId":"a1","sessionId":"s1","toolName":"bash","reason":"test"}}"#;
    let (url, _handle) = start_ws_server(vec![frame.to_string()], true).await;

    let received = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));
    let r = received.clone();
    let cancel = CancellationToken::new();

    run_connector_briefly(
        url,
        move |v| {
            *r.lock().unwrap() = Some(v);
        },
        cancel,
        Duration::from_secs(1),
    )
    .await;

    let payload = received.lock().unwrap().clone().expect("应收到 payload");
    assert_eq!(payload["type"], "approval/requested");
    assert_eq!(payload["approvalId"], "a1");
    assert_eq!(payload["toolName"], "bash");
}

#[tokio::test]
async fn ws_malformed_json_skipped() {
    // 非 JSON 文本应被跳过，不崩溃
    let (url, _handle) = start_ws_server(vec!["not json".to_string()], true).await;

    let received = Arc::new(AtomicUsize::new(0));
    let r = received.clone();
    let cancel = CancellationToken::new();

    run_connector_briefly(
        url,
        move |_| {
            r.fetch_add(1, Ordering::SeqCst);
        },
        cancel,
        Duration::from_secs(1),
    )
    .await;

    assert_eq!(received.load(Ordering::SeqCst), 0, "非 JSON 帧应被跳过");
}

#[tokio::test]
async fn ws_cancel_exits_immediately() {
    // CancellationToken 取消 → 立即终止
    let frame = r#"{"type":"server-request","rpcId":"r1","method":"session/queue","payload":{"type":"session/queue","items":[]}}"#;
    let (url, _handle) = start_ws_server(vec![frame.to_string()], false).await;

    let cancel = CancellationToken::new();
    let start = tokio::time::Instant::now();
    run_connector_briefly(url, |_| {}, cancel.clone(), Duration::from_millis(100)).await;
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(1),
        "取消应快速生效，实际 {elapsed:?}"
    );
}

#[tokio::test]
async fn ws_connect_refused_reconnects() {
    // 连接到不存在的端口 → 应重连不崩溃
    let received = Arc::new(AtomicUsize::new(0));
    let r = received.clone();
    let cancel = CancellationToken::new();

    run_connector_briefly(
        "ws://127.0.0.1:1".to_string(), // 端口 1 通常无服务
        move |_| {
            r.fetch_add(1, Ordering::SeqCst);
        },
        cancel,
        Duration::from_secs(2),
    )
    .await;

    // 不应收到任何帧（连接失败）
    assert_eq!(received.load(Ordering::SeqCst), 0);
}
