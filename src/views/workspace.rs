use super::{
    context_menu::ContextMenuView,
    multi_pane_container::MultiPaneContainerView,
    preferences::PreferencesModal,
    search_bar::{next_char_boundary, previous_char_boundary},
    sidebar::SidebarView,
    status_bar::StatusBar,
    tooltip::delayed_tooltip,
};
use crate::{
    actions::{
        CloseContextMenu, CloseQuickLook, CopyFiles, CutFiles, Duplicate, FindFiles, GetInfo,
        LayoutDualHorizontal, LayoutDualVertical, LayoutQuad, LayoutSingle, MoveToTrash, NewFolder,
        NewTextFile, NextPane, OpenPreferences, OpenTerminal, PasteFiles, PermanentDelete,
        PreviousPane, Refresh, ToggleQuickLook, ViewDetails, ViewGrid,
    },
    models::{
        AppPreferences, CreateItemKind, FileOperationController, LayoutMode, Model, MultiPaneModel,
        Pane, SessionState, home_directory,
    },
    services::{
        FileEngine, FileInspector, FileOperationEngine, PreviewKind, QuickLookService,
        SystemTerminal, ThumbnailEngine, TransferMode,
    },
    theme,
};
use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, Render, ScrollWheelEvent,
    SharedString, StyledImage, Timer, Window, black, div, img, prelude::*, px, uniform_list,
};
use std::{path::PathBuf, time::Duration};

#[derive(Clone)]
enum ModalState {
    NameInput { kind: CreateItemKind, value: String },
    PermanentDelete { paths: Vec<PathBuf> },
}

#[derive(Clone)]
enum QuickLookContent {
    Loading,
    Image,
    Text { lines: Vec<String>, truncated: bool },
    Native,
    Error(String),
}

#[derive(Clone)]
struct QuickLookState {
    path: PathBuf,
    content: QuickLookContent,
    zoom: f32,
    pan_x: f32,
    pan_y: f32,
    last_pointer: Option<(f32, f32)>,
}

pub struct WorkspaceView {
    model: Model<MultiPaneModel>,
    operations: Entity<FileOperationController>,
    _thumbnails: Entity<ThumbnailEngine>,
    quick_look_service: QuickLookService,
    _inspector: Entity<FileInspector>,
    sidebar: Entity<SidebarView>,
    multi_pane: Entity<MultiPaneContainerView>,
    status_bar: Entity<StatusBar>,
    terminal: SystemTerminal,
    context_menu: Entity<ContextMenuView>,
    preferences: Entity<PreferencesModal>,
    sidebar_visible: bool,
    focus_handle: FocusHandle,
    modal_focus_handle: FocusHandle,
    modal: Option<ModalState>,
    modal_cursor_offset: usize,
    modal_error: Option<String>,
    quick_look: Option<QuickLookState>,
    quick_look_generation: u64,
    quick_look_focus_handle: FocusHandle,
    session_save_generation: u64,
    last_saved_session: Option<SessionState>,
}

impl WorkspaceView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let engine = FileEngine::new().expect("failed to initialize FlowFile engine");
        let app_preferences = AppPreferences::load();
        theme::apply(app_preferences.theme, window.appearance());
        cx.observe_window_appearance(window, |_, window, cx| {
            theme::apply(AppPreferences::load().theme, window.appearance());
            cx.notify();
        })
        .detach();
        let operation_engine = FileOperationEngine::new(&engine);
        let home = home_directory();
        let restored_session = match SessionState::load() {
            Ok(session) => session,
            Err(error) => {
                eprintln!("FlowFile: 忽略无法恢复的会话：{error}");
                None
            }
        };
        let candidate_paths = [
            home.clone(),
            home.join("Downloads"),
            PathBuf::from("/Volumes"),
            home.join("Desktop"),
        ];
        let panes: Vec<_> = candidate_paths
            .into_iter()
            .enumerate()
            .map(|(index, path)| {
                let path = if path.is_dir() { path } else { home.clone() };
                let engine = engine.clone();
                let show_hidden = app_preferences.show_hidden;
                let restored_pane = restored_session
                    .as_ref()
                    .and_then(|session| session.panes.get(index))
                    .cloned();
                cx.new(move |_| match restored_pane {
                    Some(session) => session.restore(path, engine, show_hidden),
                    None => {
                        let mut pane = Pane::new(path, engine);
                        pane.show_hidden = show_hidden;
                        pane
                    }
                })
            })
            .collect();

        let layout_mode = restored_session
            .as_ref()
            .map(|session| session.layout_mode)
            .unwrap_or(app_preferences.default_layout);
        let pane_count = layout_mode.pane_count().min(panes.len());
        let active_pane_index = restored_session
            .as_ref()
            .map(|session| session.active_pane_index)
            .filter(|index| *index < pane_count)
            .unwrap_or(0);
        let last_active_pane_index = restored_session
            .as_ref()
            .and_then(|session| session.last_active_pane_index)
            .filter(|index| *index < pane_count && *index != active_pane_index)
            .or_else(|| (0..pane_count).find(|index| *index != active_pane_index));
        let sidebar_visible = restored_session
            .as_ref()
            .map(|session| session.sidebar_visible)
            .unwrap_or(true);

        let model = cx.new(|_| MultiPaneModel {
            layout_mode,
            panes: panes.clone(),
            active_pane_index,
            last_active_pane_index,
        });
        let operations = cx.new(|_| FileOperationController::new(model.clone(), operation_engine));
        let thumbnails =
            cx.new(|_| ThumbnailEngine::new().expect("failed to initialize thumbnail engine"));
        let quick_look_service = QuickLookService::new(&engine);
        let inspector = cx.new(|_| FileInspector::new(&engine));
        let terminal = SystemTerminal::new(&engine);

        cx.observe(&model, |this, _, cx| {
            this.schedule_session_save(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&operations, |_, _, cx| cx.notify()).detach();
        for pane in &panes {
            cx.observe(pane, |this, _, cx| {
                this.schedule_session_save(cx);
                cx.notify();
            })
            .detach();
            pane.update(cx, |pane, cx| pane.load_initial(cx));
        }

        let sidebar =
            cx.new(|cx| SidebarView::new(model.clone(), operations.clone(), engine.clone(), cx));
        let context_menu = cx.new(|_| {
            ContextMenuView::new(
                model.clone(),
                operations.clone(),
                terminal.clone(),
                engine.clone(),
            )
        });
        let multi_pane = cx.new(|cx| {
            MultiPaneContainerView::new(
                model.clone(),
                operations.clone(),
                thumbnails.clone(),
                context_menu.clone(),
                cx,
            )
        });
        let preferences = cx.new(|cx| PreferencesModal::new(model.clone(), cx));
        let status_bar = cx.new(|cx| {
            StatusBar::new(
                model.clone(),
                operations.clone(),
                inspector.clone(),
                terminal.clone(),
                cx,
            )
        });
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);

        let workspace = Self {
            model,
            operations,
            _thumbnails: thumbnails,
            quick_look_service,
            _inspector: inspector,
            sidebar,
            multi_pane,
            status_bar,
            terminal,
            context_menu,
            preferences,
            sidebar_visible,
            focus_handle,
            modal_focus_handle: cx.focus_handle(),
            modal: None,
            modal_cursor_offset: 0,
            modal_error: None,
            quick_look: None,
            quick_look_generation: 0,
            quick_look_focus_handle: cx.focus_handle(),
            session_save_generation: 0,
            last_saved_session: None,
        };

        cx.on_app_quit(|workspace, cx| {
            let session = workspace.capture_session(cx);
            async move {
                if let Err(error) = session.save() {
                    eprintln!("FlowFile: 退出时无法保存会话：{error}");
                }
            }
        })
        .detach();

        workspace
    }

    fn capture_session(&self, cx: &App) -> SessionState {
        SessionState::capture(self.model.read(cx), self.sidebar_visible, cx)
    }

    fn schedule_session_save(&mut self, cx: &mut Context<Self>) {
        self.session_save_generation = self.session_save_generation.wrapping_add(1);
        let generation = self.session_save_generation;
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(350)).await;
            let session = this
                .update(cx, |workspace, cx| {
                    if workspace.session_save_generation != generation {
                        return None;
                    }
                    let session = workspace.capture_session(cx);
                    (workspace.last_saved_session.as_ref() != Some(&session)).then_some(session)
                })
                .ok()
                .flatten();
            let Some(session) = session else {
                return;
            };

            if let Err(error) = session.save() {
                eprintln!("FlowFile: 无法自动保存会话：{error}");
                return;
            }
            let _ = this.update(cx, |workspace, _| {
                workspace.last_saved_session = Some(session);
            });
        })
        .detach();
    }

    fn active_pane(&self, cx: &App) -> Model<Pane> {
        let model = self.model.read(cx);
        model.panes[model.active_pane_index].clone()
    }

    fn on_copy(&mut self, _: &CopyFiles, _window: &mut Window, cx: &mut Context<Self>) {
        self.operations
            .update(cx, |operations, cx| operations.copy_selected(cx));
    }

    fn on_cut(&mut self, _: &CutFiles, _window: &mut Window, cx: &mut Context<Self>) {
        self.operations
            .update(cx, |operations, cx| operations.cut_selected(cx));
    }

    fn on_paste(&mut self, _: &PasteFiles, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = text.replace(['\n', '\r'], " ");
            if let Some(ModalState::NameInput { value, .. }) = &mut self.modal {
                insert_at_cursor(value, &mut self.modal_cursor_offset, &text);
                self.modal_error = None;
                cx.notify();
                return;
            }
            let pane = self.active_pane(cx);
            if pane.read(cx).rename_index.is_some() {
                pane.update(cx, |pane, cx| {
                    pane.append_rename_text(&text);
                    cx.notify();
                });
                return;
            }
        }
        self.operations
            .update(cx, |operations, cx| operations.paste_into_active(cx));
    }

    fn on_move_to_trash(&mut self, _: &MoveToTrash, _window: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_none() {
            self.operations
                .update(cx, |operations, cx| operations.move_selected_to_trash(cx));
        }
    }

    fn on_permanent_delete(
        &mut self,
        _: &PermanentDelete,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.modal.is_some() {
            return;
        }
        let paths = self.operations.read(cx).active_selected_paths(cx);
        if paths.is_empty() {
            return;
        }
        self.modal = Some(ModalState::PermanentDelete { paths });
        self.modal_cursor_offset = 0;
        self.modal_error = None;
        self.modal_focus_handle.focus(window);
        cx.notify();
    }

    fn on_new_folder(&mut self, _: &NewFolder, window: &mut Window, cx: &mut Context<Self>) {
        self.open_name_modal(CreateItemKind::Folder, "新建文件夹".to_string(), window, cx);
    }

    fn on_new_text_file(&mut self, _: &NewTextFile, window: &mut Window, cx: &mut Context<Self>) {
        self.open_name_modal(
            CreateItemKind::TextFile,
            "未命名.txt".to_string(),
            window,
            cx,
        );
    }

    fn on_duplicate(&mut self, _: &Duplicate, _window: &mut Window, cx: &mut Context<Self>) {
        self.operations
            .update(cx, |operations, cx| operations.duplicate_selected(cx));
    }

    fn on_refresh(&mut self, _: &Refresh, _window: &mut Window, cx: &mut Context<Self>) {
        self.active_pane(cx).update(cx, |pane, cx| pane.refresh(cx));
    }

    fn on_layout_single(&mut self, _: &LayoutSingle, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_layout(LayoutMode::Single, cx);
    }

    fn on_layout_dual_vertical(
        &mut self,
        _: &LayoutDualVertical,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_layout(LayoutMode::DualVertical, cx);
    }

    fn on_layout_dual_horizontal(
        &mut self,
        _: &LayoutDualHorizontal,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_layout(LayoutMode::DualHorizontal, cx);
    }

    fn on_layout_quad(&mut self, _: &LayoutQuad, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_layout(LayoutMode::Quad, cx);
    }

    fn on_next_pane(&mut self, _: &NextPane, _window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_pane(false, cx);
    }

    fn on_previous_pane(&mut self, _: &PreviousPane, _window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_pane(true, cx);
    }

    fn on_view_details(&mut self, _: &ViewDetails, _window: &mut Window, cx: &mut Context<Self>) {
        self.active_pane(cx).update(cx, |pane, cx| {
            pane.set_view_mode(crate::models::ViewMode::Details, cx)
        });
    }

    fn on_view_grid(&mut self, _: &ViewGrid, _window: &mut Window, cx: &mut Context<Self>) {
        self.active_pane(cx).update(cx, |pane, cx| {
            pane.set_view_mode(crate::models::ViewMode::Grid, cx)
        });
    }

    fn on_find_files(&mut self, _: &FindFiles, _window: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_none() {
            self.active_pane(cx)
                .update(cx, |pane, cx| pane.begin_search(cx));
        }
    }

    fn on_open_terminal(&mut self, _: &OpenTerminal, _window: &mut Window, cx: &mut Context<Self>) {
        let path = self.active_pane(cx).read(cx).current_path.clone();
        self.terminal.open(path);
    }

    fn on_open_preferences(
        &mut self,
        _: &OpenPreferences,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.modal.is_none() {
            self.preferences
                .update(cx, |preferences, cx| preferences.open(window, cx));
        }
    }

    fn on_toggle_quick_look(
        &mut self,
        _: &ToggleQuickLook,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.quick_look.is_some() {
            self.close_quick_look(window, cx);
            return;
        }
        if self.modal.is_some() {
            return;
        }
        let path = {
            let pane = self.active_pane(cx);
            let pane = pane.read(cx);
            pane.selected_index
                .and_then(|index| pane.items.get(index))
                .filter(|item| !item.is_dir)
                .map(|item| item.path.clone())
        };
        let Some(path) = path else {
            return;
        };
        self.quick_look_generation += 1;
        let generation = self.quick_look_generation;
        let kind = QuickLookService::classify(&path);
        self.quick_look = Some(QuickLookState {
            path: path.clone(),
            content: match kind {
                PreviewKind::Image => QuickLookContent::Image,
                PreviewKind::Text => QuickLookContent::Loading,
                PreviewKind::Native => QuickLookContent::Native,
            },
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            last_pointer: None,
        });
        self.quick_look_focus_handle.focus(window);
        cx.notify();

        match kind {
            PreviewKind::Text => {
                let service = self.quick_look_service.clone();
                cx.spawn(async move |this, cx| {
                    let result = service.read_text(path.clone()).await;
                    let _ = this.update(cx, |workspace, cx| {
                        if workspace.quick_look_generation != generation {
                            return;
                        }
                        if let Some(preview) = &mut workspace.quick_look
                            && preview.path == path
                        {
                            preview.content = match result {
                                Ok((text, truncated)) => QuickLookContent::Text {
                                    lines: text.lines().map(str::to_string).collect(),
                                    truncated,
                                },
                                Err(error) => QuickLookContent::Error(error.to_string()),
                            };
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            PreviewKind::Native => self.quick_look_service.open_native(path),
            PreviewKind::Image => {}
        }
    }

    fn on_close_quick_look(
        &mut self,
        _: &CloseQuickLook,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_quick_look(window, cx);
    }

    fn on_close_context_menu(
        &mut self,
        _: &CloseContextMenu,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.context_menu
            .update(cx, |context_menu, cx| context_menu.dismiss(cx));
    }

    fn on_get_info(&mut self, _: &GetInfo, _window: &mut Window, cx: &mut Context<Self>) {
        self.operations
            .update(cx, |operations, cx| operations.show_selected_info(cx));
    }

    fn close_quick_look(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quick_look.take().is_some() {
            self.quick_look_service.close_native();
            self.quick_look_generation += 1;
            self.focus_handle.focus(window);
            cx.notify();
        }
    }

    fn on_preview_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(preview) = &mut self.quick_look else {
            return;
        };
        if !matches!(preview.content, QuickLookContent::Image) {
            return;
        }
        let delta = event.delta.pixel_delta(px(18.0));
        let factor = if f32::from(delta.y) < 0.0 { 1.12 } else { 0.89 };
        preview.zoom = (preview.zoom * factor).clamp(0.25, 6.0);
        cx.notify();
    }

    fn on_preview_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if let Some(preview) = &mut self.quick_look {
            preview.last_pointer = Some((f32::from(event.position.x), f32::from(event.position.y)));
        }
    }

    fn on_preview_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(preview) = &mut self.quick_look else {
            return;
        };
        if !event.dragging() {
            preview.last_pointer = None;
            return;
        }
        if let Some((last_x, last_y)) = preview.last_pointer {
            preview.pan_x += f32::from(event.position.x) - last_x;
            preview.pan_y += f32::from(event.position.y) - last_y;
            preview.last_pointer = Some((f32::from(event.position.x), f32::from(event.position.y)));
            cx.notify();
        }
    }

    fn set_layout(&self, mode: LayoutMode, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| {
            model.set_layout(mode);
            cx.notify();
        });
    }

    fn cycle_pane(&self, reverse: bool, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| {
            model.cycle_active_pane(reverse);
            cx.notify();
        });
    }

    fn open_name_modal(
        &mut self,
        kind: CreateItemKind,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.modal.is_some() {
            return;
        }
        self.modal_cursor_offset = value.len();
        self.modal = Some(ModalState::NameInput { kind, value });
        self.modal_error = None;
        self.modal_focus_handle.focus(window);
        cx.notify();
    }

    fn confirm_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(modal) = self.modal.clone() else {
            return;
        };
        match modal {
            ModalState::NameInput { kind, value } => {
                let name = value.trim();
                if name.is_empty() || name.contains('/') || name == "." || name == ".." {
                    self.modal_error = Some("请输入不包含 “/” 的有效名称".to_string());
                    cx.notify();
                    return;
                }
                self.operations.update(cx, |operations, cx| {
                    operations.create_item(kind, name.to_string(), cx);
                });
            }
            ModalState::PermanentDelete { paths } => {
                self.operations.update(cx, |operations, cx| {
                    operations.delete_permanently(paths, cx);
                });
            }
        }
        self.close_modal(window, cx);
    }

    fn close_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.modal = None;
        self.modal_cursor_offset = 0;
        self.modal_error = None;
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_none() {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.close_modal(window, cx),
            "enter" => self.confirm_modal(window, cx),
            "left" => {
                if let Some(ModalState::NameInput { value, .. }) = &self.modal {
                    self.modal_cursor_offset =
                        previous_char_boundary(value, self.modal_cursor_offset);
                    cx.notify();
                }
            }
            "right" => {
                if let Some(ModalState::NameInput { value, .. }) = &self.modal {
                    self.modal_cursor_offset = next_char_boundary(value, self.modal_cursor_offset);
                    cx.notify();
                }
            }
            "home" => {
                if matches!(self.modal, Some(ModalState::NameInput { .. })) {
                    self.modal_cursor_offset = 0;
                    cx.notify();
                }
            }
            "end" => {
                if let Some(ModalState::NameInput { value, .. }) = &self.modal {
                    self.modal_cursor_offset = value.len();
                    cx.notify();
                }
            }
            "backspace" => {
                if let Some(ModalState::NameInput { value, .. }) = &mut self.modal {
                    backspace_at_cursor(value, &mut self.modal_cursor_offset);
                    self.modal_error = None;
                    cx.notify();
                }
            }
            "delete" => {
                if let Some(ModalState::NameInput { value, .. }) = &mut self.modal {
                    delete_at_cursor(value, &mut self.modal_cursor_offset);
                    self.modal_error = None;
                    cx.notify();
                }
            }
            _ => {
                if let Some(ModalState::NameInput { value, .. }) = &mut self.modal
                    && let Some(text) = &event.keystroke.key_char
                    && !text.chars().any(char::is_control)
                {
                    insert_at_cursor(value, &mut self.modal_cursor_offset, text);
                    self.modal_error = None;
                    cx.notify();
                }
            }
        }
        cx.stop_propagation();
    }

    fn layout_button(
        &self,
        mode: LayoutMode,
        glyph: &'static str,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_active = self.model.read(cx).layout_mode == mode;
        let model = self.model.clone();

        div()
            .id(label)
            .flex()
            .items_center()
            .gap_1()
            .h(px(34.0))
            .px_2()
            .rounded_md()
            .border_1()
            .border_color(if is_active {
                theme::accent()
            } else {
                theme::border()
            })
            .bg(if is_active {
                theme::accent_soft()
            } else {
                theme::surface()
            })
            .text_color(if is_active {
                theme::accent()
            } else {
                theme::text_secondary()
            })
            .hover(|style| style.bg(theme::accent_soft()))
            .tooltip(delayed_tooltip(format!("切换为{label}布局")))
            .on_click(move |_, _, cx| {
                model.update(cx, |model, cx| {
                    model.set_layout(mode);
                    cx.notify();
                });
            })
            .child(
                div()
                    .font_family("SF Mono")
                    .text_size(theme::font(13.0))
                    .child(glyph),
            )
            .child(div().text_size(theme::font(9.0)).child(label))
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar_visible = self.sidebar_visible;
        let (active_pane, show_hidden, has_selection, has_other_pane) = {
            let model = self.model.read(cx);
            let active_pane = model.panes[model.active_pane_index].clone();
            let pane = active_pane.read(cx);
            (
                active_pane.clone(),
                pane.show_hidden,
                pane.selection_count() > 0,
                model.other_pane_index().is_some(),
            )
        };
        let hidden_pane = active_pane;
        let copy_operations = self.operations.clone();
        let move_operations = self.operations.clone();
        let trash_operations = self.operations.clone();

        div()
            .flex()
            .items_center()
            .min_w_0()
            .flex_1()
            .gap_1()
            .h(px(52.0))
            .px_2()
            .bg(theme::surface_subtle())
            .child(
                div()
                    .id("sidebar-toggle")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(34.0))
                    .mr_1()
                    .text_size(theme::font(15.0))
                    .rounded_md()
                    .border_1()
                    .border_color(if sidebar_visible {
                        theme::accent()
                    } else {
                        theme::border()
                    })
                    .bg(theme::surface())
                    .text_color(if sidebar_visible {
                        theme::accent()
                    } else {
                        theme::text_secondary()
                    })
                    .hover(|style| style.bg(theme::accent_soft()))
                    .tooltip(delayed_tooltip(if sidebar_visible {
                        "隐藏侧边栏"
                    } else {
                        "显示侧边栏"
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sidebar_visible = !this.sidebar_visible;
                        this.schedule_session_save(cx);
                        cx.notify();
                    }))
                    .child("◧"),
            )
            .child(self.layout_button(LayoutMode::Single, "□", "单窗", cx))
            .child(self.layout_button(LayoutMode::DualVertical, "▥", "左右", cx))
            .child(self.layout_button(LayoutMode::DualHorizontal, "▤", "上下", cx))
            .child(self.layout_button(LayoutMode::Quad, "▦", "四格", cx))
            .child(
                div()
                    .id("hidden-files-toggle")
                    .flex()
                    .items_center()
                    .h(px(34.0))
                    .px_2()
                    .ml_1()
                    .rounded_md()
                    .border_1()
                    .border_color(if show_hidden {
                        theme::accent()
                    } else {
                        theme::border()
                    })
                    .bg(if show_hidden {
                        theme::accent_soft()
                    } else {
                        theme::surface()
                    })
                    .text_size(theme::font(9.0))
                    .text_color(if show_hidden {
                        theme::accent()
                    } else {
                        theme::text_secondary()
                    })
                    .hover(|style| style.bg(theme::accent_soft()))
                    .tooltip(delayed_tooltip(if show_hidden {
                        "隐藏名称以 . 开头的文件"
                    } else {
                        "显示名称以 . 开头的文件"
                    }))
                    .on_click(move |_, _, cx| {
                        hidden_pane.update(cx, |pane, cx| pane.toggle_hidden(cx));
                    })
                    .child(if show_hidden {
                        "隐藏点文件"
                    } else {
                        "点文件"
                    }),
            )
            .child(
                div()
                    .id("copy-to-other-pane")
                    .flex()
                    .items_center()
                    .h(px(34.0))
                    .px_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface())
                    .text_size(theme::font(9.0))
                    .text_color(if has_selection && has_other_pane {
                        theme::accent()
                    } else {
                        theme::text_tertiary()
                    })
                    .tooltip(delayed_tooltip("将选中项目复制到另一面板"))
                    .when(has_selection && has_other_pane, |button| {
                        button
                            .hover(|style| style.bg(theme::accent_soft()))
                            .on_click(move |_, _, cx| {
                                copy_operations.update(cx, |operations, cx| {
                                    operations.transfer_selected_to_other(TransferMode::Copy, cx);
                                });
                            })
                    })
                    .child("→ 复制"),
            )
            .child(
                div()
                    .id("move-to-other-pane")
                    .flex()
                    .items_center()
                    .h(px(34.0))
                    .px_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface())
                    .text_size(theme::font(9.0))
                    .text_color(if has_selection && has_other_pane {
                        theme::file_green()
                    } else {
                        theme::text_tertiary()
                    })
                    .tooltip(delayed_tooltip("将选中项目移动到另一面板"))
                    .when(has_selection && has_other_pane, |button| {
                        button
                            .hover(|style| style.bg(theme::accent_soft()))
                            .on_click(move |_, _, cx| {
                                move_operations.update(cx, |operations, cx| {
                                    operations.transfer_selected_to_other(TransferMode::Move, cx);
                                });
                            })
                    })
                    .child("→ 移动"),
            )
            .child(
                div()
                    .id("move-to-trash")
                    .flex()
                    .items_center()
                    .h(px(34.0))
                    .px_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface())
                    .text_size(theme::font(9.0))
                    .text_color(if has_selection {
                        theme::danger()
                    } else {
                        theme::text_tertiary()
                    })
                    .tooltip(delayed_tooltip("将选中项目移至废纸篓 (⌘⌫)"))
                    .when(has_selection, |button| {
                        button
                            .hover(|style| style.bg(theme::danger_soft()))
                            .on_click(move |_, _, cx| {
                                trash_operations.update(cx, |operations, cx| {
                                    operations.move_selected_to_trash(cx);
                                });
                            })
                    })
                    .child("废纸篓"),
            )
            .child(
                div()
                    .id("titlebar-drag-region")
                    .min_w(px(24.0))
                    .h_full()
                    .flex_1()
                    .on_mouse_down(MouseButton::Left, |event, window, _| {
                        if event.click_count >= 2 {
                            window.zoom_window();
                        } else {
                            window.start_window_move();
                        }
                    }),
            )
    }

    fn titlebar_sidebar(&self, window: &Window) -> impl IntoElement {
        let width = if window.is_fullscreen() {
            0.0
        } else if window.is_maximized() || !self.sidebar_visible {
            104.0
        } else {
            205.0
        };

        div()
            .id("titlebar-sidebar")
            .flex_shrink_0()
            .w(px(width))
            .h(px(52.0))
            .bg(theme::sidebar())
            .on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count >= 2 {
                    window.zoom_window();
                } else {
                    window.start_window_move();
                }
            })
    }

    fn titlebar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .w_full()
            .h(px(52.0))
            .flex_shrink_0()
            .border_b_1()
            .border_color(theme::border())
            .child(self.titlebar_sidebar(window))
            .child(self.toolbar(cx))
    }

    fn render_modal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let modal = self.modal.clone()?;
        let modal_error = self.modal_error.clone();

        let card = match modal {
            ModalState::NameInput { kind, value } => {
                let (title, detail, button) = match kind {
                    CreateItemKind::Folder => ("新建文件夹", "在当前面板中创建文件夹", "创建"),
                    CreateItemKind::TextFile => {
                        ("新建文本文件", "创建一个空白的 UTF-8 文件", "创建")
                    }
                };
                let cursor = self.modal_cursor_offset.min(value.len());
                let value_before_cursor: SharedString = value[..cursor].to_string().into();
                let value_after_cursor: SharedString = value[cursor..].to_string().into();
                div()
                    .w(px(390.0))
                    .p_5()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme::border_strong())
                    .bg(theme::surface())
                    .shadow_lg()
                    .child(
                        div()
                            .text_size(theme::font(15.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(title),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_size(theme::font(10.0))
                            .text_color(theme::text_tertiary())
                            .child(detail),
                    )
                    .child(
                        div()
                            .mt_4()
                            .flex()
                            .items_center()
                            .h(px(34.0))
                            .px_3()
                            .rounded_md()
                            .border_1()
                            .border_color(if modal_error.is_some() {
                                theme::danger()
                            } else {
                                theme::accent()
                            })
                            .bg(theme::surface_subtle())
                            .font_family("SF Mono")
                            .text_size(theme::font(11.0))
                            .text_color(theme::text_primary())
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .whitespace_nowrap()
                                            .child(value_before_cursor),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .w(px(1.0))
                                            .h(px(16.0))
                                            .bg(theme::accent()),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .truncate()
                                            .child(value_after_cursor),
                                    ),
                            ),
                    )
                    .when_some(modal_error, |card, error| {
                        card.child(
                            div()
                                .mt_2()
                                .text_size(theme::font(9.0))
                                .text_color(theme::danger())
                                .child(error),
                        )
                    })
                    .child(self.modal_buttons(button, false, cx))
                    .into_any_element()
            }
            ModalState::PermanentDelete { paths } => {
                let count = paths.len();
                let first_name = paths
                    .first()
                    .and_then(|path| path.file_name())
                    .and_then(|name| name.to_str())
                    .unwrap_or("所选项目")
                    .to_string();
                div()
                    .w(px(420.0))
                    .p_5()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme::danger())
                    .bg(theme::surface())
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(theme::font(15.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::danger())
                            .child("!")
                            .child("永久删除"),
                    )
                    .child(
                        div()
                            .mt_3()
                            .text_size(theme::font(11.0))
                            .text_color(theme::text_primary())
                            .child(format!("将永久删除“{first_name}”等 {count} 个项目。")),
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_size(theme::font(9.0))
                            .text_color(theme::text_tertiary())
                            .child("此操作不会经过废纸篓，且无法撤销。"),
                    )
                    .child(self.modal_buttons("永久删除", true, cx))
                    .into_any_element()
            }
        };

        Some(
            div()
                .id("workspace-modal")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .track_focus(&self.modal_focus_handle)
                .occlude()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .bg(black().opacity(0.34)),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(card),
                )
                .into_any_element(),
        )
    }

    fn render_quick_look(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let preview = self.quick_look.clone()?;
        let file_name = preview
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("预览")
            .to_string();

        let content = match preview.content {
            QuickLookContent::Loading => div()
                .flex()
                .items_center()
                .justify_center()
                .size_full()
                .text_color(theme::text_tertiary())
                .child("正在读取前 100 KB…")
                .into_any_element(),
            QuickLookContent::Image => div()
                .id("quick-look-image-stage")
                .flex()
                .items_center()
                .justify_center()
                .size_full()
                .overflow_hidden()
                .cursor_move()
                .on_scroll_wheel(cx.listener(Self::on_preview_scroll))
                .on_mouse_down(MouseButton::Left, cx.listener(Self::on_preview_mouse_down))
                .on_mouse_move(cx.listener(Self::on_preview_mouse_move))
                .child(
                    img(preview.path.clone())
                        .w(px(760.0 * preview.zoom))
                        .h(px(560.0 * preview.zoom))
                        .ml(px(preview.pan_x))
                        .mt(px(preview.pan_y))
                        .object_fit(ObjectFit::Contain)
                        .with_fallback(|| {
                            div()
                                .text_color(theme::danger())
                                .child("无法解码此图像")
                                .into_any_element()
                        }),
                )
                .into_any_element(),
            QuickLookContent::Text { lines, truncated } => {
                let line_count = lines.len();
                let lines = std::sync::Arc::new(lines);
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "quick-look-text",
                            line_count,
                            move |range: std::ops::Range<usize>, _, _| {
                                range
                                    .map(|index| {
                                        let line = lines.get(index).cloned().unwrap_or_default();
                                        let color = syntax_line_color(&line);
                                        div()
                                            .flex()
                                            .h(px(20.0))
                                            .px_3()
                                            .font_family("SF Mono")
                                            .text_size(theme::font(11.0))
                                            .text_color(color)
                                            .child(
                                                div()
                                                    .w(px(48.0))
                                                    .text_color(theme::text_tertiary())
                                                    .child(format!("{:>4}", index + 1)),
                                            )
                                            .child(div().min_w_0().flex_1().child(line))
                                    })
                                    .collect::<Vec<_>>()
                            },
                        )
                        .size_full(),
                    )
                    .when(truncated, |panel| {
                        panel.child(
                            div()
                                .h(px(24.0))
                                .px_3()
                                .text_size(theme::font(9.0))
                                .text_color(theme::text_tertiary())
                                .child("预览已截断于 100 KB"),
                        )
                    })
                    .into_any_element()
            }
            QuickLookContent::Native => div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .size_full()
                .gap_3()
                .text_color(theme::text_secondary())
                .child(
                    div()
                        .text_size(theme::font(44.0))
                        .text_color(theme::accent())
                        .child("QL"),
                )
                .child("已交给 macOS 原生 Quick Look")
                .child(
                    div()
                        .text_size(theme::font(10.0))
                        .text_color(theme::text_tertiary())
                        .child("再次按空格或 Esc 关闭此遮罩"),
                )
                .into_any_element(),
            QuickLookContent::Error(error) => div()
                .flex()
                .items_center()
                .justify_center()
                .size_full()
                .text_color(theme::danger())
                .child(error)
                .into_any_element(),
        };

        Some(
            div()
                .id("quick-look-overlay")
                .key_context("QuickLook")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .track_focus(&self.quick_look_focus_handle)
                .occlude()
                .bg(black().opacity(0.78))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w(px(920.0))
                        .h(px(680.0))
                        .overflow_hidden()
                        .rounded_lg()
                        .border_1()
                        .border_color(theme::border_strong())
                        .bg(theme::surface())
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .h(px(42.0))
                                .px_4()
                                .border_b_1()
                                .border_color(theme::border())
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .truncate()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(file_name),
                                )
                                .when(
                                    matches!(
                                        self.quick_look.as_ref().map(|preview| &preview.content),
                                        Some(QuickLookContent::Image)
                                    ),
                                    |header| {
                                        header.child(
                                            div()
                                                .mr_3()
                                                .text_size(theme::font(10.0))
                                                .text_color(theme::text_tertiary())
                                                .child(format!("{:.0}%", preview.zoom * 100.0)),
                                        )
                                    },
                                )
                                .child(
                                    div()
                                        .id("close-quick-look")
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(26.0))
                                        .rounded_md()
                                        .hover(|style| style.bg(theme::accent_soft()))
                                        .tooltip(delayed_tooltip("关闭预览 (Space / Esc)"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.close_quick_look(window, cx)
                                        }))
                                        .child("×"),
                                ),
                        )
                        .child(div().flex().min_h_0().flex_1().child(content)),
                )
                .into_any_element(),
        )
    }

    fn modal_buttons(
        &self,
        confirm_label: &'static str,
        destructive: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .justify_end()
            .gap_2()
            .mt_5()
            .child(
                div()
                    .id("modal-cancel")
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(30.0))
                    .px_4()
                    .rounded_md()
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface())
                    .text_size(theme::font(10.0))
                    .text_color(theme::text_secondary())
                    .hover(|style| style.bg(theme::surface_subtle()))
                    .tooltip(delayed_tooltip("取消并关闭对话框 (Esc)"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_modal(window, cx);
                    }))
                    .child("取消"),
            )
            .child(
                div()
                    .id("modal-confirm")
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(30.0))
                    .px_4()
                    .rounded_md()
                    .bg(if destructive {
                        theme::danger()
                    } else {
                        theme::accent()
                    })
                    .text_size(theme::font(10.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::surface())
                    .hover(|style| {
                        style.bg(if destructive {
                            theme::danger().opacity(0.86)
                        } else {
                            theme::accent().opacity(0.86)
                        })
                    })
                    .tooltip(delayed_tooltip(format!("确认{confirm_label} (Enter)")))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.confirm_modal(window, cx);
                    }))
                    .child(confirm_label),
            )
    }
}

impl Focusable for WorkspaceView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let modal = self.render_modal(cx);
        let quick_look = self.render_quick_look(cx);
        div()
            .id("workspace")
            .key_context("Workspace")
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_copy))
            .on_action(cx.listener(Self::on_cut))
            .on_action(cx.listener(Self::on_paste))
            .on_action(cx.listener(Self::on_move_to_trash))
            .on_action(cx.listener(Self::on_permanent_delete))
            .on_action(cx.listener(Self::on_new_folder))
            .on_action(cx.listener(Self::on_new_text_file))
            .on_action(cx.listener(Self::on_duplicate))
            .on_action(cx.listener(Self::on_refresh))
            .on_action(cx.listener(Self::on_layout_single))
            .on_action(cx.listener(Self::on_layout_dual_vertical))
            .on_action(cx.listener(Self::on_layout_dual_horizontal))
            .on_action(cx.listener(Self::on_layout_quad))
            .on_action(cx.listener(Self::on_next_pane))
            .on_action(cx.listener(Self::on_previous_pane))
            .on_action(cx.listener(Self::on_view_details))
            .on_action(cx.listener(Self::on_view_grid))
            .on_action(cx.listener(Self::on_toggle_quick_look))
            .on_action(cx.listener(Self::on_close_quick_look))
            .on_action(cx.listener(Self::on_close_context_menu))
            .on_action(cx.listener(Self::on_get_info))
            .on_action(cx.listener(Self::on_find_files))
            .on_action(cx.listener(Self::on_open_terminal))
            .on_action(cx.listener(Self::on_open_preferences))
            .on_key_down(cx.listener(Self::on_key_down))
            .font_family("SF Pro Text")
            .bg(theme::canvas())
            .text_color(theme::text_primary())
            .child(self.titlebar(window, cx))
            .child(
                div()
                    .flex()
                    .min_h_0()
                    .flex_1()
                    .when(self.sidebar_visible, |body| {
                        body.child(self.sidebar.clone())
                    })
                    .child(
                        div()
                            .flex()
                            .min_w_0()
                            .min_h_0()
                            .flex_1()
                            .child(self.multi_pane.clone()),
                    ),
            )
            .child(self.status_bar.clone())
            .when_some(quick_look, |workspace, preview| workspace.child(preview))
            .when_some(modal, |workspace, modal| workspace.child(modal))
            .child(self.preferences.clone())
            .child(self.context_menu.clone())
    }
}

fn syntax_line_color(line: &str) -> gpui::Hsla {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with('#') {
        theme::file_green()
    } else if trimmed.starts_with('"') || trimmed.contains("\":") {
        theme::file_blue()
    } else if [
        "fn ", "pub ", "struct ", "enum ", "impl ", "use ", "mod ", "class ", "def ", "import ",
    ]
    .iter()
    .any(|keyword| trimmed.starts_with(keyword))
    {
        theme::file_purple()
    } else {
        theme::text_primary()
    }
}

fn insert_at_cursor(value: &mut String, cursor: &mut usize, text: &str) {
    let offset = (*cursor).min(value.len());
    value.insert_str(offset, text);
    *cursor = offset + text.len();
}

fn backspace_at_cursor(value: &mut String, cursor: &mut usize) {
    let offset = (*cursor).min(value.len());
    let previous = previous_char_boundary(value, offset);
    if previous < offset {
        value.replace_range(previous..offset, "");
        *cursor = previous;
    }
}

fn delete_at_cursor(value: &mut String, cursor: &mut usize) {
    let offset = (*cursor).min(value.len());
    let next = next_char_boundary(value, offset);
    if offset < next {
        value.replace_range(offset..next, "");
    }
    *cursor = offset;
}

#[cfg(test)]
mod modal_name_input_tests {
    use super::{backspace_at_cursor, delete_at_cursor, insert_at_cursor};

    #[test]
    fn cursor_edits_work_at_the_middle_of_file_names() {
        let mut value = "新建文件夹".to_string();
        let mut cursor = "新建".len();

        insert_at_cursor(&mut value, &mut cursor, "测试");
        assert_eq!(value, "新建测试文件夹");
        assert_eq!(cursor, "新建测试".len());

        backspace_at_cursor(&mut value, &mut cursor);
        assert_eq!(value, "新建测文件夹");
        assert_eq!(cursor, "新建测".len());

        delete_at_cursor(&mut value, &mut cursor);
        assert_eq!(value, "新建测件夹");
        assert_eq!(cursor, "新建测".len());
    }
}
