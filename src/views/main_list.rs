use super::{
    context_menu::ContextMenuView,
    search_bar::{clamp_char_range, utf8_to_utf16_offset, utf16_to_utf8_offset},
    tooltip::delayed_tooltip,
};
use crate::{
    actions::{CopyFiles, CutFiles, PasteFiles, RenameSelected},
    models::{
        FileDragPayload, FileItem, FileKind, FileOperationController, Model, Pane, SortMode,
        ViewMode,
    },
    services::ThumbnailEngine,
    theme,
};
use gpui::{
    AnyElement, App, Bounds, ClickEvent, ClipboardItem, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, FontWeight,
    GlobalElementId, Half, IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ObjectFit, PaintQuad, Pixels, Point, Render, RenderImage,
    ScrollWheelEvent, ShapedLine, SharedString, Style, StyledImage, Subscription, TextRun,
    UTF16Selection, UnderlineStyle, Window, deferred, div, fill, img, point, prelude::*, px,
    relative, size, uniform_list,
};
use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap},
    ops::Range,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

const DETAILS_ICON_WIDTH: f32 = 42.0;
const GRID_CARD_TARGET_WIDTH: f32 = 112.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DetailColumn {
    Name,
    Kind,
    Size,
    Modified,
}

#[derive(Clone, Copy, Debug)]
struct DetailColumnWidths {
    name: f32,
    kind: f32,
    size: f32,
    modified: f32,
}

impl Default for DetailColumnWidths {
    fn default() -> Self {
        Self {
            name: 150.0,
            kind: 84.0,
            size: 90.0,
            modified: 128.0,
        }
    }
}

impl DetailColumnWidths {
    fn width(self, column: DetailColumn) -> f32 {
        match column {
            DetailColumn::Name => self.name,
            DetailColumn::Kind => self.kind,
            DetailColumn::Size => self.size,
            DetailColumn::Modified => self.modified,
        }
    }

    fn minimum(column: DetailColumn) -> f32 {
        match column {
            DetailColumn::Name => 96.0,
            DetailColumn::Kind => 64.0,
            DetailColumn::Size => 68.0,
            DetailColumn::Modified => 104.0,
        }
    }

    fn resize(&mut self, column: DetailColumn, delta: f32) {
        let width = (self.width(column) + delta).max(Self::minimum(column));
        match column {
            DetailColumn::Name => self.name = width,
            DetailColumn::Kind => self.kind = width,
            DetailColumn::Size => self.size = width,
            DetailColumn::Modified => self.modified = width,
        }
    }

    fn total(self) -> f32 {
        DETAILS_ICON_WIDTH + self.name + self.kind + self.size + self.modified
    }
}

struct DragPreview {
    label: String,
    position: Point<Pixels>,
}

#[derive(Clone)]
struct MarqueeSelection {
    start: Point<Pixels>,
    current: Point<Pixels>,
    base_selection: BTreeSet<usize>,
}

impl Render for DragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let preview_size = size(px(172.0), px(40.0));
        div()
            .pl(self.position.x - preview_size.width.half())
            .pt(self.position.y - preview_size.height.half())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .w(preview_size.width)
                    .h(preview_size.height + px(4.0))
                    .px_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme::accent())
                    .bg(theme::surface().opacity(0.96))
                    .shadow_lg()
                    .text_size(theme::font(10.0))
                    .text_color(theme::text_primary())
                    .child("↗")
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .child(self.label.clone()),
                    )
                    .child(
                        div()
                            .text_size(theme::font(8.0))
                            .text_color(theme::text_tertiary())
                            .child("⌥ 复制"),
                    ),
            )
    }
}

struct RenameTextElement {
    input: Entity<MainListView>,
}

struct RenamePrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for RenameTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for RenameTextElement {
    type RequestLayoutState = ();
    type PrepaintState = RenamePrepaintState;

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
        let content: SharedString = input.pane.read(cx).rename_buffer.clone().into();
        let selected_range = clamp_char_range(&content, input.rename_selected_range.clone());
        let cursor = input.rename_cursor_offset();
        let marked_range = input.rename_marked_range.clone();
        let style = window.text_style();
        let run = TextRun {
            len: content.len(),
            font: style.font(),
            color: style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = marked_range {
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
        let cursor_x = line.x_for_index(cursor);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_x, bounds.top()),
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
                            bounds.left() + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    theme::accent_soft(),
                )),
                None,
            )
        };

        RenamePrepaintState {
            line: Some(line),
            cursor,
            selection,
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
        let line = prepaint.line.take().expect("rename line was shaped");
        line.paint(bounds.origin, window.line_height(), window, cx)
            .expect("rename line should paint");
        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.input.update(cx, |input, _| {
            input.rename_layout = Some(line);
            input.rename_bounds = Some(bounds);
        });
    }
}

pub struct MainListView {
    pane_index: usize,
    pane: Model<Pane>,
    operations: Entity<FileOperationController>,
    thumbnails: Entity<ThumbnailEngine>,
    context_menu: Entity<ContextMenuView>,
    focus_handle: FocusHandle,
    detail_column_widths: DetailColumnWidths,
    resizing_column: Option<DetailColumn>,
    resize_last_x: Option<Pixels>,
    marquee: Option<MarqueeSelection>,
    item_bounds: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
    visible_indices: Rc<RefCell<BTreeSet<usize>>>,
    viewport_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    grid_columns: usize,
    rename_selected_range: Range<usize>,
    rename_selection_reversed: bool,
    rename_marked_range: Option<Range<usize>>,
    rename_layout: Option<ShapedLine>,
    rename_bounds: Option<Bounds<Pixels>>,
    rename_is_selecting: bool,
    rename_blur_subscription: Option<Subscription>,
    last_rename_index: Option<usize>,
}

impl MainListView {
    pub fn new(
        pane_index: usize,
        pane: Model<Pane>,
        operations: Entity<FileOperationController>,
        thumbnails: Entity<ThumbnailEngine>,
        context_menu: Entity<ContextMenuView>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&pane, |_, _, cx| cx.notify()).detach();
        cx.observe(&operations, |_, _, cx| cx.notify()).detach();
        cx.observe(&thumbnails, |_, _, cx| cx.notify()).detach();
        Self {
            pane_index,
            pane,
            operations,
            thumbnails,
            context_menu,
            focus_handle: cx.focus_handle(),
            detail_column_widths: DetailColumnWidths::default(),
            resizing_column: None,
            resize_last_x: None,
            marquee: None,
            item_bounds: Rc::new(RefCell::new(HashMap::new())),
            visible_indices: Rc::new(RefCell::new(BTreeSet::new())),
            viewport_bounds: Rc::new(RefCell::new(None)),
            grid_columns: 4,
            rename_selected_range: 0..0,
            rename_selection_reversed: false,
            rename_marked_range: None,
            rename_layout: None,
            rename_bounds: None,
            rename_is_selecting: false,
            rename_blur_subscription: None,
            last_rename_index: None,
        }
    }

    fn file_icon(item: &FileItem) -> AnyElement {
        if item.kind == FileKind::Folder {
            return div()
                .relative()
                .w(px(30.0))
                .h(px(24.0))
                .child(
                    div()
                        .absolute()
                        .top(px(2.0))
                        .left(px(2.0))
                        .w(px(13.0))
                        .h(px(7.0))
                        .rounded_t_sm()
                        .bg(theme::folder().opacity(0.76)),
                )
                .child(
                    div()
                        .absolute()
                        .bottom(px(1.0))
                        .left(px(1.0))
                        .w(px(28.0))
                        .h(px(19.0))
                        .rounded(px(4.0))
                        .border_1()
                        .border_color(theme::folder().opacity(0.95))
                        .bg(theme::folder().opacity(0.82))
                        .child(
                            div()
                                .mt(px(3.0))
                                .mx(px(3.0))
                                .h(px(1.0))
                                .bg(theme::surface().opacity(0.32)),
                        ),
                )
                .into_any_element();
        }

        let (fallback_label, color) = match item.kind {
            FileKind::Application => ("APP", theme::file_purple()),
            FileKind::Executable => ("BIN", theme::text_secondary()),
            FileKind::Script => (">_", theme::terminal_foreground()),
            FileKind::Document => ("DOC", theme::file_blue()),
            FileKind::Image => ("IMG", theme::file_green()),
            FileKind::Archive => ("ZIP", theme::file_purple()),
            FileKind::Audio => ("AUD", theme::file_purple()),
            FileKind::Video => ("VID", theme::file_green()),
            FileKind::Model => ("3D", theme::file_blue()),
            FileKind::Other => ("FILE", theme::text_secondary()),
            FileKind::Folder => unreachable!(),
        };
        let label = item
            .extension
            .as_deref()
            .filter(|extension| extension.len() <= 4)
            .map(str::to_ascii_uppercase)
            .unwrap_or_else(|| fallback_label.to_string());

        div()
            .relative()
            .w(px(24.0))
            .h(px(29.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(color.opacity(0.7))
            .bg(theme::surface())
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .size(px(7.0))
                    .border_l_1()
                    .border_b_1()
                    .border_color(color.opacity(0.55))
                    .bg(color.opacity(0.16)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(3.0))
                    .right(px(3.0))
                    .bottom(px(3.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(9.0))
                    .rounded(px(2.0))
                    .bg(color.opacity(0.16))
                    .text_color(color)
                    .text_size(theme::font(5.5))
                    .font_weight(FontWeight::BOLD)
                    .child(label),
            )
            .into_any_element()
    }

    fn large_file_icon(item: &FileItem) -> AnyElement {
        if item.kind == FileKind::Folder {
            return div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(86.0))
                .child(
                    div()
                        .relative()
                        .w(px(72.0))
                        .h(px(55.0))
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .left(px(4.0))
                                .w(px(32.0))
                                .h(px(13.0))
                                .rounded_t_sm()
                                .bg(theme::folder().opacity(0.82)),
                        )
                        .child(
                            div()
                                .absolute()
                                .bottom_0()
                                .left_0()
                                .w_full()
                                .h(px(47.0))
                                .rounded_sm()
                                .border_1()
                                .border_color(theme::folder().opacity(0.88))
                                .bg(theme::folder()),
                        ),
                )
                .into_any_element();
        }

        let (label, color) = match item.kind {
            FileKind::Application => ("APP".to_string(), theme::file_purple()),
            FileKind::Executable => ("BIN".to_string(), theme::text_secondary()),
            FileKind::Script => (">_".to_string(), theme::terminal_foreground()),
            FileKind::Document => (
                item.extension
                    .as_deref()
                    .unwrap_or("DOC")
                    .to_ascii_uppercase(),
                theme::file_blue(),
            ),
            FileKind::Image => ("IMG".to_string(), theme::file_green()),
            FileKind::Archive => ("ZIP".to_string(), theme::file_purple()),
            FileKind::Audio => ("AUD".to_string(), theme::file_purple()),
            FileKind::Video => ("VID".to_string(), theme::file_green()),
            FileKind::Model => ("3D".to_string(), theme::file_blue()),
            FileKind::Other | FileKind::Folder => (
                item.extension
                    .as_deref()
                    .unwrap_or("FILE")
                    .to_ascii_uppercase(),
                theme::text_secondary(),
            ),
        };

        div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(86.0))
            .child(
                div()
                    .flex()
                    .items_end()
                    .justify_center()
                    .w(px(58.0))
                    .h(px(72.0))
                    .pb_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(color.opacity(0.78))
                    .bg(theme::surface_subtle())
                    .text_size(theme::font(9.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(color)
                    .child(label),
            )
            .into_any_element()
    }

    fn file_visual(item: &FileItem, thumbnail: Option<Arc<RenderImage>>, edge: f32) -> AnyElement {
        if matches!(item.kind, FileKind::Application | FileKind::Script) {
            return Self::program_icon(item.kind, edge);
        }

        if let Some(thumbnail) = thumbnail {
            let visual_edge = if edge < 64.0 { 30.0 } else { edge };
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(edge))
                .child(
                    div()
                        .size(px(visual_edge))
                        .overflow_hidden()
                        .rounded_md()
                        .bg(theme::surface_subtle())
                        .child(
                            img(thumbnail)
                                .size_full()
                                .object_fit(ObjectFit::Contain)
                                .with_loading(|| {
                                    div()
                                        .size_full()
                                        .bg(theme::surface_subtle())
                                        .into_any_element()
                                }),
                        ),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(edge))
                .child(if edge >= 64.0 {
                    Self::large_file_icon(item)
                } else {
                    Self::file_icon(item)
                })
                .into_any_element()
        }
    }

    fn program_icon(kind: FileKind, edge: f32) -> AnyElement {
        let is_large = edge >= 64.0;

        match kind {
            FileKind::Application => {
                let icon_edge = if is_large { 66.0 } else { 30.0 };
                let inset = if is_large { 8.0 } else { 3.0 };
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(edge))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(icon_edge))
                            .rounded(if is_large { px(14.0) } else { px(6.0) })
                            .border_1()
                            .border_color(theme::file_purple().opacity(0.9))
                            .bg(theme::file_purple().opacity(0.16))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(icon_edge - inset * 2.0))
                                    .rounded(if is_large { px(10.0) } else { px(4.0) })
                                    .bg(theme::file_purple())
                                    .text_color(theme::surface())
                                    .text_size(theme::font(if is_large { 29.0 } else { 13.0 }))
                                    .font_weight(FontWeight::BOLD)
                                    .child("A"),
                            ),
                    )
                    .into_any_element()
            }
            FileKind::Script => {
                let width = if is_large { 72.0 } else { 30.0 };
                let height = if is_large { 55.0 } else { 26.0 };
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(edge))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(width))
                            .h(px(height))
                            .rounded(if is_large { px(8.0) } else { px(4.0) })
                            .border_1()
                            .border_color(theme::terminal_foreground().opacity(0.48))
                            .bg(theme::terminal_background())
                            .text_color(theme::terminal_foreground())
                            .text_size(theme::font(if is_large { 20.0 } else { 10.0 }))
                            .font_weight(FontWeight::BOLD)
                            .child(">_"),
                    )
                    .into_any_element()
            }
            _ => unreachable!("program_icon only handles application and script kinds"),
        }
    }

    fn detail_column_header(
        &self,
        label: &'static str,
        column: DetailColumn,
        mode: Option<SortMode>,
        active_mode: SortMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane = self.pane.clone();
        let is_active = mode == Some(active_mode);
        let width = self.detail_column_widths.width(column);
        div()
            .id(("detail-column", column as usize))
            .relative()
            .flex()
            .items_center()
            .flex_shrink_0()
            .w(px(width))
            .h_full()
            .text_left()
            .child(
                div()
                    .id(("detail-sort", column as usize))
                    .flex()
                    .items_center()
                    .justify_start()
                    .gap_1()
                    .size_full()
                    .min_w_0()
                    .px_2()
                    .text_left()
                    .text_size(theme::font(10.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if is_active {
                        theme::accent()
                    } else {
                        theme::text_tertiary()
                    })
                    .when_some(mode, move |header, mode| {
                        header
                            .hover(|style| style.bg(theme::accent_soft()))
                            .tooltip(delayed_tooltip(format!("按{label}排序")))
                            .on_click(move |_, _, cx| {
                                pane.update(cx, |pane, cx| pane.set_sort_mode(mode, cx));
                            })
                    })
                    .child(div().min_w_0().truncate().child(label))
                    .when(is_active, |header| header.child("↓")),
            )
            .child(
                div()
                    .id(("column-resize", column as usize))
                    .absolute()
                    .top_0()
                    .right(px(-3.0))
                    .w(px(7.0))
                    .h_full()
                    .cursor(CursorStyle::ResizeColumn)
                    .tooltip(delayed_tooltip(format!("拖动调整“{label}”列宽")))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.resizing_column = Some(column);
                            this.resize_last_x = Some(event.position.x);
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.finish_column_resize()),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.finish_column_resize()),
                    )
                    .child(
                        div()
                            .mx(px(3.0))
                            .h_full()
                            .w(px(1.0))
                            .bg(theme::border_strong()),
                    )
                    .hover(|handle| handle.bg(theme::accent_soft())),
            )
            .into_any_element()
    }

    fn resize_column_on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !event.dragging() {
            return;
        }
        let Some(column) = self.resizing_column else {
            return;
        };
        let current_x = event.position.x;
        if let Some(last_x) = self.resize_last_x.replace(current_x) {
            self.detail_column_widths
                .resize(column, f32::from(current_x - last_x));
            cx.notify();
        }
    }

    fn finish_column_resize(&mut self) {
        self.resizing_column = None;
        self.resize_last_x = None;
    }

    fn begin_marquee(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window);
        self.pane.update(cx, |pane, cx| {
            if pane.rename_index.is_some() {
                pane.commit_rename(cx);
            }
        });
        let base_selection = if event.modifiers.platform {
            self.pane.read(cx).selected_indices.clone()
        } else {
            BTreeSet::new()
        };
        self.marquee = Some(MarqueeSelection {
            start: event.position,
            current: event.position,
            base_selection,
        });
        if !event.modifiers.platform {
            self.pane.update(cx, |pane, cx| {
                if !pane.selected_indices.is_empty() {
                    pane.clear_selection();
                    cx.notify();
                }
            });
        }
        cx.notify();
        cx.stop_propagation();
    }

    fn update_marquee(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !event.dragging() || self.marquee.is_none() {
            return;
        }
        let selection = self.marquee.as_mut().expect("marquee checked above");
        selection.current = event.position;
        let area = marquee_bounds(selection.start, selection.current);
        let item_bounds = self.item_bounds.borrow();
        let visible_indices = self.visible_indices.borrow();
        let mut indices = selection.base_selection.clone();
        indices.extend(visible_indices.iter().filter_map(|index| {
            item_bounds
                .get(index)
                .filter(|bounds| bounds.intersects(&area))
                .map(|_| *index)
        }));
        drop(visible_indices);
        drop(item_bounds);

        self.pane.update(cx, |pane, cx| {
            if pane.set_selection_indices(indices) {
                cx.notify();
            }
        });
        cx.notify();
        cx.stop_propagation();
    }

    fn on_pointer_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resize_column_on_mouse_move(event, window, cx);
        self.update_marquee(event, window, cx);
    }

    fn finish_pointer_interaction(&mut self, cx: &mut Context<Self>) {
        self.finish_column_resize();
        if self.marquee.take().is_some() {
            cx.notify();
        }
    }

    fn cancel_marquee(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(selection) = self.marquee.take() else {
            return false;
        };
        self.pane.update(cx, |pane, cx| {
            if pane.set_selection_indices(selection.base_selection) {
                cx.notify();
            }
        });
        cx.notify();
        true
    }

    fn marquee_overlay(&self) -> Option<AnyElement> {
        let selection = self.marquee.as_ref()?;
        let viewport = self.viewport_bounds.borrow().as_ref().copied()?;
        let bounds = marquee_bounds(selection.start, selection.current).intersect(&viewport);
        if bounds.size.width < px(3.0) || bounds.size.height < px(3.0) {
            return None;
        }

        Some(
            div()
                .absolute()
                .left(bounds.origin.x - viewport.origin.x)
                .top(bounds.origin.y - viewport.origin.y)
                .w(bounds.size.width)
                .h(bounds.size.height)
                .rounded_sm()
                .border_1()
                .border_color(theme::accent().opacity(0.88))
                .bg(theme::accent_soft().opacity(0.42))
                .into_any_element(),
        )
    }

    fn rename_editor(
        &self,
        _rename_buffer: &str,
        width: f32,
        input_entity: Entity<Self>,
    ) -> AnyElement {
        let mouse_down_input = input_entity.clone();
        let mouse_move_input = input_entity.clone();
        let mouse_up_input = input_entity.clone();
        let mouse_up_out_input = input_entity.clone();

        div()
            .id(("rename-editor", self.pane_index))
            .relative()
            .flex()
            .items_center()
            .min_w_0()
            .flex_shrink_0()
            .w(px(width))
            .h(px(30.0))
            .px_2()
            .rounded_sm()
            .border_1()
            .border_color(theme::accent())
            .bg(theme::surface())
            .font_family("SF Mono")
            .text_size(theme::font(11.0))
            .text_color(theme::text_primary())
            .cursor_text()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                move |event: &MouseDownEvent, window, cx| {
                    mouse_down_input.update(cx, |input, cx| {
                        input.on_rename_mouse_down(event, window, cx)
                    });
                    cx.stop_propagation();
                },
            )
            .on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
                mouse_move_input.update(cx, |input, cx| {
                    input.on_rename_mouse_move(event, window, cx)
                });
                cx.stop_propagation();
            })
            .on_mouse_up(
                MouseButton::Left,
                move |event: &MouseUpEvent, window, cx| {
                    mouse_up_input
                        .update(cx, |input, cx| input.on_rename_mouse_up(event, window, cx));
                    cx.stop_propagation();
                },
            )
            .on_mouse_up_out(
                MouseButton::Left,
                move |event: &MouseUpEvent, window, cx| {
                    mouse_up_out_input
                        .update(cx, |input, cx| input.on_rename_mouse_up(event, window, cx));
                    cx.stop_propagation();
                },
            )
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(RenameTextElement {
                input: input_entity,
            })
            .into_any_element()
    }

    fn name_cell(
        &self,
        item: &FileItem,
        renaming: bool,
        rename_buffer: &str,
        width: f32,
        input_entity: Entity<Self>,
    ) -> AnyElement {
        if renaming {
            self.rename_editor(rename_buffer, width, input_entity)
        } else {
            div()
                .min_w_0()
                .flex_shrink_0()
                .w(px(width))
                .px_2()
                .truncate()
                .text_left()
                .text_size(theme::font(12.0))
                .text_color(theme::text_primary())
                .child(item.name.clone())
                .into_any_element()
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_row(
        &self,
        index: usize,
        item: FileItem,
        is_selected: bool,
        is_cut: bool,
        renaming: bool,
        rename_buffer: String,
        selected_paths: Vec<std::path::PathBuf>,
        thumbnail: Option<Arc<RenderImage>>,
        input_entity: Entity<Self>,
    ) -> AnyElement {
        let formatted_size = item.formatted_size();
        let is_folder = item.is_dir;
        let widths = self.detail_column_widths;
        let pane = self.pane.clone();
        let focus_handle = self.focus_handle.clone();
        let context_focus_handle = self.focus_handle.clone();
        let context_menu = self.context_menu.clone();
        let item_bounds = self.item_bounds.clone();
        let pane_index = self.pane_index;
        let drag_paths = if is_selected && !selected_paths.is_empty() {
            selected_paths
        } else {
            vec![item.path.clone()]
        };
        let drag_payload = FileDragPayload {
            paths: drag_paths.clone(),
            source_pane_index: self.pane_index,
        };
        let drag_label = if drag_paths.len() == 1 {
            item.name.clone()
        } else {
            format!("{} 个项目", drag_paths.len())
        };

        div()
            .on_children_prepainted(move |bounds, _, _| {
                if let Some(bounds) = bounds.into_iter().reduce(|left, right| left.union(&right)) {
                    item_bounds.borrow_mut().insert(index, bounds);
                }
            })
            .id(("file-row", index))
            .flex()
            .items_center()
            .w_full()
            .min_w(px(widths.total()))
            .h(px(40.0))
            .border_b_1()
            .border_color(theme::surface_subtle())
            .bg(if is_selected {
                theme::accent_soft()
            } else {
                theme::surface()
            })
            .opacity(if is_cut { 0.5 } else { 1.0 })
            .when(is_folder, |row| row.cursor_default())
            .when(!is_folder, |row| row.cursor_move())
            .hover(|style| style.bg(theme::accent_soft().opacity(0.7)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                context_focus_handle.focus(window);
                context_menu.update(cx, |menu, cx| {
                    menu.show_for_item(pane_index, index, event.position, cx);
                });
                cx.stop_propagation();
            })
            .on_click(move |event: &ClickEvent, window, cx| {
                focus_handle.focus(window);
                let modifiers = event.modifiers();
                pane.update(cx, |pane, cx| {
                    if pane.rename_index.is_some() {
                        pane.commit_rename(cx);
                    }
                    pane.select(index, modifiers.platform, modifiers.shift);
                    if event.click_count() >= 2 {
                        pane.activate_selected(cx);
                    } else {
                        cx.notify();
                    }
                });
            })
            .on_drag(drag_payload, move |_, position, _, cx| {
                let label = drag_label.clone();
                cx.new(|_| DragPreview { label, position })
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .w(px(DETAILS_ICON_WIDTH))
                    .pl_2()
                    .child(Self::file_visual(&item, thumbnail, 34.0)),
            )
            .child(self.name_cell(&item, renaming, &rename_buffer, widths.name, input_entity))
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(widths.kind))
                    .px_2()
                    .truncate()
                    .text_left()
                    .text_size(theme::font(10.0))
                    .text_color(theme::text_secondary())
                    .child(file_type_label(&item)),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(widths.size))
                    .px_2()
                    .truncate()
                    .text_left()
                    .text_size(theme::font(10.0))
                    .text_color(theme::text_secondary())
                    .child(formatted_size),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(widths.modified))
                    .px_2()
                    .truncate()
                    .text_left()
                    .text_size(theme::font(10.0))
                    .text_color(theme::text_secondary())
                    .child(item.modified),
            )
            .child(div().min_w_0().flex_1())
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_grid_card(
        &self,
        index: usize,
        item: FileItem,
        is_selected: bool,
        is_cut: bool,
        renaming: bool,
        rename_buffer: String,
        selected_paths: Vec<PathBuf>,
        thumbnail: Option<Arc<RenderImage>>,
        input_entity: Entity<Self>,
    ) -> AnyElement {
        let is_folder = item.is_dir;
        let pane = self.pane.clone();
        let focus_handle = self.focus_handle.clone();
        let context_focus_handle = self.focus_handle.clone();
        let context_menu = self.context_menu.clone();
        let item_bounds = self.item_bounds.clone();
        let pane_index = self.pane_index;
        let file_name = item.name.clone();
        let name_element = if renaming {
            deferred(
                div()
                    .absolute()
                    .top(px(100.0))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(self.rename_editor(&rename_buffer, 104.0, input_entity)),
            )
            .with_priority(30)
            .into_any_element()
        } else if is_selected {
            deferred(
                div()
                    .absolute()
                    .top(px(100.0))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .w(px(104.0))
                            .min_h(px(28.0))
                            .px_1()
                            .py_1()
                            .rounded_sm()
                            .bg(theme::accent_soft().opacity(0.68))
                            .whitespace_normal()
                            .text_center()
                            .text_size(theme::font(11.0))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(theme::text_primary())
                            .child(file_name),
                    ),
            )
            .with_priority(20)
            .into_any_element()
        } else {
            div()
                .mt_2()
                .w_full()
                .h(px(34.0))
                .overflow_hidden()
                .whitespace_normal()
                .line_clamp(2)
                .text_ellipsis()
                .text_center()
                .px_1()
                .text_size(theme::font(11.0))
                .font_weight(FontWeight::NORMAL)
                .text_color(theme::text_primary())
                .child(file_name)
                .into_any_element()
        };
        let drag_paths = if is_selected && !selected_paths.is_empty() {
            selected_paths
        } else {
            vec![item.path.clone()]
        };
        let drag_payload = FileDragPayload {
            paths: drag_paths.clone(),
            source_pane_index: self.pane_index,
        };
        let drag_label = if drag_paths.len() == 1 {
            item.name.clone()
        } else {
            format!("{} 个项目", drag_paths.len())
        };
        let file_visual = div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(90.0))
            .rounded_sm()
            .border_1()
            .border_color(if is_selected {
                theme::accent()
            } else {
                theme::surface()
            })
            .bg(theme::surface())
            .child(Self::file_visual(&item, thumbnail, 86.0));

        div()
            .on_children_prepainted(move |bounds, _, _| {
                if let Some(bounds) = bounds.into_iter().reduce(|left, right| left.union(&right)) {
                    item_bounds.borrow_mut().insert(index, bounds);
                }
            })
            .id(("file-card", index))
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .min_w_0()
            .flex_1()
            .h(px(136.0))
            .px_1()
            .pt_2()
            .pb_1()
            .mx(px(2.0))
            .rounded_sm()
            .border_1()
            .border_color(theme::surface())
            .bg(theme::surface())
            .opacity(if is_cut { 0.5 } else { 1.0 })
            .when(is_folder, |card| card.cursor_default())
            .when(!is_folder, |card| card.cursor_move())
            .when(!is_selected, |card| {
                card.hover(|style| style.border_color(theme::accent().opacity(0.42)))
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                context_focus_handle.focus(window);
                context_menu.update(cx, |menu, cx| {
                    menu.show_for_item(pane_index, index, event.position, cx);
                });
                cx.stop_propagation();
            })
            .on_click(move |event: &ClickEvent, window, cx| {
                focus_handle.focus(window);
                let modifiers = event.modifiers();
                pane.update(cx, |pane, cx| {
                    if pane.rename_index.is_some() {
                        pane.commit_rename(cx);
                    }
                    pane.select(index, modifiers.platform, modifiers.shift);
                    if event.click_count() >= 2 {
                        pane.activate_selected(cx);
                    } else {
                        cx.notify();
                    }
                });
            })
            .on_drag(drag_payload, move |_, position, _, cx| {
                let label = drag_label.clone();
                cx.new(|_| DragPreview { label, position })
            })
            .child(file_visual)
            .child(name_element)
            .into_any_element()
    }

    fn rename_selected(
        &mut self,
        _: &RenameSelected,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane.update(cx, |pane, cx| {
            pane.commit_rename(cx);
            cx.notify();
        });
    }

    fn on_copy_rename(&mut self, _: &CopyFiles, _window: &mut Window, cx: &mut Context<Self>) {
        if self.pane.read(cx).rename_index.is_none() {
            return;
        }
        let value = self.pane.read(cx).rename_buffer.clone();
        let range = clamp_char_range(&value, self.rename_selected_range.clone());
        if !range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(value[range].to_string()));
        }
        cx.stop_propagation();
    }

    fn on_cut_rename(&mut self, _: &CutFiles, _window: &mut Window, cx: &mut Context<Self>) {
        if self.pane.read(cx).rename_index.is_none() {
            return;
        }
        let value = self.pane.read(cx).rename_buffer.clone();
        let range = clamp_char_range(&value, self.rename_selected_range.clone());
        if !range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(value[range.clone()].to_string()));
            self.rename_selected_range = range;
            self.rename_selection_reversed = false;
            self.replace_rename_selection("", cx);
        }
        cx.stop_propagation();
    }

    fn on_paste_rename(&mut self, _: &PasteFiles, _window: &mut Window, cx: &mut Context<Self>) {
        if self.pane.read(cx).rename_index.is_none() {
            return;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_rename_selection(&text.replace(['\n', '\r'], " "), cx);
        }
        cx.stop_propagation();
    }

    fn replace_rename_selection(&mut self, new_text: &str, cx: &mut Context<Self>) {
        let value = self.pane.read(cx).rename_buffer.clone();
        let range = clamp_char_range(&value, self.rename_selected_range.clone());
        let mut replacement = String::with_capacity(value.len() + new_text.len());
        replacement.push_str(&value[..range.start]);
        replacement.push_str(new_text);
        replacement.push_str(&value[range.end..]);
        let cursor = range.start + new_text.len();
        self.rename_selected_range = cursor..cursor;
        self.rename_selection_reversed = false;
        self.rename_marked_range = None;
        self.pane.update(cx, |pane, cx| {
            pane.set_rename_buffer(replacement);
            cx.notify();
        });
    }

    fn rename_offset_from_utf16(&self, offset: usize, cx: &App) -> usize {
        utf16_to_utf8_offset(&self.pane.read(cx).rename_buffer, offset)
    }

    fn rename_offset_to_utf16(&self, offset: usize, cx: &App) -> usize {
        utf8_to_utf16_offset(&self.pane.read(cx).rename_buffer, offset)
    }

    fn rename_range_from_utf16(&self, range: &Range<usize>, cx: &App) -> Range<usize> {
        self.rename_offset_from_utf16(range.start, cx)..self.rename_offset_from_utf16(range.end, cx)
    }

    fn rename_range_to_utf16(&self, range: &Range<usize>, cx: &App) -> Range<usize> {
        self.rename_offset_to_utf16(range.start, cx)..self.rename_offset_to_utf16(range.end, cx)
    }

    fn rename_cursor_offset(&self) -> usize {
        if self.rename_selection_reversed {
            self.rename_selected_range.start
        } else {
            self.rename_selected_range.end
        }
    }

    fn previous_rename_boundary(value: &str, offset: usize) -> usize {
        value
            .char_indices()
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next_rename_boundary(value: &str, offset: usize) -> usize {
        value
            .char_indices()
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(value.len())
    }

    fn move_rename_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.rename_selected_range = offset..offset;
        self.rename_selection_reversed = false;
        self.rename_marked_range = None;
        cx.notify();
    }

    fn select_rename_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.rename_selection_reversed {
            self.rename_selected_range.start = offset;
        } else {
            self.rename_selected_range.end = offset;
        }
        if self.rename_selected_range.end < self.rename_selected_range.start {
            self.rename_selection_reversed = !self.rename_selection_reversed;
            self.rename_selected_range =
                self.rename_selected_range.end..self.rename_selected_range.start;
        }
        self.rename_marked_range = None;
        cx.notify();
    }

    fn rename_index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (self.rename_bounds.as_ref(), self.rename_layout.as_ref())
        else {
            return self.rename_cursor_offset();
        };
        if position.x <= bounds.left() {
            return 0;
        }
        if position.x >= bounds.right() {
            return line.text.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn on_rename_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window);
        self.rename_is_selecting = true;
        let offset = self.rename_index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_rename_to(offset, cx);
        } else {
            self.move_rename_to(offset, cx);
        }
        cx.stop_propagation();
    }

    fn on_rename_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.rename_is_selecting && event.dragging() {
            let offset = self.rename_index_for_mouse_position(event.position);
            self.select_rename_to(offset, cx);
        }
        cx.stop_propagation();
    }

    fn on_rename_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rename_is_selecting = false;
        cx.stop_propagation();
    }

    fn move_rename_horizontally(
        &mut self,
        move_left: bool,
        extend_selection: bool,
        cx: &mut Context<Self>,
    ) {
        let value = self.pane.read(cx).rename_buffer.clone();
        let cursor = self.rename_cursor_offset().min(value.len());
        if extend_selection {
            let next = if move_left {
                Self::previous_rename_boundary(&value, cursor)
            } else {
                Self::next_rename_boundary(&value, cursor)
            };
            self.select_rename_to(next, cx);
        } else if self.rename_selected_range.is_empty() {
            let next = if move_left {
                Self::previous_rename_boundary(&value, cursor)
            } else {
                Self::next_rename_boundary(&value, cursor)
            };
            self.move_rename_to(next, cx);
        } else {
            let next = if move_left {
                self.rename_selected_range.start
            } else {
                self.rename_selected_range.end
            };
            self.move_rename_to(next, cx);
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" && self.cancel_marquee(cx) {
            return;
        }
        let renaming = self.pane.read(cx).rename_index.is_some();
        if renaming {
            if event.keystroke.modifiers.platform && event.keystroke.key == "a" {
                let value_len = self.pane.read(cx).rename_buffer.len();
                self.rename_selected_range = 0..value_len;
                self.rename_selection_reversed = false;
                self.rename_marked_range = None;
                cx.notify();
                cx.stop_propagation();
                return;
            }
            match event.keystroke.key.as_str() {
                "left" if self.rename_marked_range.is_none() => {
                    self.move_rename_horizontally(true, event.keystroke.modifiers.shift, cx);
                    cx.stop_propagation();
                }
                "right" if self.rename_marked_range.is_none() => {
                    self.move_rename_horizontally(false, event.keystroke.modifiers.shift, cx);
                    cx.stop_propagation();
                }
                "home" if self.rename_marked_range.is_none() => {
                    if event.keystroke.modifiers.shift {
                        self.select_rename_to(0, cx);
                    } else {
                        self.move_rename_to(0, cx);
                    }
                    cx.stop_propagation();
                }
                "end" if self.rename_marked_range.is_none() => {
                    let end = self.pane.read(cx).rename_buffer.len();
                    if event.keystroke.modifiers.shift {
                        self.select_rename_to(end, cx);
                    } else {
                        self.move_rename_to(end, cx);
                    }
                    cx.stop_propagation();
                }
                "enter" if self.rename_marked_range.is_none() => {
                    self.pane.update(cx, |pane, cx| pane.commit_rename(cx));
                    cx.stop_propagation();
                }
                "escape" => self.pane.update(cx, |pane, cx| {
                    pane.cancel_rename();
                    cx.notify();
                }),
                "backspace" if self.rename_marked_range.is_none() => {
                    let value = self.pane.read(cx).rename_buffer.clone();
                    let cursor = self.rename_selected_range.end.min(value.len());
                    if self.rename_selected_range.is_empty() {
                        let previous = value[..cursor]
                            .char_indices()
                            .next_back()
                            .map(|(index, _)| index)
                            .unwrap_or(0);
                        self.rename_selected_range = previous..cursor;
                    }
                    self.replace_rename_selection("", cx);
                    cx.stop_propagation();
                }
                "delete" if self.rename_marked_range.is_none() => {
                    if self.rename_selected_range.is_empty() {
                        let value = self.pane.read(cx).rename_buffer.clone();
                        let cursor = self.rename_cursor_offset().min(value.len());
                        let next = Self::next_rename_boundary(&value, cursor);
                        self.rename_selected_range = cursor..next;
                        self.rename_selection_reversed = false;
                    }
                    self.replace_rename_selection("", cx);
                    cx.stop_propagation();
                }
                _ => {}
            }
            return;
        }

        match event.keystroke.key.as_str() {
            "enter" => self.pane.update(cx, |pane, cx| {
                pane.begin_rename();
                cx.notify();
            }),
            "up" => self.pane.update(cx, |pane, cx| {
                pane.move_selection(-1);
                cx.notify();
            }),
            "down" => self.pane.update(cx, |pane, cx| {
                pane.move_selection(1);
                cx.notify();
            }),
            _ => {}
        }
    }
}

impl Focusable for MainListView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for MainListView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        self.pane.read(cx).rename_index?;
        let value = self.pane.read(cx).rename_buffer.clone();
        let range = clamp_char_range(&value, self.rename_range_from_utf16(&range_utf16, cx));
        actual_range.replace(self.rename_range_to_utf16(&range, cx));
        Some(value[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        self.pane.read(cx).rename_index.map(|_| UTF16Selection {
            range: self.rename_range_to_utf16(&self.rename_selected_range, cx),
            reversed: self.rename_selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.rename_marked_range
            .as_ref()
            .map(|range| self.rename_range_to_utf16(range, cx))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.rename_marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rename_selected_range = range_utf16
            .as_ref()
            .map(|range| self.rename_range_from_utf16(range, cx))
            .or_else(|| self.rename_marked_range.clone())
            .unwrap_or_else(|| self.rename_selected_range.clone());
        self.replace_rename_selection(new_text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = self.pane.read(cx).rename_buffer.clone();
        let range = clamp_char_range(
            &value,
            range_utf16
                .as_ref()
                .map(|range| self.rename_range_from_utf16(range, cx))
                .or_else(|| self.rename_marked_range.clone())
                .unwrap_or_else(|| self.rename_selected_range.clone()),
        );
        let mut replacement = String::with_capacity(value.len() + new_text.len());
        replacement.push_str(&value[..range.start]);
        replacement.push_str(new_text);
        replacement.push_str(&value[range.end..]);

        let inserted = range.start..range.start + new_text.len();
        self.rename_marked_range = (!new_text.is_empty()).then_some(inserted.clone());
        self.rename_selected_range = new_selected_range_utf16
            .as_ref()
            .map(|selection| {
                let start = utf16_to_utf8_offset(new_text, selection.start);
                let end = utf16_to_utf8_offset(new_text, selection.end);
                inserted.start + start..inserted.start + end
            })
            .unwrap_or(inserted.end..inserted.end);
        self.rename_selection_reversed = false;

        self.pane.update(cx, |pane, cx| {
            pane.set_rename_buffer(replacement);
            cx.notify();
        });
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.rename_layout.as_ref()?;
        let range = self.rename_range_from_utf16(&range_utf16, cx);
        Some(Bounds::from_corners(
            point(
                element_bounds.left() + line.x_for_index(range.start),
                element_bounds.top(),
            ),
            point(
                element_bounds.left() + line.x_for_index(range.end),
                element_bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let index = self.rename_index_for_mouse_position(point);
        Some(self.rename_offset_to_utf16(index, cx))
    }
}

impl Render for MainListView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.rename_blur_subscription.is_none() {
            self.rename_blur_subscription =
                Some(cx.on_blur(&self.focus_handle, window, |this, _window, cx| {
                    if this.pane.read(cx).rename_index.is_some() {
                        this.pane.update(cx, |pane, cx| pane.commit_rename(cx));
                    }
                }));
        }
        let (items, rename_index, sort_mode, is_loading, view_mode) = {
            let pane = self.pane.read(cx);
            (
                pane.items.clone(),
                pane.rename_index,
                pane.sort_mode,
                pane.is_loading,
                pane.view_mode,
            )
        };
        let key_context = if rename_index.is_some() {
            "RenameInput"
        } else {
            "FileList"
        };
        if rename_index != self.last_rename_index {
            self.rename_marked_range = None;
            self.rename_layout = None;
            self.rename_bounds = None;
            self.rename_is_selecting = false;
            if let Some(index) = rename_index {
                self.rename_selected_range = items
                    .get(index)
                    .map(initial_rename_selection)
                    .unwrap_or_else(|| {
                        let cursor = self.pane.read(cx).rename_buffer.len();
                        cursor..cursor
                    });
                self.rename_selection_reversed = false;
                self.focus_handle.focus(window);
            } else {
                self.rename_selected_range = 0..0;
                self.rename_selection_reversed = false;
            }
            self.last_rename_index = rename_index;
        }
        let background_context_menu = self.context_menu.clone();
        let background_focus_handle = self.focus_handle.clone();
        let viewport_bounds = self.viewport_bounds.clone();
        let pane_index = self.pane_index;
        if items.is_empty() {
            self.thumbnails.update(cx, |engine, cx| {
                engine.set_visible(self.pane_index, &[], cx)
            });
        }

        let body = match view_mode {
            ViewMode::Details => {
                let item_count = items.len();
                uniform_list(
                    ("file-details", self.pane_index),
                    item_count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        this.visible_indices.borrow_mut().clear();
                        this.visible_indices.borrow_mut().extend(range.clone());
                        let visible = {
                            let pane = this.pane.read(cx);
                            pane.items[range.clone()].to_vec()
                        };
                        this.thumbnails.update(cx, |engine, cx| {
                            engine.set_visible(this.pane_index, &visible, cx)
                        });
                        visible
                            .into_iter()
                            .enumerate()
                            .map(|(offset, item)| {
                                let index = range.start + offset;
                                let (is_selected, selected_paths, rename_index, rename_buffer) = {
                                    let pane = this.pane.read(cx);
                                    (
                                        pane.selected_indices.contains(&index),
                                        pane.selected_paths(),
                                        pane.rename_index,
                                        pane.rename_buffer.clone(),
                                    )
                                };
                                let is_cut = this.operations.read(cx).is_cut_path(&item.path);
                                let thumbnail = this
                                    .thumbnails
                                    .update(cx, |engine, _| engine.image_for(&item));
                                this.render_row(
                                    index,
                                    item,
                                    is_selected,
                                    is_cut,
                                    rename_index == Some(index),
                                    rename_buffer,
                                    selected_paths,
                                    thumbnail,
                                    cx.entity(),
                                )
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .size_full()
                .into_any_element()
            }
            ViewMode::Grid => {
                let columns = self.grid_columns;
                let row_count = items.len().div_ceil(columns);
                uniform_list(
                    ("file-grid", self.pane_index),
                    row_count,
                    cx.processor(move |this, rows: std::ops::Range<usize>, _window, cx| {
                        let item_range = (rows.start * columns)
                            ..(rows.end * columns).min(this.pane.read(cx).items.len());
                        this.visible_indices.borrow_mut().clear();
                        this.visible_indices.borrow_mut().extend(item_range.clone());
                        let visible = {
                            let pane = this.pane.read(cx);
                            pane.items[item_range.clone()].to_vec()
                        };
                        this.thumbnails.update(cx, |engine, cx| {
                            engine.set_visible(this.pane_index, &visible, cx)
                        });

                        rows.map(|row| {
                            let first = row * columns;
                            let mut row_element =
                                div().flex().items_start().w_full().h(px(144.0)).p_1();
                            for index in first..(first + columns) {
                                let item = this.pane.read(cx).items.get(index).cloned();
                                if let Some(item) = item {
                                    let (is_selected, rename_index, rename_buffer, selected_paths) = {
                                        let pane = this.pane.read(cx);
                                        (
                                            pane.selected_indices.contains(&index),
                                            pane.rename_index,
                                            pane.rename_buffer.clone(),
                                            pane.selected_paths(),
                                        )
                                    };
                                    let is_cut = this.operations.read(cx).is_cut_path(&item.path);
                                    let thumbnail = this
                                        .thumbnails
                                        .update(cx, |engine, _| engine.image_for(&item));
                                    row_element = row_element.child(this.render_grid_card(
                                        index,
                                        item,
                                        is_selected,
                                        is_cut,
                                        rename_index == Some(index),
                                        rename_buffer,
                                        selected_paths,
                                        thumbnail,
                                        cx.entity(),
                                    ));
                                } else {
                                    row_element = row_element.child(div().flex_1().mx_1());
                                }
                            }
                            row_element
                        })
                        .collect::<Vec<_>>()
                    }),
                )
                .size_full()
                .into_any_element()
            }
        };

        let details_width = self.detail_column_widths.total();
        let details_header = if view_mode == ViewMode::Details {
            let name = self.detail_column_header(
                "名称",
                DetailColumn::Name,
                Some(SortMode::Name),
                sort_mode,
                cx,
            );
            let kind = self.detail_column_header("类型", DetailColumn::Kind, None, sort_mode, cx);
            let size = self.detail_column_header(
                "大小",
                DetailColumn::Size,
                Some(SortMode::Size),
                sort_mode,
                cx,
            );
            let modified = self.detail_column_header(
                "修改日期",
                DetailColumn::Modified,
                Some(SortMode::Modified),
                sort_mode,
                cx,
            );
            Some(
                div()
                    .flex()
                    .items_center()
                    .w_full()
                    .min_w(px(details_width))
                    .h(px(32.0))
                    .border_b_1()
                    .border_color(theme::border())
                    .bg(theme::surface_subtle())
                    .text_left()
                    .child(div().flex_shrink_0().w(px(DETAILS_ICON_WIDTH)).h_full())
                    .child(name)
                    .child(kind)
                    .child(size)
                    .child(modified)
                    .child(div().min_w_0().flex_1()),
            )
        } else {
            None
        };
        let marquee_overlay = self.marquee_overlay();
        let this = cx.weak_entity();

        div()
            .on_children_prepainted(move |bounds, _, cx| {
                if let Some(bounds) = bounds.last() {
                    *viewport_bounds.borrow_mut() = Some(*bounds);
                    if view_mode == ViewMode::Grid {
                        let columns = grid_columns_for_width(bounds.size.width);
                        let this = this.clone();
                        cx.defer(move |cx| {
                            let _ = this.update(cx, |this, cx| {
                                if this.grid_columns != columns {
                                    this.grid_columns = columns;
                                    cx.notify();
                                }
                            });
                        });
                    }
                }
            })
            .id("main-file-list")
            .key_context(key_context)
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .bg(theme::surface())
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::rename_selected))
            .on_action(cx.listener(Self::on_copy_rename))
            .on_action(cx.listener(Self::on_cut_rename))
            .on_action(cx.listener(Self::on_paste_rename))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_move(cx.listener(Self::on_pointer_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.finish_pointer_interaction(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.finish_pointer_interaction(cx)),
            )
            .when(view_mode == ViewMode::Details, |mut root| {
                // GPUI otherwise redirects a vertical wheel delta into this
                // horizontal-only scroller, which makes detail rows drift left
                // and right while the inner uniform list scrolls vertically.
                root.style().restrict_scroll_to_axis = Some(true);
                root.overflow_x_scroll().on_scroll_wheel(|event, _, cx| {
                    // A trackpad commonly reports a tiny X component during a
                    // vertical gesture. Let the inner list consume that gesture,
                    // but keep genuinely horizontal gestures available here.
                    if is_vertical_scroll(event) {
                        cx.stop_propagation();
                    }
                })
            })
            .when_some(details_header, |root, header| root.child(header))
            .child(
                div()
                    .id("file-list-scroll")
                    .relative()
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .flex_1()
                    .when(view_mode == ViewMode::Details, |list| {
                        list.w_full().min_w(px(details_width))
                    })
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::begin_marquee))
                    .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                        background_focus_handle.focus(window);
                        background_context_menu.update(cx, |menu, cx| {
                            menu.show_for_background(pane_index, event.position, cx);
                        });
                        cx.stop_propagation();
                    })
                    .child(body)
                    .when_some(marquee_overlay, |list, overlay| list.child(overlay))
                    .when(is_loading, |list| {
                        list.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .h(px(48.0))
                                .text_size(theme::font(10.0))
                                .text_color(theme::text_tertiary())
                                .child("正在读取目录…"),
                        )
                    }),
            )
    }
}

fn is_vertical_scroll(event: &ScrollWheelEvent) -> bool {
    let delta = event.delta.pixel_delta(px(1.0));
    delta.y.abs() > delta.x.abs()
}

fn marquee_bounds(first: Point<Pixels>, second: Point<Pixels>) -> Bounds<Pixels> {
    let left = if first.x < second.x {
        first.x
    } else {
        second.x
    };
    let right = if first.x > second.x {
        first.x
    } else {
        second.x
    };
    let top = if first.y < second.y {
        first.y
    } else {
        second.y
    };
    let bottom = if first.y > second.y {
        first.y
    } else {
        second.y
    };
    Bounds::from_corners(point(left, top), point(right, bottom))
}

fn grid_columns_for_width(width: Pixels) -> usize {
    (f32::from(width) / GRID_CARD_TARGET_WIDTH).floor().max(1.0) as usize
}

fn file_type_label(item: &FileItem) -> String {
    match item.kind {
        FileKind::Application => return "应用程序".to_string(),
        FileKind::Executable => return "Unix 可执行程序".to_string(),
        FileKind::Script => return "脚本".to_string(),
        FileKind::Folder => return "文件夹".to_string(),
        _ => {}
    }
    item.extension
        .as_deref()
        .map(|extension| extension.to_ascii_uppercase())
        .unwrap_or_else(|| "文件".to_string())
}

fn initial_rename_selection(item: &FileItem) -> Range<usize> {
    if item.is_dir {
        return 0..item.name.len();
    }

    let extension_start = item
        .name
        .rfind('.')
        .filter(|index| *index > 0)
        .unwrap_or(item.name.len());
    0..extension_start
}

#[cfg(test)]
mod grid_name_tests {
    use super::{
        DetailColumn, DetailColumnWidths, MainListView, grid_columns_for_width,
        initial_rename_selection, is_vertical_scroll, marquee_bounds,
    };
    use crate::models::{FileItem, FileKind};
    use gpui::{Modifiers, ScrollDelta, ScrollWheelEvent, TouchPhase, point, px};
    use std::path::PathBuf;

    #[test]
    fn detail_columns_resize_independently_and_keep_minimum_widths() {
        let mut widths = DetailColumnWidths::default();
        let original_kind = widths.kind;

        widths.resize(DetailColumn::Name, 24.0);
        assert_eq!(widths.name, 174.0);
        assert_eq!(widths.kind, original_kind);

        widths.resize(DetailColumn::Size, -500.0);
        assert_eq!(widths.size, DetailColumnWidths::minimum(DetailColumn::Size));
    }

    #[test]
    fn detail_scroll_ignores_trackpad_cross_axis_noise() {
        let vertical = ScrollWheelEvent {
            delta: ScrollDelta::Pixels(point(px(0.7), px(12.0))),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
            ..Default::default()
        };
        let horizontal = ScrollWheelEvent {
            delta: ScrollDelta::Pixels(point(px(12.0), px(0.7))),
            ..vertical.clone()
        };

        assert!(is_vertical_scroll(&vertical));
        assert!(!is_vertical_scroll(&horizontal));
    }

    #[test]
    fn marquee_bounds_support_dragging_in_every_direction() {
        let forward = marquee_bounds(point(px(12.0), px(18.0)), point(px(72.0), px(54.0)));
        let reverse = marquee_bounds(point(px(72.0), px(54.0)), point(px(12.0), px(18.0)));

        assert_eq!(forward, reverse);
        assert_eq!(forward.origin, point(px(12.0), px(18.0)));
        assert_eq!(forward.size.width, px(60.0));
        assert_eq!(forward.size.height, px(36.0));
    }

    #[test]
    fn grid_column_count_tracks_available_pane_width() {
        assert_eq!(grid_columns_for_width(px(80.0)), 1);
        assert_eq!(grid_columns_for_width(px(450.0)), 4);
        assert_eq!(grid_columns_for_width(px(900.0)), 8);
    }

    #[test]
    fn file_rename_selects_the_stem_and_preserves_the_extension() {
        let file = FileItem {
            path: PathBuf::from("/tmp/archive.tar.gz"),
            name: "archive.tar.gz".to_string(),
            is_dir: false,
            extension: Some("gz".to_string()),
            size: 1,
            modified_unix: 0,
            modified: String::new(),
            is_hidden: false,
            kind: FileKind::Archive,
        };
        let hidden_file = FileItem {
            name: ".env".to_string(),
            path: PathBuf::from("/tmp/.env"),
            extension: None,
            kind: FileKind::Other,
            ..file.clone()
        };

        assert_eq!(initial_rename_selection(&file), 0..11);
        assert_eq!(initial_rename_selection(&hidden_file), 0..4);
    }

    #[test]
    fn rename_cursor_moves_across_multibyte_characters() {
        let value = "A中文😀B";
        let after_emoji = value.find('B').unwrap();

        assert_eq!(MainListView::next_rename_boundary(value, 0), 1);
        assert_eq!(MainListView::next_rename_boundary(value, 1), 4);
        assert_eq!(
            MainListView::previous_rename_boundary(value, after_emoji),
            value.find('😀').unwrap()
        );
    }
}
