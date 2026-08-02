use super::tooltip::delayed_tooltip;
use crate::{models::Model, models::Pane, theme};
use gpui::{
    App, Bounds, Context, ElementInputHandler, EntityInputHandler, FocusHandle, Focusable,
    IntoElement, KeyDownEvent, Pixels, Point, Render, SharedString, UTF16Selection, Window, canvas,
    div, prelude::*, px,
};
use std::ops::Range;

pub struct SearchBar {
    pane: Model<Pane>,
    focus_handle: FocusHandle,
    was_active: bool,
    selected_range: Range<usize>,
    marked_range: Option<Range<usize>>,
}

impl SearchBar {
    pub fn new(pane: Model<Pane>, cx: &mut Context<Self>) -> Self {
        cx.observe(&pane, |_, _, cx| cx.notify()).detach();
        Self {
            pane,
            focus_handle: cx.focus_handle(),
            was_active: false,
            selected_range: 0..0,
            marked_range: None,
        }
    }

    fn activate_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pane.update(cx, |pane, cx| pane.begin_search(cx));
        let query_len = self.pane.read(cx).search_query.len();
        self.selected_range = query_len..query_len;
        self.marked_range = None;
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.pane.update(cx, |pane, cx| pane.exit_search(cx)),
            "backspace" if self.marked_range.is_none() => {
                let query = self.pane.read(cx).search_query.clone();
                let cursor = self.selected_range.end.min(query.len());
                if self.selected_range.is_empty() {
                    let previous = query[..cursor]
                        .char_indices()
                        .next_back()
                        .map(|(index, _)| index)
                        .unwrap_or(0);
                    self.selected_range = previous..cursor;
                }
                self.replace_search_selection("", cx);
            }
            "enter" if self.marked_range.is_none() => {
                self.pane.update(cx, |pane, cx| pane.activate_selected(cx))
            }
            _ => {}
        }
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
            reversed: false,
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
        Some(self.offset_to_utf16(self.selected_range.end, cx))
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
            self.marked_range = None;
            self.focus_handle.focus(window);
        }
        self.was_active = active;
        let pane_for_scope = self.pane.clone();
        let pane_for_close = self.pane.clone();
        let input_entity = cx.entity();
        let input_focus = self.focus_handle.clone();
        let prepaint_focus = self.focus_handle.clone();
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
                bar.on_key_down(cx.listener(Self::on_key_down))
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
                    .tooltip(delayed_tooltip("搜索当前面板 (⌘F)"))
                    .on_click(cx.listener(|this, _, window, cx| this.activate_search(window, cx)))
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
                    .child(div().min_w_0().flex_1().truncate().child(display_text))
                    .when(active, |input| {
                        input.child(div().w(px(1.0)).h(px(14.0)).bg(theme::accent()))
                    })
                    .when(active, |input| {
                        input.child(
                            canvas(
                                move |_, window, cx| {
                                    window.set_focus_handle(&prepaint_focus, cx);
                                },
                                move |bounds, _, window, cx| {
                                    window.handle_input(
                                        &input_focus,
                                        ElementInputHandler::new(bounds, input_entity),
                                        cx,
                                    );
                                },
                            )
                            .absolute()
                            .size_full(),
                        )
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

#[cfg(test)]
mod tests {
    use super::{clamp_char_range, utf8_to_utf16_offset, utf16_to_utf8_offset};

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
}
