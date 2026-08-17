use super::tooltip::delayed_tooltip;
use crate::{
    actions,
    models::{AppPreferences, LayoutMode, Model, MultiPaneModel},
    theme,
};
use gpui::{
    App, Context, FocusHandle, Focusable, FontWeight, IntoElement, KeyDownEvent, Render, Window,
    black, div, prelude::*, px,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShortcutTarget {
    Search,
    Terminal,
    QuickLook,
}

pub struct PreferencesModal {
    model: Model<MultiPaneModel>,
    preferences: AppPreferences,
    visible: bool,
    capturing: Option<ShortcutTarget>,
    error: Option<String>,
    focus_handle: FocusHandle,
    return_focus_handle: FocusHandle,
}

impl PreferencesModal {
    pub fn new(
        model: Model<MultiPaneModel>,
        return_focus_handle: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            model,
            preferences: AppPreferences::load(),
            visible: false,
            capturing: None,
            error: None,
            focus_handle: cx.focus_handle(),
            return_focus_handle,
        }
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preferences = AppPreferences::load();
        self.visible = true;
        self.capturing = None;
        self.error = None;
        self.focus_handle.focus(window);
        cx.notify();
    }

    #[cfg(target_os = "macos")]
    pub fn accessibility_summary(&self) -> Option<String> {
        self.visible.then(|| {
            let capture = self
                .capturing
                .map(|_| "，正在录入快捷键")
                .unwrap_or_default();
            let error = self
                .error
                .as_deref()
                .map(|error| format!("，错误：{error}"))
                .unwrap_or_default();
            format!(
                "主题：{}；默认布局：{}；显示隐藏文件：{}；搜索快捷键：{}；终端快捷键：{}；Quick Look 快捷键：{}{}{}",
                self.preferences.theme.label(),
                self.preferences.default_layout.label(),
                if self.preferences.show_hidden { "是" } else { "否" },
                display_keystroke(&self.preferences.search_shortcut),
                display_keystroke(&self.preferences.terminal_shortcut),
                display_keystroke(&self.preferences.quick_look_shortcut),
                capture,
                error,
            )
        })
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = false;
        self.capturing = None;
        self.error = None;
        self.return_focus_handle.focus(window);
        cx.notify();
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preferences = AppPreferences::load();
        theme::apply(self.preferences.theme, window.appearance());
        self.close(window, cx);
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shortcuts = [
            self.preferences.search_shortcut.as_str(),
            self.preferences.terminal_shortcut.as_str(),
            self.preferences.quick_look_shortcut.as_str(),
        ];
        if shortcuts.iter().any(|shortcut| shortcut.trim().is_empty()) {
            self.error = Some("快捷键不能为空".to_string());
            cx.notify();
            return;
        }
        if shortcuts[0] == shortcuts[1]
            || shortcuts[0] == shortcuts[2]
            || shortcuts[1] == shortcuts[2]
        {
            self.error = Some("核心快捷键不能重复".to_string());
            cx.notify();
            return;
        }
        if let Err(error) = self.preferences.save() {
            self.error = Some(error.to_string());
            cx.notify();
            return;
        }

        theme::apply(self.preferences.theme, window.appearance());
        self.model.update(cx, |model, cx| {
            model.set_layout(self.preferences.default_layout);
            for pane in &model.panes {
                let show_hidden = self.preferences.show_hidden;
                pane.update(cx, |pane, cx| {
                    if pane.show_hidden != show_hidden {
                        pane.show_hidden = show_hidden;
                        pane.refresh(cx);
                    }
                });
            }
            cx.notify();
        });
        actions::register_keybindings_with_preferences(cx, &self.preferences);
        self.close(window, cx);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(target) = self.capturing {
            if event.keystroke.key == "escape" {
                self.capturing = None;
                cx.notify();
                return;
            }
            if is_modifier_key(&event.keystroke.key) {
                return;
            }
            let binding = format_keystroke(event);
            match target {
                ShortcutTarget::Search => self.preferences.search_shortcut = binding,
                ShortcutTarget::Terminal => self.preferences.terminal_shortcut = binding,
                ShortcutTarget::QuickLook => self.preferences.quick_look_shortcut = binding,
            }
            self.capturing = None;
            self.error = None;
            cx.notify();
            return;
        }

        match event.keystroke.key.as_str() {
            "escape" => self.cancel(window, cx),
            "enter" => self.save(window, cx),
            _ => {}
        }
    }

    fn on_close(
        &mut self,
        _: &actions::ClosePreferences,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.capturing.is_some() {
            self.capturing = None;
            cx.notify();
        } else {
            self.cancel(window, cx);
        }
    }

    fn setting_row(
        label: &'static str,
        detail: &'static str,
        control: impl IntoElement,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .min_h(px(48.0))
            .py_2()
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .text_size(theme::font(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::text_primary())
                            .child(label),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_size(theme::font(9.0))
                            .text_color(theme::text_tertiary())
                            .child(detail),
                    ),
            )
            .child(control)
    }

    fn shortcut_row(
        &self,
        label: &'static str,
        target: ShortcutTarget,
        value: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let capturing = self.capturing == Some(target);
        Self::setting_row(
            label,
            "点击后按下新的组合键",
            div()
                .id(("shortcut-capture", target as usize))
                .flex()
                .items_center()
                .justify_center()
                .min_w(px(112.0))
                .h(px(28.0))
                .px_3()
                .rounded_md()
                .border_1()
                .border_color(if capturing {
                    theme::accent()
                } else {
                    theme::border()
                })
                .bg(if capturing {
                    theme::accent_soft()
                } else {
                    theme::surface_subtle()
                })
                .font_family("SF Mono")
                .text_size(theme::font(10.0))
                .text_color(if capturing {
                    theme::accent()
                } else {
                    theme::text_secondary()
                })
                .hover(|style| style.bg(theme::accent_soft()))
                .tooltip(delayed_tooltip(format!("重新绑定“{label}”快捷键")))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.capturing = Some(target);
                    this.error = None;
                    this.focus_handle.focus(window);
                    cx.notify();
                }))
                .child(if capturing {
                    "请按快捷键…".to_string()
                } else {
                    display_keystroke(&value)
                }),
        )
    }
}

impl Focusable for PreferencesModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PreferencesModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }
        let theme_label = self.preferences.theme.label();
        let layout_label = self.preferences.default_layout.label();
        let show_hidden = self.preferences.show_hidden;
        let error = self.error.clone();
        let search_shortcut = self.preferences.search_shortcut.clone();
        let terminal_shortcut = self.preferences.terminal_shortcut.clone();
        let quicklook_shortcut = self.preferences.quick_look_shortcut.clone();

        div()
            .id("preferences-modal")
            .key_context("Preferences")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .bg(black().opacity(0.48))
            .flex()
            .items_center()
            .justify_center()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_close))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(540.0))
                    .max_h(px(680.0))
                    .p_5()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme::border_strong())
                    .bg(theme::surface())
                    .shadow_lg()
                    .child(
                        div()
                            .text_size(theme::font(16.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child("FlowFile 设置"),
                    )
                    .child(
                        div()
                            .mt_1()
                            .mb_3()
                            .text_size(theme::font(9.0))
                            .text_color(theme::text_tertiary())
                            .child("外观、启动布局与核心快捷键"),
                    )
                    .child(Self::setting_row(
                        "外观主题",
                        "自动模式跟随 macOS 外观",
                        div()
                            .id("preference-theme")
                            .h(px(28.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .rounded_md()
                            .border_1()
                            .border_color(theme::border())
                            .hover(|style| style.bg(theme::accent_soft()))
                            .tooltip(delayed_tooltip("切换外观主题"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.preferences.theme = this.preferences.theme.next();
                                theme::apply(this.preferences.theme, window.appearance());
                                cx.notify();
                            }))
                            .child(theme_label),
                    ))
                    .child(Self::setting_row(
                        "默认布局",
                        "同时应用到当前窗口",
                        div()
                            .id("preference-layout")
                            .h(px(28.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .rounded_md()
                            .border_1()
                            .border_color(theme::border())
                            .hover(|style| style.bg(theme::accent_soft()))
                            .tooltip(delayed_tooltip("切换启动时默认分屏布局"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.preferences.default_layout =
                                    next_layout(this.preferences.default_layout);
                                cx.notify();
                            }))
                            .child(layout_label),
                    ))
                    .child(Self::setting_row(
                        "默认显示隐藏文件",
                        "显示名称以 . 开头的项目",
                        div()
                            .id("preference-hidden")
                            .w(px(44.0))
                            .h(px(24.0))
                            .p(px(3.0))
                            .rounded_full()
                            .bg(if show_hidden {
                                theme::accent()
                            } else {
                                theme::border_strong()
                            })
                            .tooltip(delayed_tooltip("切换默认隐藏文件显示状态"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.preferences.show_hidden = !this.preferences.show_hidden;
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .size(px(18.0))
                                    .rounded_full()
                                    .bg(theme::surface())
                                    .when(show_hidden, |thumb| thumb.ml(px(20.0))),
                            ),
                    ))
                    .child(
                        div()
                            .mt_4()
                            .mb_1()
                            .text_size(theme::font(9.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::text_tertiary())
                            .child("核心快捷键"),
                    )
                    .child(self.shortcut_row("搜索", ShortcutTarget::Search, search_shortcut, cx))
                    .child(self.shortcut_row(
                        "系统终端",
                        ShortcutTarget::Terminal,
                        terminal_shortcut,
                        cx,
                    ))
                    .child(self.shortcut_row(
                        "Quick Look",
                        ShortcutTarget::QuickLook,
                        quicklook_shortcut,
                        cx,
                    ))
                    .when_some(error, |card, error| {
                        card.child(
                            div()
                                .mt_3()
                                .text_size(theme::font(9.0))
                                .text_color(theme::danger())
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .mt_4()
                            .child(
                                div()
                                    .id("preferences-cancel")
                                    .h(px(30.0))
                                    .px_4()
                                    .flex()
                                    .items_center()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(theme::border())
                                    .hover(|style| style.bg(theme::surface_subtle()))
                                    .tooltip(delayed_tooltip("放弃更改并关闭 (Esc)"))
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.cancel(window, cx)),
                                    )
                                    .child("取消"),
                            )
                            .child(
                                div()
                                    .id("preferences-save")
                                    .h(px(30.0))
                                    .px_4()
                                    .flex()
                                    .items_center()
                                    .rounded_md()
                                    .bg(theme::accent())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::surface())
                                    .tooltip(delayed_tooltip("保存设置"))
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.save(window, cx)),
                                    )
                                    .child("保存设置"),
                            ),
                    ),
            )
            .into_any_element()
    }
}

fn next_layout(layout: LayoutMode) -> LayoutMode {
    match layout {
        LayoutMode::Single => LayoutMode::DualVertical,
        LayoutMode::DualVertical => LayoutMode::DualHorizontal,
        LayoutMode::DualHorizontal => LayoutMode::Quad,
        LayoutMode::Quad => LayoutMode::Single,
    }
}

fn is_modifier_key(key: &str) -> bool {
    matches!(key, "shift" | "control" | "alt" | "command" | "function")
}

fn format_keystroke(event: &KeyDownEvent) -> String {
    let modifiers = event.keystroke.modifiers;
    let mut parts = Vec::new();
    if modifiers.control {
        parts.push("ctrl");
    }
    if modifiers.alt {
        parts.push("alt");
    }
    if modifiers.shift {
        parts.push("shift");
    }
    if modifiers.platform {
        parts.push("cmd");
    }
    if modifiers.function {
        parts.push("fn");
    }
    parts.push(event.keystroke.key.as_str());
    parts.join("-")
}

fn display_keystroke(value: &str) -> String {
    value
        .replace("cmd-", "⌘")
        .replace("shift-", "⇧")
        .replace("alt-", "⌥")
        .replace("ctrl-", "⌃")
}
