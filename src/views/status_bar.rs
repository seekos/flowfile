use super::tooltip::delayed_tooltip;
use crate::{
    icons::{IconName, icon},
    models::{FileOperationController, Model, MultiPaneModel},
    services::{FileInspector, SystemTerminal},
    theme,
};
use gpui::{Context, Entity, IntoElement, Render, Window, div, prelude::*, px};

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
        let pane = model.panes[active_index].read(cx);
        let item_count = pane.items.len();
        let folder_count = pane.items.iter().filter(|item| item.is_dir).count();
        let path = pane.display_path();
        let terminal_path = pane.current_path.clone();
        let selection = match pane.selection_count() {
            0 => None,
            1 => pane
                .selected_index
                .and_then(|index| pane.items.get(index))
                .map(|item| format!("已选择 {}", item.name))
                .or_else(|| Some("已选择 1 个项目".to_string())),
            count => Some(format!("已选择 {count} 个项目")),
        };
        let selected_path = if pane.selection_count() == 1 {
            pane.selected_index
                .and_then(|index| pane.items.get(index))
                .map(|item| item.path.clone())
        } else {
            None
        };
        if let Some(path) = selected_path {
            self.inspector
                .update(cx, |inspector, cx| inspector.request(path, cx));
        } else {
            self.inspector
                .update(cx, |inspector, cx| inspector.clear(cx));
        }
        let inspector = self.inspector.read(cx);
        let inspector_summary = inspector.metadata.as_ref().map(|metadata| {
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
            facts.join(" · ")
        });
        let inspector_loading = inspector.is_loading;
        let operations = self.operations.read(cx);
        let notice = operations.notice.clone();
        let notice_is_error = operations.notice_is_error;
        let terminal = self.terminal.clone();

        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .border_t_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .h(px(30.0))
                    .px_3()
                    .text_size(theme::font(10.0))
                    .text_color(theme::text_secondary())
                    .child(format!("{item_count} 个项目 · {folder_count} 个文件夹"))
                    .when_some(selection, |status, selection| status.child(selection))
                    .when_some(inspector_summary, |status, summary| {
                        status.child(
                            div()
                                .flex()
                                .min_w_0()
                                .max_w(px(480.0))
                                .items_center()
                                .gap_2()
                                .child(div().min_w_0().truncate().child(summary)),
                        )
                    })
                    .when(inspector_loading, |status| {
                        status.child(
                            div()
                                .min_w_0()
                                .max_w(px(220.0))
                                .truncate()
                                .text_color(theme::text_tertiary())
                                .child("正在读取元数据…"),
                        )
                    })
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
                            .text_right()
                            .text_color(theme::text_tertiary())
                            .child(path),
                    )
                    .when(self.terminal.is_available(), |status| {
                        status.child(
                            div()
                                .id("status-open-system-terminal")
                                .flex()
                                .items_center()
                                .gap_1()
                                .h(px(23.0))
                                .px_2()
                                .rounded(px(5.0))
                                .bg(theme::hover_subtle())
                                .text_color(theme::text_secondary())
                                .cursor_pointer()
                                .hover(|style| {
                                    style.bg(theme::accent_soft()).text_color(theme::accent())
                                })
                                .tooltip(delayed_tooltip("在系统终端中打开当前文件夹 (⌘`)"))
                                .on_click(move |_, _, _| {
                                    terminal.open(terminal_path.clone());
                                })
                                .child(icon(IconName::Terminal, 12.0, theme::text_secondary()))
                                .child("终端"),
                        )
                    }),
            )
    }
}
