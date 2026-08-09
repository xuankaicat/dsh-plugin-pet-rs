//! 状态提示音播放器：rodio + symphonia（跨平台）。
//!
//! 对应 main.js L461-477 的 playSoundFile()。

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Mutex;

use dsh_pet_core::Mode;
use rodio::{Decoder, OutputStream, Sink};

pub struct AudioPlayer {
    // OutputStream 必须存活才能播放声音；StreamHandle 用于创建 Sink
    _stream: OutputStream,
    stream_handle: rodio::OutputStreamHandle,
    sink: Mutex<Option<Sink>>,
    sound_on: bool,
    custom_dir: PathBuf,
}

impl AudioPlayer {
    pub fn new() -> Self {
        let (stream, handle) = OutputStream::try_default().unwrap_or_else(|e| {
            tracing::warn!("音频输出不可用: {e}，将静默运行");
            // 返回一个哑的 stream（rodio 在无设备时仍可构造）
            // 如果连构造都失败，则 panic
            panic!("无可用音频设备: {e}")
        });
        Self {
            _stream: stream,
            stream_handle: handle,
            sink: Mutex::new(None),
            sound_on: true,
            custom_dir: crate::platform::custom_dir(),
        }
    }

    /// 根据状态播放对应提示音
    pub fn play_for_mode(&self, mode: Mode) {
        match mode {
            Mode::Attention => self.play("attention"),
            Mode::Done => self.play("done"),
            _ => {}
        }
    }

    #[allow(dead_code)]
    pub fn play_attention(&self) {
        self.play("attention");
    }
    #[allow(dead_code)]
    pub fn play_done(&self) {
        self.play("done");
    }

    #[allow(dead_code)]
    pub fn set_sound_on(&mut self, on: bool) {
        self.sound_on = on;
    }

    fn play(&self, name: &str) {
        if !self.sound_on {
            return;
        }

        // 查找顺序：custom/ → 内嵌音效，扩展名 m4a > mp4 > mp3 > wav > ogg
        let candidates = [
            self.custom_dir.join(format!("{name}.m4a")),
            self.custom_dir.join(format!("{name}.mp4")),
            self.custom_dir.join(format!("{name}.mp3")),
            self.custom_dir.join(format!("{name}.wav")),
            self.custom_dir.join(format!("{name}.ogg")),
        ];

        let (data, source) = candidates
            .iter()
            .find_map(|p| {
                if p.exists() {
                    std::fs::read(p).ok().map(|d| (d, "file"))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| {
                // 回退到内嵌音效（mp3 格式，symphonia mp3 解码器始终可用）
                match name {
                    "attention" => (
                        include_bytes!("../../../assets/sounds/attention.mp3").to_vec(),
                        "embed",
                    ),
                    "done" => (
                        include_bytes!("../../../assets/sounds/done.mp3").to_vec(),
                        "embed",
                    ),
                    _ => (vec![], "none"),
                }
            });

        if data.is_empty() {
            return;
        }

        // 防重叠：stop 旧 sink
        let mut guard = self.sink.lock().unwrap();
        if let Some(old) = guard.take() {
            old.stop();
        }

        match Decoder::new(Cursor::new(data)) {
            Ok(decoder) => match Sink::try_new(&self.stream_handle) {
                Ok(sink) => {
                    sink.append(decoder);
                    *guard = Some(sink);
                    tracing::info!("播放提示音: {name} ({source})");
                }
                Err(e) => tracing::warn!("创建音频 Sink 失败: {e}"),
            },
            Err(e) => tracing::warn!("音频解码失败 ({name}): {e}"),
        }
    }
}
