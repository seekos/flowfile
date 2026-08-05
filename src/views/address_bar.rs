use super::{
    search_bar::{
        clamp_char_range, next_char_boundary, previous_char_boundary, utf8_to_utf16_offset,
        utf16_to_utf8_offset,
    },
    tooltip::delayed_tooltip,
};
use crate::{models::Model, models::Pane, theme};
use gpui::{
    AnyElement, App, Bounds, Context, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, IntoElement, KeyDownEvent,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    Render, ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, div,
    fill, point, prelude::*, px, relative, size,
};
use std::ops::Range;
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
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    text_layout: Option<ShapedLine>,
    text_bounds: Option<Bounds<Pixels>>,
    text_scroll: Pixels,
    is_selecting: bool,
}

impl AddressBar {
    pub fn new(pane: Model<Pane>, cx: &mut Context<Self>) -> Self {
        cx.observe(&pane, |_, _, cx| cx.notify()).detach();
        Self {
            pane,
            editing: false,
            edit_buffer: String::new(),
            focus_handle: cx.focus_handle(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            text_layout: None,
            text_bounds: None,
            text_scroll: px(0.0),
            is_selecting: false,
        }
    }

    fn begin_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_buffer = self.pane.read(cx).current_path.display().to_string();
        self.editing = true;
        self.selected_range = 0..self.edit_buffer.len();
        self.selection_reversed = false;
        self.marked_range = None;
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.platform && event.keystroke.key == "a" {
            self.selected_range = 0..self.edit_buffer.len();
            self.selection_reversed = false;
            self.marked_range = None;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        match event.keystroke.key.as_str() {
            "left" if self.marked_range.is_none() => {
                self.move_horizontally(true, event.keystroke.modifiers.shift, cx);
                cx.stop_propagation();
            }
            "right" if self.marked_range.is_none() => {
                self.move_horizontally(false, event.keystroke.modifiers.shift, cx);
                cx.stop_propagation();
            }
            "home" if self.marked_range.is_none() => {
                if event.keystroke.modifiers.shift {
                    self.select_to(0, cx);
                } else {
                    self.move_to(0, cx);
                }
                cx.stop_propagation();
            }
            "end" if self.marked_range.is_none() => {
                let end = self.edit_buffer.len();
                if event.keystroke.modifiers.shift {
                    self.select_to(end, cx);
                } else {
                    self.move_to(end, cx);
                }
                cx.stop_propagation();
            }
            "enter" if self.marked_range.is_none() => {
                let input = self.edit_buffer.trim().to_string();
                if !input.is_empty() {
                    self.pane
                        .update(cx, |pane, cx| pane.navigate_to(PathBuf::from(input), cx));
                }
                self.editing = false;
                cx.notify();
                cx.stop_propagation();
            }
            "escape" => {
                self.editing = false;
                cx.notify();
                cx.stop_propagation();
            }
            "backspace" if self.marked_range.is_none() => {
                let cursor = self.cursor_offset().min(self.edit_buffer.len());
                if self.selected_range.is_empty() {
                    self.selected_range = previous_char_boundary(&self.edit_buffer, cursor)..cursor;
                    self.selection_reversed = false;
                }
                self.replace_selection("", cx);
                cx.stop_propagation();
            }
            "delete" if self.marked_range.is_none() => {
                if self.selected_range.is_empty() {
                    let cursor = self.cursor_offset().min(self.edit_buffer.len());
                    self.selected_range = cursor..next_char_boundary(&self.edit_buffer, cursor);
                    self.selection_reversed = false;
                }
                self.replace_selection("", cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn replace_selection(&mut self, new_text: &str, cx: &mut Context<Self>) {
        let range = clamp_char_range(&self.edit_buffer, self.selected_range.clone());
        self.edit_buffer.replace_range(range.clone(), new_text);
        let cursor = range.start + new_text.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.marked_range = None;
        cx.notify();
    }

    fn move_horizontally(
        &mut self,
        move_left: bool,
        extend_selection: bool,
        cx: &mut Context<Self>,
    ) {
        let cursor = self.cursor_offset().min(self.edit_buffer.len());
        if extend_selection {
            let next = if move_left {
                previous_char_boundary(&self.edit_buffer, cursor)
            } else {
                next_char_boundary(&self.edit_buffer, cursor)
            };
            self.select_to(next, cx);
        } else if self.selected_range.is_empty() {
            let next = if move_left {
                previous_char_boundary(&self.edit_buffer, cursor)
            } else {
                next_char_boundary(&self.edit_buffer, cursor)
            };
            self.move_to(next, cx);
        } else {
            let next = if move_left {
                self.selected_range.start
            } else {
                self.selected_range.end
            };
            self.move_to(next, cx);
        }
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        utf16_to_utf8_offset(&self.edit_buffer, offset)
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        utf8_to_utf16_offset(&self.edit_buffer, offset)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (self.text_bounds.as_ref(), self.text_layout.as_ref())
        else {
            return self.cursor_offset();
        };
        if position.x <= bounds.left() {
            return 0;
        }
        if position.x >= bounds.right() {
            return line.text.len();
        }
        line.closest_index_for_x(position.x - bounds.left() + self.text_scroll)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window);
        self.is_selecting = true;
        let offset = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
        cx.stop_propagation();
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_selecting && event.dragging() {
            let offset = self.index_for_mouse_position(event.position);
            self.select_to(offset, cx);
        }
    }

    fn on_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.is_selecting = false;
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
        let input_entity = cx.entity();
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
            .cursor_text()
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, {
                let input_entity = input_entity.clone();
                move |event, window, cx| {
                    input_entity.update(cx, |input, cx| input.on_mouse_down(event, window, cx));
                }
            })
            .on_mouse_move({
                let input_entity = input_entity.clone();
                move |event, window, cx| {
                    input_entity.update(cx, |input, cx| input.on_mouse_move(event, window, cx));
                }
            })
            .on_mouse_up(MouseButton::Left, {
                let input_entity = input_entity.clone();
                move |event, window, cx| {
                    input_entity.update(cx, |input, cx| input.on_mouse_up(event, window, cx));
                }
            })
            .on_mouse_up_out(MouseButton::Left, {
                let input_entity = input_entity.clone();
                move |event, window, cx| {
                    input_entity.update(cx, |input, cx| input.on_mouse_up(event, window, cx));
                }
            })
            .child(AddressTextElement {
                input: input_entity,
            })
            .into_any_element()
    }
}

impl Focusable for AddressBar {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for AddressBar {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = clamp_char_range(&self.edit_buffer, self.range_from_utf16(&range_utf16));
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.edit_buffer[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        self.selection_reversed = false;
        self.replace_selection(new_text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = clamp_char_range(
            &self.edit_buffer,
            range_utf16
                .as_ref()
                .map(|range| self.range_from_utf16(range))
                .or_else(|| self.marked_range.clone())
                .unwrap_or_else(|| self.selected_range.clone()),
        );
        self.edit_buffer.replace_range(range.clone(), new_text);
        let inserted = range.start..range.start + new_text.len();
        self.marked_range = (!new_text.is_empty()).then_some(inserted.clone());
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|selection| {
                inserted.start + utf16_to_utf8_offset(new_text, selection.start)
                    ..inserted.start + utf16_to_utf8_offset(new_text, selection.end)
            })
            .unwrap_or(inserted.end..inserted.end);
        self.selection_reversed = false;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.cursor_offset()))
    }
}

struct AddressTextElement {
    input: Entity<AddressBar>,
}

struct AddressPrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    scroll: Pixels,
}

impl IntoElement for AddressTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for AddressTextElement {
    type RequestLayoutState = ();
    type PrepaintState = AddressPrepaintState;

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
        let content: SharedString = input.edit_buffer.clone().into();
        let selected_range = clamp_char_range(&content, input.selected_range.clone());
        let style = window.text_style();
        let run = TextRun {
            len: content.len(),
            font: style.font(),
            color: style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.clone() {
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
                    len: content.len().saturating_sub(marked_range.end),
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(content, font_size, &runs, None);
        let cursor_x = line.x_for_index(input.cursor_offset());
        let visible_width = (bounds.size.width - px(2.0)).max(px(0.0));
        let scroll = (cursor_x - visible_width).max(px(0.0));
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_x - scroll, bounds.top()),
                        size(px(1.0), bounds.size.height),
                    ),
                    theme::accent(),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selected_range.start) - scroll,
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected_range.end) - scroll,
                            bounds.bottom(),
                        ),
                    ),
                    theme::accent_soft(),
                )),
                None,
            )
        };
        AddressPrepaintState {
            line: Some(line),
            cursor,
            selection,
            scroll,
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
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        let line = prepaint.line.take().expect("address line was shaped");
        line.paint(
            point(bounds.left() - prepaint.scroll, bounds.top()),
            window.line_height(),
            window,
            cx,
        )
        .expect("address line should paint");
        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.input.update(cx, |input, _| {
            input.text_layout = Some(line);
            input.text_bounds = Some(bounds);
            input.text_scroll = prepaint.scroll;
        });
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
