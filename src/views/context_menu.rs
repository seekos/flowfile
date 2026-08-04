use crate::{
    actions::{NewFolder, NewTextFile, ToggleQuickLook},
    models::{FileOperationController, Model, MultiPaneModel, Pane},
    services::{FileEngine, OpenWithApplication, SystemTerminal, TransferMode},
    theme,
};
use gpui::{
    AnyElement, Context, Edges, Entity, FontWeight, IntoElement, MouseButton, Pixels, Point,
    Render, Size, Window, anchored, deferred, div, point, prelude::*, px,
};

const CONTEXT_MENU_WIDTH: Pixels = px(268.0);
const OPEN_WITH_SUBMENU_WIDTH: Pixels = px(254.0);
const SUBMENU_GAP: Pixels = px(6.0);
const WINDOW_MARGIN: Pixels = px(8.0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContextMenuTarget {
    Selection,
    Background,
}

#[derive(Clone)]
struct ContextMenuState {
    pane: Model<Pane>,
    target: ContextMenuTarget,
    position: Point<Pixels>,
}

#[derive(Clone)]
struct OpenWithMenuState {
    path: std::path::PathBuf,
    applications: Vec<OpenWithApplication>,
    text_opening_supported: bool,
    loading: bool,
    expanded: bool,
    error: Option<String>,
    generation: usize,
}

#[derive(Clone, Copy)]
enum MenuCommand {
    Open,
    QuickLook,
    Cut,
    Copy,
    Paste,
    CopyToOther,
    MoveToOther,
    Rename,
    Trash,
    GetInfo,
    NewFolder,
    NewTextFile,
    OpenTerminal,
}

pub struct ContextMenuView {
    model: Model<MultiPaneModel>,
    operations: Entity<FileOperationController>,
    terminal: SystemTerminal,
    engine: FileEngine,
    state: Option<ContextMenuState>,
    open_with: Option<OpenWithMenuState>,
    open_with_generation: usize,
}

impl ContextMenuView {
    pub fn new(
        model: Model<MultiPaneModel>,
        operations: Entity<FileOperationController>,
        terminal: SystemTerminal,
        engine: FileEngine,
    ) -> Self {
        Self {
            model,
            operations,
            terminal,
            engine,
            state: None,
            open_with: None,
            open_with_generation: 0,
        }
    }

    pub fn show_for_item(
        &mut self,
        pane_index: usize,
        item_index: usize,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.model.update(cx, |model, cx| {
            model.set_active_pane(pane_index);
            cx.notify();
        });
        let Some(pane) = self.model.read(cx).panes.get(pane_index).cloned() else {
            return;
        };
        pane.update(cx, |pane, cx| {
            if pane.rename_index.is_some() {
                pane.commit_rename(cx);
            }
            // Finder-style behavior: preserve a multi-selection when the pointer
            // is already over one of its members; otherwise select only this item.
            pane.select_for_context_menu(item_index);
            cx.notify();
        });
        self.state = Some(ContextMenuState {
            pane,
            target: ContextMenuTarget::Selection,
            position,
        });
        let open_with_path = self.state.as_ref().and_then(|state| {
            let pane = state.pane.read(cx);
            (pane.selection_count() == 1)
                .then(|| pane.selected_index.and_then(|index| pane.items.get(index)))
                .flatten()
                .filter(|item| !item.is_dir)
                .map(|item| item.path.clone())
        });
        if let Some(path) = open_with_path {
            self.load_open_with_applications(path, cx);
        } else {
            self.open_with = None;
        }
        cx.notify();
    }

    pub fn show_for_background(
        &mut self,
        pane_index: usize,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.model.update(cx, |model, cx| {
            model.set_active_pane(pane_index);
            cx.notify();
        });
        let Some(pane) = self.model.read(cx).panes.get(pane_index).cloned() else {
            return;
        };
        pane.update(cx, |pane, cx| {
            if pane.rename_index.is_some() {
                pane.commit_rename(cx);
            }
            pane.clear_selection();
            cx.notify();
        });
        self.state = Some(ContextMenuState {
            pane,
            target: ContextMenuTarget::Background,
            position,
        });
        self.open_with = None;
        cx.notify();
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.open_with_generation += 1;
        self.open_with = None;
        if self.state.take().is_some() {
            cx.notify();
        }
    }

    fn load_open_with_applications(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        self.open_with_generation += 1;
        let generation = self.open_with_generation;
        self.open_with = Some(OpenWithMenuState {
            text_opening_supported: FileEngine::supports_text_opening(&path),
            path: path.clone(),
            applications: Vec::new(),
            loading: true,
            expanded: false,
            error: None,
            generation,
        });
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let result = engine.applications_for_path(path).await;
            let _ = this.update(cx, |menu, cx| {
                let Some(open_with) = menu.open_with.as_mut() else {
                    return;
                };
                if open_with.generation != generation {
                    return;
                }
                open_with.loading = false;
                match result {
                    Ok(applications) => open_with.applications = applications,
                    Err(error) => open_with.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn execute(&mut self, command: MenuCommand, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.state.take() else {
            return;
        };
        self.open_with_generation += 1;
        self.open_with = None;
        cx.notify();

        match command {
            MenuCommand::Open => {
                state.pane.update(cx, |pane, cx| pane.activate_selected(cx));
            }
            MenuCommand::QuickLook => {
                window.dispatch_action(Box::new(ToggleQuickLook), cx);
            }
            MenuCommand::Cut => {
                self.operations
                    .update(cx, |operations, cx| operations.cut_selected(cx));
            }
            MenuCommand::Copy => {
                self.operations
                    .update(cx, |operations, cx| operations.copy_selected(cx));
            }
            MenuCommand::Paste => {
                self.operations
                    .update(cx, |operations, cx| operations.paste_into_active(cx));
            }
            MenuCommand::CopyToOther => {
                self.operations.update(cx, |operations, cx| {
                    operations.transfer_selected_to_other(TransferMode::Copy, cx);
                });
            }
            MenuCommand::MoveToOther => {
                self.operations.update(cx, |operations, cx| {
                    operations.transfer_selected_to_other(TransferMode::Move, cx);
                });
            }
            MenuCommand::Rename => {
                state.pane.update(cx, |pane, cx| {
                    pane.begin_rename();
                    cx.notify();
                });
            }
            MenuCommand::Trash => {
                self.operations.update(cx, |operations, cx| {
                    operations.move_selected_to_trash(cx);
                });
            }
            MenuCommand::GetInfo => {
                self.operations
                    .update(cx, |operations, cx| operations.show_selected_info(cx));
            }
            MenuCommand::NewFolder => {
                window.dispatch_action(Box::new(NewFolder), cx);
            }
            MenuCommand::NewTextFile => {
                window.dispatch_action(Box::new(NewTextFile), cx);
            }
            MenuCommand::OpenTerminal => {
                let path = state.pane.read(cx).current_path.clone();
                self.terminal.open(path);
            }
        }
    }

    fn expand_open_with(&mut self, cx: &mut Context<Self>) {
        if let Some(open_with) = self.open_with.as_mut()
            && !open_with.expanded
        {
            open_with.expanded = true;
            cx.notify();
        }
    }

    fn open_with_application(&mut self, application: OpenWithApplication, cx: &mut Context<Self>) {
        let Some(state) = self.state.take() else {
            return;
        };
        let Some(open_with) = self.open_with.take() else {
            return;
        };
        self.open_with_generation += 1;
        cx.notify();

        let pane = state.pane;
        let engine = self.engine.clone();
        cx.spawn(async move |_, cx| {
            if let Err(error) = engine
                .open_path_with(open_with.path, application.path)
                .await
            {
                let _ = pane.update(cx, |pane, cx| {
                    pane.error_message = Some(error.to_string());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn choose_custom_open_with_application(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.state.take() else {
            return;
        };
        let Some(open_with) = self.open_with.take() else {
            return;
        };
        self.open_with_generation += 1;
        cx.notify();

        let pane = state.pane;
        let engine = self.engine.clone();
        cx.spawn(async move |_, cx| {
            let result = match engine.choose_open_with_application().await {
                Ok(Some(application)) => engine.open_path_with(open_with.path, application).await,
                Ok(None) => Ok(()),
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                let _ = pane.update(cx, |pane, cx| {
                    pane.error_message = Some(error.to_string());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn open_as_text(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.state.take() else {
            return;
        };
        let Some(open_with) = self.open_with.take() else {
            return;
        };
        self.open_with_generation += 1;
        cx.notify();

        let pane = state.pane;
        let engine = self.engine.clone();
        cx.spawn(async move |_, cx| {
            if let Err(error) = engine.open_path_as_text(open_with.path).await {
                let _ = pane.update(cx, |pane, cx| {
                    pane.error_message = Some(error.to_string());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn item(
        &self,
        id: &'static str,
        icon: &'static str,
        label: &'static str,
        shortcut: &'static str,
        enabled: bool,
        command: MenuCommand,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .h(px(33.0))
            .mx_1()
            .px_2()
            .rounded_sm()
            .text_size(theme::font(11.0))
            .text_color(if enabled {
                theme::text_primary()
            } else {
                theme::text_tertiary().opacity(0.52)
            })
            .when(enabled, |item| {
                item.cursor_pointer()
                    .hover(|style| style.bg(theme::accent()).text_color(theme::surface()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.execute(command, window, cx);
                    }))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(27.0))
                    .mr_1()
                    .text_size(theme::font(12.0))
                    .child(icon),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .font_weight(FontWeight::NORMAL)
                    .child(label),
            )
            .when(!shortcut.is_empty(), |item| {
                item.child(
                    div()
                        .ml_4()
                        .font_family("SF Mono")
                        .text_size(theme::font(9.0))
                        .text_color(if enabled {
                            theme::text_secondary()
                        } else {
                            theme::text_tertiary().opacity(0.45)
                        })
                        .child(shortcut),
                )
            })
            .into_any_element()
    }

    fn separator() -> AnyElement {
        div()
            .h(px(1.0))
            .mx_2()
            .my_1()
            .bg(theme::border())
            .into_any_element()
    }

    fn open_with_trigger(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.open_with.is_some();
        div()
            .id("context-open-with")
            .flex()
            .items_center()
            .h(px(33.0))
            .mx_1()
            .px_2()
            .rounded_sm()
            .text_size(theme::font(11.0))
            .text_color(if enabled {
                theme::text_primary()
            } else {
                theme::text_tertiary().opacity(0.52)
            })
            .when(enabled, |item| {
                item.cursor_pointer()
                    .hover(|style| style.bg(theme::accent()).text_color(theme::surface()))
                    .on_hover(cx.listener(|this, hovered, _, cx| {
                        if *hovered {
                            this.expand_open_with(cx);
                        }
                    }))
                    .on_click(cx.listener(|this, _, _, cx| this.expand_open_with(cx)))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(27.0))
                    .mr_1()
                    .text_size(theme::font(12.0))
                    .child("▣"),
            )
            .child(div().min_w_0().flex_1().child("打开方式"))
            .child(
                div()
                    .ml_4()
                    .font_family("SF Mono")
                    .text_size(theme::font(11.0))
                    .text_color(if enabled {
                        theme::text_secondary()
                    } else {
                        theme::text_tertiary().opacity(0.45)
                    })
                    .child("›"),
            )
            .into_any_element()
    }

    fn open_with_submenu(
        &self,
        menu_position: Point<Pixels>,
        viewport_size: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open_with = self.open_with.clone()?;
        if !open_with.expanded {
            return None;
        }
        let text_opening_supported = open_with.text_opening_supported;

        let mut children = if open_with.loading {
            vec![Self::submenu_message("正在查找可用应用…")]
        } else if open_with.error.is_some() {
            vec![Self::submenu_message("无法读取可用应用")]
        } else if open_with.applications.is_empty() {
            vec![Self::submenu_message("未找到可用应用")]
        } else {
            open_with
                .applications
                .into_iter()
                .take(16)
                .enumerate()
                .map(|(index, application)| {
                    let badge = application.name.chars().next().unwrap_or('A').to_string();
                    let label = application.name.clone();
                    div()
                        .id(("open-with-application", index))
                        .flex()
                        .items_center()
                        .h(px(33.0))
                        .mx_1()
                        .px_2()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|style| style.bg(theme::accent()).text_color(theme::surface()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_with_application(application.clone(), cx);
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(22.0))
                                .mr_2()
                                .rounded_sm()
                                .border_1()
                                .border_color(theme::border())
                                .bg(theme::surface_subtle())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(theme::font(9.0))
                                .text_color(theme::accent())
                                .child(badge),
                        )
                        .child(div().min_w_0().flex_1().truncate().child(label))
                        .into_any_element()
                })
                .collect()
        };
        children.push(Self::separator());
        if text_opening_supported {
            children.push(self.text_open_item(cx));
        }
        children.push(self.custom_open_with_item(cx));

        let submenu = div()
            .id("open-with-submenu")
            .flex()
            .flex_col()
            .w(OPEN_WITH_SUBMENU_WIDTH)
            .max_h(px(420.0))
            .py_1()
            .overflow_y_scroll()
            .rounded_lg()
            .border_1()
            .border_color(theme::border_strong())
            .bg(theme::surface().opacity(0.985))
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .children(children);
        let submenu_position = Self::open_with_submenu_position(menu_position, viewport_size);
        Some(
            deferred(
                anchored()
                    .position(submenu_position)
                    .snap_to_window_with_margin(Edges::all(WINDOW_MARGIN))
                    .child(submenu),
            )
            .with_priority(110)
            .into_any_element(),
        )
    }

    fn open_with_submenu_position(
        menu_position: Point<Pixels>,
        viewport_size: Size<Pixels>,
    ) -> Point<Pixels> {
        // The main menu is snapped into the viewport by `anchored`. Resolve the
        // same horizontal position here so the submenu never uses the stale
        // pointer coordinate after the main menu has moved away from an edge.
        let max_menu_x = viewport_size.width - WINDOW_MARGIN - CONTEXT_MENU_WIDTH;
        let menu_x = if menu_position.x < WINDOW_MARGIN {
            WINDOW_MARGIN
        } else if menu_position.x > max_menu_x {
            max_menu_x
        } else {
            menu_position.x
        };
        let right_x = menu_x + CONTEXT_MENU_WIDTH + SUBMENU_GAP;
        let submenu_x = if right_x + OPEN_WITH_SUBMENU_WIDTH + WINDOW_MARGIN <= viewport_size.width
        {
            right_x
        } else {
            menu_x - SUBMENU_GAP - OPEN_WITH_SUBMENU_WIDTH
        };

        point(submenu_x, menu_position.y + px(33.0))
    }

    fn submenu_message(message: &'static str) -> AnyElement {
        div()
            .flex()
            .items_center()
            .h(px(36.0))
            .px_3()
            .text_size(theme::font(10.0))
            .text_color(theme::text_tertiary())
            .child(message)
            .into_any_element()
    }

    fn custom_open_with_item(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("open-with-custom-application")
            .flex()
            .items_center()
            .h(px(33.0))
            .mx_1()
            .px_2()
            .rounded_sm()
            .cursor_pointer()
            .hover(|style| style.bg(theme::accent()).text_color(theme::surface()))
            .on_click(cx.listener(|this, _, _, cx| {
                this.choose_custom_open_with_application(cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(22.0))
                    .mr_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface_subtle())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(theme::font(11.0))
                    .text_color(theme::accent())
                    .child("＋"),
            )
            .child(div().min_w_0().flex_1().child("自定义打开方式…"))
            .into_any_element()
    }

    fn text_open_item(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("open-as-text")
            .flex()
            .items_center()
            .h(px(33.0))
            .mx_1()
            .px_2()
            .rounded_sm()
            .cursor_pointer()
            .hover(|style| style.bg(theme::accent()).text_color(theme::surface()))
            .on_click(cx.listener(|this, _, _, cx| {
                this.open_as_text(cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(22.0))
                    .mr_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface_subtle())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(theme::font(10.0))
                    .text_color(theme::accent())
                    .child("✎"),
            )
            .child(div().min_w_0().flex_1().child("以文本方式打开"))
            .into_any_element()
    }

    fn selection_menu(
        &self,
        selection_count: usize,
        quick_look_enabled: bool,
        has_other_pane: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let has_selection = selection_count > 0;
        vec![
            self.item(
                "context-open",
                "📄",
                "打开",
                "↩",
                has_selection,
                MenuCommand::Open,
                cx,
            ),
            self.open_with_trigger(cx),
            self.item(
                "context-quick-look",
                "👁",
                "QuickLook 预览",
                "Space",
                quick_look_enabled,
                MenuCommand::QuickLook,
                cx,
            ),
            Self::separator(),
            self.item(
                "context-cut",
                "✂",
                "剪切",
                "⌘X",
                has_selection,
                MenuCommand::Cut,
                cx,
            ),
            self.item(
                "context-copy",
                "📋",
                "复制",
                "⌘C",
                has_selection,
                MenuCommand::Copy,
                cx,
            ),
            self.item(
                "context-paste-disabled",
                "📥",
                "粘贴",
                "⌘V",
                false,
                MenuCommand::Paste,
                cx,
            ),
            self.item(
                "context-copy-other",
                "➡",
                "复制到另一面板",
                "",
                has_selection && has_other_pane,
                MenuCommand::CopyToOther,
                cx,
            ),
            self.item(
                "context-move-other",
                "🚚",
                "移动到另一面板",
                "",
                has_selection && has_other_pane,
                MenuCommand::MoveToOther,
                cx,
            ),
            Self::separator(),
            self.item(
                "context-rename",
                "✏",
                "重命名",
                "F2",
                selection_count == 1,
                MenuCommand::Rename,
                cx,
            ),
            self.item(
                "context-trash",
                "🗑",
                "移至废纸篓",
                "⌘⌫",
                has_selection,
                MenuCommand::Trash,
                cx,
            ),
            Self::separator(),
            self.item(
                "context-get-info",
                "ⓘ",
                "显示简介",
                "⌘I",
                has_selection,
                MenuCommand::GetInfo,
                cx,
            ),
        ]
    }

    fn background_menu(&self, can_paste: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            self.item(
                "context-new-folder",
                "📁",
                "新建文件夹",
                "⌘N",
                true,
                MenuCommand::NewFolder,
                cx,
            ),
            self.item(
                "context-new-file",
                "📄",
                "新建文本文件",
                "⇧⌘N",
                true,
                MenuCommand::NewTextFile,
                cx,
            ),
            Self::separator(),
            self.item(
                "context-background-paste",
                "📥",
                "粘贴",
                "⌘V",
                can_paste,
                MenuCommand::Paste,
                cx,
            ),
            Self::separator(),
            self.item(
                "context-open-terminal",
                "⌘",
                "在系统终端中打开",
                "",
                true,
                MenuCommand::OpenTerminal,
                cx,
            ),
        ]
    }
}

impl Render for ContextMenuView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(state) = self.state.clone() else {
            return div().into_any_element();
        };
        let pane = state.pane.read(cx);
        let selection_count = pane.selection_count();
        let quick_look_enabled = selection_count == 1
            && pane
                .selected_index
                .and_then(|index| pane.items.get(index))
                .is_some_and(|item| !item.is_dir);
        let has_other_pane = self.model.read(cx).other_pane_index().is_some();
        let can_paste = self.operations.read(cx).can_paste(cx);
        let children = match state.target {
            ContextMenuTarget::Selection => {
                self.selection_menu(selection_count, quick_look_enabled, has_other_pane, cx)
            }
            ContextMenuTarget::Background => self.background_menu(can_paste, cx),
        };
        let open_with_submenu = self.open_with_submenu(state.position, window.viewport_size(), cx);

        let menu = div()
            .id("flowfile-context-menu")
            .flex()
            .flex_col()
            .w(CONTEXT_MENU_WIDTH)
            .py_1()
            .rounded_lg()
            .border_1()
            .border_color(theme::border_strong())
            .bg(theme::surface().opacity(0.985))
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .children(children);

        div()
            .id("context-menu-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.dismiss(cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| this.dismiss(cx)),
            )
            .child(
                deferred(
                    anchored()
                        .position(state.position)
                        .snap_to_window_with_margin(Edges::all(WINDOW_MARGIN))
                        .child(menu),
                )
                .with_priority(100),
            )
            .when_some(open_with_submenu, |overlay, submenu| overlay.child(submenu))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    #[test]
    fn open_with_submenu_avoids_the_main_menu_at_the_right_edge() {
        let viewport = size(px(1200.0), px(800.0));

        let right_edge =
            ContextMenuView::open_with_submenu_position(point(px(1180.0), px(100.0)), viewport);
        let snapped_menu_x = viewport.width - WINDOW_MARGIN - CONTEXT_MENU_WIDTH;
        assert_eq!(
            right_edge.x + OPEN_WITH_SUBMENU_WIDTH + SUBMENU_GAP,
            snapped_menu_x
        );

        let left_side =
            ContextMenuView::open_with_submenu_position(point(px(100.0), px(100.0)), viewport);
        assert_eq!(left_side.x, px(100.0) + CONTEXT_MENU_WIDTH + SUBMENU_GAP);
    }
}
