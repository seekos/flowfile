use anyhow::{Context as _, Result};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static TEMPORARY_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn load_json_or_default<T>(path: PathBuf, label: &str) -> T
where
    T: DeserializeOwned + Default,
{
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return T::default(),
        Err(error) => {
            eprintln!("FlowFile: 无法读取{label} {}：{error}", path.display());
            return T::default();
        }
    };

    match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(error) => {
            let backup = corrupt_backup_path(&path);
            match fs::rename(&path, &backup) {
                Ok(()) => eprintln!(
                    "FlowFile: {label}损坏，已保留为 {}：{error}",
                    backup.display()
                ),
                Err(backup_error) => eprintln!(
                    "FlowFile: {label}损坏且无法保留备份 {}：{error}；{backup_error}",
                    path.display()
                ),
            }
            T::default()
        }
    }
}

pub(crate) fn atomic_write_json<T>(path: &Path, value: &T, label: &str) -> Result<()>
where
    T: Serialize + ?Sized,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建{label}目录 {}", parent.display()))?;
    }

    let bytes = serde_json::to_vec_pretty(value)?;
    let (temporary, mut file) = loop {
        let candidate = unique_sibling_path(path, "tmp");
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&candidate)
        {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("无法创建临时{label}文件 {}", candidate.display()));
            }
        }
    };
    let result = (|| -> Result<()> {
        file.write_all(&bytes)
            .with_context(|| format!("无法写入临时{label}文件 {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("无法同步临时{label}文件 {}", temporary.display()))?;
        fs::rename(&temporary, path)
            .with_context(|| format!("无法更新{label}文件 {}", path.display()))?;
        if let Some(parent) = path.parent()
            && let Ok(directory) = fs::File::open(parent)
        {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn unique_sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let sequence = TEMPORARY_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut name = OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(format!(".{suffix}-{}-{sequence}", std::process::id()));
    path.with_file_name(name)
}

fn corrupt_backup_path(path: &Path) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    unique_sibling_path(path, &format!("corrupt-{timestamp}"))
}

#[cfg(test)]
mod tests {
    use super::{atomic_write_json, load_json_or_default};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
    struct Settings {
        value: u32,
    }

    #[test]
    fn atomically_written_json_round_trips() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("settings.json");
        let settings = Settings { value: 42 };

        atomic_write_json(&path, &settings, "测试设置").expect("write settings");

        assert_eq!(load_json_or_default::<Settings>(path, "测试设置"), settings);
    }

    #[test]
    fn corrupt_json_is_quarantined_before_using_defaults() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("settings.json");
        std::fs::write(&path, b"{").expect("write corrupt settings");

        assert_eq!(
            load_json_or_default::<Settings>(path.clone(), "测试设置"),
            Settings::default()
        );
        assert!(!path.exists());
        assert!(
            std::fs::read_dir(directory.path())
                .expect("read directory")
                .any(|entry| entry
                    .expect("directory entry")
                    .file_name()
                    .to_string_lossy()
                    .contains("corrupt"))
        );
    }
}
