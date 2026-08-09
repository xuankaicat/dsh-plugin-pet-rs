//! DSH JSON-RPC 客户端。
//!
//! 对应 main.js L51-68 的 rpc() 函数。

use anyhow::{bail, Result};
use serde_json::json;

use crate::types::{RpcResponse, SessionItem};

pub struct RpcClient {
    base_url: String,
    http: reqwest::Client,
}

impl RpcClient {
    pub fn new(base_url: String, http: reqwest::Client) -> Self {
        Self { base_url, http }
    }

    /// 调用 session.list，返回当前所有会话
    pub async fn session_list(&self) -> Result<Vec<SessionItem>> {
        let rpc_id = format!("pet-{}-{:x}", crate::types::now_ms(), rand::random::<u32>());
        let body = json!({
            "type": "client-request",
            "rpcId": rpc_id,
            "method": "session.list",
            "payload": {},
        });
        let resp = self
            .http
            .post(format!("{}/api/session.list", self.base_url))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("HTTP {}", resp.status());
        }
        let rpc_resp: RpcResponse = resp.json().await?;
        if !rpc_resp.result.ok {
            bail!(
                "{}",
                rpc_resp
                    .result
                    .error
                    .map(|e| e.message)
                    .unwrap_or_else(|| "rpc error".into())
            );
        }
        let value = rpc_resp.result.value.unwrap_or(json!({}));
        let items = value
            .get("items")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let sessions: Vec<SessionItem> = items
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();
        Ok(sessions)
    }
}
