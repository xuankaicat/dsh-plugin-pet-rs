//! 状态聚合器：消费 StateEvent，维护内部状态，构建渲染快照。
//!
//! 对应 main.js 中 pollSessions / handleFrame / handleHostFrame / buildSnapshot 的逻辑。

use std::collections::HashMap;

use crate::types::*;

/// 完成待查看条目的保留时长（2 分钟）
pub const DONE_WINDOW_MS: u64 = 120_000;
/// 待决审批/提问的 TTL（30 分钟）
pub const PENDING_TTL_MS: u64 = 30 * 60 * 1000;

pub struct PetState {
    pub mode: Mode,
    pub connected: bool,
    pub last_error: Option<String>,
    pub sessions: HashMap<String, Session>,
    pub pending_approvals: HashMap<String, Approval>,
    pub pending_questions: HashMap<String, Question>,
    pub done: HashMap<String, DoneEntry>,
    pub queued_count: usize,
    /// 上一帧的 mode，用于变化时打日志
    last_emitted_mode: Option<Mode>,
    /// 触发"完成音"播放的信号（main.js L231-236：turn/end completed）
    /// true 表示有未消费的 done 音信号
    pub done_sound_pending: bool,
}

impl Default for PetState {
    fn default() -> Self {
        Self::new()
    }
}

impl PetState {
    pub fn new() -> Self {
        Self {
            mode: Mode::Starting,
            connected: false,
            last_error: None,
            sessions: HashMap::new(),
            pending_approvals: HashMap::new(),
            pending_questions: HashMap::new(),
            done: HashMap::new(),
            queued_count: 0,
            last_emitted_mode: None,
            done_sound_pending: false,
        }
    }

    /// 消费一个事件，更新内部状态。对应 main.js 中 pollSessions / handleFrame / handleHostFrame 的逻辑。
    pub fn apply(&mut self, event: StateEvent) {
        match event {
            StateEvent::Poll {
                sessions,
                ok,
                error,
            } => self.apply_poll(sessions, ok, error),
            StateEvent::MuxFrame(frame) => self.apply_mux_frame(frame),
            StateEvent::HostFrame(frame) => self.apply_host_frame(frame),
            StateEvent::Tick => self.expire_ttl(),
        }
    }

    fn apply_poll(&mut self, items: Vec<SessionItem>, ok: bool, error: Option<String>) {
        if !ok {
            self.connected = false;
            self.last_error = error;
            return;
        }
        self.connected = true;
        self.last_error = None;

        let now = now_ms();
        // 先收集所有 session_id（owned），避免后续 for 循环 move items 时借用冲突
        let seen: std::collections::HashSet<String> =
            items.iter().map(|s| s.session_id.clone()).collect();

        for s in items {
            let title = s
                .projections
                .as_ref()
                .and_then(|p| p.values.as_ref())
                .and_then(|v| v.title.clone())
                .unwrap_or_else(|| "未命名会话".to_string());
            let todos = s.projections.and_then(|p| p.values).and_then(|v| v.todos);
            let was_running = self
                .sessions
                .get(&s.session_id)
                .map(|p| p.running)
                .unwrap_or(false);
            let cur = Session {
                id: s.session_id.clone(),
                title: title.clone(),
                running: s.running,
                todos,
            };
            self.sessions.insert(s.session_id.clone(), cur);
            // running true → false：标记完成待查看
            if was_running && !s.running {
                self.done.insert(
                    s.session_id.clone(),
                    DoneEntry {
                        session_id: s.session_id,
                        title,
                        at: now,
                    },
                );
            }
        }

        // 清除已消失的会话
        self.sessions.retain(|id, _| seen.contains(id));
        // 过期完成的会话（超过 2 分钟窗口）
        self.done.retain(|_, d| now - d.at < DONE_WINDOW_MS);
    }

    fn apply_mux_frame(&mut self, p: serde_json::Value) {
        let frame_type = p.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match frame_type {
            "approval/requested" => {
                if let (Some(id), Some(sid), Some(tool)) = (
                    p.get("approvalId").and_then(|v| v.as_str()),
                    p.get("sessionId").and_then(|v| v.as_str()),
                    p.get("toolName").and_then(|v| v.as_str()),
                ) {
                    self.pending_approvals.insert(
                        id.to_string(),
                        Approval {
                            approval_id: id.to_string(),
                            session_id: sid.to_string(),
                            tool_name: tool.to_string(),
                            reason: p
                                .get("reason")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            requested_at: now_ms(),
                        },
                    );
                }
            }
            "approval/resolved" => {
                if let Some(id) = p.get("approvalId").and_then(|v| v.as_str()) {
                    self.pending_approvals.remove(id);
                }
            }
            "question/requested" => {
                if let Some(id) = p.get("questionRpcId").and_then(|v| v.as_str()) {
                    let text = p
                        .get("questions")
                        .and_then(|q| q.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|q| q.get("question"))
                        .and_then(|q| q.as_str())
                        .unwrap_or("");
                    self.pending_questions.insert(
                        id.to_string(),
                        Question {
                            question_rpc_id: id.to_string(),
                            session_id: p
                                .get("sessionId")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            text: text.to_string(),
                            requested_at: now_ms(),
                        },
                    );
                }
            }
            "question/resolved" => {
                if let Some(id) = p.get("questionRpcId").and_then(|v| v.as_str()) {
                    self.pending_questions.remove(id);
                }
            }
            "session/queue" => {
                self.queued_count = p
                    .get("items")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
            }
            "session/event"
                // 一轮回答完成（turn/end completed）：触发完成音
                // main.js L232-235
                if p.get("event").and_then(|e| e.get("type")).and_then(|t| t.as_str())
                    == Some("turn/end")
                    && p
                        .get("event")
                        .and_then(|e| e.get("reason"))
                        .and_then(|r| r.get("kind"))
                        .and_then(|k| k.as_str())
                        == Some("completed")
                => {
                    self.done_sound_pending = true;
                }
            _ => {}
        }
    }

    fn apply_host_frame(&mut self, p: serde_json::Value) {
        let frame_type = p.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if frame_type != "host/session-status" {
            return;
        }
        let session_id = match p.get("sessionId").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return,
        };
        let running = p.get("running").and_then(|v| v.as_bool()).unwrap_or(false);

        if let Some(cur) = self.sessions.get_mut(&session_id) {
            let was_running = cur.running;
            cur.running = running;
            if was_running && !running {
                self.done.insert(
                    session_id.clone(),
                    DoneEntry {
                        session_id: session_id.clone(),
                        title: cur.title.clone(),
                        at: now_ms(),
                    },
                );
            }
        }
        // 会话结束：清除残留待决项（resolved 事件可能未送达，避免跨会话残留）
        if !running {
            self.pending_approvals
                .retain(|_, a| a.session_id != session_id);
            self.pending_questions
                .retain(|_, q| q.session_id != session_id);
        }
    }

    fn expire_ttl(&mut self) {
        let now = now_ms();
        self.done.retain(|_, d| now - d.at < DONE_WINDOW_MS);
        self.pending_approvals
            .retain(|_, a| now - a.requested_at < PENDING_TTL_MS);
        self.pending_questions
            .retain(|_, q| now - q.requested_at < PENDING_TTL_MS);
    }

    /// 构建渲染快照。1:1 对应 main.js buildSnapshot()。
    pub fn build_snapshot(&mut self) -> Snapshot {
        let running: Vec<SessionRef> = self
            .sessions
            .values()
            .filter(|s| s.running)
            .map(|s| SessionRef {
                session_id: s.id.clone(),
                title: s.title.clone(),
            })
            .collect();

        let mut attention = Vec::new();
        for a in self.pending_approvals.values() {
            attention.push(AttentionItem {
                kind: AttentionKind::Approval,
                session_id: a.session_id.clone(),
                text: format!(
                    "「{}」请求使用 {}",
                    self.session_title(&a.session_id),
                    a.tool_name
                ),
            });
        }
        for q in self.pending_questions.values() {
            attention.push(AttentionItem {
                kind: AttentionKind::Question,
                session_id: q.session_id.clone(),
                text: format!("「{}」：{}", self.session_title(&q.session_id), q.text),
            });
        }

        let done_list: Vec<DoneRef> = self
            .done
            .values()
            .map(|d| DoneRef {
                session_id: d.session_id.clone(),
                title: d.title.clone(),
            })
            .collect();

        let (mode, title, body) = if !self.connected {
            (
                Mode::Offline,
                "连不上 DSH 😢".to_string(),
                self.last_error
                    .clone()
                    .map(|e| format!("GUI 无响应（{e}）"))
                    .unwrap_or_else(|| "GUI 未启动，我会自动重试".to_string()),
            )
        } else if !attention.is_empty() {
            (
                Mode::Attention,
                format!("需要你确认 · {} 项", attention.len()),
                attention
                    .iter()
                    .enumerate()
                    .map(|(i, a)| format!("{}. {}", i + 1, a.text))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        } else if !running.is_empty() {
            let lines: Vec<String> = self
                .sessions
                .values()
                .filter(|s| s.running)
                .enumerate()
                .map(|(i, s)| format!("{}. 「{}」", i + 1, s.title))
                .collect();
            let mut body = lines.join("\n");
            if !done_list.is_empty() {
                body.push_str(&format!("\n—\n另有 {} 个已完成待查看", done_list.len()));
            }
            (
                Mode::Working,
                format!("正在干活…（{} 个会话）", running.len()),
                body,
            )
        } else if !done_list.is_empty() {
            (
                Mode::Done,
                "任务完成啦 🎉".to_string(),
                done_list
                    .iter()
                    .enumerate()
                    .map(|(i, d)| format!("{}. 「{}」", i + 1, d.title))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        } else {
            (
                Mode::Idle,
                "休息中 💤".to_string(),
                "没有运行中的任务".to_string(),
            )
        };

        self.mode = mode;
        if self.last_emitted_mode != Some(mode) {
            tracing::info!(
                "state={} running={} approvals={} questions={} done={}",
                mode.as_str(),
                running.len(),
                self.pending_approvals.len(),
                self.pending_questions.len(),
                done_list.len()
            );
            self.last_emitted_mode = Some(mode);
        }

        Snapshot {
            mode,
            bubble: Bubble { title, body },
            running,
            attention,
            done: done_list,
            queued: self.queued_count,
        }
    }

    fn session_title(&self, id: &str) -> String {
        self.sessions
            .get(id)
            .map(|s| s.title.clone())
            .unwrap_or_else(|| "某会话".to_string())
    }
}
