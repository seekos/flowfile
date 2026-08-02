use super::tooltip::delayed_tooltip;
use crate::{models::Model, models::Pane, theme};
use gpui::{
    AnyElement, App, Context, FocusHandle, Focusable, IntoElement, KeyDownEvent, Render,
    SharedString, Window, div, prelude::*, px,
};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy)]
enum NavigationAction {
    Back,
    Forward,
    Up,
    Refresh,
}

pub struct AddressBar {
    pane: Model<Pane>,
    editing: bool,
    edit_buffer: String,
    focus_handle: FocusHandle,
}

impl AddressBar {
    pub fn new(pane: Model<Pane>, cx: &mut Context<Self>) -> Self {
        cx.observe(&pane, |_, _, cx| cx.notify()).detach();
        Self {
            pane,
            editing: false,
            edit_buffer: String::new(),
            focus_handle: cx.focus_handle(),
        }
    }

    fn begin_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_buffer = self.pane.read(cx).current_path.display().to_string();
        self.editing = true;
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "enter" => {
                let input = self.edit_buffer.trim();
                if !input.is_empty() {
                    self.pane
                        .update(cx, |pane, cx| pane.navigate_to(PathBuf::from(input), cx));
                }
                self.editing = false;
                cx.notify();
            }
            "escape" => {
                self.editing = false;
                cx.notify();
            }
            "backspace" => {
                self.edit_buffer.pop();
                cx.notify();
            }
            _ => {
                if let Some(text) = &event.keystroke.key_char
                    && !text.chars().any(char::is_control)
                {
                    self.edit_buffer.push_str(text);
                    cx.notify();
                }
            }
        }
    }

    fn navigation_button(
        &self,
        id: &'static str,
        glyph: &'static str,
        action: NavigationAction,
        enabled: bool,
    ) -> impl IntoElement {
        let pane = self.pane.clone();
        let tooltip = match action {
            NavigationAction::Back => "返回上一个位置",
            NavigationAction::Forward => "前往下一个位置",
            NavigationAction::Up => "前往上级文件夹",
            NavigationAction::Refresh => "刷新当前文件夹 (⌘R)",
        };
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(29.0))
            .rounded_sm()
            .text_size(theme::font(15.0))
            .text_color(if enabled {
                theme::text_secondary()
            } else {
                theme::text_tertiary().opacity(0.45)
            })
            .tooltip(delayed_tooltip(tooltip))
            .when(enabled, |button| {
                button
                    .hover(|style| style.bg(theme::accent_soft()))
                    .on_click(move |_, _, cx| {
                        pane.update(cx, |pane, cx| match action {
                            NavigationAction::Back => pane.go_back(cx),
                            NavigationAction::Forward => pane.go_forward(cx),
                            NavigationAction::Up => pane.go_up(cx),
                            NavigationAction::Refresh => pane.refresh(cx),
                        });
                    })
            })
            .child(glyph)
    }

    fn breadcrumb(&self, path: &Path, cx: &mut Context<Self>) -> AnyElement {
        let pane = self.pane.clone();
        let nodes = breadcrumb_nodes(path);

        div()
            .flex()
            .items_center()
            .min_w_0()
            .flex_1()
            .h(px(30.0))
            .px_1()
            .rounded_md()
            .border_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .overflow_hidden()
            .children(
                nodes
                    .into_iter()
                    .enumerate()
                    .flat_map(|(index, (label, path))| {
                        let pane = pane.clone();
                        let tooltip = format!("前往 {}", path.display());
                        let label: SharedString = label.into();
                        let node = div()
                            .id(("breadcrumb", index))
                            .flex()
                            .items_center()
                            .h(px(26.0))
                            .px_2()
                            .rounded_sm()
                            .font_family("SF Mono")
                            .text_size(theme::font(10.0))
                            .text_color(theme::text_secondary())
                            .hover(|style| {
                                style.bg(theme::accent_soft()).text_color(theme::accent())
                            })
                            .tooltip(delayed_tooltip(tooltip))
                            .on_click(move |_, _, cx| {
                                pane.update(cx, |pane, cx| pane.navigate_to(path.clone(), cx));
                            })
                            .child(label)
                            .into_any_element();
                        let separator = (index > 0).then(|| {
                            div()
                                .text_size(theme::font(10.0))
                                .text_color(theme::text_tertiary())
                                .child("›")
                                .into_any_element()
                        });

                        separator.into_iter().chain(std::iter::once(node))
                    }),
            )
            .child(
                div()
                    .id("address-edit-trigger")
                    .h_full()
                    .min_w(px(24.0))
                    .flex_1()
                    .tooltip(delayed_tooltip("编辑当前路径"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.begin_edit(window, cx);
                    })),
            )
            .into_any_element()
    }

    fn editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let value: SharedString = self.edit_buffer.clone().into();
        div()
            .id("address-path-editor")
            .flex()
            .items_center()
            .min_w_0()
            .flex_1()
            .h(px(30.0))
            .px_2()
            .rounded_md()
            .border_1()
            .border_color(theme::accent())
            .bg(theme::surface())
            .font_family("SF Mono")
            .text_size(theme::font(11.0))
            .text_color(theme::text_primary())
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(div().min_w_0().truncate().child(value))
            .child(div().w(px(1.0)).h(px(15.0)).ml(px(1.0)).bg(theme::accent()))
            .into_any_element()
    }
}

impl Focusable for AddressBar {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AddressBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (path, can_go_back, can_go_forward, can_go_up, error) = {
            let pane = self.pane.read(cx);
            (
                pane.current_path.clone(),
                pane.can_go_back(),
                pane.can_go_forward(),
                pane.can_go_up(),
                pane.error_message.clone(),
            )
        };

        let path_control = if self.editing {
            self.editor(cx)
        } else {
            self.breadcrumb(&path, cx)
        };

        div()
            .flex()
            .flex_col()
            .min_w_0()
            .w_full()
            .flex_1()
            .border_b_1()
            .border_color(if error.is_some() {
                theme::danger()
            } else {
                theme::border()
            })
            .bg(theme::surface_subtle())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(px(44.0))
                    .px_2()
                    .child(self.navigation_button(
                        "navigate-back",
                        "‹",
                        NavigationAction::Back,
                        can_go_back,
                    ))
                    .child(self.navigation_button(
                        "navigate-forward",
                        "›",
                        NavigationAction::Forward,
                        can_go_forward,
                    ))
                    .child(self.navigation_button(
                        "navigate-up",
                        "↑",
                        NavigationAction::Up,
                        can_go_up,
                    ))
                    .child(path_control)
                    .child(self.navigation_button(
                        "refresh-directory",
                        "↻",
                        NavigationAction::Refresh,
                        true,
                    )),
            )
            .when_some(error, |bar, error| {
                bar.child(
                    div()
                        .h(px(23.0))
                        .px_3()
                        .flex()
                        .items_center()
                        .bg(theme::danger_soft())
                        .text_size(theme::font(9.0))
                        .text_color(theme::danger())
                        .child(error),
                )
            })
    }
}

fn breadcrumb_nodes(path: &Path) -> Vec<(String, PathBuf)> {
    let mut nodes = vec![("Mac".to_string(), PathBuf::from("/"))];
    let mut current = PathBuf::from("/");

    for component in path.components() {
        if let Component::Normal(name) = component {
            current.push(name);
            nodes.push((name.to_string_lossy().into_owned(), current.clone()));
        }
    }
    nodes
}
