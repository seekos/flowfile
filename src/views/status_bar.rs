use super::tooltip::delayed_tooltip;
use crate::{
    models::{FileOperationController, Model, MultiPaneModel},
    services::{FileInspector, SystemTerminal, TransferMode},
    theme,
};
use gpui::{Context, Entity, IntoElement, Render, Window, div, prelude::*, px, relative};

pub struct StatusBar {
    model: Model<MultiPaneModel>,
    operations: Entity<FileOperationController>,
    inspector: Entity<FileInspector>,
    terminal: SystemTerminal,
}

impl StatusBar {
    pub fn new(
        model: Model<MultiPaneModel>,
        operations: Entity<FileOperationController>,
        inspector: Entity<FileInspector>,
        terminal: SystemTerminal,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        cx.observe(&operations, |_, _, cx| cx.notify()).detach();
        cx.observe(&inspector, |_, _, cx| cx.notify()).detach();
        let panes = model.read(cx).panes.clone();
        for pane in &panes {
            cx.observe(pane, |_, _, cx| cx.notify()).detach();
        }
        Self {
            model,
            operations,
            inspector,
            terminal,
        }
    }
}

impl Render for StatusBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let active_index = model.active_pane_index;
        let layout_label = model.layout_mode.label();
        let pane = model.panes[active_index].read(cx);
        let item_count = pane.items.len();
        let folder_count = pane.items.iter().filter(|item| item.is_dir).count();
        let path = pane.display_path();
        let terminal_path = pane.current_path.clone();
        let selection = match pane.selection_count() {
            0 => "未选择项目".to_string(),
            1 => pane
                .selected_index
                .and_then(|index| pane.items.get(index))
                .map(|item| format!("已选择：{}", item.name))
                .unwrap_or_else(|| "已选择 1 个项目".to_string()),
            count => format!("已选择 {count} 个项目"),
        };
        let selected_path = if pane.selection_count() == 1 {
            pane.selected_index
                .and_then(|index| pane.items.get(index))
                .map(|item| item.path.clone())
        } else {
            None
        };
        let activity_label = if pane.is_loading {
            "正在读取"
        } else if pane.error_message.is_some() {
            "读取出错"
        } else if pane.is_smb_server_root() {
            "SMB 共享列表"
        } else {
            "实时监听"
        };
        if let Some(path) = selected_path {
            self.inspector
                .update(cx, |inspector, cx| inspector.request(path, cx));
        } else {
            self.inspector
                .update(cx, |inspector, cx| inspector.clear(cx));
        }
        let inspector = self.inspector.read(cx);
        let inspector_metadata = inspector.metadata.clone();
        let inspector_loading = inspector.is_loading;
        let operations = self.operations.read(cx);
        let transfer = operations.transfer.clone();
        let notice = operations.notice.clone();
        let notice_is_error = operations.notice_is_error;
        let terminal = self.terminal.clone();

        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .border_t_1()
            .border_color(theme::border())
            .bg(theme::surface_subtle())
            .when_some(transfer, |bar, transfer| {
                let progress = transfer.progress();
                let mode = match transfer.mode {
                    TransferMode::Copy => "复制",
                    TransferMode::Move => "移动",
                };
                bar.child(
                    div().h(px(2.0)).w_full().bg(theme::border()).child(
                        div()
                            .h_full()
                            .w(relative(progress))
                            .bg(if transfer.running {
                                theme::accent()
                            } else {
                                theme::file_green()
                            }),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .h(px(20.0))
                        .px_3()
                        .text_size(theme::font(9.0))
                        .text_color(theme::text_secondary())
                        .child(if transfer.running { "●" } else { "✓" })
                        .child(format!(
                            "{} · {} · {:.0}% · {}/s",
                            mode,
                            transfer.current_file,
                            progress * 100.0,
                            format_size(transfer.speed_bytes_per_second)
                        )),
                )
            })
            .when_some(inspector_metadata, |bar, metadata| {
                let mut facts = vec![format!("权限 {}", metadata.permissions)];
                if let Some((width, height)) = metadata.dimensions {
                    facts.push(format!("图像 {width} × {height}"));
                }
                if let (Some(lines), Some(words)) = (metadata.line_count, metadata.word_count) {
                    facts.push(format!("{lines} 行 · {words} 词"));
                }
                for (label, value) in metadata.exif.iter().take(3) {
                    facts.push(format!("{label} {value}"));
                }
                bar.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .h(px(24.0))
                        .px_3()
                        .border_b_1()
                        .border_color(theme::border())
                        .text_size(theme::font(9.0))
                        .text_color(theme::text_secondary())
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme::accent())
                                .child("检查器"),
                        )
                        .children(
                            facts
                                .into_iter()
                                .map(|fact| div().max_w(px(210.0)).truncate().child(fact)),
                        ),
                )
            })
            .when(inspector_loading, |bar| {
                bar.child(
                    div()
                        .h(px(20.0))
                        .px_3()
                        .text_size(theme::font(9.0))
                        .text_color(theme::text_tertiary())
                        .child("正在读取文件元数据…"),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .h(px(26.0))
                    .px_3()
                    .text_size(theme::font(10.0))
                    .text_color(theme::text_secondary())
                    .child(
                        div()
                            .text_color(theme::accent())
                            .child(format!("面板 {}", active_index + 1)),
                    )
                    .child(format!("{} 个项目 · {} 个文件夹", item_count, folder_count))
                    .child(selection)
                    .when_some(notice, |status, notice| {
                        status.child(
                            div()
                                .max_w(px(240.0))
                                .truncate()
                                .text_color(if notice_is_error {
                                    theme::danger()
                                } else {
                                    theme::file_green()
                                })
                                .child(notice),
                        )
                    })
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_color(theme::text_tertiary())
                            .child(path),
                    )
                    .child(activity_label)
                    .child(layout_label)
                    .child(
                        div()
                            .id("status-open-system-terminal")
                            .flex()
                            .items_center()
                            .h(px(20.0))
                            .px_2()
                            .rounded_sm()
                            .bg(theme::surface())
                            .text_color(theme::text_secondary())
                            .hover(|style| style.bg(theme::accent_soft()))
                            .tooltip(delayed_tooltip("在系统终端中打开当前文件夹 (⌘`)"))
                            .on_click(move |_, _, _| {
                                terminal.open(terminal_path.clone());
                            })
                            .child(">_ 系统终端"),
                    ),
            )
    }
}

fn format_size(bytes: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes / KB)
    } else {
        format!("{bytes:.0} B")
    }
}
