# DSH Endpoint 配置项与热切换设计

## 目标

- 在 `Config` 中新增 `endpoint` 字段，持久化到 `config.json`。
- 设置面板内增加可编辑文本框，用户可直接修改 DSH 地址。
- 修改 endpoint 后立即热切换网络任务（RPC + 双 SSE），无需重启。
- 环境变量 `DSH_PET_URL` 仍优先于配置文件（用于 CI/测试）。

## Config 变更

- `Config` 新增 `endpoint: String`，`#[serde(default = "Config::default_endpoint")]`。
- `Config::ENDPOINT_DEFAULT` 常量 `"http://127.0.0.1:3080"` 移至 `dsh-pet-core/src/config.rs`，`tasks.rs` 引用之。
- `Config::normalized_endpoint()` 返回 `trim_end_matches('/')` 后的地址。
- 旧 `config.json` 缺少 `endpoint` 字段时 serde 自动填充默认值（向后兼容）。

## 热切换架构

### 取消 + 重建策略

endpoint 变更时：

1. 取消旧 `net_cancel`（CancellationToken），终止旧 poll / sse_mux / sse_host 任务。
2. 用新 endpoint 构造 `RpcClient` + 2 个 `SseConnector`。
3. 在 `rt` 上重新 spawn 三个网络任务，返回新的 `CancellationToken`。
4. `tick_task` 和状态聚合任务不受影响（使用独立的 `app_cancel`）。

### `spawn_network_tasks` 辅助函数

位于 `tasks.rs`，签名：

```rust
fn spawn_network_tasks(
    rt: &tokio::runtime::Runtime,
    endpoint: &str,
    http: &reqwest::Client,
    event_tx: mpsc::Sender<StateEvent>,
) -> CancellationToken
```

### 事件循环闭包捕获

- `rt`、`http`、`event_tx`、`net_cancel`、`dsh_url` 均 move 进 `event_loop.run` 闭包。
- `net_cancel` 和 `dsh_url` 为 `mut`，在 endpoint 提交时被替换。
- `app_cancel` 取代原 `cancel`，仅用于 `LoopExiting` 和 `tick_task`。

## 设置面板 UI

### 布局

`SETTINGS_H` 从 86 增至 132（确保 pet_scale 1.1 + dpi 1.0 时不溢出 340px 窗口）。

```
y+0   ┌───────────────────────────────┐
      │ 设置                      ×   │  标题 16px
y+46  │ 声音提醒              [ON/OFF]│  开关行
y+74  │ DSH 地址                       │  标签 13px
y+94  │┌─────────────────────────────┐│  输入框 (h=28)
      ││ http://127.0.0.1:3080│      ││  文本 13px + 光标
y+132 │└─────────────────────────────┘│
      └───────────────────────────────┘
```

### 新增类型

- `SettingsHit::EndpointInput` — 点击输入框区域时返回。
- Renderer 新增字段：`endpoint_text: String`、`endpoint_focused: bool`、`settings_endpoint_rect: Option<HitRect>`。

### 光标

- 焦点时绘制 1px 竖线，500ms 闪烁。
- `AboutToWait` 中：若 `endpoint_focused`，设 `WaitUntil(now + 500ms)` 并 `request_redraw`。

### 文本测量

- 用 `font.as_scaled(size).h_advance(glyph_id)` 逐字累加，定位光标 x 坐标。

## 交互

### 打开设置面板

- 双击鲸鱼 → `set_settings_visible(true)` + `set_endpoint_text(dsh_url.clone())`。

### 点击输入框

- `SettingsHit::EndpointInput` → `set_endpoint_focused(true)`。

### 点击输入框外（设置面板内）

- `SettingsHit::None` / `ToggleSound` / `Close` → 若输入框有焦点，提交 endpoint 并失焦。
- `ClickTarget::Whale` → 同上。

### 键盘输入

- `Key::Character(s)` — 追加字符串中的可打印字符（过滤控制字符，上限 256 字节）。
- `Key::Named(Backspace)` — 删除末尾字符。
- `Key::Named(Enter)` — 提交 endpoint，失焦。
- `Key::Named(Escape)` — 若输入框有焦点：恢复原值并失焦（不关闭面板）；否则关闭面板。

> 注：winit 0.30 已移除 `WindowEvent::ReceivedCharacter`，改用 `KeyboardInput` 中的 `Key::Character(SmolStr)`。

### 提交逻辑

1. `trim()` 文本。
2. 校验非空且以 `http://` 或 `https://` 开头。
3. 若与当前 `dsh_url` 不同：更新 `config.endpoint`、`dsh_url`、`config.save()`、热切换。
4. 若无效：恢复原值。
5. 无论有效与否，失焦。

## 验证

- Config 向后兼容测试（旧 JSON 缺 endpoint → 默认值）。
- Config 序�回测试（round-trip）。
- `normalized_endpoint()` 去 trailing `/` 测试。
- `cargo test --all`、Clippy `-D warnings`、rustfmt。
- `--shot` 截图包含 endpoint 输入框。
- Release 构建。
