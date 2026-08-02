use anyhow::{Context as _, Result};
use async_channel::{Receiver, bounded};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;

/// Keeps the native watcher alive for as long as a pane is displaying a path.
pub struct FileWatcher {
    _watcher: RecommendedWatcher,
}

impl FileWatcher {
    pub fn watch(path: &Path) -> Result<(Self, Receiver<()>)> {
        let (sender, receiver) = bounded(32);
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if event.is_ok() {
                    // A full channel already represents a pending refresh, so dropping
                    // duplicate events here is intentional.
                    let _ = sender.try_send(());
                }
            })
            .context("无法创建目录监听器")?;

        watcher
            .watch(path, RecursiveMode::NonRecursive)
            .with_context(|| format!("无法监听目录 {}", path.display()))?;

        Ok((Self { _watcher: watcher }, receiver))
    }
}
