use super::tooltip::delayed_tooltip;
use crate::{
    actions::{CopyFiles, CutFiles, PasteFiles},
    models::Model,
    models::Pane,
    theme,
};
use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, IntoElement, KeyDownEvent,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    Render, ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, div,
    fill, point, prelude::*, px, relative, size,
};
use std::ops::Range;

pub struct SearchBar {
    pane: Model<Pane>,
    focus_handle: FocusHandle,
    was_active: bool,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    text_layout: Option<ShapedLine>,
    text_bounds: Option<Bounds<Pixels>>,
    text_scroll: Pixels,
    is_selecting: bool,
}

impl SearchBar {
    pub fn new(pane: Model<Pane>, cx: &mut Context<Self>) -> Self {
        cx.observe(&pane, |_, _, cx| cx.notify()).detach();
        Self {
            pane,
            focus_handle: cx.focus_handle(),
            was_active: false,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            text_layout: None,
            text_bounds: None,
            text_scroll: px(0.0),
            is_selecting: false,
        }
    }

    fn activate_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pane.update(cx, |pane, cx| pane.begin_search(cx));
        let query_len = self.pane.read(cx).search_query.len();
        self.selected_range = query_len..query_len;
        self.selection_reversed = false;
        self.marked_range = None;
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.platform && event.keystroke.key == "a" {
            let query_len = self.pane.read(cx).search_query.len();
            self.selected_range = 0..query_len;
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
                let end = self.pane.read(cx).search_query.len();
                if event.keystroke.modifiers.shift {
                    self.select_to(end, cx);
                } else {
                    self.move_to(end, cx);
                }
                cx.stop_propagation();
            }
            "escape" => {
                self.pane.update(cx, |pane, cx| pane.exit_search(cx));
                cx.stop_propagation();
            }
            "backspace" if self.marked_range.is_none() => {
                let query = self.pane.read(cx).search_query.clone();
                let cursor = self.cursor_offset().min(query.len());
                if self.selected_range.is_empty() {
                    let previous = previous_char_boundary(&query, cursor);
                    self.selected_range = previous..cursor;
                    self.selection_reversed = false;
                }
                self.replace_search_selection("", cx);
                cx.stop_propagation();
            }
            "delete" if self.marked_range.is_none() => {
                if self.selected_range.is_empty() {
                    let query = self.pane.read(cx).search_query.clone();
                    let cursor = self.cursor_offset().min(query.len());
                    self.selected_range = cursor..next_char_boundary(&query, cursor);
                    self.selection_reversed = false;
                }
                self.replace_search_selection("", cx);
                cx.stop_propagation();
            }
            "enter" if self.marked_range.is_none() => {
                self.pane.update(cx, |pane, cx| pane.activate_selected(cx));
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn on_copy_text(&mut self, _: &CopyFiles, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.pane.read(cx).search_active {
            return;
        }
        let query = self.pane.read(cx).search_query.clone();
        let range = clamp_char_range(&query, self.selected_range.clone());
        if !range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(query[range].to_string()));
        }
        cx.stop_propagation();
    }

    fn on_cut_text(&mut self, _: &CutFiles, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.pane.read(cx).search_active {
            return;
        }
        let query = self.pane.read(cx).search_query.clone();
        let range = clamp_char_range(&query, self.selected_range.clone());
        if !range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(query[range.clone()].to_string()));
            self.selected_range = range;
            self.selection_reversed = false;
            self.replace_search_selection("", cx);
        }
        cx.stop_propagation();
    }

    fn on_paste_text(&mut self, _: &PasteFiles, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.pane.read(cx).search_active {
            return;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_search_selection(&text.replace(['\n', '\r'], " "), cx);
        }
        cx.stop_propagation();
    }

    fn replace_search_selection(&mut self, new_text: &str, cx: &mut Context<Self>) {
        let query = self.pane.read(cx).search_query.clone();
        let range = clamp_char_range(&query, self.selected_range.clone());
        let mut replacement = String::with_capacity(query.len() + new_text.len());
        replacement.push_str(&query[..range.start]);
        replacement.push_str(new_text);
        replacement.push_str(&query[range.end..]);
        let cursor = range.start + new_text.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        self.pane
            .update(cx, |pane, cx| pane.set_search_query(replacement, cx));
    }

    fn offset_from_utf16(&self, offset: usize, cx: &App) -> usize {
        utf16_to_utf8_offset(&self.pane.read(cx).search_query, offset)
    }

    fn offset_to_utf16(&self, offset: usize, cx: &App) -> usize {
        utf8_to_utf16_offset(&self.pane.read(cx).search_query, offset)
    }

    fn range_from_utf16(&self, range: &Range<usize>, cx: &App) -> Range<usize> {
        self.offset_from_utf16(range.start, cx)..self.offset_from_utf16(range.end, cx)
    }

    fn range_to_utf16(&self, range: &Range<usize>, cx: &App) -> Range<usize> {
        self.offset_to_utf16(range.start, cx)..self.offset_to_utf16(range.end, cx)
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
        let query = self.pane.read(cx).search_query.clone();
        let cursor = self.cursor_offset().min(query.len());
        if extend_selection {
            let next = if move_left {
                previous_char_boundary(&query, cursor)
            } else {
                next_char_boundary(&query, cursor)
            };
            self.select_to(next, cx);
        } else if self.selected_range.is_empty() {
            let next = if move_left {
                previous_char_boundary(&query, cursor)
            } else {
                next_char_boundary(&query, cursor)
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
        if !self.pane.read(cx).search_active {
            self.activate_search(window, cx);
        } else {
            self.focus_handle.focus(window);
        }
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
}

impl Focusable for SearchBar {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for SearchBar {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let query = self.pane.read(cx).search_query.clone();
        let range = clamp_char_range(&query, self.range_from_utf16(&range_utf16, cx));
        actual_range.replace(self.range_to_utf16(&range, cx));
        Some(query[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range, cx),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range, cx))
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
            .map(|range| self.range_from_utf16(range, cx))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        self.selection_reversed = false;
        self.replace_search_selection(new_text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = self.pane.read(cx).search_query.clone();
        let range = clamp_char_range(
            &query,
            range_utf16
                .as_ref()
                .map(|range| self.range_from_utf16(range, cx))
                .or_else(|| self.marked_range.clone())
                .unwrap_or_else(|| self.selected_range.clone()),
        );
        let mut replacement = String::with_capacity(query.len() + new_text.len());
        replacement.push_str(&query[..range.start]);
        replacement.push_str(new_text);
        replacement.push_str(&query[range.end..]);

        let inserted = range.start..range.start + new_text.len();
        self.marked_range = (!new_text.is_empty()).then_some(inserted.clone());
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|selection| {
                let start = utf16_to_utf8_offset(new_text, selection.start);
                let end = utf16_to_utf8_offset(new_text, selection.end);
                inserted.start + start..inserted.start + end
            })
            .unwrap_or(inserted.end..inserted.end);
        self.selection_reversed = false;

        self.pane
            .update(cx, |pane, cx| pane.set_search_query(replacement, cx));
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
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.cursor_offset(), cx))
    }
}

struct SearchTextElement {
    input: Entity<SearchBar>,
}

struct SearchPrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    scroll: Pixels,
}

impl IntoElement for SearchTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SearchTextElement {
    type RequestLayoutState = ();
    type PrepaintState = SearchPrepaintState;

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
        let content: SharedString = input.pane.read(cx).search_query.clone().into();
        let selected_range = clamp_char_range(&content, input.selected_range.clone());
        let cursor = input.cursor_offset();
        let marked_range = input.marked_range.clone();
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
        let cursor_x = line.x_for_index(cursor);
        let visible_width = (bounds.size.width - px(2.0)).max(px(0.0));
        let scroll = (cursor_x - visible_width).max(px(0.0));
        let (selection, cursor) =
            selection_and_cursor(bounds, &line, selected_range, cursor_x, scroll);

        SearchPrepaintState {
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
        let line = prepaint.line.take().expect("search line was shaped");
        line.paint(
            point(bounds.left() - prepaint.scroll, bounds.top()),
            window.line_height(),
            window,
            cx,
        )
        .expect("search line should paint");
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

impl Render for SearchBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (active, query, scope, count, loading) = {
            let pane = self.pane.read(cx);
            (
                pane.search_active,
                pane.search_query.clone(),
                pane.search_scope,
                pane.search_result_count,
                pane.is_loading,
            )
        };
        if active && !self.was_active {
            self.selected_range = query.len()..query.len();
            self.selection_reversed = false;
            self.marked_range = None;
            self.focus_handle.focus(window);
        }
        self.was_active = active;
        let pane_for_scope = self.pane.clone();
        let pane_for_close = self.pane.clone();
        let input_entity = cx.entity();
        let query_is_empty = query.is_empty();
        let display_text: SharedString = if query_is_empty {
            if active {
                "输入文件名…".to_string()
            } else {
                "搜索当前文件夹".to_string()
            }
        } else {
            query
        }
        .into();

        div()
            .key_context("SearchInput")
            .flex()
            .items_center()
            .gap_1()
            .w(px(220.0))
            .min_w(px(150.0))
            .h(px(44.0))
            .flex_shrink()
            .px_2()
            .border_l_1()
            .border_b_1()
            .border_color(if active {
                theme::accent()
            } else {
                theme::border()
            })
            .bg(if active {
                theme::accent_soft().opacity(0.68)
            } else {
                theme::surface_subtle()
            })
            .track_focus(&self.focus_handle)
            .when(active, |bar| {
                bar.on_action(cx.listener(Self::on_copy_text))
                    .on_action(cx.listener(Self::on_cut_text))
                    .on_action(cx.listener(Self::on_paste_text))
                    .on_key_down(cx.listener(Self::on_key_down))
            })
            .child(
                div()
                    .id("pane-search-input")
                    .relative()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .flex_1()
                    .h(px(30.0))
                    .px_2()
                    .rounded_md()
                    .border(px(if active { 2.0 } else { 1.0 }))
                    .border_color(if active {
                        theme::accent()
                    } else {
                        theme::border_strong()
                    })
                    .bg(theme::surface())
                    .font_family("SF Mono")
                    .text_size(theme::font(10.0))
                    .text_color(if query_is_empty {
                        theme::text_tertiary()
                    } else {
                        theme::text_primary()
                    })
                    .hover(|style| style.border_color(theme::accent().opacity(0.72)))
                    .when(active, |input| input.shadow_sm())
                    .cursor_text()
                    .tooltip(delayed_tooltip("搜索当前面板 (⌘F)"))
                    .on_mouse_down(MouseButton::Left, {
                        let input_entity = input_entity.clone();
                        move |event, window, cx| {
                            input_entity
                                .update(cx, |input, cx| input.on_mouse_down(event, window, cx));
                        }
                    })
                    .on_mouse_move({
                        let input_entity = input_entity.clone();
                        move |event, window, cx| {
                            input_entity
                                .update(cx, |input, cx| input.on_mouse_move(event, window, cx));
                        }
                    })
                    .on_mouse_up(MouseButton::Left, {
                        let input_entity = input_entity.clone();
                        move |event, window, cx| {
                            input_entity
                                .update(cx, |input, cx| input.on_mouse_up(event, window, cx));
                        }
                    })
                    .child(
                        div()
                            .mr_2()
                            .text_color(if active {
                                theme::accent()
                            } else {
                                theme::text_secondary()
                            })
                            .child(if loading { "◌" } else { "⌕" }),
                    )
                    .child(
                        div()
                            .relative()
                            .min_w_0()
                            .flex_1()
                            .h(px(16.0))
                            .overflow_hidden()
                            .when(!active || query_is_empty, |text| {
                                text.child(
                                    div().absolute().size_full().truncate().child(display_text),
                                )
                            })
                            .when(active, |text| {
                                text.child(SearchTextElement {
                                    input: input_entity.clone(),
                                })
                            }),
                    )
                    .when(active, |input| {
                        input.on_mouse_up_out(MouseButton::Left, {
                            let input_entity = input_entity.clone();
                            move |event, window, cx| {
                                input_entity
                                    .update(cx, |input, cx| input.on_mouse_up(event, window, cx));
                            }
                        })
                    }),
            )
            .when(active, |bar| {
                bar.child(
                    div()
                        .id("search-scope")
                        .flex()
                        .items_center()
                        .justify_center()
                        .h(px(30.0))
                        .px_2()
                        .rounded_md()
                        .text_size(theme::font(8.0))
                        .text_color(theme::text_secondary())
                        .hover(|style| style.bg(theme::surface()))
                        .tooltip(delayed_tooltip(format!(
                            "切换当前文件夹 / 全盘搜索 · {count} 项"
                        )))
                        .on_click(move |_, _, cx| {
                            pane_for_scope.update(cx, |pane, cx| pane.toggle_search_scope(cx));
                        })
                        .child(scope.label()),
                )
                .child(
                    div()
                        .id("close-search")
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(28.0))
                        .rounded_md()
                        .text_color(theme::text_secondary())
                        .hover(|style| style.bg(theme::surface()))
                        .tooltip(delayed_tooltip("关闭搜索 (Esc)"))
                        .on_click(move |_, _, cx| {
                            pane_for_close.update(cx, |pane, cx| pane.exit_search(cx));
                        })
                        .child("×"),
                )
            })
    }
}

pub(super) fn utf16_to_utf8_offset(text: &str, offset: usize) -> usize {
    let mut utf8_offset = 0;
    let mut utf16_offset = 0;
    for character in text.chars() {
        if utf16_offset >= offset {
            break;
        }
        utf16_offset += character.len_utf16();
        utf8_offset += character.len_utf8();
    }
    utf8_offset
}

pub(super) fn utf8_to_utf16_offset(text: &str, offset: usize) -> usize {
    let offset = clamp_char_boundary(text, offset);
    text[..offset].encode_utf16().count()
}

fn clamp_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

pub(super) fn clamp_char_range(text: &str, range: Range<usize>) -> Range<usize> {
    let start = clamp_char_boundary(text, range.start);
    let end = clamp_char_boundary(text, range.end).max(start);
    start..end
}

pub(super) fn previous_char_boundary(text: &str, offset: usize) -> usize {
    text.char_indices()
        .rev()
        .find_map(|(index, _)| (index < offset).then_some(index))
        .unwrap_or(0)
}

pub(super) fn next_char_boundary(text: &str, offset: usize) -> usize {
    text.char_indices()
        .find_map(|(index, _)| (index > offset).then_some(index))
        .unwrap_or(text.len())
}

fn marked_text_runs(
    content_len: usize,
    marked_range: Option<Range<usize>>,
    run: TextRun,
) -> Vec<TextRun> {
    if let Some(marked_range) = marked_range {
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
    } else {
        vec![run]
    }
}

fn selection_and_cursor(
    bounds: Bounds<Pixels>,
    line: &ShapedLine,
    selected_range: Range<usize>,
    cursor_x: Pixels,
    scroll: Pixels,
) -> (Option<PaintQuad>, Option<PaintQuad>) {
    if selected_range.is_empty() {
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
    }
}

#[cfg(test)]
mod tests {
    use super::{
        clamp_char_range, next_char_boundary, previous_char_boundary, utf8_to_utf16_offset,
        utf16_to_utf8_offset,
    };

    #[test]
    fn converts_chinese_and_emoji_offsets_for_macos_input_methods() {
        let text = "文件😀a";
        assert_eq!(utf16_to_utf8_offset(text, 2), "文件".len());
        assert_eq!(utf16_to_utf8_offset(text, 4), "文件😀".len());
        assert_eq!(utf8_to_utf16_offset(text, "文件".len()), 2);
        assert_eq!(utf8_to_utf16_offset(text, "文件😀".len()), 4);
    }

    #[test]
    fn text_replacement_ranges_never_split_utf8_characters() {
        assert_eq!(clamp_char_range("中文", 1..5), 0..3);
    }

    #[test]
    fn cursor_boundaries_move_across_multibyte_characters() {
        let text = "a中😀b";
        assert_eq!(next_char_boundary(text, 1), 4);
        assert_eq!(next_char_boundary(text, 4), 8);
        assert_eq!(previous_char_boundary(text, 8), 4);
        assert_eq!(previous_char_boundary(text, 4), 1);
    }
}
