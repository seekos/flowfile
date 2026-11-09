use super::{
    search_bar::{
        clamp_char_range, next_char_boundary, previous_char_boundary, utf8_to_utf16_offset,
        utf16_to_utf8_offset,
    },
    tooltip::delayed_tooltip,
};
use crate::{
    actions, distribution,
    models::{AppPreferences, LayoutMode, Model, MultiPaneModel},
    services::FileEngine,
    theme,
};
use gpui::{
    AnyElement, App, Bounds, Context, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, FontWeight, GlobalElementId, IntoElement,
    KeyDownEvent, LayoutId, PaintQuad, Pixels, Render, ShapedLine, SharedString, Style, TextRun,
    UTF16Selection, UnderlineStyle, Window, black, div, fill, point, prelude::*, px, relative,
    size,
};
use std::{
    ops::Range,
    path::{Path, PathBuf},
};

const PROJECT_URL: &str = "https://github.com/seekos/flowfile";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShortcutTarget {
    Search,
    Terminal,
    QuickLook,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum PreferencesTab {
    #[default]
    General,
    DefaultApplications,
}

impl PreferencesTab {
    fn label(self) -> &'static str {
        match self {
            Self::General => "常规",
            Self::DefaultApplications => "默认程序",
        }
    }
}

#[derive(Clone, Debug, Default)]
struct DefaultApplicationDraft {
    extension: String,
    application: Option<PathBuf>,
    choosing_application: bool,
}

struct DefaultApplicationExtensionElement {
    input: Entity<PreferencesModal>,
}

struct DefaultApplicationExtensionPrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
}

impl IntoElement for DefaultApplicationExtensionElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for DefaultApplicationExtensionElement {
    type RequestLayoutState = ();
    type PrepaintState = DefaultApplicationExtensionPrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content: SharedString = input
            .default_application_extension()
            .unwrap_or_default()
            .to_string()
            .into();
        let marked_range = input.default_application_extension_marked_range.clone();
        let cursor_offset = input
            .default_application_extension_selection
            .end
            .min(content.len());
        let style = window.text_style();
        let run = TextRun {
            len: content.len(),
            font: style.font(),
            color: style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = marked_text_runs(content.len(), marked_range, run);
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(content, font_size, &runs, None);
        let cursor = fill(
            Bounds::new(
                point(
                    bounds.left() + line.x_for_index(cursor_offset),
                    bounds.top(),
                ),
                size(px(1.0), bounds.size.height),
            ),
            theme::accent(),
        );

        DefaultApplicationExtensionPrepaintState {
            line: Some(line),
            cursor: Some(cursor),
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        prepaint
            .line
            .take()
            .expect("default application extension line was shaped")
            .paint(bounds.origin, window.line_height(), window, cx)
            .expect("default application extension line should paint");
        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
    }
}

pub struct PreferencesModal {
    model: Model<MultiPaneModel>,
    engine: FileEngine,
    preferences: AppPreferences,
    version: String,
    visible: bool,
    selected_tab: PreferencesTab,
    capturing: Option<ShortcutTarget>,
    default_application_draft: Option<DefaultApplicationDraft>,
    default_application_extension_selection: Range<usize>,
    default_application_extension_marked_range: Option<Range<usize>>,
    error: Option<String>,
    focus_handle: FocusHandle,
    return_focus_handle: FocusHandle,
}

impl PreferencesModal {
    pub fn new(
        model: Model<MultiPaneModel>,
        engine: FileEngine,
        return_focus_handle: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            model,
            engine,
            preferences: AppPreferences::load(),
            version: application_version(),
            visible: false,
            selected_tab: PreferencesTab::General,
            capturing: None,
            default_application_draft: None,
            default_application_extension_selection: 0..0,
            default_application_extension_marked_range: None,
            error: None,
            focus_handle: cx.focus_handle(),
            return_focus_handle,
        }
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preferences = AppPreferences::load();
        self.visible = true;
        self.selected_tab = PreferencesTab::General;
        self.capturing = None;
        self.default_application_draft = None;
        self.default_application_extension_selection = 0..0;
        self.default_application_extension_marked_range = None;
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
                "当前设置页：{}；版本：{}；主题：{}；默认布局：{}；显示隐藏文件：{}；默认程序规则：{} 条；搜索快捷键：{}；终端快捷键：{}；Quick Look 快捷键：{}{}{}",
                self.selected_tab.label(),
                self.version,
                self.preferences.theme.label(),
                self.preferences.default_layout.label(),
                if self.preferences.show_hidden { "是" } else { "否" },
                self.preferences.default_applications.len(),
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
        self.default_application_draft = None;
        self.default_application_extension_selection = 0..0;
        self.default_application_extension_marked_range = None;
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

    fn open_project_page(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = open::that(PROJECT_URL) {
            self.error = Some(format!("无法打开项目页面：{error}"));
            cx.notify();
        }
    }

    fn start_adding_default_application(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.default_application_draft = Some(DefaultApplicationDraft::default());
        self.default_application_extension_selection = 0..0;
        self.default_application_extension_marked_range = None;
        self.error = None;
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn default_application_extension(&self) -> Option<&str> {
        self.default_application_draft
            .as_ref()
            .map(|draft| draft.extension.as_str())
    }

    fn replace_default_application_extension(&mut self, range: Range<usize>, new_text: &str) {
        let Some(draft) = self.default_application_draft.as_mut() else {
            return;
        };
        let range = clamp_char_range(&draft.extension, range);
        let mut replacement = draft.extension.clone();
        replacement.replace_range(range, new_text);
        replacement.retain(|character| !character.is_whitespace());
        let replacement = replacement.trim_start_matches('.').to_string();
        let cursor = replacement.len();
        draft.extension = replacement;
        self.default_application_extension_selection = cursor..cursor;
        self.default_application_extension_marked_range = None;
        self.error = None;
    }

    fn focus_default_application_extension(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cursor = self
            .default_application_extension()
            .map(str::len)
            .unwrap_or_default();
        self.default_application_extension_selection = cursor..cursor;
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn choose_default_application(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.default_application_draft.as_mut() else {
            return;
        };
        if draft.choosing_application {
            return;
        }
        draft.choosing_application = true;
        self.error = None;
        cx.notify();

        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let result = engine.choose_open_with_application().await;
            let _ = this.update(cx, |this, cx| {
                let Some(draft) = this.default_application_draft.as_mut() else {
                    return;
                };
                draft.choosing_application = false;
                match result {
                    Ok(Some(application)) => draft.application = Some(application),
                    Ok(None) => {}
                    Err(error) => this.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn commit_default_application(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.default_application_draft.clone() else {
            return;
        };
        let Some(application) = draft.application else {
            self.error = Some("请选择用于打开文件的应用程序".to_string());
            cx.notify();
            return;
        };
        if !application.is_dir()
            || !application
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
        {
            self.error = Some("所选应用不可用，请重新选择".to_string());
            cx.notify();
            return;
        }
        match self
            .preferences
            .set_default_application(&draft.extension, application)
        {
            Ok(()) => {
                self.default_application_draft = None;
                self.default_application_extension_selection = 0..0;
                self.default_application_extension_marked_range = None;
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
        cx.notify();
    }

    fn remove_default_application(&mut self, extension: &str, cx: &mut Context<Self>) {
        self.preferences
            .default_applications
            .retain(|preference| preference.extension != extension);
        self.error = None;
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.default_application_draft.is_some() {
            match event.keystroke.key.as_str() {
                "escape" => {
                    self.default_application_draft = None;
                    self.default_application_extension_selection = 0..0;
                    self.default_application_extension_marked_range = None;
                    self.error = None;
                    cx.notify();
                }
                "enter" => self.commit_default_application(cx),
                "backspace" => {
                    let value = self.default_application_extension().unwrap_or_default();
                    let mut range = clamp_char_range(
                        value,
                        self.default_application_extension_selection.clone(),
                    );
                    if range.is_empty() {
                        range.start = previous_char_boundary(value, range.start);
                    }
                    self.replace_default_application_extension(range, "");
                    cx.notify();
                }
                "delete" => {
                    let value = self.default_application_extension().unwrap_or_default();
                    let mut range = clamp_char_range(
                        value,
                        self.default_application_extension_selection.clone(),
                    );
                    if range.is_empty() {
                        range.end = next_char_boundary(value, range.end);
                    }
                    self.replace_default_application_extension(range, "");
                    cx.notify();
                }
                "v" if event.keystroke.modifiers.platform => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.replace_default_application_extension(
                            self.default_application_extension_selection.clone(),
                            &text,
                        );
                        cx.notify();
                    }
                }
                _ => {}
            }
            return;
        }

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
            "left" => {
                self.selected_tab = PreferencesTab::General;
                cx.notify();
            }
            "right" => {
                self.selected_tab = PreferencesTab::DefaultApplications;
                cx.notify();
            }
            _ => {}
        }
    }

    fn on_close(
        &mut self,
        _: &actions::ClosePreferences,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.default_application_draft.is_some() {
            self.default_application_draft = None;
            self.default_application_extension_selection = 0..0;
            self.default_application_extension_marked_range = None;
            self.error = None;
            cx.notify();
        } else if self.capturing.is_some() {
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

    fn tab(&self, tab: PreferencesTab, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected_tab == tab;
        div()
            .id(("preferences-tab", tab as usize))
            .flex_1()
            .h(px(30.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .font_weight(if selected {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::NORMAL
            })
            .text_size(theme::font(10.0))
            .text_color(if selected {
                theme::text_primary()
            } else {
                theme::text_secondary()
            })
            .bg(if selected {
                theme::surface()
            } else {
                theme::surface_subtle()
            })
            .hover(|style| style.bg(theme::accent_soft()))
            .tooltip(delayed_tooltip(format!("切换到{}设置", tab.label())))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected_tab = tab;
                this.error = None;
                cx.notify();
            }))
            .child(tab.label())
    }

    fn render_general_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme_label = self.preferences.theme.label();
        let layout_label = self.preferences.default_layout.label();
        let show_hidden = self.preferences.show_hidden;
        let search_shortcut = self.preferences.search_shortcut.clone();
        let terminal_shortcut = self.preferences.terminal_shortcut.clone();
        let quicklook_shortcut = self.preferences.quick_look_shortcut.clone();

        div()
            .id("preferences-general-scroll")
            .size_full()
            .min_h_0()
            .pr_2()
            .overflow_y_scroll()
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
            .when(!distribution::is_app_store(), |settings| {
                settings.child(self.shortcut_row(
                    "系统终端",
                    ShortcutTarget::Terminal,
                    terminal_shortcut,
                    cx,
                ))
            })
            .child(self.shortcut_row(
                "Quick Look",
                ShortcutTarget::QuickLook,
                quicklook_shortcut,
                cx,
            ))
            .child(
                div()
                    .mt_4()
                    .mb_1()
                    .text_size(theme::font(9.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_tertiary())
                    .child("关于"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface_subtle())
                    .child(
                        div()
                            .text_size(theme::font(10.0))
                            .text_color(theme::text_primary())
                            .child("FlowFile 是一款面向 macOS 的轻量、高效多面板文件管理器。"),
                    )
                    .child(
                        div()
                            .id("preferences-project-link")
                            .mt_2()
                            .flex()
                            .items_center()
                            .text_size(theme::font(9.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::accent())
                            .cursor_pointer()
                            .hover(|style| style.opacity(0.75))
                            .tooltip(delayed_tooltip("在浏览器中打开 FlowFile 项目页面"))
                            .on_click(cx.listener(|this, _, _, cx| this.open_project_page(cx)))
                            .child(PROJECT_URL),
                    ),
            )
            .into_any_element()
    }

    fn render_default_applications_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        if distribution::is_app_store() {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(theme::font(10.0))
                        .text_color(theme::text_secondary())
                        .child("Mac App Store 版暂不支持自定义默认程序"),
                )
                .into_any_element();
        }

        let applications = self.preferences.default_applications.clone();
        let mut list = div()
            .id("default-applications-scroll")
            .min_h_0()
            .flex_1()
            .overflow_y_scroll()
            .rounded_md()
            .border_1()
            .border_color(theme::border());

        if applications.is_empty() {
            list = list.child(
                div()
                    .h_full()
                    .min_h(px(180.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(theme::font(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::text_primary())
                            .child("还没有默认程序规则"),
                    )
                    .child(
                        div()
                            .text_size(theme::font(9.0))
                            .text_color(theme::text_tertiary())
                            .child("添加后，双击对应后缀的文件会优先使用指定应用打开"),
                    ),
            );
        } else {
            for (index, preference) in applications.iter().enumerate() {
                let extension = preference.extension.clone();
                let path = preference.application.to_string_lossy().into_owned();
                let application_name = preference.application_name();
                list = list.child(
                    div()
                        .id(SharedString::from(format!(
                            "default-application-{extension}"
                        )))
                        .flex()
                        .items_center()
                        .min_h(px(58.0))
                        .px_3()
                        .gap_3()
                        .when(index + 1 < applications.len(), |row| {
                            row.border_b_1().border_color(theme::border())
                        })
                        .child(
                            div()
                                .w(px(76.0))
                                .h(px(26.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_md()
                                .bg(theme::surface_subtle())
                                .font_family("SF Mono")
                                .text_size(theme::font(10.0))
                                .text_color(theme::text_primary())
                                .child(format!(".{extension}")),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .child(
                                    div()
                                        .text_size(theme::font(10.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme::text_primary())
                                        .child(application_name),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "default-application-path-{extension}"
                                        )))
                                        .mt_1()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .text_size(theme::font(8.0))
                                        .text_color(theme::text_tertiary())
                                        .tooltip(delayed_tooltip(path.clone()))
                                        .child(path),
                                ),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!(
                                    "remove-default-application-{extension}"
                                )))
                                .h(px(28.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded_md()
                                .text_size(theme::font(9.0))
                                .text_color(theme::danger())
                                .hover(|style| style.bg(theme::danger_soft()))
                                .tooltip(delayed_tooltip(format!("移除 .{extension} 的规则")))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove_default_application(&extension, cx)
                                }))
                                .child("移除"),
                        ),
                );
            }
        }

        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mb_3()
                    .child(
                        div()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(theme::font(11.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme::text_primary())
                                    .child("按文件后缀指定应用"),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .text_size(theme::font(9.0))
                                    .text_color(theme::text_tertiary())
                                    .child("规则只影响 FlowFile 中的文件打开行为"),
                            ),
                    )
                    .child(
                        div()
                            .id("add-default-application")
                            .h(px(30.0))
                            .px_4()
                            .flex()
                            .items_center()
                            .rounded_md()
                            .border_1()
                            .border_color(theme::border())
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(theme::font(9.0))
                            .text_color(theme::text_primary())
                            .hover(|style| style.bg(theme::accent_soft()))
                            .tooltip(delayed_tooltip("添加文件后缀和默认程序"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_adding_default_application(window, cx)
                            }))
                            .child("添加规则…"),
                    ),
            )
            .child(list)
            .into_any_element()
    }

    fn render_default_application_dialog(
        &self,
        draft: DefaultApplicationDraft,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let extension = draft.extension.clone();
        let application = draft.application.clone();
        let application_name = application
            .as_deref()
            .map(application_name)
            .unwrap_or_else(|| "尚未选择应用".to_string());
        let application_path = application
            .as_deref()
            .map(|path| path.to_string_lossy().into_owned());
        let choosing_application = draft.choosing_application;
        let error = self.error.clone();
        let input_entity = cx.entity();

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .bg(black().opacity(0.36))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(420.0))
                    .p_5()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme::border_strong())
                    .bg(theme::surface())
                    .shadow_lg()
                    .child(
                        div()
                            .text_size(theme::font(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child("添加默认程序规则"),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_size(theme::font(9.0))
                            .text_color(theme::text_tertiary())
                            .child("输入单个文件后缀，并选择用于打开该类文件的应用"),
                    )
                    .child(
                        div()
                            .mt_4()
                            .text_size(theme::font(9.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::text_secondary())
                            .child("文件后缀"),
                    )
                    .child(
                        div()
                            .id("default-application-extension")
                            .mt_1()
                            .h(px(34.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .rounded_md()
                            .border_1()
                            .border_color(theme::accent())
                            .bg(theme::surface_subtle())
                            .font_family("SF Mono")
                            .text_size(theme::font(10.0))
                            .text_color(theme::text_primary())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.focus_default_application_extension(window, cx)
                            }))
                            .child(".")
                            .child(
                                div()
                                    .relative()
                                    .ml_1()
                                    .min_w_0()
                                    .flex_1()
                                    .h(px(16.0))
                                    .when(extension.is_empty(), |input| {
                                        input.child(
                                            div()
                                                .absolute()
                                                .size_full()
                                                .text_color(theme::text_tertiary())
                                                .child("例如 txt"),
                                        )
                                    })
                                    .child(DefaultApplicationExtensionElement {
                                        input: input_entity,
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .mt_4()
                            .text_size(theme::font(9.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::text_secondary())
                            .child("应用程序"),
                    )
                    .child(
                        div()
                            .mt_1()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .h(px(48.0))
                                    .px_3()
                                    .flex()
                                    .flex_col()
                                    .justify_center()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(theme::border())
                                    .bg(theme::surface_subtle())
                                    .child(
                                        div()
                                            .text_size(theme::font(10.0))
                                            .text_color(theme::text_primary())
                                            .child(application_name),
                                    )
                                    .when_some(application_path, |card, path| {
                                        card.child(
                                            div()
                                                .id("selected-default-application-path")
                                                .mt_1()
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .text_size(theme::font(8.0))
                                                .text_color(theme::text_tertiary())
                                                .tooltip(delayed_tooltip(path.clone()))
                                                .child(path),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .id("choose-default-application")
                                    .h(px(32.0))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(theme::border())
                                    .text_size(theme::font(9.0))
                                    .text_color(theme::text_primary())
                                    .opacity(if choosing_application { 0.55 } else { 1.0 })
                                    .when(!choosing_application, |button| {
                                        button
                                            .hover(|style| style.bg(theme::accent_soft()))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.choose_default_application(cx)
                                            }))
                                    })
                                    .child(if choosing_application {
                                        "正在选择…"
                                    } else {
                                        "选择应用…"
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .h(px(28.0))
                            .pt_2()
                            .text_size(theme::font(9.0))
                            .text_color(theme::danger())
                            .when_some(error, |message, error| message.child(error)),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                div()
                                    .id("cancel-default-application")
                                    .h(px(30.0))
                                    .px_4()
                                    .flex()
                                    .items_center()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(theme::border())
                                    .hover(|style| style.bg(theme::surface_subtle()))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.default_application_draft = None;
                                        this.default_application_extension_selection = 0..0;
                                        this.default_application_extension_marked_range = None;
                                        this.error = None;
                                        cx.notify();
                                    }))
                                    .child("取消"),
                            )
                            .child(
                                div()
                                    .id("save-default-application")
                                    .h(px(30.0))
                                    .px_4()
                                    .flex()
                                    .items_center()
                                    .rounded_md()
                                    .bg(theme::accent())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::surface())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.commit_default_application(cx)
                                    }))
                                    .child("添加"),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl Focusable for PreferencesModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for PreferencesModal {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let value = self.default_application_extension()?.to_string();
        let range = clamp_char_range(
            &value,
            utf16_to_utf8_offset(&value, range_utf16.start)
                ..utf16_to_utf8_offset(&value, range_utf16.end),
        );
        actual_range.replace(
            utf8_to_utf16_offset(&value, range.start)..utf8_to_utf16_offset(&value, range.end),
        );
        Some(value[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let value = self.default_application_extension()?;
        let range = clamp_char_range(value, self.default_application_extension_selection.clone());
        Some(UTF16Selection {
            range: utf8_to_utf16_offset(value, range.start)..utf8_to_utf16_offset(value, range.end),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let value = self.default_application_extension()?;
        self.default_application_extension_marked_range
            .as_ref()
            .map(|range| {
                utf8_to_utf16_offset(value, range.start)..utf8_to_utf16_offset(value, range.end)
            })
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.default_application_extension_marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(value) = self.default_application_extension().map(str::to_string) else {
            return;
        };
        let range = range_utf16
            .map(|range| {
                utf16_to_utf8_offset(&value, range.start)..utf16_to_utf8_offset(&value, range.end)
            })
            .or_else(|| self.default_application_extension_marked_range.clone())
            .unwrap_or_else(|| self.default_application_extension_selection.clone());
        self.replace_default_application_extension(range, new_text);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(value) = self.default_application_extension().map(str::to_string) else {
            return;
        };
        let range = range_utf16
            .map(|range| {
                utf16_to_utf8_offset(&value, range.start)..utf16_to_utf8_offset(&value, range.end)
            })
            .or_else(|| self.default_application_extension_marked_range.clone())
            .unwrap_or_else(|| self.default_application_extension_selection.clone());
        self.replace_default_application_extension(range, new_text);
        let inserted_end = self.default_application_extension_selection.end;
        let inserted_len = new_text
            .chars()
            .filter(|character| !character.is_whitespace())
            .map(char::len_utf8)
            .sum::<usize>()
            .min(inserted_end);
        self.default_application_extension_marked_range =
            (inserted_len > 0).then_some(inserted_end - inserted_len..inserted_end);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.default_application_extension()?;
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let value = self.default_application_extension()?;
        Some(utf8_to_utf16_offset(
            value,
            self.default_application_extension_selection.end,
        ))
    }
}

impl Render for PreferencesModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }

        let tab_content = match self.selected_tab {
            PreferencesTab::General => self.render_general_tab(cx),
            PreferencesTab::DefaultApplications => self.render_default_applications_tab(cx),
        };
        let default_application_dialog = self
            .default_application_draft
            .clone()
            .map(|draft| self.render_default_application_dialog(draft, cx));
        let error = self.error.clone();
        let version = self.version.clone();

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
                    .w(px(560.0))
                    .h(px(520.0))
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
                            .text_size(theme::font(9.0))
                            .text_color(theme::text_tertiary())
                            .child("常规设置与按文件后缀指定默认程序"),
                    )
                    .child(
                        div()
                            .mt_3()
                            .p(px(3.0))
                            .flex()
                            .gap_1()
                            .rounded_lg()
                            .border_1()
                            .border_color(theme::border())
                            .bg(theme::surface_subtle())
                            .child(self.tab(PreferencesTab::General, cx))
                            .child(self.tab(PreferencesTab::DefaultApplications, cx)),
                    )
                    .child(div().mt_3().min_h_0().flex_1().child(tab_content))
                    .child(
                        div()
                            .h(px(22.0))
                            .pt_2()
                            .text_size(theme::font(9.0))
                            .text_color(theme::danger())
                            .when_some(error, |message, error| message.child(error)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .mt_2()
                            .child(
                                div()
                                    .text_size(theme::font(9.0))
                                    .text_color(theme::text_tertiary())
                                    .child(format!("FlowFile v{version}")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
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
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.cancel(window, cx)
                                            }))
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
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.save(window, cx)
                                            }))
                                            .child("保存设置"),
                                    ),
                            ),
                    ),
            )
            .when_some(default_application_dialog, |overlay, dialog| {
                overlay.child(dialog)
            })
            .into_any_element()
    }
}
pub(super) fn application_version() -> String {
    let bundled_version = std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(std::path::Path::to_path_buf))
        .filter(|directory| directory.file_name().and_then(|name| name.to_str()) == Some("MacOS"))
        .and_then(|macos_directory| macos_directory.parent().map(std::path::Path::to_path_buf))
        .filter(|directory| {
            directory.file_name().and_then(|name| name.to_str()) == Some("Contents")
        })
        .map(|contents_directory| contents_directory.join("Info.plist"))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|plist| short_version_from_plist(&plist));

    bundled_version.unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

fn short_version_from_plist(plist: &str) -> Option<String> {
    let (_, remainder) = plist.split_once("<key>CFBundleShortVersionString</key>")?;
    let (_, remainder) = remainder.split_once("<string>")?;
    let (version, _) = remainder.split_once("</string>")?;
    let version = version.trim();
    (!version.is_empty()).then(|| version.to_string())
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

fn marked_text_runs(
    content_len: usize,
    marked_range: Option<Range<usize>>,
    run: TextRun,
) -> Vec<TextRun> {
    let Some(marked_range) = marked_range.map(|range| {
        range.start.min(content_len)..range.end.min(content_len).max(range.start.min(content_len))
    }) else {
        return vec![run];
    };

    vec![
        TextRun {
            len: marked_range.start,
            ..run.clone()
        },
        TextRun {
            len: marked_range.end.saturating_sub(marked_range.start),
            underline: Some(UnderlineStyle {
                color: Some(run.color),
                thickness: px(1.0),
                wavy: false,
            }),
            ..run.clone()
        },
        TextRun {
            len: content_len.saturating_sub(marked_range.end),
            ..run
        },
    ]
    .into_iter()
    .filter(|run| run.len > 0)
    .collect()
}

fn application_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("未知应用")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::short_version_from_plist;

    #[test]
    fn reads_short_version_from_bundle_plist() {
        let plist = r#"
            <key>CFBundleName</key><string>FlowFile</string>
            <key>CFBundleShortVersionString</key>
            <string>1.4.2</string>
        "#;

        assert_eq!(short_version_from_plist(plist).as_deref(), Some("1.4.2"));
    }
}
