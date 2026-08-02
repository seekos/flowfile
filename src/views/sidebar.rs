use super::tooltip::delayed_tooltip;
use crate::{
    models::{FileDragPayload, FileOperationController, Model, MultiPaneModel, home_directory},
    services::{FileEngine, FileWatcher, TransferMode},
    theme,
};
use gpui::{
    Context, Entity, FontWeight, IntoElement, Render, SharedString, Timer, Window, div, prelude::*,
    px,
};
use std::{path::PathBuf, time::Duration};

#[derive(Clone, Eq, PartialEq)]
struct SidebarLocation {
    icon: &'static str,
    label: String,
    path: PathBuf,
}

pub struct SidebarView {
    model: Model<MultiPaneModel>,
    operations: Entity<FileOperationController>,
    quick_access: Vec<SidebarLocation>,
    volumes: Vec<SidebarLocation>,
    volumes_loading: bool,
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
            cx.spawn(async move |this, cx| {
                while volume_events.recv().await.is_ok() {
                    Timer::after(Duration::from_millis(150)).await;
                    while volume_events.try_recv().is_ok() {}

                    let result = engine.list_volumes().await;
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
            _volumes_watcher: volumes_watcher,
        }
    }

    fn apply_volumes(&mut self, paths: Option<Vec<PathBuf>>, cx: &mut Context<Self>) {
        self.volumes_loading = false;
        if let Some(paths) = paths {
            let volumes = paths
                .into_iter()
                .map(|path| SidebarLocation {
                    icon: "◉",
                    label: volume_label(&path),
                    path,
                })
                .collect::<Vec<_>>();
            if self.volumes != volumes {
                self.volumes = volumes;
                cx.notify();
            }
        }
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

    fn item(&self, id: usize, location: SidebarLocation, is_active: bool) -> impl IntoElement {
        let model = self.model.clone();
        let operations = self.operations.clone();
        let path = location.path.clone();
        let drop_path = path.clone();
        let tooltip = format!("在当前面板中打开 {}", path.display());
        let label: SharedString = location.label.into();

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
            .child(div().min_w_0().flex_1().truncate().child(label))
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
                        self.item(index, location, is_active)
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
                self.item(100 + index, location, is_active)
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
