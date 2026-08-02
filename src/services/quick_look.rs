use super::FileEngine;
use anyhow::{Context as _, Result};
use std::{
    path::PathBuf,
    process::{Child, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::{fs::File, io::AsyncReadExt, runtime::Handle};

pub const TEXT_PREVIEW_LIMIT: usize = 100 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewKind {
    Image,
    Text,
    Native,
}

#[derive(Clone)]
pub struct QuickLookService {
    runtime: Handle,
    native_child: Arc<Mutex<Option<Child>>>,
    native_generation: Arc<AtomicU64>,
}

impl QuickLookService {
    pub fn new(engine: &FileEngine) -> Self {
        Self {
            runtime: engine.runtime_handle(),
            native_child: Arc::new(Mutex::new(None)),
            native_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn classify(path: &std::path::Path) -> PreviewKind {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if matches!(
            extension.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "svg"
        ) {
            PreviewKind::Image
        } else if is_text_extension(&extension) {
            PreviewKind::Text
        } else {
            PreviewKind::Native
        }
    }

    pub async fn read_text(&self, path: PathBuf) -> Result<(String, bool)> {
        self.runtime
            .spawn(async move {
                let file = File::open(&path)
                    .await
                    .with_context(|| format!("无法打开 {}", path.display()))?;
                let mut buffer = Vec::with_capacity(TEXT_PREVIEW_LIMIT);
                file.take(TEXT_PREVIEW_LIMIT as u64 + 1)
                    .read_to_end(&mut buffer)
                    .await?;
                let truncated = buffer.len() > TEXT_PREVIEW_LIMIT;
                buffer.truncate(TEXT_PREVIEW_LIMIT);
                Ok((String::from_utf8_lossy(&buffer).into_owned(), truncated))
            })
            .await
            .context("文本预览任务异常终止")?
    }

    pub fn open_native(&self, path: PathBuf) {
        self.close_native();
        let native_child = self.native_child.clone();
        let native_generation = self.native_generation.clone();
        let generation = native_generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.runtime.spawn_blocking(move || {
            let result = std::process::Command::new("/usr/bin/qlmanage")
                .arg("-p")
                .arg(&path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            match result {
                Ok(mut child) => {
                    if native_generation.load(Ordering::Acquire) == generation {
                        if let Ok(mut slot) = native_child.lock() {
                            *slot = Some(child);
                        }
                    } else {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                }
                Err(error) => eprintln!("FlowFile: 无法启动 macOS Quick Look：{error}"),
            }
        });
    }

    pub fn close_native(&self) {
        self.native_generation.fetch_add(1, Ordering::AcqRel);
        let child = self
            .native_child
            .lock()
            .ok()
            .and_then(|mut child| child.take());
        if let Some(mut child) = child {
            self.runtime.spawn_blocking(move || {
                let _ = child.kill();
                let _ = child.wait();
            });
        }
    }
}

pub fn is_text_extension(extension: &str) -> bool {
    matches!(
        extension,
        "txt"
            | "md"
            | "log"
            | "rs"
            | "json"
            | "toml"
            | "yaml"
            | "yml"
            | "py"
            | "go"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "css"
            | "html"
            | "xml"
            | "c"
            | "cc"
            | "cpp"
            | "h"
            | "hpp"
            | "sh"
            | "zsh"
            | "swift"
    )
}
