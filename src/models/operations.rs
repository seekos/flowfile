use super::{Model, MultiPaneModel};
use crate::services::{
    ConflictPolicy, FileOperationEngine, TransferControl, TransferMode, TransferProgress,
    clear_file_clipboard, file_clipboard_change_count, is_transfer_cancelled, read_file_clipboard,
    write_file_clipboard,
};
use gpui::{Context, EventEmitter};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::Duration,
};

const CLIPBOARD_SIGNATURE: &str = "com.flowfile.files.v1";
const SUCCESS_NOTICE_DURATION: Duration = Duration::from_secs(4);
const TRANSFER_PROGRESS_UPDATE_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ClipboardMode {
    Copy,
    Cut,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ClipboardPayload {
    signature: String,
    mode: ClipboardMode,
    paths: Vec<PathBuf>,
}

fn native_clipboard_mode(
    native_paths: &[PathBuf],
    internal: Option<&ClipboardPayload>,
    internal_change_count: Option<isize>,
    current_change_count: Option<isize>,
) -> ClipboardMode {
    internal
        .filter(|clipboard| clipboard.paths == native_paths)
        .filter(|_| internal_change_count == current_change_count)
        .map_or(ClipboardMode::Copy, |clipboard| clipboard.mode)
}

fn take_latest_progress(
    receiver: &async_channel::Receiver<TransferProgress>,
    mut latest: TransferProgress,
) -> TransferProgress {
    while let Ok(progress) = receiver.try_recv() {
        latest = progress;
    }
    latest
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreateItemKind {
    Folder,
    TextFile,
}

#[derive(Clone, Debug)]
pub struct FileDragPayload {
    pub paths: Vec<PathBuf>,
    pub source_pane_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileOperationEvent {
    TransferStarted,
    TransferChanged,
}

#[derive(Clone, Debug)]
pub struct TransferActivity {
    pub running: bool,
    pub paused: bool,
    pub cancelling: bool,
    pub completed_successfully: bool,
    pub current_file: String,
    pub bytes_done: u64,
    pub total_bytes: u64,
    pub speed_bytes_per_second: f64,
    pub mode: TransferMode,
    pub item_count: usize,
    pub source_location: PathBuf,
    pub destination: PathBuf,
}

impl TransferActivity {
    pub fn progress(&self) -> f32 {
        if self.completed_successfully {
            return 1.0;
        }
        if self.total_bytes == 0 {
            return 0.0;
        }
        // Copying metadata and removing the source during a cross-volume move
        // happen after the last file byte is written. Reserve the final 1% for
        // actual task completion so a running transfer never appears finished.
        (self.bytes_done as f32 / self.total_bytes as f32).clamp(0.0, 0.99)
    }
}

pub struct FileOperationController {
    model: Model<MultiPaneModel>,
    engine: FileOperationEngine,
    clipboard: Option<ClipboardPayload>,
    clipboard_change_count: Option<isize>,
    pub transfer: Option<TransferActivity>,
    pub notice: Option<String>,
    pub notice_is_error: bool,
    notice_generation: u64,
    transfer_generation: u64,
    transfer_control: Option<TransferControl>,
}

impl EventEmitter<FileOperationEvent> for FileOperationController {}

impl FileOperationController {
    pub fn new(model: Model<MultiPaneModel>, engine: FileOperationEngine) -> Self {
        Self {
            model,
            engine,
            clipboard: None,
            clipboard_change_count: None,
            transfer: None,
            notice: None,
            notice_is_error: false,
            notice_generation: 0,
            transfer_generation: 0,
            transfer_control: None,
        }
    }

    pub fn cut_paths(&self) -> HashSet<PathBuf> {
        if self.clipboard_change_count != file_clipboard_change_count() {
            return HashSet::new();
        }
        self.clipboard
            .as_ref()
            .filter(|clipboard| clipboard.mode == ClipboardMode::Cut)
            .map(|clipboard| clipboard.paths.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn copy_selected(&mut self, cx: &mut Context<Self>) {
        self.set_clipboard(ClipboardMode::Copy, cx);
    }

    pub fn cut_selected(&mut self, cx: &mut Context<Self>) {
        self.set_clipboard(ClipboardMode::Cut, cx);
    }

    fn set_clipboard(&mut self, mode: ClipboardMode, cx: &mut Context<Self>) {
        let paths = self.active_selected_paths(cx);
        if paths.is_empty() {
            self.set_notice("请先选择文件", true, cx);
            return;
        }

        let payload = ClipboardPayload {
            signature: CLIPBOARD_SIGNATURE.to_string(),
            mode,
            paths,
        };
        let change_count = match write_file_clipboard(&payload.paths) {
            Ok(change_count) => change_count,
            Err(error) => {
                self.set_notice(error.to_string(), true, cx);
                return;
            }
        };
        let count = payload.paths.len();
        self.clipboard = Some(payload);
        self.clipboard_change_count = Some(change_count);
        self.set_notice(
            match mode {
                ClipboardMode::Copy => format!("已复制 {count} 个项目"),
                ClipboardMode::Cut => format!("已剪切 {count} 个项目"),
            },
            false,
            cx,
        );
    }

    pub fn paste_into_active(&mut self, cx: &mut Context<Self>) {
        self.paste_into_active_with_mode(None, cx);
    }

    pub fn move_clipboard_into_active(&mut self, cx: &mut Context<Self>) {
        self.paste_into_active_with_mode(Some(TransferMode::Move), cx);
    }

    fn paste_into_active_with_mode(
        &mut self,
        forced_mode: Option<TransferMode>,
        cx: &mut Context<Self>,
    ) {
        if !self.active_accepts_file_operations(cx) {
            self.set_notice("请先打开一个 SMB 共享文件夹再粘贴", true, cx);
            return;
        }
        let Some(payload) = self.read_clipboard(cx) else {
            self.set_notice("剪贴板中没有可粘贴的文件路径", true, cx);
            return;
        };
        let destination = self.active_path(cx);
        let mode = forced_mode.unwrap_or(match payload.mode {
            ClipboardMode::Copy => TransferMode::Copy,
            ClipboardMode::Cut => TransferMode::Move,
        });
        self.start_transfer(payload.paths, destination, mode, cx);
    }

    pub fn duplicate_selected(&mut self, cx: &mut Context<Self>) {
        let paths = self.active_selected_paths(cx);
        if paths.is_empty() {
            self.set_notice("请先选择要创建副本的项目", true, cx);
            return;
        }
        self.start_transfer(paths, self.active_path(cx), TransferMode::Copy, cx);
    }

    pub fn transfer_selected_to_other(&mut self, mode: TransferMode, cx: &mut Context<Self>) {
        let paths = self.active_selected_paths(cx);
        if paths.is_empty() {
            self.set_notice("请先选择要传输的项目", true, cx);
            return;
        }

        let destination = {
            let model = self.model.read(cx);
            let Some(index) = model.other_pane_index() else {
                self.set_notice("当前布局中没有其他目标面板", true, cx);
                return;
            };
            let pane = model.panes[index].read(cx);
            if pane.is_smb_server_root() {
                self.set_notice("请先在目标面板打开一个 SMB 共享文件夹", true, cx);
                return;
            }
            pane.current_path.clone()
        };
        self.start_transfer(paths, destination, mode, cx);
    }

    pub fn transfer_to_path(
        &mut self,
        paths: Vec<PathBuf>,
        destination: PathBuf,
        mode: TransferMode,
        cx: &mut Context<Self>,
    ) {
        self.start_transfer(paths, destination, mode, cx);
    }

    pub fn show_notice(
        &mut self,
        notice: impl Into<String>,
        is_error: bool,
        cx: &mut Context<Self>,
    ) {
        self.set_notice(notice, is_error, cx);
    }

    pub fn pause_transfer(&mut self, cx: &mut Context<Self>) {
        let Some(transfer) = &mut self.transfer else {
            return;
        };
        if !transfer.running || transfer.paused || transfer.cancelling {
            return;
        }
        if let Some(control) = &self.transfer_control {
            control.pause();
            transfer.paused = true;
            transfer.speed_bytes_per_second = 0.0;
            cx.emit(FileOperationEvent::TransferChanged);
        }
    }

    pub fn resume_transfer(&mut self, cx: &mut Context<Self>) {
        let Some(transfer) = &mut self.transfer else {
            return;
        };
        if !transfer.running || !transfer.paused || transfer.cancelling {
            return;
        }
        if let Some(control) = &self.transfer_control {
            control.resume();
            transfer.paused = false;
            cx.emit(FileOperationEvent::TransferChanged);
        }
    }

    pub fn cancel_transfer(&mut self, cx: &mut Context<Self>) {
        let Some(transfer) = &mut self.transfer else {
            return;
        };
        if !transfer.running || transfer.cancelling {
            return;
        }
        if let Some(control) = &self.transfer_control {
            control.cancel();
            transfer.paused = false;
            transfer.cancelling = true;
            transfer.speed_bytes_per_second = 0.0;
            cx.emit(FileOperationEvent::TransferChanged);
        }
    }

    pub fn move_selected_to_trash(&mut self, cx: &mut Context<Self>) {
        let paths = self.active_selected_paths(cx);
        if paths.is_empty() {
            self.set_notice("请先选择要移到废纸篓的项目", true, cx);
            return;
        }
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let result = engine.move_to_trash(paths).await;
            let _ = this.update(cx, |controller, cx| match result {
                Ok(()) => {
                    controller.set_notice("已移到废纸篓", false, cx);
                    controller.refresh_all_panes(cx);
                }
                Err(error) => controller.set_notice(error.to_string(), true, cx),
            });
        })
        .detach();
    }

    pub fn delete_permanently(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let result = engine.delete_permanently(paths).await;
            let _ = this.update(cx, |controller, cx| match result {
                Ok(()) => {
                    controller.set_notice("所选项目已永久删除", false, cx);
                    controller.refresh_all_panes(cx);
                }
                Err(error) => controller.set_notice(error.to_string(), true, cx),
            });
        })
        .detach();
    }

    pub fn create_item(&mut self, kind: CreateItemKind, name: String, cx: &mut Context<Self>) {
        if !self.active_accepts_file_operations(cx) {
            self.set_notice("请先打开一个 SMB 共享文件夹再新建项目", true, cx);
            return;
        }
        let parent = self.active_path(cx);
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let result = match kind {
                CreateItemKind::Folder => engine.create_directory(parent, name).await,
                CreateItemKind::TextFile => engine.create_text_file(parent, name).await,
            };
            let _ = this.update(cx, |controller, cx| match result {
                Ok(path) => {
                    controller.set_notice(
                        format!(
                            "已创建 {}",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        ),
                        false,
                        cx,
                    );
                    controller.refresh_all_panes(cx);
                }
                Err(error) => controller.set_notice(error.to_string(), true, cx),
            });
        })
        .detach();
    }

    pub fn active_selected_paths(&self, cx: &gpui::App) -> Vec<PathBuf> {
        let model = self.model.read(cx);
        model.panes[model.active_pane_index]
            .read(cx)
            .selected_paths()
    }

    pub fn can_paste(&self, cx: &gpui::App) -> bool {
        self.read_clipboard(cx).is_some()
    }

    pub fn show_selected_info(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.active_selected_paths(cx).into_iter().next() else {
            self.set_notice("请先选择要查看简介的项目", true, cx);
            return;
        };
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let result = engine.show_info(path).await;
            if let Err(error) = result {
                let _ = this.update(cx, |controller, cx| {
                    controller.set_notice(error.to_string(), true, cx);
                });
            }
        })
        .detach();
    }

    fn active_path(&self, cx: &gpui::App) -> PathBuf {
        let model = self.model.read(cx);
        model.panes[model.active_pane_index]
            .read(cx)
            .current_path
            .clone()
    }

    fn active_accepts_file_operations(&self, cx: &gpui::App) -> bool {
        let model = self.model.read(cx);
        !model.panes[model.active_pane_index]
            .read(cx)
            .is_smb_server_root()
    }

    fn read_clipboard(&self, cx: &gpui::App) -> Option<ClipboardPayload> {
        let native_paths = read_file_clipboard();
        if !native_paths.is_empty() {
            let mode = native_clipboard_mode(
                &native_paths,
                self.clipboard.as_ref(),
                self.clipboard_change_count,
                file_clipboard_change_count(),
            );
            return Some(ClipboardPayload {
                signature: CLIPBOARD_SIGNATURE.to_string(),
                mode,
                paths: native_paths,
            });
        }

        let item = cx.read_from_clipboard()?;
        if let Some(metadata) = item.metadata()
            && let Ok(payload) = serde_json::from_str::<ClipboardPayload>(metadata)
            && payload.signature == CLIPBOARD_SIGNATURE
        {
            return Some(payload);
        }

        let paths = item
            .text()?
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .collect::<Vec<_>>();
        (!paths.is_empty()).then_some(ClipboardPayload {
            signature: CLIPBOARD_SIGNATURE.to_string(),
            mode: ClipboardMode::Copy,
            paths,
        })
    }

    fn start_transfer(
        &mut self,
        sources: Vec<PathBuf>,
        destination: PathBuf,
        mode: TransferMode,
        cx: &mut Context<Self>,
    ) {
        if self
            .transfer
            .as_ref()
            .is_some_and(|transfer| transfer.running)
        {
            self.set_notice("已有文件传输正在进行", true, cx);
            return;
        }

        self.transfer_generation += 1;
        let generation = self.transfer_generation;
        // Progress values are snapshots. A single-slot channel lets the engine
        // replace a stale pending value with the newest byte count.
        let (progress_sender, progress_receiver) = async_channel::bounded(1);
        let engine = self.engine.clone();
        let sources_for_task = sources.clone();
        let source_location = sources
            .first()
            .and_then(|path| path.parent())
            .unwrap_or_else(|| Path::new(""))
            .to_path_buf();
        let control = TransferControl::default();
        self.transfer_control = Some(control.clone());
        self.transfer = Some(TransferActivity {
            running: true,
            paused: false,
            cancelling: false,
            completed_successfully: false,
            current_file: sources
                .first()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .unwrap_or("准备传输")
                .to_string(),
            bytes_done: 0,
            total_bytes: 0,
            speed_bytes_per_second: 0.0,
            mode,
            item_count: sources.len(),
            source_location,
            destination: destination.clone(),
        });
        self.notice = None;
        cx.emit(FileOperationEvent::TransferStarted);
        cx.notify();

        cx.spawn(async move |this, cx| {
            while let Ok(progress) = progress_receiver.recv().await {
                // A fast local copy can produce thousands of 256 KiB updates
                // per second. Keep the newest snapshot queued, but repaint the
                // progress window at a modest cadence so typing, selection, and
                // rename input keep priority on the GPUI foreground executor.
                cx.background_executor()
                    .timer(TRANSFER_PROGRESS_UPDATE_INTERVAL)
                    .await;
                let progress = take_latest_progress(&progress_receiver, progress);
                let should_continue = this
                    .update(cx, |controller, cx| {
                        if controller.transfer_generation != generation {
                            return false;
                        }
                        if controller.apply_progress(progress) {
                            cx.emit(FileOperationEvent::TransferChanged);
                        }
                        true
                    })
                    .unwrap_or(false);
                if !should_continue {
                    break;
                }
            }
        })
        .detach();

        cx.spawn(async move |this, cx| {
            let result = engine
                .transfer(
                    sources_for_task,
                    destination,
                    mode,
                    ConflictPolicy::from_overwrite(false),
                    progress_sender,
                    control,
                )
                .await;
            let _ = this.update(cx, |controller, cx| {
                if controller.transfer_generation != generation {
                    return;
                }
                controller.transfer_control = None;
                match result {
                    Ok(destinations) => {
                        if let Some(transfer) = &mut controller.transfer {
                            transfer.running = false;
                            transfer.completed_successfully = true;
                            if transfer.total_bytes > 0 {
                                transfer.bytes_done = transfer.total_bytes;
                            }
                        }
                        if mode == TransferMode::Move
                            && controller.clipboard.as_ref().is_some_and(|clipboard| {
                                clipboard.mode == ClipboardMode::Cut && clipboard.paths == sources
                            })
                            && controller.clipboard_change_count == file_clipboard_change_count()
                        {
                            if let Some(change_count) = controller.clipboard_change_count {
                                clear_file_clipboard(change_count);
                            }
                            controller.clipboard = None;
                            controller.clipboard_change_count = None;
                        }
                        controller.set_notice(
                            format!("已完成 {} 个项目", destinations.len()),
                            false,
                            cx,
                        );
                        controller.refresh_all_panes(cx);
                    }
                    Err(error) => {
                        let cancelled = is_transfer_cancelled(&error);
                        if let Some(transfer) = &mut controller.transfer {
                            transfer.running = false;
                            transfer.paused = false;
                            transfer.cancelling = false;
                        }
                        if cancelled {
                            controller.set_notice("已停止文件传输", false, cx);
                            controller.refresh_all_panes(cx);
                        } else {
                            controller.set_notice(error.to_string(), true, cx);
                        }
                    }
                }
                cx.emit(FileOperationEvent::TransferChanged);
            });
        })
        .detach();
    }

    fn apply_progress(&mut self, progress: TransferProgress) -> bool {
        if let Some(transfer) = &mut self.transfer {
            if !transfer.running {
                return false;
            }
            transfer.current_file = progress.current_file;
            transfer.bytes_done = progress.bytes_done;
            transfer.total_bytes = progress.total_bytes;
            transfer.speed_bytes_per_second = progress.speed_bytes_per_second;
            true
        } else {
            false
        }
    }

    fn refresh_all_panes(&self, cx: &mut Context<Self>) {
        let panes = self.model.read(cx).panes.clone();
        for pane in panes {
            pane.update(cx, |pane, cx| pane.refresh(cx));
        }
    }

    fn set_notice(&mut self, notice: impl Into<String>, is_error: bool, cx: &mut Context<Self>) {
        self.notice_generation = self.notice_generation.wrapping_add(1);
        let generation = self.notice_generation;
        self.notice = Some(notice.into());
        self.notice_is_error = is_error;
        cx.notify();

        if !is_error {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(SUCCESS_NOTICE_DURATION)
                    .await;
                let _ = this.update(cx, |controller, cx| {
                    if controller.notice_generation == generation {
                        controller.notice = None;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CLIPBOARD_SIGNATURE, ClipboardMode, ClipboardPayload, TransferActivity,
        native_clipboard_mode, take_latest_progress,
    };
    use crate::services::{TransferMode, TransferProgress};
    use std::path::PathBuf;

    #[test]
    fn running_transfer_reserves_completion_until_the_task_finishes() {
        let transfer = TransferActivity {
            running: true,
            paused: false,
            cancelling: false,
            completed_successfully: false,
            current_file: "example.bin".to_string(),
            bytes_done: 100,
            total_bytes: 100,
            speed_bytes_per_second: 1.0,
            mode: TransferMode::Copy,
            item_count: 1,
            source_location: PathBuf::from("/source"),
            destination: PathBuf::from("/destination"),
        };

        assert_eq!(transfer.progress(), 0.99);
    }

    #[test]
    fn completed_transfer_is_always_full_even_after_a_stale_snapshot() {
        let transfer = TransferActivity {
            running: false,
            paused: false,
            cancelling: false,
            completed_successfully: true,
            current_file: "example.bin".to_string(),
            bytes_done: 25,
            total_bytes: 100,
            speed_bytes_per_second: 1.0,
            mode: TransferMode::Move,
            item_count: 1,
            source_location: PathBuf::from("/source"),
            destination: PathBuf::from("/destination"),
        };

        assert_eq!(transfer.progress(), 1.0);
    }

    #[test]
    fn failed_transfer_keeps_its_last_real_progress() {
        let transfer = TransferActivity {
            running: false,
            paused: false,
            cancelling: false,
            completed_successfully: false,
            current_file: "example.bin".to_string(),
            bytes_done: 25,
            total_bytes: 100,
            speed_bytes_per_second: 0.0,
            mode: TransferMode::Copy,
            item_count: 1,
            source_location: PathBuf::from("/source"),
            destination: PathBuf::from("/destination"),
        };

        assert_eq!(transfer.progress(), 0.25);
    }

    #[test]
    fn progress_repaint_uses_the_latest_snapshot_in_the_frame() {
        let (sender, receiver) = async_channel::bounded(1);
        let initial = TransferProgress {
            current_file: "large.bin".to_string(),
            bytes_done: 1,
            total_bytes: 100,
            speed_bytes_per_second: 1.0,
        };
        sender
            .force_send(TransferProgress {
                current_file: "large.bin".to_string(),
                bytes_done: 50,
                total_bytes: 100,
                speed_bytes_per_second: 50.0,
            })
            .expect("queue progress");
        sender
            .force_send(TransferProgress {
                current_file: "large.bin".to_string(),
                bytes_done: 75,
                total_bytes: 100,
                speed_bytes_per_second: 75.0,
            })
            .expect("replace queued progress");

        let progress = take_latest_progress(&receiver, initial);

        assert_eq!(progress.bytes_done, 75);
        assert_eq!(progress.speed_bytes_per_second, 75.0);
    }

    fn payload(mode: ClipboardMode, paths: &[&str]) -> ClipboardPayload {
        ClipboardPayload {
            signature: CLIPBOARD_SIGNATURE.to_string(),
            mode,
            paths: paths.iter().map(PathBuf::from).collect(),
        }
    }

    #[test]
    fn native_clipboard_keeps_internal_cut_when_pasteboard_is_unchanged() {
        let clipboard = payload(ClipboardMode::Cut, &["/tmp/a", "/tmp/b"]);
        let paths = vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")];

        assert_eq!(
            native_clipboard_mode(&paths, Some(&clipboard), Some(12), Some(12)),
            ClipboardMode::Cut
        );
    }

    #[test]
    fn externally_replaced_clipboard_defaults_to_copy() {
        let clipboard = payload(ClipboardMode::Cut, &["/tmp/a"]);
        let paths = vec![PathBuf::from("/tmp/a")];

        assert_eq!(
            native_clipboard_mode(&paths, Some(&clipboard), Some(12), Some(13)),
            ClipboardMode::Copy
        );
    }

    #[test]
    fn finder_paths_do_not_inherit_unrelated_internal_cut() {
        let clipboard = payload(ClipboardMode::Cut, &["/tmp/a"]);
        let paths = vec![PathBuf::from("/tmp/b")];

        assert_eq!(
            native_clipboard_mode(&paths, Some(&clipboard), Some(12), Some(12)),
            ClipboardMode::Copy
        );
    }
}
