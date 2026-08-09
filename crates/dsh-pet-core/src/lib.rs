//! DSH 桌宠核心：状态机、DSH API 客户端、SSE 连接器、配置、素材包。
//!
//! 本 crate 纯逻辑无 UI 依赖，可独立测试。

pub mod assets;
pub mod config;
pub mod rpc;
pub mod sse;
pub mod state;
pub mod types;

pub use assets::{Spout, SpritePack};
pub use config::Config;
pub use rpc::RpcClient;
pub use sse::SseConnector;
pub use state::PetState;
pub use types::{
    now_ms, Approval, AttentionItem, AttentionKind, Bubble, DoneEntry, DoneRef, Mode, Question,
    Session, SessionItem, SessionRef, Snapshot, StateEvent,
};
