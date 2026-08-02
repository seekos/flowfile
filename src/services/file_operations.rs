use super::FileEngine;
use anyhow::{Context as _, Result, bail};
use async_channel::Sender;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
    runtime::Handle,
};

const COPY_BUFFER_SIZE: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TransferMode {
    Copy,
    Move,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ConflictPolicy {
    #[default]
    AutoRename,
    Overwrite,
}

impl ConflictPolicy {
    pub fn from_overwrite(overwrite: bool) -> Self {
        if overwrite {
            Self::Overwrite
        } else {
            Self::AutoRename
        }
    }
}

#[derive(Clone, Debug)]
pub struct TransferProgress {
    pub current_file: String,
    pub bytes_done: u64,
    pub total_bytes: u64,
    pub speed_bytes_per_second: f64,
}

#[derive(Clone)]
pub struct FileOperationEngine {
    runtime: Handle,
}

impl FileOperationEngine {
    pub fn new(file_engine: &FileEngine) -> Self {
        Self {
            runtime: file_engine.runtime_handle(),
        }
    }

    pub async fn transfer(
        &self,
        sources: Vec<PathBuf>,
        destination: PathBuf,
        mode: TransferMode,
        conflict_policy: ConflictPolicy,
        progress: Sender<TransferProgress>,
    ) -> Result<Vec<PathBuf>> {
        self.runtime
            .spawn(async move {
                execute_transfer(sources, destination, mode, conflict_policy, progress).await
            })
            .await
            .context("文件传输任务异常终止")?
    }

    pub async fn create_directory(&self, parent: PathBuf, name: String) -> Result<PathBuf> {
        self.runtime
            .spawn(async move {
                validate_file_name(&name)?;
                let path = available_path(&parent.join(name)).await?;
                fs::create_dir(&path)
                    .await
                    .with_context(|| format!("无法创建文件夹 {}", path.display()))?;
                Ok(path)
            })
            .await
            .context("新建文件夹任务异常终止")?
    }

    pub async fn create_text_file(&self, parent: PathBuf, name: String) -> Result<PathBuf> {
        self.runtime
            .spawn(async move {
                validate_file_name(&name)?;
                let path = available_path(&parent.join(name)).await?;
                fs::File::create(&path)
                    .await
                    .with_context(|| format!("无法创建文件 {}", path.display()))?;
                Ok(path)
            })
            .await
            .context("新建文件任务异常终止")?
    }

    pub async fn rename(&self, source: PathBuf, new_name: String) -> Result<PathBuf> {
        self.runtime
            .spawn(async move {
                validate_file_name(&new_name)?;
                let parent = source.parent().context("无法确定文件所在目录")?;
                let destination = parent.join(new_name);
                if source == destination {
                    return Ok(source);
                }
                if fs::try_exists(&destination).await? {
                    bail!("目标名称已存在：{}", destination.display());
                }
                fs::rename(&source, &destination).await.with_context(|| {
                    format!(
                        "无法将 {} 重命名为 {}",
                        source.display(),
                        destination.display()
                    )
                })?;
                Ok(destination)
            })
            .await
            .context("重命名任务异常终止")?
    }

    pub async fn move_to_trash(&self, paths: Vec<PathBuf>) -> Result<()> {
        self.runtime
            .spawn_blocking(move || {
                trash::delete_all(paths.iter()).context("无法将所选项目移到废纸篓")
            })
            .await
            .context("废纸篓任务异常终止")?
    }

    pub async fn delete_permanently(&self, paths: Vec<PathBuf>) -> Result<()> {
        self.runtime
            .spawn(async move {
                for path in paths {
                    let metadata = fs::symlink_metadata(&path)
                        .await
                        .with_context(|| format!("无法读取 {}", path.display()))?;
                    if metadata.is_dir() {
                        fs::remove_dir_all(&path)
                            .await
                            .with_context(|| format!("无法删除文件夹 {}", path.display()))?;
                    } else {
                        fs::remove_file(&path)
                            .await
                            .with_context(|| format!("无法删除文件 {}", path.display()))?;
                    }
                }
                Ok(())
            })
            .await
            .context("永久删除任务异常终止")?
    }

    pub async fn show_info(&self, path: PathBuf) -> Result<()> {
        self.runtime
            .spawn_blocking(move || {
                let status = Command::new("/usr/bin/osascript")
                    .args([
                        "-e",
                        "on run argv",
                        "-e",
                        "tell application \"Finder\" to open information window of (POSIX file (item 1 of argv) as alias)",
                        "-e",
                        "end run",
                    ])
                    .arg(path)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .context("无法调用 Finder 显示简介")?;
                if !status.success() {
                    bail!("Finder 无法显示所选项目的简介");
                }
                Ok(())
            })
            .await
            .context("显示简介任务异常终止")?
    }
}

struct TransferPlan {
    source_root: PathBuf,
    destination_root: PathBuf,
    entries: Vec<PlanEntry>,
    total_bytes: u64,
}

struct PlanEntry {
    source: PathBuf,
    relative: PathBuf,
    is_dir: bool,
    size: u64,
}

async fn execute_transfer(
    sources: Vec<PathBuf>,
    destination: PathBuf,
    mode: TransferMode,
    conflict_policy: ConflictPolicy,
    progress: Sender<TransferProgress>,
) -> Result<Vec<PathBuf>> {
    if sources.is_empty() {
        bail!("没有可传输的项目");
    }
    let destination = fs::canonicalize(&destination)
        .await
        .with_context(|| format!("目标目录不存在：{}", destination.display()))?;
    if !fs::metadata(&destination).await?.is_dir() {
        bail!("目标不是文件夹：{}", destination.display());
    }

    let mut plans = Vec::with_capacity(sources.len());
    let mut reserved_destinations = HashSet::new();
    for source in sources {
        if source == destination || destination.starts_with(&source) {
            bail!("不能将文件夹传输到自身内部：{}", source.display());
        }
        if mode == TransferMode::Move && source.parent() == Some(destination.as_path()) {
            bail!("源文件已经位于目标文件夹中");
        }
        let plan = build_plan(
            source,
            &destination,
            conflict_policy,
            &reserved_destinations,
        )
        .await?;
        reserved_destinations.insert(plan.destination_root.clone());
        plans.push(plan);
    }

    let total_bytes = plans.iter().map(|plan| plan.total_bytes).sum();
    let started = Instant::now();
    let mut bytes_done = 0_u64;
    let mut destinations = Vec::with_capacity(plans.len());

    for plan in plans {
        let current_file = plan
            .source_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("项目")
            .to_string();

        if mode == TransferMode::Move
            && fs::rename(&plan.source_root, &plan.destination_root)
                .await
                .is_ok()
        {
            bytes_done += plan.total_bytes;
            send_progress(&progress, &current_file, bytes_done, total_bytes, started);
            destinations.push(plan.destination_root);
            continue;
        }

        copy_plan(&plan, &progress, total_bytes, &mut bytes_done, started).await?;

        if mode == TransferMode::Move {
            let metadata = fs::symlink_metadata(&plan.source_root).await?;
            if metadata.is_dir() {
                fs::remove_dir_all(&plan.source_root).await?;
            } else {
                fs::remove_file(&plan.source_root).await?;
            }
        }
        destinations.push(plan.destination_root);
    }

    if total_bytes == 0 {
        send_progress(&progress, "完成", 1, 1, started);
    }
    Ok(destinations)
}

async fn build_plan(
    source_root: PathBuf,
    destination: &Path,
    conflict_policy: ConflictPolicy,
    reserved_destinations: &HashSet<PathBuf>,
) -> Result<TransferPlan> {
    let file_name = source_root
        .file_name()
        .context("无法确定源文件名称")?
        .to_owned();
    let desired_destination = destination.join(file_name);
    let destination_root = match conflict_policy {
        ConflictPolicy::AutoRename => {
            available_path_with_reserved(&desired_destination, reserved_destinations).await?
        }
        ConflictPolicy::Overwrite => {
            remove_existing(&desired_destination).await?;
            desired_destination
        }
    };

    let mut entries = Vec::new();
    let mut stack = vec![(source_root.clone(), PathBuf::new())];
    let mut total_bytes = 0_u64;

    while let Some((source, relative)) = stack.pop() {
        let metadata = fs::metadata(&source)
            .await
            .with_context(|| format!("无法读取 {}", source.display()))?;
        if metadata.is_dir() {
            entries.push(PlanEntry {
                source: source.clone(),
                relative: relative.clone(),
                is_dir: true,
                size: 0,
            });
            let mut directory = fs::read_dir(&source).await?;
            while let Some(entry) = directory.next_entry().await? {
                stack.push((entry.path(), relative.join(entry.file_name())));
            }
        } else {
            total_bytes += metadata.len();
            entries.push(PlanEntry {
                source,
                relative,
                is_dir: false,
                size: metadata.len(),
            });
        }
    }

    entries.sort_by_key(|entry| !entry.is_dir);
    Ok(TransferPlan {
        source_root,
        destination_root,
        entries,
        total_bytes,
    })
}

async fn copy_plan(
    plan: &TransferPlan,
    progress: &Sender<TransferProgress>,
    total_bytes: u64,
    bytes_done: &mut u64,
    started: Instant,
) -> Result<()> {
    for entry in &plan.entries {
        let destination = if entry.relative.as_os_str().is_empty() {
            plan.destination_root.clone()
        } else {
            plan.destination_root.join(&entry.relative)
        };
        if entry.is_dir {
            fs::create_dir_all(&destination).await?;
            continue;
        }

        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
        }
        copy_file_streamed(
            entry,
            &destination,
            progress,
            total_bytes,
            bytes_done,
            started,
        )
        .await?;
    }
    Ok(())
}

async fn copy_file_streamed(
    entry: &PlanEntry,
    destination: &Path,
    progress: &Sender<TransferProgress>,
    total_bytes: u64,
    bytes_done: &mut u64,
    started: Instant,
) -> Result<()> {
    let mut source = fs::File::open(&entry.source).await?;
    let mut output = fs::File::create(destination).await?;
    let buffer_len = COPY_BUFFER_SIZE.min(usize::try_from(entry.size.max(1)).unwrap_or(usize::MAX));
    let mut buffer = vec![0_u8; buffer_len];
    let current_file = entry
        .source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("文件");

    loop {
        let read = source.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read]).await?;
        *bytes_done += read as u64;
        send_progress(progress, current_file, *bytes_done, total_bytes, started);
    }
    output.flush().await?;
    Ok(())
}

fn send_progress(
    sender: &Sender<TransferProgress>,
    current_file: &str,
    bytes_done: u64,
    total_bytes: u64,
    started: Instant,
) {
    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    let _ = sender.try_send(TransferProgress {
        current_file: current_file.to_string(),
        bytes_done,
        total_bytes,
        speed_bytes_per_second: bytes_done as f64 / elapsed,
    });
}

async fn available_path(path: &Path) -> Result<PathBuf> {
    available_path_with_reserved(path, &HashSet::new()).await
}

async fn available_path_with_reserved(
    path: &Path,
    reserved_destinations: &HashSet<PathBuf>,
) -> Result<PathBuf> {
    if !reserved_destinations.contains(path) && !fs::try_exists(path).await? {
        return Ok(path.to_path_buf());
    }

    let parent = path.parent().context("目标路径没有父目录")?;
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("副本");
    let extension = path.extension().and_then(|extension| extension.to_str());

    for index in 1.. {
        let name = match extension {
            Some(extension) => format!("{stem} ({index}).{extension}"),
            None => format!("{stem} ({index})"),
        };
        let candidate = parent.join(name);
        if !reserved_destinations.contains(&candidate) && !fs::try_exists(&candidate).await? {
            return Ok(candidate);
        }
    }
    unreachable!()
}

async fn remove_existing(path: &Path) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path).await else {
        return Ok(());
    };
    if metadata.is_dir() {
        fs::remove_dir_all(path).await?;
    } else {
        fs::remove_file(path).await?;
    }
    Ok(())
}

fn validate_file_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("名称不能为空");
    }
    if name.contains('/') || name == "." || name == ".." {
        bail!("名称不能包含 “/”，也不能是 “.” 或 “..”");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ConflictPolicy, TransferMode, available_path, execute_transfer};
    use std::fs;

    #[tokio::test]
    async fn copy_uses_numbered_name_when_destination_exists() {
        let directory = tempfile::tempdir().expect("temp directory");
        let source = directory.path().join("notes.txt");
        fs::write(&source, b"flowfile").expect("write source");
        let (progress, _receiver) = async_channel::bounded(8);

        let destinations = execute_transfer(
            vec![source],
            directory.path().to_path_buf(),
            TransferMode::Copy,
            ConflictPolicy::AutoRename,
            progress,
        )
        .await
        .expect("copy file");

        assert_eq!(
            destinations[0].file_name().unwrap().to_string_lossy(),
            "notes (1).txt"
        );
        assert_eq!(fs::read(&destinations[0]).unwrap(), b"flowfile");
    }

    #[tokio::test]
    async fn move_removes_source_and_keeps_contents() {
        let source_directory = tempfile::tempdir().expect("source directory");
        let destination_directory = tempfile::tempdir().expect("destination directory");
        let source = source_directory.path().join("project");
        fs::create_dir(&source).expect("create source folder");
        fs::write(source.join("README.md"), b"hello").expect("write source file");
        let (progress, _receiver) = async_channel::bounded(8);

        let destinations = execute_transfer(
            vec![source.clone()],
            destination_directory.path().to_path_buf(),
            TransferMode::Move,
            ConflictPolicy::AutoRename,
            progress,
        )
        .await
        .expect("move folder");

        assert!(!source.exists());
        assert_eq!(
            fs::read(destinations[0].join("README.md")).unwrap(),
            b"hello"
        );
    }

    #[tokio::test]
    async fn available_path_preserves_extension() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("archive.tar.gz");
        fs::write(&path, b"one").expect("write file");

        let candidate = available_path(&path).await.expect("available name");

        assert_eq!(
            candidate.file_name().unwrap().to_string_lossy(),
            "archive.tar (1).gz"
        );
    }

    #[tokio::test]
    async fn batch_copy_reserves_names_before_transfer_starts() {
        let first = tempfile::tempdir().expect("first source");
        let second = tempfile::tempdir().expect("second source");
        let destination = tempfile::tempdir().expect("destination");
        let first_file = first.path().join("same.txt");
        let second_file = second.path().join("same.txt");
        fs::write(&first_file, b"first").expect("write first");
        fs::write(&second_file, b"second").expect("write second");
        let (progress, _receiver) = async_channel::bounded(8);

        let destinations = execute_transfer(
            vec![first_file, second_file],
            destination.path().to_path_buf(),
            TransferMode::Copy,
            ConflictPolicy::AutoRename,
            progress,
        )
        .await
        .expect("copy batch");

        assert_ne!(destinations[0], destinations[1]);
        assert!(destinations.iter().all(|path| path.exists()));
    }
}
