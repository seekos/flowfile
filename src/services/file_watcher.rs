use super::{file_operations::TRANSFER_STAGING_PREFIX, git_status};
use anyhow::{Context as _, Result};
use async_channel::{Receiver, bounded};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Component, Path, PathBuf};

/// Keeps the native watcher alive for as long as a pane is displaying a path.
pub struct FileWatcher {
    _watcher: RecommendedWatcher,
}

impl FileWatcher {
    pub fn watch(path: &Path) -> Result<(Self, Receiver<()>)> {
        let (sender, receiver) = bounded(32);
        let watched_path = path.to_path_buf();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if event
                    .as_ref()
                    .is_ok_and(|event| should_refresh(&watched_path, &event.paths))
                {
                    // A full channel already represents a pending refresh, so dropping
                    // duplicate events here is intentional.
                    let _ = sender.try_send(());
                }
            })
            .context("无法创建目录监听器")?;

        watcher
            .watch(path, RecursiveMode::NonRecursive)
            .with_context(|| format!("无法监听目录 {}", path.display()))?;
        for metadata_directory in git_status::metadata_directories(path) {
            let _ = watcher.watch(&metadata_directory, RecursiveMode::NonRecursive);
        }

        Ok((Self { _watcher: watcher }, receiver))
    }
}

fn should_refresh(watched_path: &Path, changed_paths: &[PathBuf]) -> bool {
    changed_paths.is_empty()
        || changed_paths
            .iter()
            .any(|path| !is_transfer_staging_path(watched_path, path))
}

fn is_transfer_staging_path(watched_path: &Path, path: &Path) -> bool {
    path.strip_prefix(watched_path)
        .ok()
        .and_then(|relative| relative.components().next())
        .is_some_and(|component| {
            matches!(component, Component::Normal(name) if name.to_string_lossy().starts_with(TRANSFER_STAGING_PREFIX))
        })
}

#[cfg(test)]
mod tests {
    use super::should_refresh;
    use std::path::PathBuf;

    #[test]
    fn ignores_internal_transfer_writes_until_the_final_path_changes() {
        let root = PathBuf::from("/tmp/files");
        let staging = root.join(".flowfile-transfer-42-0-video.mov");

        assert!(!should_refresh(&root, std::slice::from_ref(&staging)));
        assert!(!should_refresh(&root, &[staging.join("partial-data.bin")]));
        assert!(should_refresh(&root, &[staging, root.join("video.mov")]));
        assert!(should_refresh(&root, &[]));
    }
}
