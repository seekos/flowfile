use crate::{distribution, models::home_directory};
use anyhow::{Context as _, Result};
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
use objc2_foundation::{
    MainThreadMarker, NSData, NSString, NSURL, NSURLBookmarkCreationOptions,
    NSURLBookmarkResolutionOptions,
};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BookmarkRecord {
    path: PathBuf,
    data: Vec<u8>,
}

struct ActiveSecurityScope {
    url: Retained<NSURL>,
    started: bool,
}

/// Owns the security-scoped URLs for the lifetime of the application.
///
/// App Sandbox grants a temporary extension when NSOpenPanel returns. We turn
/// that URL into an app-scoped bookmark and resolve it again on later launches.
/// The retained URL and balanced start/stop calls keep the extension valid for
/// all async file operations while FlowFile is running.
#[derive(Default)]
pub struct SandboxAccess {
    records: Vec<BookmarkRecord>,
    active_scopes: Vec<ActiveSecurityScope>,
}

impl SandboxAccess {
    pub fn load() -> Self {
        if !distribution::is_app_store() {
            return Self::default();
        }

        let records = fs::read(bookmarks_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<BookmarkRecord>>(&bytes).ok())
            .unwrap_or_default();
        let mut access = Self {
            records,
            active_scopes: Vec::new(),
        };
        access.restore_scopes();
        access
    }

    pub fn authorized_paths(&self) -> Vec<PathBuf> {
        self.records
            .iter()
            .map(|record| record.path.clone())
            .filter(|path| path.is_dir())
            .collect()
    }

    /// Shows the system folder picker and persists the exact NSURL returned by
    /// Powerbox. Reconstructing this URL from a string would discard the
    /// security scope and make the bookmark unreliable.
    pub fn choose_and_authorize_folder(&mut self) -> Result<Option<PathBuf>> {
        if !distribution::is_app_store() {
            return Ok(None);
        }

        let mtm = MainThreadMarker::new().context("文件夹授权选择器只能从 macOS 主线程打开")?;
        let panel = unsafe { NSOpenPanel::openPanel(mtm) };
        unsafe {
            panel.setCanChooseDirectories(true);
            panel.setCanChooseFiles(false);
            panel.setAllowsMultipleSelection(false);
            panel.setCanCreateDirectories(true);
            panel.setResolvesAliases(true);
            panel.setTitle(Some(&NSString::from_str("授权 FlowFile 管理文件夹")));
            panel.setMessage(Some(&NSString::from_str(
                "请选择一个文件夹。FlowFile 只会访问你明确授权的位置，并在本机保存授权书签。",
            )));
            panel.setPrompt(Some(&NSString::from_str("授权文件夹")));
        }

        if unsafe { panel.runModal() } != NSModalResponseOK {
            return Ok(None);
        }
        let urls = unsafe { panel.URLs() };
        if urls.is_empty() {
            return Ok(None);
        }
        let url = unsafe { urls.objectAtIndex(0) };
        let path = url_path(&url).context("系统没有返回可用的文件夹路径")?;
        let data = create_bookmark(&url)?;
        let started = unsafe { url.startAccessingSecurityScopedResource() };

        self.records.retain(|record| record.path != path);
        self.records.push(BookmarkRecord {
            path: path.clone(),
            data,
        });
        let mut retained_scopes = Vec::with_capacity(self.active_scopes.len() + 1);
        for scope in self.active_scopes.drain(..) {
            if url_path(&scope.url).is_some_and(|existing| existing == path) {
                if scope.started {
                    unsafe { scope.url.stopAccessingSecurityScopedResource() };
                }
            } else {
                retained_scopes.push(scope);
            }
        }
        self.active_scopes = retained_scopes;
        self.active_scopes
            .push(ActiveSecurityScope { url, started });
        self.save()?;
        Ok(Some(path))
    }

    fn restore_scopes(&mut self) {
        let mut restored_records = Vec::new();
        for record in self.records.drain(..) {
            let bookmark = NSData::with_bytes(&record.data);
            let mut stale = Bool::NO;
            let resolution = unsafe {
                NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
                    &bookmark,
                    NSURLBookmarkResolutionOptions::NSURLBookmarkResolutionWithSecurityScope
                        | NSURLBookmarkResolutionOptions::NSURLBookmarkResolutionWithoutUI,
                    None,
                    &mut stale,
                )
            };
            let Ok(url) = resolution else {
                continue;
            };
            let Some(path) = url_path(&url) else {
                continue;
            };
            let started = unsafe { url.startAccessingSecurityScopedResource() };
            let data = if stale.as_bool() {
                create_bookmark(&url).unwrap_or(record.data)
            } else {
                record.data
            };
            restored_records.push(BookmarkRecord {
                path: path.clone(),
                data,
            });
            self.active_scopes
                .push(ActiveSecurityScope { url, started });
        }
        self.records = restored_records;
        if let Err(error) = self.save() {
            eprintln!("FlowFile: 无法刷新文件夹授权书签：{error}");
        }
    }

    fn save(&self) -> Result<()> {
        if !distribution::is_app_store() {
            return Ok(());
        }
        crate::models::persistence::atomic_write_json(&bookmarks_path(), &self.records, "授权书签")
    }
}

impl Drop for SandboxAccess {
    fn drop(&mut self) {
        for scope in self.active_scopes.drain(..) {
            if scope.started {
                unsafe { scope.url.stopAccessingSecurityScopedResource() };
            }
        }
    }
}

fn create_bookmark(url: &NSURL) -> Result<Vec<u8>> {
    let data = unsafe {
        url.bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
            NSURLBookmarkCreationOptions::NSURLBookmarkCreationWithSecurityScope,
            None,
            None,
        )
    }
    .map_err(|error| anyhow::anyhow!("无法创建安全范围书签：{error:?}"))?;
    Ok(data.bytes().to_vec())
}

fn url_path(url: &NSURL) -> Option<PathBuf> {
    unsafe { url.path() }.map(|path| PathBuf::from(path.to_string()))
}

fn bookmarks_path() -> PathBuf {
    home_directory()
        .join("Library")
        .join("Application Support")
        .join("FlowFile")
        .join("security-scoped-bookmarks.json")
}

#[cfg(test)]
mod tests {
    use super::BookmarkRecord;
    use std::path::PathBuf;

    #[test]
    fn bookmark_records_round_trip() {
        let records = vec![BookmarkRecord {
            path: PathBuf::from("/Users/example/Documents"),
            data: vec![1, 2, 3, 4],
        }];
        let bytes = serde_json::to_vec(&records).expect("serialize");
        let restored: Vec<BookmarkRecord> = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(restored[0].path, records[0].path);
        assert_eq!(restored[0].data, records[0].data);
    }
}
