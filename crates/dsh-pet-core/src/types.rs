//! 核心类型定义：鲸鱼状态、会话、待决项、UI 快照、内部事件。

use serde::{Deserialize, Serialize};

/// 鲸鱼状态（优先级从高到低：starting > offline > attention > working > done > idle）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mode {
    Starting,
    Offline,
    Attention,
    Working,
    Done,
    Idle,
}

impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Starting => "starting",
            Mode::Offline => "offline",
            Mode::Attention => "attention",
            Mode::Working => "working",
            Mode::Done => "done",
            Mode::Idle => "idle",
        }
    }

    /// 是否为有动画的状态（用于决定是否 request_redraw）
    pub fn is_animating(self) -> bool {
        matches!(
            self,
            Mode::Working | Mode::Done | Mode::Idle | Mode::Attention | Mode::Starting
        )
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// DSH 会话（state 内部存储用）
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub running: bool,
    pub todos: Option<serde_json::Value>,
}

/// 待审批项
#[derive(Debug, Clone)]
pub struct Approval {
    pub approval_id: String,
    pub session_id: String,
    pub tool_name: String,
    pub reason: String,
    pub requested_at: u64, // epoch_ms
}

/// 待回答提问
#[derive(Debug, Clone)]
pub struct Question {
    pub question_rpc_id: String,
    pub session_id: String,
    pub text: String,
    pub requested_at: u64,
}

/// 完成待查看条目
#[derive(Debug, Clone)]
pub struct DoneEntry {
    pub session_id: String,
    pub title: String,
    pub at: u64, // epoch_ms
}

/// UI 渲染用的快照（等价于 main.js buildSnapshot() 的返回值）
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Snapshot {
    pub mode: Mode,
    pub bubble: Bubble,
    pub running: Vec<SessionRef>,
    pub attention: Vec<AttentionItem>,
    pub done: Vec<DoneRef>,
    pub queued: usize,
}

impl Snapshot {
    /// 启动初始快照
    pub fn starting() -> Self {
        Self {
            mode: Mode::Starting,
            bubble: Bubble {
                title: "启动中…".to_string(),
                body: "正在连接 DSH".to_string(),
            },
            running: vec![],
            attention: vec![],
            done: vec![],
            queued: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Bubble {
    pub title: String,
    pub body: String, // \n 分隔多行
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SessionRef {
    pub session_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AttentionItem {
    pub kind: AttentionKind,
    pub session_id: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttentionKind {
    Approval,
    Question,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DoneRef {
    pub session_id: String,
    pub title: String,
}

/// 内部状态事件（三个异步任务 → state 聚合器）
#[derive(Debug)]
pub enum StateEvent {
    /// session.list 轮询结果
    Poll {
        sessions: Vec<SessionItem>,
        ok: bool,
        error: Option<String>,
    },
    /// /api/events.mux 推送帧
    MuxFrame(serde_json::Value),
    /// /api/events.host 推送帧
    HostFrame(serde_json::Value),
    /// 定时心跳（30s），用于过期 TTL 清理
    Tick,
}

/// session.list 返回的原始项（DSH API 格式）
#[derive(Debug, Clone, Deserialize)]
pub struct SessionItem {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub running: bool,
    #[serde(default)]
    pub projections: Option<Projections>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Projections {
    #[serde(default)]
    pub values: Option<ProjectionValues>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectionValues {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub todos: Option<serde_json::Value>,
}

/// RPC 响应
#[derive(Debug, Deserialize)]
pub struct RpcResponse {
    pub result: RpcResult,
}

#[derive(Debug, Deserialize)]
pub struct RpcResult {
    pub ok: bool,
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
pub struct RpcError {
    pub message: String,
}

/// 当前 epoch 毫秒
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
