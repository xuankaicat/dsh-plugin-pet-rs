//! 截图自检模式：--shot [输出目录] → 5 状态 × 3 时间点 → PNG
//!
//! 对应 main.js L603-677 的 runShotMode()。

use std::path::Path;
use std::sync::Arc;

use dsh_pet_core::{Bubble, Mode, Snapshot, SpritePack};
use dsh_pet_ui::Renderer;

use crate::platform;

/// 运行截图自检：渲染 5 种状态 × 3 个动画相位，保存 PNG
pub fn run_shot_mode(output_dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(output_dir)?;
    let custom_dir = platform::custom_dir();
    let assets = Arc::new(SpritePack::load(&custom_dir)?);
    let font = platform::load_font();
    let mut renderer = Renderer::new(assets, font, 1.0, 1.0); // pet 100%，DPI 100%

    let modes = [
        Mode::Working,
        Mode::Attention,
        Mode::Done,
        Mode::Idle,
        Mode::Offline,
    ];
    let fixed_times = [0u64, 800, 1600]; // 采样 3 个动画相位

    for mode in &modes {
        for &time_ms in &fixed_times {
            let snapshot = fake_snapshot(*mode);
            let _ = renderer.render(&snapshot, time_ms);
            let name = format!("pixel-{}-t{time_ms}.png", mode.as_str());
            let path = output_dir.join(&name);
            renderer.pixmap().save_png(&path)?;
            tracing::info!("已截图: {name}");
        }
    }
    Ok(())
}

/// 构造合成快照（与 main.js fakeSnapshot 一致）
fn fake_snapshot(mode: Mode) -> Snapshot {
    let (title, body, running, attention, done) = match mode {
        Mode::Working => {
            let running: Vec<_> = (1..=6)
                .map(|i| dsh_pet_core::SessionRef {
                    session_id: format!("s{i}"),
                    title: format!("示例会话 {i}：市场调研与竞品分析报告"),
                })
                .collect();
            let body = running
                .iter()
                .enumerate()
                .map(|(i, s)| format!("{}. 「{}」", i + 1, s.title))
                .collect::<Vec<_>>()
                .join("\n");
            (
                "正在干活…（6 个会话）".to_string(),
                body,
                running,
                vec![],
                vec![],
            )
        }
        Mode::Attention => {
            let tools = ["bash", "write", "edit", "glob"];
            let attention: Vec<_> = tools
                .iter()
                .enumerate()
                .map(|(i, tool)| dsh_pet_core::AttentionItem {
                    kind: dsh_pet_core::AttentionKind::Approval,
                    session_id: format!("s{}", i + 1),
                    text: format!("「会话 {}」请求使用 {}", i + 1, tool),
                })
                .collect();
            let body = attention
                .iter()
                .enumerate()
                .map(|(i, a)| format!("{}. {}", i + 1, a.text))
                .collect::<Vec<_>>()
                .join("\n");
            (
                "需要你确认 · 4 项".to_string(),
                body,
                vec![],
                attention,
                vec![],
            )
        }
        Mode::Done => {
            let done: Vec<_> = (1..=3)
                .map(|i| dsh_pet_core::DoneRef {
                    session_id: format!("s{i}"),
                    title: format!("示例：任务 {i} 完成"),
                })
                .collect();
            let body = done
                .iter()
                .enumerate()
                .map(|(i, d)| format!("{}. 「{}」", i + 1, d.title))
                .collect::<Vec<_>>()
                .join("\n");
            ("任务完成啦 🎉".to_string(), body, vec![], vec![], done)
        }
        Mode::Idle => (
            "休息中 💤".to_string(),
            "没有运行中的任务".to_string(),
            vec![],
            vec![],
            vec![],
        ),
        Mode::Offline => (
            "连不上 DSH 😢".to_string(),
            "GUI 无响应，我会自动重试".to_string(),
            vec![],
            vec![],
            vec![],
        ),
        Mode::Starting => (
            "启动中…".to_string(),
            "正在连接 DSH".to_string(),
            vec![],
            vec![],
            vec![],
        ),
    };
    Snapshot {
        mode,
        bubble: Bubble { title, body },
        running,
        attention,
        done,
        queued: 0,
    }
}
