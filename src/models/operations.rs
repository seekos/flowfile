use super::{Model, MultiPaneModel};
use crate::services::{ConflictPolicy, FileOperationEngine, TransferMode, TransferProgress};
use gpui::{ClipboardItem, Context};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const CLIPBOARD_SIGNATURE: &str = "com.flowfile.files.v1";

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

#[derive(Clone, Debug)]
pub struct TransferActivity {
    pub running: bool,
    pub current_file: String,
    pub bytes_done: u64,
    pub total_bytes: u64,
    pub speed_bytes_per_second: f64,
    pub mode: TransferMode,
}

impl TransferActivity {
    pub fn progress(&self) -> f32 {
        if self.total_bytes == 0 {
            return if self.running { 0.0 } else { 1.0 };
        }
        (self.bytes_done as f32 / self.total_bytes as f32).clamp(0.0, 1.0)
    }
}

pub struct FileOperationController {
    model: Model<MultiPaneModel>,
    engine: FileOperationEngine,
    clipboard: Option<ClipboardPayload>,
    pub transfer: Option<TransferActivity>,
    pub notice: Option<String>,
    pub notice_is_error: bool,
    transfer_generation: u64,
}

impl FileOperationController {
    pub fn new(model: Model<MultiPaneModel>, engine: FileOperationEngine) -> Self {
        Self {
            model,
            engine,
            clipboard: None,
            transfer: None,
            notice: None,
            notice_is_error: false,
            transfer_generation: 0,
        }
    }

    pub fn is_cut_path(&self, path: &Path) -> bool {
        self.clipboard
            .as_ref()
            .filter(|clipboard| clipboard.mode == ClipboardMode::Cut)
            .is_some_and(|clipboard| clipboard.paths.iter().any(|cut_path| cut_path == path))
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
        let text = payload
            .paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
            text,
            payload.clone(),
        ));
        let count = payload.paths.len();
        self.clipboard = Some(payload);
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
        let Some(payload) = self.read_clipboard(cx) else {
            self.set_notice("剪贴板中没有可粘贴的文件路径", true, cx);
            return;
        };
        let destination = self.active_path(cx);
        let mode = match payload.mode {
            ClipboardMode::Copy => TransferMode::Copy,
            ClipboardMode::Cut => TransferMode::Move,
        };
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
            model.panes[index].read(cx).current_path.clone()
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

    fn read_clipboard(&self, cx: &gpui::App) -> Option<ClipboardPayload> {
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
        let (progress_sender, progress_receiver) = async_channel::bounded(64);
        let engine = self.engine.clone();
        let sources_for_task = sources.clone();
        self.transfer = Some(TransferActivity {
            running: true,
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
        });
        self.notice = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            while let Ok(progress) = progress_receiver.recv().await {
                let should_continue = this
                    .update(cx, |controller, cx| {
                        if controller.transfer_generation != generation {
                            return false;
                        }
                        controller.apply_progress(progress);
                        cx.notify();
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
                )
                .await;
            let _ = this.update(cx, |controller, cx| {
                if controller.transfer_generation != generation {
                    return;
                }
                match result {
                    Ok(destinations) => {
                        if let Some(transfer) = &mut controller.transfer {
                            transfer.running = false;
                            if transfer.total_bytes > 0 {
                                transfer.bytes_done = transfer.total_bytes;
                            }
                        }
                        if mode == TransferMode::Move
                            && controller.clipboard.as_ref().is_some_and(|clipboard| {
                                clipboard.mode == ClipboardMode::Cut && clipboard.paths == sources
                            })
                        {
                            controller.clipboard = None;
                            cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
                        }
                        controller.set_notice(
                            format!("已完成 {} 个项目", destinations.len()),
                            false,
                            cx,
                        );
                        controller.refresh_all_panes(cx);
                    }
                    Err(error) => {
                        if let Some(transfer) = &mut controller.transfer {
                            transfer.running = false;
                        }
                        controller.set_notice(error.to_string(), true, cx);
                    }
                }
            });
        })
        .detach();
    }

    fn apply_progress(&mut self, progress: TransferProgress) {
        if let Some(transfer) = &mut self.transfer {
            transfer.current_file = progress.current_file;
            transfer.bytes_done = progress.bytes_done;
            transfer.total_bytes = progress.total_bytes;
            transfer.speed_bytes_per_second = progress.speed_bytes_per_second;
        }
    }

    fn refresh_all_panes(&self, cx: &mut Context<Self>) {
        let panes = self.model.read(cx).panes.clone();
        for pane in panes {
            pane.update(cx, |pane, cx| pane.refresh(cx));
        }
    }

    fn set_notice(&mut self, notice: impl Into<String>, is_error: bool, cx: &mut Context<Self>) {
        self.notice = Some(notice.into());
        self.notice_is_error = is_error;
        cx.notify();
    }
}
