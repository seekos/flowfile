use crate::models::{FileItem, SortMode};
use anyhow::{Context as _, Result};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};
use tokio::runtime::{Handle, Runtime};

#[derive(Clone, Debug)]
pub struct DirectorySnapshot {
    pub path: PathBuf,
    pub items: Vec<FileItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenWithApplication {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone)]
pub struct FileEngine {
    runtime: Arc<Runtime>,
}

impl FileEngine {
    pub fn new() -> Result<Self> {
        let runtime = Runtime::new().context("无法创建 Tokio 运行时")?;
        Ok(Self {
            runtime: Arc::new(runtime),
        })
    }

    pub async fn read_directory(
        &self,
        path: PathBuf,
        show_hidden: bool,
        sort_mode: SortMode,
    ) -> Result<DirectorySnapshot> {
        self.runtime
            .spawn(async move {
                let canonical_path = tokio::fs::canonicalize(&path)
                    .await
                    .with_context(|| format!("路径不存在：{}", path.display()))?;
                let items = if !show_hidden && sort_mode == SortMode::Name {
                    fetch_dir_entries(&canonical_path).await?
                } else {
                    fetch_dir_entries_with_options(&canonical_path, show_hidden, sort_mode).await?
                };
                Ok(DirectorySnapshot {
                    path: canonical_path,
                    items,
                })
            })
            .await
            .context("目录读取任务异常终止")?
    }

    pub async fn list_volumes(&self) -> Result<Vec<PathBuf>> {
        self.runtime
            .spawn(discover_volume_paths(
                PathBuf::from("/"),
                PathBuf::from("/Volumes"),
            ))
            .await
            .context("挂载卷读取任务异常终止")?
    }

    pub async fn open_path(&self, path: PathBuf) -> Result<()> {
        self.runtime
            .spawn_blocking(move || {
                open::that(&path).with_context(|| format!("无法打开 {}", path.display()))
            })
            .await
            .context("打开文件任务异常终止")?
    }

    pub async fn applications_for_path(&self, path: PathBuf) -> Result<Vec<OpenWithApplication>> {
        self.runtime
            .spawn_blocking(move || query_applications_for_path(&path))
            .await
            .context("读取可用应用任务异常终止")?
    }

    pub async fn open_path_with(&self, path: PathBuf, application: PathBuf) -> Result<()> {
        self.runtime
            .spawn_blocking(move || {
                let status = Command::new("/usr/bin/open")
                    .arg("-a")
                    .arg(&application)
                    .arg(&path)
                    .status()
                    .with_context(|| format!("无法启动 {}", application.display()))?;
                if !status.success() {
                    anyhow::bail!("无法使用 {} 打开 {}", application.display(), path.display());
                }
                Ok(())
            })
            .await
            .context("指定应用打开文件任务异常终止")?
    }

    pub(crate) fn runtime_handle(&self) -> Handle {
        self.runtime.handle().clone()
    }
}

const OPEN_WITH_APPLICATIONS_SCRIPT: &str = r#"
function run(argv) {
    ObjC.import('AppKit');
    const fileURL = $.NSURL.fileURLWithPath(argv[0]);
    const urls = $.NSWorkspace.sharedWorkspace.URLsForApplicationsToOpenURL(fileURL);
    const paths = [];
    for (let index = 0; index < urls.count; index++) {
        paths.push(ObjC.unwrap(urls.objectAtIndex(index).path));
    }
    return JSON.stringify(paths);
}
"#;

fn query_applications_for_path(path: &Path) -> Result<Vec<OpenWithApplication>> {
    let output = Command::new("/usr/bin/osascript")
        .args([
            "-l",
            "JavaScript",
            "-e",
            OPEN_WITH_APPLICATIONS_SCRIPT,
            "--",
        ])
        .arg(path)
        .output()
        .with_context(|| format!("无法查询可打开 {} 的应用", path.display()))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        anyhow::bail!("Launch Services 查询失败：{message}");
    }
    let mut applications = parse_open_with_applications(&String::from_utf8_lossy(&output.stdout))?;
    ensure_text_editor(&mut applications);
    Ok(applications)
}

fn ensure_text_editor(applications: &mut Vec<OpenWithApplication>) {
    const TEXT_EDIT_PATHS: [&str; 2] = [
        "/System/Applications/TextEdit.app",
        "/Applications/TextEdit.app",
    ];
    let Some(path) = TEXT_EDIT_PATHS
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_dir())
    else {
        return;
    };

    applications.retain(|application| {
        application.path != path && !application.name.eq_ignore_ascii_case("TextEdit")
    });
    applications.insert(
        0,
        OpenWithApplication {
            name: "TextEdit".to_string(),
            path,
        },
    );
}

fn parse_open_with_applications(json: &str) -> Result<Vec<OpenWithApplication>> {
    let paths: Vec<PathBuf> = serde_json::from_str(json.trim()).context("无法解析可用应用列表")?;
    let mut seen_names = HashSet::new();
    let mut applications = Vec::new();
    for path in paths {
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let name = file_name
            .strip_suffix(".app")
            .unwrap_or(file_name)
            .to_string();
        if seen_names.insert(name.to_lowercase()) {
            applications.push(OpenWithApplication { name, path });
        }
    }
    Ok(applications)
}

async fn discover_volume_paths(root: PathBuf, volumes_directory: PathBuf) -> Result<Vec<PathBuf>> {
    let root_identity = tokio::fs::canonicalize(&root)
        .await
        .unwrap_or_else(|_| root.clone());
    let mut seen = HashSet::from([root_identity]);
    let mut volumes = vec![root];

    if let Ok(mut entries) = tokio::fs::read_dir(volumes_directory).await {
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }

            let path = entry.path();
            let is_directory = tokio::fs::metadata(&path)
                .await
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false);
            if !is_directory {
                continue;
            }

            // macOS commonly exposes `/Volumes/Macintosh HD` as a symlink to
            // `/`. Compare canonical identities, but retain the original mount
            // path so real external volumes keep their user-facing names.
            let identity = tokio::fs::canonicalize(&path)
                .await
                .unwrap_or_else(|_| path.clone());
            if seen.insert(identity) {
                volumes.push(path);
            }
        }
    }

    volumes[1..].sort_by_cached_key(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    Ok(volumes)
}

/// Reads a directory using FlowFile's default policy: hide dot-files, place
/// folders first, and sort names ascending within each group.
pub async fn fetch_dir_entries(path: &Path) -> Result<Vec<FileItem>> {
    fetch_dir_entries_with_options(path, false, SortMode::Name).await
}

pub async fn fetch_dir_entries_with_options(
    path: &Path,
    show_hidden: bool,
    sort_mode: SortMode,
) -> Result<Vec<FileItem>> {
    let mut directory = tokio::fs::read_dir(path)
        .await
        .with_context(|| format!("无法读取目录 {}", path.display()))?;
    let mut items = Vec::new();

    while let Some(entry) = directory
        .next_entry()
        .await
        .with_context(|| format!("读取目录项失败：{}", path.display()))?
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_hidden = name.starts_with('.');
        if is_hidden && !show_hidden {
            continue;
        }

        let entry_path = entry.path();
        match tokio::fs::metadata(&entry_path).await {
            Ok(metadata) => items.push(FileItem::from_metadata(
                entry_path, name, metadata, is_hidden,
            )),
            Err(error) => {
                eprintln!("FlowFile: 跳过无法读取元数据的项目：{error}");
            }
        }
    }

    FileItem::sort_items(&mut items, sort_mode);
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::{
        OpenWithApplication, discover_volume_paths, ensure_text_editor, fetch_dir_entries,
        fetch_dir_entries_with_options, parse_open_with_applications,
    };
    use crate::models::SortMode;
    use std::{fs, path::PathBuf};

    #[tokio::test]
    async fn reads_metadata_hides_dot_files_and_sorts_folders_first() {
        let directory = tempfile::tempdir().expect("temp directory");
        fs::create_dir(directory.path().join("Beta")).expect("create folder");
        fs::create_dir(directory.path().join("Alpha")).expect("create folder");
        fs::write(directory.path().join("notes.txt"), b"hello").expect("create file");
        fs::write(directory.path().join(".secret"), b"hidden").expect("create hidden file");

        let items = fetch_dir_entries(directory.path())
            .await
            .expect("read directory");
        let names: Vec<_> = items.iter().map(|item| item.name.as_str()).collect();

        assert_eq!(names, ["Alpha", "Beta", "notes.txt"]);
        assert!(items[0].is_dir);
        assert_eq!(items[2].size, 5);
        assert_eq!(items[2].extension.as_deref(), Some("txt"));

        let visible_hidden = fetch_dir_entries_with_options(directory.path(), true, SortMode::Name)
            .await
            .expect("read hidden entries");
        assert!(visible_hidden.iter().any(|item| item.name == ".secret"));
    }

    #[tokio::test]
    async fn volume_discovery_deduplicates_symlinks_to_the_system_volume() {
        let directory = tempfile::tempdir().expect("temp directory");
        let root = directory.path().join("system-root");
        let volumes_directory = directory.path().join("Volumes");
        let external = volumes_directory.join("External SSD");
        fs::create_dir(&root).expect("create root");
        fs::create_dir(&volumes_directory).expect("create volumes directory");
        fs::create_dir(&external).expect("create external volume");
        std::os::unix::fs::symlink(&root, volumes_directory.join("Macintosh HD"))
            .expect("create system volume alias");

        let volumes = discover_volume_paths(root.clone(), volumes_directory)
            .await
            .expect("discover volumes");

        assert_eq!(volumes, vec![root, external]);
    }

    #[test]
    fn open_with_applications_use_app_names_and_remove_duplicates() {
        let applications = parse_open_with_applications(
            r#"["/Applications/CotEditor.app","/Applications/Microsoft Edge.app","/Library/Updater/Microsoft Edge.app"]"#,
        )
        .expect("parse applications");

        assert_eq!(
            applications,
            vec![
                OpenWithApplication {
                    name: "CotEditor".to_string(),
                    path: "/Applications/CotEditor.app".into(),
                },
                OpenWithApplication {
                    name: "Microsoft Edge".to_string(),
                    path: "/Applications/Microsoft Edge.app".into(),
                },
            ]
        );
    }

    #[test]
    fn open_with_applications_always_include_text_edit() {
        let mut applications = vec![OpenWithApplication {
            name: "TextEdit".to_string(),
            path: "/Applications/TextEdit.app".into(),
        }];

        ensure_text_editor(&mut applications);

        assert_eq!(applications.len(), 1);
        assert_eq!(applications[0].name, "TextEdit");
        assert_eq!(
            applications[0].path,
            PathBuf::from("/System/Applications/TextEdit.app")
        );
    }
}
