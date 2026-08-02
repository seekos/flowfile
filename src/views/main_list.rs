use super::{
    context_menu::ContextMenuView,
    search_bar::{clamp_char_range, utf8_to_utf16_offset, utf16_to_utf8_offset},
    tooltip::delayed_tooltip,
};
use crate::{
    actions::RenameSelected,
    models::{
        FileDragPayload, FileItem, FileKind, FileOperationController, Model, Pane, SortMode,
        ViewMode,
    },
    services::ThumbnailEngine,
    theme,
};
use gpui::{
    AnyElement, App, Bounds, ClickEvent, Context, CursorStyle, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, FontWeight, Half, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, Pixels, Point, Render, RenderImage,
    SharedString, StyledImage, UTF16Selection, Window, canvas, deferred, div, img, point,
    prelude::*, px, size, uniform_list,
};
use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap},
    ops::Range,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

const DETAILS_ICON_WIDTH: f32 = 40.0;
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
    rename_marked_range: Option<Range<usize>>,
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
            rename_marked_range: None,
            last_rename_index: None,
        }
    }

    fn file_icon(item: &FileItem) -> AnyElement {
        let (label, color) = match item.kind {
            FileKind::Folder => ("▰", theme::folder()),
            FileKind::Document => ("DOC", theme::file_blue()),
            FileKind::Image => ("IMG", theme::file_green()),
            FileKind::Archive => ("ZIP", theme::file_purple()),
            FileKind::Audio => ("AUD", theme::file_purple()),
            FileKind::Video => ("VID", theme::file_green()),
            FileKind::Other => ("FILE", theme::text_secondary()),
        };

        div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(32.0))
            .h(px(26.0))
            .rounded_sm()
            .bg(color.opacity(0.13))
            .text_color(color)
            .text_size(theme::font(if item.is_dir { 17.0 } else { 8.0 }))
            .font_weight(FontWeight::BOLD)
            .child(label)
            .into_any_element()
    }

    fn large_file_icon(item: &FileItem) -> AnyElement {
        if item.is_dir {
            return div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(82.0))
                .child(
                    div()
                        .relative()
                        .w(px(68.0))
                        .h(px(52.0))
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .left(px(4.0))
                                .w(px(30.0))
                                .h(px(12.0))
                                .rounded_t_sm()
                                .bg(theme::folder().opacity(0.82)),
                        )
                        .child(
                            div()
                                .absolute()
                                .bottom_0()
                                .left_0()
                                .w_full()
                                .h(px(44.0))
                                .rounded_sm()
                                .border_1()
                                .border_color(theme::folder().opacity(0.88))
                                .bg(theme::folder()),
                        ),
                )
                .into_any_element();
        }

        let (label, color) = match item.kind {
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
            .size(px(82.0))
            .child(
                div()
                    .flex()
                    .items_end()
                    .justify_center()
                    .w(px(54.0))
                    .h(px(68.0))
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
        if let Some(thumbnail) = thumbnail {
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(edge))
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
        rename_buffer: &str,
        width: f32,
        input_entity: Entity<Self>,
    ) -> AnyElement {
        let value: SharedString = rename_buffer.to_string().into();
        let input_focus = self.focus_handle.clone();
        let prepaint_focus = self.focus_handle.clone();

        div()
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
            .child(div().min_w_0().truncate().child(value))
            .child(div().w(px(1.0)).h(px(15.0)).bg(theme::accent()))
            .child(
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
            .h(px(38.0))
            .border_b_1()
            .border_color(theme::surface_subtle())
            .bg(if is_selected {
                theme::accent_soft()
            } else {
                theme::surface()
            })
            .opacity(if is_cut { 0.5 } else { 1.0 })
            .cursor_move()
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
                    .child(Self::file_visual(&item, thumbnail, 32.0)),
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
        let pane = self.pane.clone();
        let focus_handle = self.focus_handle.clone();
        let context_focus_handle = self.focus_handle.clone();
        let context_menu = self.context_menu.clone();
        let item_bounds = self.item_bounds.clone();
        let pane_index = self.pane_index;
        let file_name = item.name.clone();
        let abbreviated_name = abbreviated_grid_name(&file_name, 16);
        let selected_name_width =
            (file_name.chars().count() as f32 * 7.0 + 16.0).clamp(36.0, 104.0);
        let name_element = if renaming {
            deferred(
                div()
                    .absolute()
                    .top(px(96.0))
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
                    .top(px(96.0))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .w(px(selected_name_width))
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
                .h(px(22.0))
                .truncate()
                .text_center()
                .text_size(theme::font(11.0))
                .font_weight(FontWeight::NORMAL)
                .text_color(theme::text_primary())
                .child(abbreviated_name)
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
            .size(px(86.0))
            .rounded_sm()
            .border_1()
            .border_color(if is_selected {
                theme::accent()
            } else {
                theme::surface()
            })
            .bg(theme::surface())
            .child(Self::file_visual(&item, thumbnail, 82.0));

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
            .h(px(132.0))
            .px_1()
            .pt_2()
            .pb_1()
            .mx(px(2.0))
            .rounded_sm()
            .border_1()
            .border_color(theme::surface())
            .bg(theme::surface())
            .opacity(if is_cut { 0.5 } else { 1.0 })
            .cursor_move()
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

    fn replace_rename_selection(&mut self, new_text: &str, cx: &mut Context<Self>) {
        let value = self.pane.read(cx).rename_buffer.clone();
        let range = clamp_char_range(&value, self.rename_selected_range.clone());
        let mut replacement = String::with_capacity(value.len() + new_text.len());
        replacement.push_str(&value[..range.start]);
        replacement.push_str(new_text);
        replacement.push_str(&value[range.end..]);
        let cursor = range.start + new_text.len();
        self.rename_selected_range = cursor..cursor;
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

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" && self.cancel_marquee(cx) {
            return;
        }
        let renaming = self.pane.read(cx).rename_index.is_some();
        if renaming {
            match event.keystroke.key.as_str() {
                "enter" if self.rename_marked_range.is_none() => {
                    self.pane.update(cx, |pane, cx| pane.commit_rename(cx));
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
        if self.pane.read(cx).rename_index.is_none() {
            return None;
        }
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
            reversed: false,
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

        self.pane.update(cx, |pane, cx| {
            pane.set_rename_buffer(replacement);
            cx.notify();
        });
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
        Some(self.rename_offset_to_utf16(self.rename_selected_range.end, cx))
    }
}

impl Render for MainListView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            if rename_index.is_some() {
                let cursor = self.pane.read(cx).rename_buffer.len();
                self.rename_selected_range = cursor..cursor;
                self.focus_handle.focus(window);
            } else {
                self.rename_selected_range = 0..0;
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
                                div().flex().items_start().w_full().h(px(140.0)).p_1();
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
            .when(view_mode == ViewMode::Details, |root| {
                root.overflow_x_scroll()
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
    if item.is_dir {
        return "文件夹".to_string();
    }
    item.extension
        .as_deref()
        .map(|extension| extension.to_ascii_uppercase())
        .unwrap_or_else(|| "文件".to_string())
}

fn abbreviated_grid_name(name: &str, max_units: usize) -> String {
    let display_units = |character: char| if character.is_ascii() { 1 } else { 2 };
    if name.chars().map(display_units).sum::<usize>() <= max_units {
        return name.to_string();
    }

    let mut abbreviated = String::new();
    let mut used = 0;
    for character in name.chars() {
        let units = display_units(character);
        if used + units > max_units.saturating_sub(1) {
            break;
        }
        abbreviated.push(character);
        used += units;
    }
    abbreviated.push('…');
    abbreviated
}

#[cfg(test)]
mod grid_name_tests {
    use super::{
        DetailColumn, DetailColumnWidths, abbreviated_grid_name, grid_columns_for_width,
        marquee_bounds,
    };
    use gpui::{point, px};

    #[test]
    fn unselected_grid_names_have_a_visible_ellipsis() {
        assert_eq!(
            abbreviated_grid_name("balenaEtcher-2.1.6-arm64.dmg", 16),
            "balenaEtcher-2.…"
        );
        assert_eq!(
            abbreviated_grid_name("超长中文文件名称.txt", 10),
            "超长中文…"
        );
        assert_eq!(abbreviated_grid_name("notes.txt", 16), "notes.txt");
    }

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
}
