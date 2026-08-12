use super::tooltip::delayed_tooltip;
use crate::{
    models::{FileDragPayload, FileOperationController, Model, MultiPaneModel, home_directory},
    services::{FileEngine, FileWatcher, TransferMode, VolumeInfo},
    theme,
};
use gpui::{
    Context, Entity, FontWeight, IntoElement, Render, SharedString, Timer, Window, div, prelude::*,
    px,
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Eq, PartialEq)]
struct SidebarLocation {
    icon: &'static str,
    label: String,
    path: PathBuf,
    detail: Option<String>,
}

pub struct SidebarView {
    model: Model<MultiPaneModel>,
    operations: Entity<FileOperationController>,
    quick_access: Vec<SidebarLocation>,
    volumes: Vec<SidebarLocation>,
    volumes_loading: bool,
    engine: FileEngine,
    ntfs_mounting: HashSet<PathBuf>,
    ntfs_mount_failures: HashSet<PathBuf>,
    ejecting_volumes: HashSet<PathBuf>,
    _volumes_watcher: Option<FileWatcher>,
}

impl SidebarView {
    pub fn new(
        model: Model<MultiPaneModel>,
        operations: Entity<FileOperationController>,
        engine: FileEngine,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        let panes = model.read(cx).panes.clone();
        for pane in &panes {
            cx.observe(pane, |_, _, cx| cx.notify()).detach();
        }

        let home = home_directory();
        let quick_access = [
            ("⌂", "个人文件夹", home.clone()),
            ("↓", "下载", home.join("Downloads")),
            ("▧", "桌面", home.join("Desktop")),
            ("◇", "文稿", home.join("Documents")),
        ]
        .into_iter()
        .filter(|(_, _, path)| path.is_dir())
        .map(|(icon, label, path)| SidebarLocation {
            icon,
            label: label.to_string(),
            path,
            detail: None,
        })
        .collect();

        let initial_engine = engine.clone();
        cx.spawn(async move |this, cx| {
            let result = initial_engine.list_volumes().await;
            let _ = this.update(cx, |sidebar, cx| {
                sidebar.apply_volumes(result.ok(), cx);
            });
        })
        .detach();

        let (volumes_watcher, volume_events) = FileWatcher::watch(std::path::Path::new("/Volumes"))
            .map(|(watcher, receiver)| (Some(watcher), Some(receiver)))
            .unwrap_or((None, None));
        if let Some(volume_events) = volume_events {
            let watcher_engine = engine.clone();
            cx.spawn(async move |this, cx| {
                while volume_events.recv().await.is_ok() {
                    Timer::after(Duration::from_millis(150)).await;
                    while volume_events.try_recv().is_ok() {}

                    let result = watcher_engine.list_volumes().await;
                    if this
                        .update(cx, |sidebar, cx| sidebar.apply_volumes(result.ok(), cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        }

        Self {
            model,
            operations,
            quick_access,
            volumes: Vec::new(),
            volumes_loading: true,
            engine,
            ntfs_mounting: HashSet::new(),
            ntfs_mount_failures: HashSet::new(),
            ejecting_volumes: HashSet::new(),
            _volumes_watcher: volumes_watcher,
        }
    }

    fn apply_volumes(&mut self, paths: Option<Vec<VolumeInfo>>, cx: &mut Context<Self>) {
        self.volumes_loading = false;
        if let Some(paths) = paths {
            let present_paths = paths
                .iter()
                .map(|volume| volume.path.clone())
                .collect::<HashSet<_>>();
            self.ntfs_mount_failures
                .retain(|path| present_paths.contains(path) || self.ntfs_mounting.contains(path));
            let ntfs_to_mount = paths
                .iter()
                .find(|volume| {
                    volume.path.starts_with(Path::new("/Volumes"))
                        && volume.is_ntfs()
                        && volume.read_only
                        && !self.ntfs_mounting.contains(&volume.path)
                        && !self.ntfs_mount_failures.contains(&volume.path)
                })
                .map(|volume| volume.path.clone());
            let volumes = paths
                .into_iter()
                .map(|volume| SidebarLocation {
                    icon: "◉",
                    label: volume_label(&volume.path),
                    detail: volume.status_label().map(str::to_string),
                    path: volume.path,
                })
                .collect::<Vec<_>>();
            if self.volumes != volumes {
                self.volumes = volumes;
                cx.notify();
            }
            if let Some(path) = ntfs_to_mount
                && self.engine.ntfs_auto_mount_available()
            {
                self.start_ntfs_auto_mount(path, cx);
            }
        }
    }

    fn start_ntfs_auto_mount(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.ntfs_mounting.insert(path.clone());
        self.operations.update(cx, |operations, cx| {
            operations.show_notice(
                format!("正在将 {} 挂载为可写…", volume_label(&path)),
                false,
                cx,
            );
        });

        let engine = self.engine.clone();
        let operations = self.operations.clone();
        cx.spawn(async move |this, cx| {
            let result = engine.auto_mount_ntfs(path.clone()).await;
            let refreshed_volumes = engine.list_volumes().await.ok();
            let _ = this.update(cx, |sidebar, cx| {
                sidebar.ntfs_mounting.remove(&path);
                match result {
                    Ok(true) => operations.update(cx, |operations, cx| {
                        operations.show_notice(
                            format!("{} 已自动挂载为 NTFS 可写", volume_label(&path)),
                            false,
                            cx,
                        );
                    }),
                    Ok(false) => {
                        sidebar.ntfs_mount_failures.insert(path.clone());
                    }
                    Err(error) => {
                        sidebar.ntfs_mount_failures.insert(path.clone());
                        operations.update(cx, |operations, cx| {
                            operations.show_notice(error.to_string(), true, cx);
                        });
                    }
                }
                sidebar.apply_volumes(refreshed_volumes, cx);
            });
        })
        .detach();
    }

    fn section_title(title: &'static str) -> impl IntoElement {
        div()
            .px_3()
            .pt_4()
            .pb_2()
            .text_size(theme::font(10.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::text_tertiary())
            .child(title)
    }

    fn eject_volume(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.ejecting_volumes.insert(path.clone()) {
            return;
        }
        let label = volume_label(&path);
        self.operations.update(cx, |operations, cx| {
            operations.show_notice(format!("正在弹出 {label}…"), false, cx);
        });
        cx.notify();

        let engine = self.engine.clone();
        let operations = self.operations.clone();
        cx.spawn(async move |this, cx| {
            let result = engine.eject_volume(path.clone()).await;
            let _ = this.update(cx, |sidebar, cx| {
                sidebar.ejecting_volumes.remove(&path);
                match result {
                    Ok(()) => {
                        let fallback = home_directory();
                        let panes = sidebar.model.read(cx).panes.clone();
                        for pane in panes {
                            if pane.read(cx).current_path.starts_with(&path) {
                                pane.update(cx, |pane, cx| pane.navigate_to(fallback.clone(), cx));
                            }
                        }
                        operations.update(cx, |operations, cx| {
                            operations.show_notice(format!("已弹出 {label}"), false, cx);
                        });
                        sidebar.volumes.retain(|volume| volume.path != path);
                    }
                    Err(error) => operations.update(cx, |operations, cx| {
                        operations.show_notice(error.to_string(), true, cx);
                    }),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn item(
        &self,
        id: usize,
        location: SidebarLocation,
        is_active: bool,
        can_eject: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let model = self.model.clone();
        let operations = self.operations.clone();
        let path = location.path.clone();
        let drop_path = path.clone();
        let tooltip = format!("在当前面板中打开 {}", path.display());
        let label: SharedString = location.label.into();
        let detail = location.detail.map(SharedString::from);
        let eject_path = location.path.clone();
        let is_ejecting = self.ejecting_volumes.contains(&eject_path);

        div()
            .id(("sidebar-location", id))
            .flex()
            .items_center()
            .gap_2()
            .mx_2()
            .h(px(36.0))
            .px_2()
            .rounded_md()
            .text_size(theme::font(12.0))
            .text_color(if is_active {
                theme::accent()
            } else {
                theme::text_primary()
            })
            .bg(if is_active {
                theme::accent_soft()
            } else {
                theme::sidebar()
            })
            .hover(|style| style.bg(theme::surface().opacity(0.8)))
            .tooltip(delayed_tooltip(tooltip))
            .drag_over::<FileDragPayload>(|style, _, _, _| {
                style.bg(theme::accent_soft()).text_color(theme::accent())
            })
            .on_drop(move |payload: &FileDragPayload, window, cx| {
                let mode = if window.modifiers().alt {
                    TransferMode::Copy
                } else {
                    TransferMode::Move
                };
                operations.update(cx, |operations, cx| {
                    operations.transfer_to_path(payload.paths.clone(), drop_path.clone(), mode, cx);
                });
            })
            .on_click(move |_, _, cx| {
                let pane = {
                    let model = model.read(cx);
                    model.panes[model.active_pane_index].clone()
                };
                pane.update(cx, |pane, cx| pane.navigate_to(path.clone(), cx));
            })
            .child(
                div()
                    .w(px(24.0))
                    .text_size(theme::font(14.0))
                    .text_color(if is_active {
                        theme::accent()
                    } else {
                        theme::file_blue()
                    })
                    .child(location.icon),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .child(div().truncate().child(label))
                    .when_some(detail, |location, detail| {
                        location
                            .text_size(theme::font(8.0))
                            .text_color(theme::text_tertiary())
                            .child(detail)
                    }),
            )
            .when(can_eject, |item| {
                item.child(
                    div()
                        .id(("sidebar-eject", id))
                        .w(px(24.0))
                        .h(px(24.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .cursor_pointer()
                        .text_size(theme::font(12.0))
                        .text_color(theme::text_tertiary())
                        .hover(|style| style.bg(theme::surface()).text_color(theme::text_primary()))
                        .tooltip(delayed_tooltip(if is_ejecting {
                            "正在弹出…".to_string()
                        } else {
                            format!("弹出 {}", volume_label(&eject_path))
                        }))
                        .child(if is_ejecting { "…" } else { "⏏" })
                        .on_click(cx.listener(move |sidebar, _, _, cx| {
                            cx.stop_propagation();
                            sidebar.eject_volume(eject_path.clone(), cx);
                        })),
                )
            })
    }
}

impl Render for SidebarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_path = {
            let model = self.model.read(cx);
            model.panes[model.active_pane_index]
                .read(cx)
                .current_path
                .clone()
        };
        let quick_access = self.quick_access.clone();
        let volumes = self.volumes.clone();

        div()
            .flex()
            .flex_col()
            .w(px(205.0))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(theme::border())
            .bg(theme::sidebar())
            .child(Self::section_title("快速访问"))
            .children(
                quick_access
                    .into_iter()
                    .enumerate()
                    .map(|(index, location)| {
                        let is_active = current_path == location.path;
                        self.item(index, location, is_active, false, cx)
                    }),
            )
            .child(Self::section_title("卷"))
            .when(self.volumes_loading, |sidebar| {
                sidebar.child(
                    div()
                        .mx_3()
                        .text_size(theme::font(10.0))
                        .text_color(theme::text_tertiary())
                        .child("正在读取挂载卷…"),
                )
            })
            .children(volumes.into_iter().enumerate().map(|(index, location)| {
                let is_active = current_path == location.path;
                let can_eject = location.path.starts_with(Path::new("/Volumes"));
                self.item(100 + index, location, is_active, can_eject, cx)
            }))
            .child(
                div()
                    .mt_auto()
                    .mx_3()
                    .mb_3()
                    .pt_3()
                    .border_t_1()
                    .border_color(theme::border_strong())
                    .text_size(theme::font(9.0))
                    .text_color(theme::text_tertiary())
                    .child("FlowFile · macOS local filesystem"),
            )
    }
}

fn volume_label(path: &std::path::Path) -> String {
    if path == std::path::Path::new("/") {
        return "Macintosh HD".to_string();
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Volume")
        .to_string()
}
