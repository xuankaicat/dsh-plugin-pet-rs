[![dshfind](https://dshfind.com/api/badge/huanlinoto/dsh-plugin-pet-rs?lang=zh)](https://dshfind.com/zh/plugins/huanlinoto/dsh-plugin-pet-rs?ref=badge)

> 📌 本插件已收录于 [dshfind](https://dshfind.com/zh) 插件超市，点击上方徽章直达主页。

# DSH 桌宠（Rust 版）

> ⚠️ 因为穷鲸组织的 GitHub Actions 账单额度问题无法运行，所以请自行构建。

DeepSeek Harness 桌面宠物鲸鱼，Rust 原生实现，三端支持（Windows / macOS / Linux）。

> 原 Electron 版见 `../dsh-pet/`。本版本用纯 Rust 重写，目标体积 < 10MB（原 ~100MB）。

## 功能

- 🐋 5 态鲸鱼：`offline > attention > working > done > idle`
- ⚡ 双 SSE 实时推送（`events.mux` + `events.host`）+ 2s 轮询兜底
- 🎨 HD 像素画鲸鱼（80×58 网格）+ 喷水水滴动画 + zzz/spark 叠层
- 💬 状态气泡（多会话聚合列表，可滚动，popIn 动画）
- 🔔 状态提示音（attention / done，custom/ 可覆盖）
- 🖼️ 透明置顶悬浮窗 + 系统托盘 + 拖拽 + 大小调节
- ⚙️ 内嵌设置面板（右键鲸鱼打开）：声音开关、「由桌宠启动 DSH」开关、地址编辑/只读、热切换、重启
- 📦 `custom/sprites.json` 素材包 + `custom/*.m4a|mp3` 自定义音效
- 🔒 对 DSH 零侵入（纯只读 HTTP/SSE）

## 操作指南

### 鲸鱼交互

| 操作 | 效果 |
|------|------|
| **单击鲸鱼** | 立即显示 / 隐藏状态气泡（无延迟） |
| **右键鲸鱼** | 打开 / 关闭内嵌设置面板 |
| **按住鲸鱼拖拽** | 移动桌宠窗口位置 |
| **滚轮（气泡上）** | 滚动会话列表 |

> 单击即时响应、无延迟；设置面板改为右键打开/关闭，右键不会触发气泡开关。
> 设置面板打开时，单击鲸鱼可关闭面板（丢弃未提交的编辑），Esc 或点击 × 亦可。
> 打开 DSH GUI 请使用托盘菜单「打开 DSH GUI」，或单击状态气泡。

### 设置面板

右键鲸鱼打开设置面板，面板替换状态气泡显示（鲸鱼动画继续）：

```
┌───────────────────────────────┐
│ 设置                      ×   │
│                               │
│ 声音提醒              [ON/OFF] │
│ 由桌宠启动 DSH        [OFF/ON] │
│ DSH 地址（只读）              │
│ ┌───────────────────────────┐ │
│ │ http://127.0.0.1:3080 重启│ │
│ └───────────────────────────┘ │
└───────────────────────────────┘
```

**声音提醒开关**：点击立即切换并持久化，与托盘菜单同步。

**由桌宠启动 DSH**：开启后桌宠会自动拉起 DSH Web 作为子进程并自动切换连接，无需手动填地址。启动方式自动按顺序尝试：`dsh`（PATH 中已安装）→ `cmd /C dsh`（Windows shim）→ `npx --yes @deepseek-ai/dsh`（npm on-demand，即日常的 `npx @deepseek-ai/dsh web` 用法）；均使用 `--port 0` 由系统分配空闲端口（后台静默启动，不弹 CLI 窗口）。若全部失败会给出安装提示（`npm install -g @deepseek-ai/dsh`）。此时「DSH 地址」为只读显示，右侧「重启」按钮可重启该子进程（重启后自动重连新地址）。关闭该开关时会弹出确认警告（Windows）：将终止当前由桌宠启动的 DSH 进程；确认后停止子进程并恢复手动地址输入。桌宠退出（含托盘退出）时会一并回收子进程。

> **启动自动检测**：若桌宠启动时（且配置为子进程模式）检测到默认端口 `http://127.0.0.1:3080` 已有 DSH 服务，会自动关闭子进程模式并直接连接现有实例（配置同步保存，下次不再尝试拉起）。

**DSH 地址输入框**：

| 操作 | 效果 |
|------|------|
| 点击输入框 | 获得焦点，可编辑文本 |
| 输入字符 | 实时显示（支持中文 IME） |
| `Enter` | 提交地址 → 热切换网络连接 |
| `Esc` | 恢复原值并失焦（不关闭面板） |
| 点击输入框外 | 提交地址并失焦 |
| `←` / `→` | 移动光标 |
| `Home` / `End` | 跳到行首 / 行尾 |
| `Backspace` | 删除光标前字符 |
| `Delete` | 删除光标后字符 |
| `Ctrl+A` | 全选 |
| `Ctrl+C` / `Ctrl+X` | 复制 / 剪切选中文本 |
| `Ctrl+V` | 粘贴剪贴板内容 |
| `Ctrl+Z` | 撤销上次编辑 |

> 地址必须以 `http://` 或 `https://` 开头，否则提交时恢复原值。
> 提交有效地址后，RPC 轮询和双 SSE 连接会立即用新地址重建（热切换），无需重启。
> 输入框支持中文输入法（IME），预编辑文本会带下划线显示。

**关闭设置**：点击右上角 `×` 或按 `Esc`（输入框无焦点时）。

### 系统托盘

| 菜单项 | 效果 |
|--------|------|
| 打开 DSH GUI | 在浏览器打开 DSH |
| 打开设置 | 打开内嵌设置面板 |
| 隐藏/显示气泡 | 切换状态气泡可见性 |
| 状态提示音 | 勾选框，切换声音并持久化 |
| 测试提示音 | 播放 done 音效 |
| 放大 / 缩小 / 重置 | 调节鲸鱼大小（50%–110%，默认 67%） |
| 退出桌宠 | 保存配置并退出 |

### 窗口位置

- 拖拽鲸鱼可移动窗口，位置自动保存。
- 下次启动恢复上次位置；首次启动定位到屏幕右下角。
- 启动时会校验窗口是否在可见显示器内：若位置在屏幕外（如显示器变更/多屏拔插导致），自动移回主显示器右下角，保证桌宠一定显示。

## 构建

```bash
# 开发
cargo build

# Release
cargo build --release

# 测试
cargo test --all

# Lint
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

## 运行

```bash
# 默认连接 http://127.0.0.1:3080
cargo run --release

# 指定 DSH URL（优先于配置文件）
DSH_PET_URL=http://localhost:3080 cargo run --release

# 截图自检（5 状态 × 3 时间点 → PNG）
cargo run --release -- --shot .shots
```

> DSH 地址优先级：环境变量 `DSH_PET_URL` > 配置文件 > 默认值 `http://127.0.0.1:3080`。
> 运行时通过设置面板修改的地址会保存到配置文件，下次启动自动使用。

## 配置文件

配置文件路径：

| 平台 | 路径 |
|------|------|
| Windows | `%APPDATA%\dsh\pet\config\config.json` |
| macOS | `~/Library/Application Support/com.dsh.pet/config/config.json` |
| Linux | `~/.config/pet/config/config.json` |

```json
{
  "scale": 0.67,
  "bubble_visible": true,
  "sound_on": true,
  "endpoint": "http://127.0.0.1:3080",
  "spawn_dsh": false,
  "window_x": 2268,
  "window_y": 1084
}
```

> 字段均可省略，缺省时使用默认值。旧版配置文件（无 `endpoint` / `spawn_dsh` 字段）自动兼容。

## 自定义素材

在可执行文件同目录下创建 `custom/` 文件夹：

```
custom/
├── sprites.json   # 覆盖像素素材（palette + sprites）
├── attention.m4a  # 或 .mp3/.wav/.ogg
└── done.m4a
```

`sprites.json` 格式见 `assets/sprites/whale-sprites-hd.json`。

## 架构

```
crates/
├── dsh-pet-core/   # 状态机、RPC、SSE、配置、素材包（纯逻辑，可独立测试）
├── dsh-pet-ui/     # 窗口、渲染器、输入、emoji 图集（winit + tiny-skia + softbuffer）
└── dsh-pet-app/    # 主入口、异步任务、音频、托盘、平台适配
```

### 渲染管线

```
┌──────────────────────────────────────────┐
│         tiny-skia Pixmap (合成面)         │
│  ┌────────────────────────────────────┐  │
│  │ 气泡：圆角矩形 + 阴影 + 文本 + 尾巴 │  │
│  └────────────────────────────────────┘  │
│  ┌────────────────────────────────────┐  │
│  │ 鲸鱼：80×58 sprite blit + bob 偏移  │  │
│  │ + 水滴动画 + zzz/spark 文字叠层     │  │
│  └────────────────────────────────────┘  │
└──────────────────┬───────────────────────┘
                   │ pixmap.data() → &[u32]
                   ▼
            softbuffer::present()
```

## 平台支持

| 平台 | 状态 | 备注 |
|---|---|---|
| Windows | ✅ 完整 | DWM per-pixel alpha 透明窗 |
| macOS | ✅ 完整 | NSWindow 透明 + Dock 隐藏 |
| Linux (X11) | ✅ 完整 | 透明窗 + 置顶 + skip-taskbar |
| Linux (Wayland) | ⚠️ 降级 | 不置顶、不跨全屏；建议用 XWayland |

### Linux GNOME 托盘

GNOME 默认无系统托盘，需安装 [AppIndicator and KStatusNotifierItem Support](https://extensions.gnome.org/extension/615/appindicator-support/) 扩展。托盘创建失败时，**右键鲸鱼本体会在窗口内弹出菜单**（打开 DSH GUI / 打开设置 / 气泡 / 提示音 / 缩放 / 退出），功能与托盘菜单一致。

## 字体

运行时探测系统 CJK 字体：
- Windows: `Microsoft YaHei` / `SimHei`
- macOS: `PingFang SC` / `Heiti SC`
- Linux: `Noto Sans CJK SC` / `WenQuanYi Micro Hei`

> v1 偏离 spec：未内嵌 Noto Sans CJK 子集（需 HarfBuzz subsetting 工具链）。
> 如系统无 CJK 字体，启动时会 panic 并给出安装指引。

## 测试

- `dsh-pet-core` 单元测试：状态机 5 态转换、TTL 过期、SSE 帧解析
- SSE 重连集成测试（wiremock）：覆盖正常关闭、部分帧、非 JSON、无空格 `data:`、HTTP 5xx、取消等 6 case
- `--shot` 截图回归：5 状态 × 3 时间点 → PNG 像素 diff

## License

MIT
