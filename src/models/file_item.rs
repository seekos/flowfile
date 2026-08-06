use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    fs::Metadata,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FileKind {
    Folder,
    Application,
    Executable,
    Script,
    Document,
    Image,
    Archive,
    Audio,
    Video,
    Other,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum SortMode {
    #[default]
    Name,
    Size,
    Modified,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileItem {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub extension: Option<String>,
    pub size: u64,
    pub modified_unix: i64,
    pub modified: String,
    pub is_hidden: bool,
    pub kind: FileKind,
}

impl FileItem {
    pub fn from_metadata(path: PathBuf, name: String, metadata: Metadata, is_hidden: bool) -> Self {
        let is_dir = metadata.is_dir();
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_lowercase);
        let modified_time = metadata.modified().unwrap_or(UNIX_EPOCH);
        let modified_unix = modified_time
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or_default();

        let is_executable = has_unix_execute_bit(&metadata);
        let kind =
            if is_script_extension(extension.as_deref()) || (is_executable && has_shebang(&path)) {
                FileKind::Script
            } else if is_executable {
                FileKind::Executable
            } else {
                Self::kind_for(is_dir, extension.as_deref())
            };

        Self {
            kind,
            path,
            name,
            is_dir,
            extension,
            size: if is_dir { 0 } else { metadata.len() },
            modified_unix,
            modified: format_modified_time(modified_time),
            is_hidden,
        }
    }

    pub(crate) fn kind_for(is_dir: bool, extension: Option<&str>) -> FileKind {
        if is_dir {
            return if extension == Some("app") {
                FileKind::Application
            } else {
                FileKind::Folder
            };
        }

        match extension.unwrap_or_default() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "avif" | "svg" | "tif" | "tiff"
            | "bmp" => FileKind::Image,
            "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" | "dmg" => FileKind::Archive,
            "mp3" | "wav" | "aac" | "m4a" | "flac" => FileKind::Audio,
            "mp4" | "mov" | "mkv" | "avi" | "webm" => FileKind::Video,
            "txt" | "md" | "rtf" | "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx"
            | "rs" | "go" | "js" | "ts" | "json" | "toml" | "yaml" | "yml" => FileKind::Document,
            _ => FileKind::Other,
        }
    }

    pub fn formatted_size(&self) -> String {
        if self.is_dir {
            return "—".to_string();
        }

        const KB: f64 = 1024.0;
        const MB: f64 = KB * 1024.0;
        const GB: f64 = MB * 1024.0;
        let size = self.size as f64;

        if size >= GB {
            format!("{:.1} GB", size / GB)
        } else if size >= MB {
            format!("{:.1} MB", size / MB)
        } else if size >= KB {
            format!("{:.1} KB", size / KB)
        } else {
            format!("{} B", self.size)
        }
    }

    pub fn sort_items(items: &mut [Self], mode: SortMode) {
        items.sort_by(|left, right| {
            right.is_dir.cmp(&left.is_dir).then_with(|| match mode {
                SortMode::Name => natural_name_cmp(left, right),
                SortMode::Size => left
                    .size
                    .cmp(&right.size)
                    .then_with(|| natural_name_cmp(left, right)),
                SortMode::Modified => right
                    .modified_unix
                    .cmp(&left.modified_unix)
                    .then_with(|| natural_name_cmp(left, right)),
            })
        });
    }
}

#[cfg(unix)]
fn has_unix_execute_bit(metadata: &Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn has_unix_execute_bit(_metadata: &Metadata) -> bool {
    false
}

fn is_script_extension(extension: Option<&str>) -> bool {
    matches!(
        extension.unwrap_or_default(),
        "sh" | "bash"
            | "zsh"
            | "fish"
            | "command"
            | "py"
            | "pyw"
            | "rb"
            | "pl"
            | "php"
            | "lua"
            | "js"
            | "mjs"
            | "cjs"
            | "ts"
            | "jsx"
            | "tsx"
            | "awk"
            | "sed"
            | "tcl"
            | "expect"
            | "scpt"
            | "applescript"
    )
}

fn has_shebang(path: &Path) -> bool {
    use std::io::Read as _;

    let mut prefix = [0; 2];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut prefix))
        .is_ok()
        && prefix == *b"#!"
}

fn natural_name_cmp(left: &FileItem, right: &FileItem) -> Ordering {
    left.name
        .to_lowercase()
        .cmp(&right.name.to_lowercase())
        .then_with(|| left.name.cmp(&right.name))
}

fn format_modified_time(time: SystemTime) -> String {
    let local: DateTime<Local> = time.into();
    local.format("%Y-%m-%d %H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::{FileItem, FileKind};
    use std::{fs, path::PathBuf};
    use tempfile::tempdir;

    #[test]
    fn app_bundle_has_application_kind() {
        let directory = tempdir().expect("create temporary directory");
        let path = directory.path().join("Example.app");
        fs::create_dir(&path).expect("create app bundle directory");

        let item = FileItem::from_metadata(
            path.clone(),
            "Example.app".to_string(),
            fs::metadata(&path).expect("read app bundle metadata"),
            false,
        );

        assert_eq!(item.kind, FileKind::Application);
        assert!(item.is_dir);
        assert_eq!(FileItem::kind_for(true, None), FileKind::Folder);
    }

    #[cfg(unix)]
    #[test]
    fn executable_script_has_script_kind() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().expect("create temporary directory");
        let path = directory.path().join("launcher");
        fs::write(&path, b"#!/bin/sh\n").expect("create executable file");
        let mut permissions = fs::metadata(&path)
            .expect("read executable metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("set executable permission");

        let item = FileItem::from_metadata(
            PathBuf::from(&path),
            "launcher".to_string(),
            fs::metadata(&path).expect("read executable metadata"),
            false,
        );

        assert_eq!(item.kind, FileKind::Script);
        assert!(!item.is_dir);
    }

    #[test]
    fn script_extension_has_script_kind_without_execute_permission() {
        let directory = tempdir().expect("create temporary directory");
        let path = directory.path().join("build.sh");
        fs::write(&path, b"echo ready\n").expect("create script file");

        let item = FileItem::from_metadata(
            PathBuf::from(&path),
            "build.sh".to_string(),
            fs::metadata(&path).expect("read script metadata"),
            false,
        );

        assert_eq!(item.kind, FileKind::Script);
    }

    #[cfg(unix)]
    #[test]
    fn executable_binary_keeps_a_non_script_kind() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().expect("create temporary directory");
        let path = directory.path().join("binary");
        fs::write(&path, [0xcf, 0xfa, 0xed, 0xfe]).expect("create executable file");
        let mut permissions = fs::metadata(&path)
            .expect("read executable metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("set executable permission");

        let item = FileItem::from_metadata(
            PathBuf::from(&path),
            "binary".to_string(),
            fs::metadata(&path).expect("read executable metadata"),
            false,
        );

        assert_eq!(item.kind, FileKind::Executable);
    }
}
