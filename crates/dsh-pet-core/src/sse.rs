//! 事件连接器：通过 WebSocket 连接 DSH 的 /api/events.* 端点。
//!
//! DSH 服务器的事件端点要求 WebSocket 升级（HTTP GET 会返回 426 Upgrade Required）。
//! 每个 WebSocket 文本消息是一个 JSON 信封 `{type:'server-request', rpcId, method, payload}`，
//! 其中 `payload` 是实际的 MuxFrame / HostFrame。
//!
//! 连接器是单向的（server → client only）；客户端发送任何消息都会被服务端关闭。
//! 断线自动重连（3s 退避），支持 CancellationToken 取消。
//!
//! 注：类型名保留 `SseConnector` 是历史原因（spec 原假设为 SSE，实际 DSH 用 WebSocket）。

use std::future::Future;
use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

pub struct SseConnector {
    url: String,
}

impl SseConnector {
    pub fn new(url: String, _http: reqwest::Client) -> Self {
        Self { url }
    }

    /// 主循环：连接 → 读帧 → 断开后退避 3s 重连，直到 cancel。
    ///
    /// - `on_frame`：每收到一个 WebSocket 文本消息的 payload 调用
    /// - `on_reconnect`：每次断开重连前调用（用于清空 pending 状态等）
    pub async fn run<F, G>(&self, on_frame: F, on_reconnect: Option<G>, cancel: CancellationToken)
    where
        F: Fn(serde_json::Value) + Send + Sync + 'static,
        G: Fn() + Send + Sync + 'static,
    {
        loop {
            match self.connect_and_read(&on_frame).await {
                Ok(()) => tracing::info!("WebSocket {} 流正常结束", self.url),
                Err(e) => {
                    if cancel.is_cancelled() {
                        return;
                    }
                    tracing::warn!("WebSocket {} 错误: {e}", self.url);
                }
            }
            if cancel.is_cancelled() {
                return;
            }
            if let Some(g) = &on_reconnect {
                g();
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(3)) => {}
                _ = cancel.cancelled() => return,
            }
        }
    }

    /// 单次连接：WebSocket 握手 → 读文本帧 → 解析 JSON → 提取 payload
    async fn connect_and_read<F: Fn(serde_json::Value)>(&self, on_frame: &F) -> Result<()> {
        // http:// → ws://, https:// → wss://
        let ws_url = self
            .url
            .replace("https://", "wss://")
            .replace("http://", "ws://");

        tracing::debug!("WebSocket 连接 {ws_url}");
        let (ws_stream, response) = tokio_tungstenite::connect_async(&ws_url).await?;
        tracing::info!("WebSocket {} 已连接 (HTTP {})", self.url, response.status());

        let mut stream = ws_stream;

        while let Some(msg_result) = stream.next().await {
            let msg = msg_result?;
            match msg {
                tokio_tungstenite::tungstenite::Message::Text(text) => {
                    if let Some(payload) = parse_frame(&text) {
                        on_frame(payload);
                    }
                }
                tokio_tungstenite::tungstenite::Message::Close(_) => {
                    tracing::info!("WebSocket {} 收到关闭帧", self.url);
                    break;
                }
                tokio_tungstenite::tungstenite::Message::Ping(_)
                | tokio_tungstenite::tungstenite::Message::Pong(_) => {
                    // tungstenite 自动处理 Ping/Pong
                }
                tokio_tungstenite::tungstenite::Message::Binary(_) => {
                    // 忽略二进制消息
                }
                _ => {}
            }
        }

        anyhow::bail!("websocket closed");
    }
}

/// 解析 WebSocket 文本帧：JSON 信封 → 提取 payload
///
/// 信封格式：`{type:'server-request', rpcId, method, payload}`
/// 返回 `payload` 字段（即 MuxFrame / HostFrame），无 payload 则返回整个 JSON。
pub fn parse_frame(text: &str) -> Option<serde_json::Value> {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(envelope) => {
            let payload = envelope.get("payload").unwrap_or(&envelope).clone();
            Some(payload)
        }
        Err(e) => {
            tracing::warn!("WebSocket 帧解析失败: {e}");
            None
        }
    }
}

/// 便捷封装：启动一个事件任务，返回一个 Future（spawn 用）
#[allow(clippy::manual_async_fn)]
pub fn spawn_event_task<F, G>(
    connector: std::sync::Arc<SseConnector>,
    on_frame: F,
    on_reconnect: Option<G>,
    cancel: CancellationToken,
) -> impl Future<Output = ()>
where
    F: Fn(serde_json::Value) + Send + Sync + 'static,
    G: Fn() + Send + Sync + 'static,
{
    async move { connector.run(on_frame, on_reconnect, cancel).await }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frame_extracts_payload() {
        let text = r#"{"type":"server-request","rpcId":"r1","method":"session/queue","payload":{"type":"session/queue","items":[]}}"#;
        let payload = parse_frame(text).unwrap();
        assert_eq!(payload["type"], "session/queue");
        assert_eq!(payload["items"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn parse_frame_no_payload_returns_whole() {
        let text = r#"{"type":"session/queue","items":[]}"#;
        let payload = parse_frame(text).unwrap();
        assert_eq!(payload["type"], "session/queue");
    }

    #[test]
    fn parse_frame_malformed_returns_none() {
        assert!(parse_frame("not json").is_none());
        assert!(parse_frame("").is_none());
    }

    #[test]
    fn parse_frame_approval_requested() {
        let text = r#"{"type":"server-request","rpcId":"r1","method":"approval/requested","payload":{"type":"approval/requested","approvalId":"a1","sessionId":"s1","toolName":"bash","reason":"test"}}"#;
        let payload = parse_frame(text).unwrap();
        assert_eq!(payload["type"], "approval/requested");
        assert_eq!(payload["approvalId"], "a1");
        assert_eq!(payload["toolName"], "bash");
    }
}
