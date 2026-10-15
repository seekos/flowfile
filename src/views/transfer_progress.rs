use crate::{
    models::{AppPreferences, FileOperationController, FileOperationEvent},
    services::TransferMode,
    theme,
};
use gpui::{
    App, Bounds, Context, Entity, FontWeight, IntoElement, Render, TitlebarOptions, Window,
    WindowBounds, WindowKind, WindowOptions, div, prelude::*, px, relative, size,
};

const WINDOW_WIDTH: f32 = 560.0;
const WINDOW_HEIGHT: f32 = 286.0;

pub(super) struct TransferProgressWindow {
    operations: Entity<FileOperationController>,
}

impl TransferProgressWindow {
    pub(super) fn window_options(cx: &App) -> WindowOptions {
        let bounds = Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);
        WindowOptions {
            focus: true,
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT))),
            app_id: Some("cc.bso.flowfile".to_string()),
            titlebar: Some(TitlebarOptions {
                title: Some("文件传输".into()),
                appears_transparent: false,
                traffic_light_position: None,
            }),
            kind: WindowKind::Floating,
            is_movable: true,
            is_resizable: false,
            is_minimizable: true,
            ..Default::default()
        }
    }

    pub(super) fn new(
        operations: Entity<FileOperationController>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        theme::apply(AppPreferences::load().theme, window.appearance());
        cx.observe_window_appearance(window, |_, window, cx| {
            theme::apply(AppPreferences::load().theme, window.appearance());
            cx.notify();
        })
        .detach();

        cx.subscribe_in(
            &operations,
            window,
            |_, operations, event: &FileOperationEvent, window, cx| {
                if *event != FileOperationEvent::TransferChanged {
                    return;
                }
                let finished = operations
                    .read(cx)
                    .transfer
                    .as_ref()
                    .is_none_or(|transfer| !transfer.running);
                if finished {
                    window.remove_window();
                } else {
                    cx.notify();
                }
            },
        )
        .detach();

        let close_operations = operations.clone();
        window.on_window_should_close(cx, move |_, cx| {
            close_operations.update(cx, |operations, cx| {
                operations.cancel_transfer(cx);
            });
            true
        });

        Self { operations }
    }
}

impl Render for TransferProgressWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let transfer = self.operations.read(cx).transfer.clone();
        let operations_for_pause = self.operations.clone();
        let operations_for_stop = self.operations.clone();

        let Some(transfer) = transfer else {
            return div().size_full().bg(theme::canvas());
        };

        let progress = transfer.progress();
        let percent = progress * 100.0;
        let action = match transfer.mode {
            TransferMode::Copy => "复制",
            TransferMode::Move => "移动",
        };
        let state_label = if transfer.cancelling {
            "正在停止…".to_string()
        } else if transfer.paused {
            format!("已暂停{} {} 个项目", action, transfer.item_count)
        } else {
            format!("正在{} {} 个项目", action, transfer.item_count)
        };
        let pause_label = if transfer.paused { "继续" } else { "暂停" };
        let source = location_label(&transfer.source_location);
        let destination = location_label(&transfer.destination);

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .text_color(theme::text_primary())
            .p_5()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(theme::font(16.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(state_label),
                            )
                            .child(
                                div()
                                    .text_size(theme::font(10.0))
                                    .text_color(theme::text_secondary())
                                    .child(format!("从 {source} 到 {destination}")),
                            ),
                    )
                    .child(
                        div()
                            .text_size(theme::font(22.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::accent())
                            .child(format!("{percent:.0}%")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .h(px(12.0))
                            .w_full()
                            .rounded_md()
                            .overflow_hidden()
                            .bg(theme::border())
                            .child(div().h_full().w(relative(progress)).rounded_md().bg(
                                if transfer.paused || transfer.cancelling {
                                    theme::text_tertiary()
                                } else {
                                    theme::accent()
                                },
                            )),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(theme::font(10.0))
                            .text_color(theme::text_secondary())
                            .child(transfer.current_file.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_5()
                    .text_size(theme::font(10.0))
                    .text_color(theme::text_secondary())
                    .child(format!(
                        "已完成 {} / {}",
                        format_bytes(transfer.bytes_done),
                        format_bytes(transfer.total_bytes)
                    ))
                    .child(if transfer.paused {
                        "速度：已暂停".to_string()
                    } else if transfer.cancelling {
                        "正在清理临时文件".to_string()
                    } else {
                        format!(
                            "速度：{}/s",
                            format_bytes_f64(transfer.speed_bytes_per_second)
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .mt_auto()
                    .child(
                        div()
                            .id("transfer-pause-resume")
                            .h(px(32.0))
                            .min_w(px(82.0))
                            .px_4()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .border_1()
                            .border_color(theme::border_strong())
                            .bg(theme::surface())
                            .text_size(theme::font(10.0))
                            .when(!transfer.cancelling, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::accent_soft()))
                                    .on_click(move |_, _, cx| {
                                        operations_for_pause.update(cx, |operations, cx| {
                                            if transfer.paused {
                                                operations.resume_transfer(cx);
                                            } else {
                                                operations.pause_transfer(cx);
                                            }
                                        });
                                    })
                            })
                            .when(transfer.cancelling, |button| {
                                button.text_color(theme::text_tertiary())
                            })
                            .child(pause_label),
                    )
                    .child(
                        div()
                            .id("transfer-stop")
                            .h(px(32.0))
                            .min_w(px(82.0))
                            .px_4()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .border_1()
                            .border_color(if transfer.cancelling {
                                theme::border()
                            } else {
                                theme::danger()
                            })
                            .bg(theme::surface())
                            .text_size(theme::font(10.0))
                            .text_color(if transfer.cancelling {
                                theme::text_tertiary()
                            } else {
                                theme::danger()
                            })
                            .when(!transfer.cancelling, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::danger_soft()))
                                    .on_click(move |_, _, cx| {
                                        operations_for_stop.update(cx, |operations, cx| {
                                            operations.cancel_transfer(cx);
                                        });
                                    })
                            })
                            .child(if transfer.cancelling {
                                "停止中"
                            } else {
                                "停止"
                            }),
                    ),
            )
    }
}

fn location_label(path: &std::path::Path) -> String {
    path.file_name()
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn format_bytes(bytes: u64) -> String {
    format_bytes_f64(bytes as f64)
}

fn format_bytes_f64(bytes: f64) -> String {
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

#[cfg(test)]
mod tests {
    use super::{format_bytes, location_label};
    use std::path::Path;

    #[test]
    fn formats_transfer_sizes_for_the_progress_window() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
    }

    #[test]
    fn shows_the_last_path_component_as_a_location() {
        assert_eq!(
            location_label(Path::new("/Users/example/Downloads")),
            "Downloads"
        );
    }
}
