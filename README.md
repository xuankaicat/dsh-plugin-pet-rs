# DSH 桌宠（Rust 版）

DeepSeek Harness 桌面宠物鲸鱼，Rust 原生实现，三端支持（Windows / macOS / Linux）。

> 原 Electron 版见 `../dsh-pet/`。本版本用纯 Rust 重写，目标体积 < 10MB（原 ~100MB）。

## 功能

- 🐋 5 态鲸鱼：`offline > attention > working > done > idle`
- ⚡ 双 SSE 实时推送（`events.mux` + `events.host`）+ 2s 轮询兜底
- 🎨 HD 像素画鲸鱼（80×58 网格）+ 喷水水滴动画 + zzz/spark 叠层
- 💬 状态气泡（多会话聚合列表，可滚动，popIn 动画）
- 🔔 状态提示音（attention / done，custom/ 可覆盖）
- 🖼️ 透明置顶悬浮窗 + 系统托盘 + 拖拽 + 大小调节
- 📦 `custom/sprites.json` 素材包 + `custom/*.m4a|mp3` 自定义音效
- 🔒 对 DSH 零侵入（纯只读 HTTP/SSE）

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

# 指定 DSH URL
DSH_PET_URL=http://localhost:3080 cargo run --release

# 截图自检（5 状态 × 3 时间点 → PNG）
cargo run --release -- --shot .shots
```

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

GNOME 默认无系统托盘，需安装 [AppIndicator and KStatusNotifierItem Support](https://extensions.gnome.org/extension/615/appindicator-support/) 扩展。托盘创建失败时，右键鲸鱼本体仍可访问菜单。

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
