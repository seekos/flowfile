use super::{
    tooltip::delayed_tooltip,
    workspace::{
        MAX_IMAGE_ZOOM, MIN_IMAGE_ZOOM, format_file_size, is_opposite_scroll_tail,
        scaled_image_size, zoom_from_scroll,
    },
};
use crate::{
    actions::{CloseQuickLook, ToggleQuickLook},
    models::{Model, Pane},
    services::{FileEngine, FileInspector, QuickLookService},
    theme,
};
use gpui::{
    AnyElement, App, Bounds, Context, Entity, FocusHandle, FontWeight, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, Render, ScrollWheelEvent,
    SharedString, TitlebarOptions, TouchPhase, Window, WindowBounds, WindowOptions, black, div,
    img, prelude::*, px, size,
};
use std::{path::PathBuf, time::Instant};

const DEFAULT_WINDOW_WIDTH: f32 = 1040.0;
const DEFAULT_WINDOW_HEIGHT: f32 = 760.0;
const MIN_WINDOW_WIDTH: f32 = 640.0;
const MIN_WINDOW_HEIGHT: f32 = 420.0;
const TOOLBAR_HEIGHT: f32 = 48.0;
const METADATA_WIDTH: f32 = 280.0;
const VIEWPORT_PADDING: f32 = 32.0;
const NAVIGATION_HOVER_WIDTH: f32 = 112.0;
const NAVIGATION_HOVER_HEIGHT: f32 = 180.0;
const NAVIGATION_BUTTON_SIZE: f32 = 64.0;

#[derive(Clone)]
enum ImageViewerContent {
    Loading,
    Image { display_path: PathBuf },
    Error(String),
}

#[derive(Clone)]
struct ImageViewerState {
    path: PathBuf,
    content: ImageViewerContent,
    image_paths: Vec<PathBuf>,
    image_index: usize,
    metadata_visible: bool,
    zoom: f32,
    pan_x: f32,
    pan_y: f32,
    last_pointer: Option<(f32, f32)>,
    image_dimensions: Option<(u32, u32)>,
    scroll_direction: f32,
    last_scroll_at: Option<Instant>,
}

pub(super) struct ImageViewerWindow {
    state: ImageViewerState,
    service: QuickLookService,
    inspector: Entity<FileInspector>,
    source_pane: Model<Pane>,
    focus_handle: FocusHandle,
    generation: u64,
}

impl ImageViewerWindow {
    pub(super) fn window_options(path: &std::path::Path, cx: &App) -> WindowOptions {
        let bounds = Bounds::centered(
            None,
            size(px(DEFAULT_WINDOW_WIDTH), px(DEFAULT_WINDOW_HEIGHT)),
            cx,
        );
        WindowOptions {
            focus: true,
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(MIN_WINDOW_WIDTH), px(MIN_WINDOW_HEIGHT))),
            app_id: Some("cc.bso.flowfile".to_string()),
            titlebar: Some(TitlebarOptions {
                title: Some(Self::window_title(path).into()),
                appears_transparent: false,
                traffic_light_position: None,
            }),
            is_movable: true,
            is_resizable: true,
            is_minimizable: true,
            ..Default::default()
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        path: PathBuf,
        image_paths: Vec<PathBuf>,
        image_index: usize,
        metadata_visible: bool,
        source_pane: Model<Pane>,
        service: QuickLookService,
        engine: FileEngine,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        theme::apply(
            crate::models::AppPreferences::load().theme,
            window.appearance(),
        );
        cx.observe_window_appearance(window, |_, window, cx| {
            theme::apply(
                crate::models::AppPreferences::load().theme,
                window.appearance(),
            );
            cx.notify();
        })
        .detach();

        let inspector = cx.new(|_| FileInspector::new(&engine));
        cx.observe(&inspector, |this, inspector, cx| {
            let inspected = inspector.read(cx);
            if inspected.current_path.as_ref() == Some(&this.state.path) {
                this.state.image_dimensions = inspected
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.dimensions);
            }
            cx.notify();
        })
        .detach();

        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        let mut viewer = Self {
            state: ImageViewerState {
                path: PathBuf::new(),
                content: ImageViewerContent::Loading,
                image_paths: Vec::new(),
                image_index: 0,
                metadata_visible: false,
                zoom: 1.0,
                pan_x: 0.0,
                pan_y: 0.0,
                last_pointer: None,
                image_dimensions: None,
                scroll_direction: 0.0,
                last_scroll_at: None,
            },
            service,
            inspector,
            source_pane: source_pane.clone(),
            focus_handle,
            generation: 0,
        };
        viewer.show_image(
            path,
            image_paths,
            image_index,
            metadata_visible,
            source_pane,
            window,
            cx,
        );
        viewer
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn show_image(
        &mut self,
        path: PathBuf,
        mut image_paths: Vec<PathBuf>,
        image_index: usize,
        metadata_visible: bool,
        source_pane: Model<Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if image_paths.is_empty() {
            image_paths.push(path.clone());
        }
        self.generation += 1;
        let generation = self.generation;
        let content = if QuickLookService::can_render_image_directly(&path) {
            ImageViewerContent::Image {
                display_path: path.clone(),
            }
        } else {
            ImageViewerContent::Loading
        };
        self.source_pane = source_pane;
        self.state = ImageViewerState {
            path: path.clone(),
            content,
            image_paths,
            image_index,
            metadata_visible,
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            last_pointer: None,
            image_dimensions: None,
            scroll_direction: 0.0,
            last_scroll_at: None,
        };
        window.set_window_title(&Self::window_title(&path));
        self.focus_handle.focus(window);
        self.inspector
            .update(cx, |inspector, cx| inspector.request(path.clone(), cx));
        cx.notify();

        if !QuickLookService::can_render_image_directly(&path) {
            let service = self.service.clone();
            cx.spawn(async move |this, cx| {
                let result = service.prepare_image(path.clone()).await;
                let _ = this.update(cx, |viewer, cx| {
                    if viewer.generation != generation || viewer.state.path != path {
                        return;
                    }
                    viewer.state.content = match result {
                        Ok(display_path) => ImageViewerContent::Image { display_path },
                        Err(error) => ImageViewerContent::Error(error.to_string()),
                    };
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn window_title(path: &std::path::Path) -> String {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("图片");
        format!("{name} — FlowFile 图片浏览")
    }

    fn switch_image(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(next_index) =
            next_image_index(self.state.image_index, self.state.image_paths.len(), delta)
        else {
            return;
        };
        let path = self.state.image_paths[next_index].clone();
        let image_paths = self.state.image_paths.clone();
        let metadata_visible = self.state.metadata_visible;
        let source_pane = self.source_pane.clone();
        source_pane.update(cx, |pane, cx| {
            if let Some(index) = pane.items.iter().position(|item| item.path == path) {
                pane.select(index, false, false);
                cx.notify();
            }
        });
        self.show_image(
            path,
            image_paths,
            next_index,
            metadata_visible,
            source_pane,
            window,
            cx,
        );
    }

    fn toggle_metadata(&mut self, cx: &mut Context<Self>) {
        self.state.metadata_visible = !self.state.metadata_visible;
        if self.state.metadata_visible {
            let path = self.state.path.clone();
            self.inspector
                .update(cx, |inspector, cx| inspector.request(path, cx));
        }
        cx.notify();
    }

    fn reset_transform(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((image_width, image_height)) = self.state.image_dimensions else {
            return;
        };
        let window_size = window.bounds().size;
        self.state.zoom = fit_image_zoom_to_viewport(
            image_width,
            image_height,
            self.state.metadata_visible,
            f32::from(window_size.width),
            f32::from(window_size.height),
        );
        self.state.pan_x = 0.0;
        self.state.pan_y = 0.0;
        self.state.last_pointer = None;
        self.state.scroll_direction = 0.0;
        self.state.last_scroll_at = None;
        cx.notify();
    }

    fn close(&mut self, window: &mut Window) {
        window.remove_window();
    }

    fn on_toggle_quick_look(
        &mut self,
        _: &ToggleQuickLook,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.close(window);
    }

    fn on_close_quick_look(
        &mut self,
        _: &CloseQuickLook,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.close(window);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "left" => self.switch_image(-1, window, cx),
            "right" => self.switch_image(1, window, cx),
            "i" => self.toggle_metadata(cx),
            "0" | "f" => self.reset_transform(window, cx),
            "w" if event.keystroke.modifiers.platform => self.close(window),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.state.content, ImageViewerContent::Image { .. })
            || self.state.image_dimensions.is_none()
        {
            cx.stop_propagation();
            return;
        }
        let delta = event.delta.pixel_delta(px(18.0));
        let vertical_delta = f32::from(delta.y);
        let now = Instant::now();
        let elapsed = self
            .state
            .last_scroll_at
            .and_then(|previous| now.checked_duration_since(previous));
        if is_opposite_scroll_tail(
            self.state.scroll_direction,
            elapsed,
            vertical_delta,
            event.delta.precise(),
            event.touch_phase,
        ) {
            if matches!(event.touch_phase, TouchPhase::Ended) {
                self.state.scroll_direction = 0.0;
                self.state.last_scroll_at = None;
            } else {
                self.state.last_scroll_at = Some(now);
            }
            cx.stop_propagation();
            return;
        }
        if vertical_delta.abs() < f32::EPSILON {
            if matches!(event.touch_phase, TouchPhase::Ended) {
                self.state.scroll_direction = 0.0;
                self.state.last_scroll_at = None;
            }
            cx.stop_propagation();
            return;
        }
        self.state.scroll_direction = vertical_delta.signum();
        self.state.last_scroll_at = if matches!(event.touch_phase, TouchPhase::Ended) {
            None
        } else {
            Some(now)
        };
        let old_zoom = self.state.zoom;
        let new_zoom = zoom_from_scroll(old_zoom, vertical_delta);
        let ratio = new_zoom / old_zoom;
        self.state.zoom = new_zoom;
        self.state.pan_x *= ratio;
        self.state.pan_y *= ratio;
        cx.notify();
        cx.stop_propagation();
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.state.content, ImageViewerContent::Image { .. }) {
            self.state.last_pointer =
                Some((f32::from(event.position.x), f32::from(event.position.y)));
            cx.stop_propagation();
        }
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.state.content, ImageViewerContent::Image { .. }) {
            return;
        }
        if !event.dragging() {
            self.state.last_pointer = None;
            return;
        }
        if let Some((last_x, last_y)) = self.state.last_pointer {
            self.state.pan_x += f32::from(event.position.x) - last_x;
            self.state.pan_y += f32::from(event.position.y) - last_y;
            self.state.last_pointer =
                Some((f32::from(event.position.x), f32::from(event.position.y)));
            cx.notify();
            cx.stop_propagation();
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.last_pointer.take().is_some() {
            cx.notify();
            cx.stop_propagation();
        }
    }

    fn metadata(&self, cx: &App) -> (Vec<(String, String)>, Option<String>) {
        if !self.state.metadata_visible {
            return (Vec::new(), None);
        }
        let inspector = self.inspector.read(cx);
        if inspector.current_path.as_ref() != Some(&self.state.path) || inspector.is_loading {
            return (Vec::new(), Some("正在读取图片元信息…".to_string()));
        }
        if let Some(error) = &inspector.error {
            return (Vec::new(), Some(format!("无法读取元信息：{error}")));
        }
        let Some(metadata) = &inspector.metadata else {
            return (Vec::new(), Some("没有可显示的图片元信息".to_string()));
        };
        let mut facts = Vec::new();
        if let Some(format) = &metadata.format {
            facts.push(("格式".to_string(), format.clone()));
        }
        facts.push(("文件大小".to_string(), format_file_size(metadata.file_size)));
        if let Some((width, height)) = metadata.dimensions {
            facts.push(("分辨率".to_string(), format!("{width} × {height}")));
        }
        facts.push(("修改时间".to_string(), metadata.modified.clone()));
        facts.push(("权限".to_string(), metadata.permissions.clone()));
        facts.extend(metadata.exif.iter().cloned());
        (facts, None)
    }

    fn render_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let can_show_previous = self.state.image_index > 0;
        let can_show_next = self.state.image_index + 1 < self.state.image_paths.len();
        match self.state.content.clone() {
            ImageViewerContent::Loading => div()
                .flex()
                .items_center()
                .justify_center()
                .min_w_0()
                .min_h_0()
                .flex_1()
                .text_color(theme::text_tertiary())
                .child("正在准备图片预览…")
                .into_any_element(),
            ImageViewerContent::Error(error) => div()
                .flex()
                .items_center()
                .justify_center()
                .min_w_0()
                .min_h_0()
                .flex_1()
                .text_color(theme::danger())
                .child(error)
                .into_any_element(),
            ImageViewerContent::Image { display_path } => {
                let image_element_id =
                    SharedString::from(display_path.to_string_lossy().into_owned());
                let rendered_image = img(display_path)
                    // GPUI only preserves per-frame timing and requests animation
                    // frames for stateful image elements. A path-specific id keeps
                    // GIF/WebP animation running and resets it when images change.
                    .id(image_element_id)
                    .flex_none()
                    .when_some(self.state.image_dimensions, |image, (width, height)| {
                        let (width, height) = scaled_image_size(width, height, self.state.zoom);
                        image.w(px(width)).h(px(height))
                    })
                    .relative()
                    .left(px(self.state.pan_x))
                    .top(px(self.state.pan_y))
                    .object_fit(ObjectFit::Contain)
                    .with_fallback(|| {
                        div()
                            .text_color(theme::danger())
                            .child("无法解码此图像")
                            .into_any_element()
                    });
                div()
                    .id("image-viewer-stage")
                    .relative()
                    .min_w_0()
                    .min_h_0()
                    .flex_1()
                    .overflow_hidden()
                    .cursor_move()
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(rendered_image),
                    )
                    .child(
                        div()
                            .id("image-viewer-interaction")
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .cursor_move()
                            .on_scroll_wheel(cx.listener(Self::on_scroll))
                            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                            .on_mouse_move(cx.listener(Self::on_mouse_move))
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .h_full()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .id("previous-image-hover-zone")
                                    .group("previous-image-hover-zone")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .w(px(NAVIGATION_HOVER_WIDTH))
                                    .h(px(NAVIGATION_HOVER_HEIGHT))
                                    .child(
                                        div()
                                            .id("previous-image-overlay")
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .size(px(NAVIGATION_BUTTON_SIZE))
                                            .rounded_full()
                                            .border_1()
                                            .border_color(gpui::rgba(if can_show_previous {
                                                0xffffff33
                                            } else {
                                                0xffffff14
                                            }))
                                            .bg(black().opacity(if can_show_previous {
                                                0.22
                                            } else {
                                                0.08
                                            }))
                                            .text_size(theme::font(30.0))
                                            .text_color(gpui::rgba(if can_show_previous {
                                                0xffffffb8
                                            } else {
                                                0xffffff38
                                            }))
                                            .invisible()
                                            .group_hover("previous-image-hover-zone", |style| {
                                                style.visible()
                                            })
                                            .when(can_show_previous, |button| {
                                                button
                                                    .cursor_pointer()
                                                    .hover(|style| style.bg(black().opacity(0.3)))
                                                    .tooltip(delayed_tooltip("上一张（左方向键）"))
                                                    .on_click(cx.listener(|this, _, window, cx| {
                                                        this.switch_image(-1, window, cx)
                                                    }))
                                            })
                                            .child("‹"),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .h_full()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .id("next-image-hover-zone")
                                    .group("next-image-hover-zone")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .w(px(NAVIGATION_HOVER_WIDTH))
                                    .h(px(NAVIGATION_HOVER_HEIGHT))
                                    .child(
                                        div()
                                            .id("next-image-overlay")
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .size(px(NAVIGATION_BUTTON_SIZE))
                                            .rounded_full()
                                            .border_1()
                                            .border_color(gpui::rgba(if can_show_next {
                                                0xffffff33
                                            } else {
                                                0xffffff14
                                            }))
                                            .bg(black().opacity(if can_show_next {
                                                0.22
                                            } else {
                                                0.08
                                            }))
                                            .text_size(theme::font(30.0))
                                            .text_color(gpui::rgba(if can_show_next {
                                                0xffffffb8
                                            } else {
                                                0xffffff38
                                            }))
                                            .invisible()
                                            .group_hover("next-image-hover-zone", |style| {
                                                style.visible()
                                            })
                                            .when(can_show_next, |button| {
                                                button
                                                    .cursor_pointer()
                                                    .hover(|style| style.bg(black().opacity(0.3)))
                                                    .tooltip(delayed_tooltip("下一张（右方向键）"))
                                                    .on_click(cx.listener(|this, _, window, cx| {
                                                        this.switch_image(1, window, cx)
                                                    }))
                                            })
                                            .child("›"),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom(px(12.0))
                            .left(px(12.0))
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(black().opacity(0.52))
                            .text_size(theme::font(9.0))
                            .text_color(gpui::rgba(0xffffffd1))
                            .child("滚轮缩放 · 按住左键拖动 · F / 0 适合窗口"),
                    )
                    .into_any_element()
            }
        }
    }
}

impl Render for ImageViewerWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let can_show_previous = self.state.image_index > 0;
        let can_show_next = self.state.image_index + 1 < self.state.image_paths.len();
        let file_name = self
            .state
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("图片")
            .to_string();
        let content = self.render_content(cx);
        let (metadata_facts, metadata_status) = self.metadata(cx);
        let metadata_panel = self.state.metadata_visible.then(|| {
            div()
                .flex()
                .flex_col()
                .w(px(METADATA_WIDTH))
                .h_full()
                .flex_shrink_0()
                .border_l_1()
                .border_color(theme::border())
                .bg(theme::surface_subtle())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .h(px(40.0))
                        .px_3()
                        .border_b_1()
                        .border_color(theme::border())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(theme::font(10.0))
                        .child("图片元信息"),
                )
                .child(
                    div()
                        .id("image-viewer-metadata-scroll")
                        .flex()
                        .flex_col()
                        .min_h_0()
                        .flex_1()
                        .overflow_y_scroll()
                        .p_3()
                        .gap_3()
                        .when_some(metadata_status, |panel, status| {
                            panel.child(
                                div()
                                    .text_size(theme::font(10.0))
                                    .text_color(theme::text_tertiary())
                                    .child(status),
                            )
                        })
                        .children(metadata_facts.into_iter().map(|(label, value)| {
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(theme::font(9.0))
                                        .text_color(theme::text_tertiary())
                                        .child(label),
                                )
                                .child(
                                    div()
                                        .text_size(theme::font(10.0))
                                        .text_color(theme::text_primary())
                                        .child(value),
                                )
                        })),
                )
        });

        div()
            .id("image-viewer-window")
            .key_context("QuickLook")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_toggle_quick_look))
            .on_action(cx.listener(Self::on_close_quick_look))
            .on_key_down(cx.listener(Self::on_key_down))
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .font_family("SF Pro Text")
            .bg(theme::surface())
            .text_color(theme::text_primary())
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(TOOLBAR_HEIGHT))
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
                    .child(
                        div()
                            .mr_2()
                            .text_size(theme::font(9.0))
                            .text_color(theme::text_tertiary())
                            .child(format!(
                                "{} / {}",
                                self.state.image_index + 1,
                                self.state.image_paths.len()
                            )),
                    )
                    .child(
                        div()
                            .id("previous-viewer-image")
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(28.0))
                            .rounded_md()
                            .text_color(if can_show_previous {
                                theme::text_primary()
                            } else {
                                theme::text_tertiary().opacity(0.4)
                            })
                            .when(can_show_previous, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::accent_soft()))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.switch_image(-1, window, cx)
                                    }))
                            })
                            .child("←"),
                    )
                    .child(
                        div()
                            .id("next-viewer-image")
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(28.0))
                            .rounded_md()
                            .text_color(if can_show_next {
                                theme::text_primary()
                            } else {
                                theme::text_tertiary().opacity(0.4)
                            })
                            .when(can_show_next, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::accent_soft()))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.switch_image(1, window, cx)
                                    }))
                            })
                            .child("→"),
                    )
                    .child(
                        div()
                            .mx_2()
                            .min_w(px(58.0))
                            .text_size(theme::font(10.0))
                            .text_color(theme::text_tertiary())
                            .child(format!("{:.0}%", self.state.zoom * 100.0)),
                    )
                    .child(
                        div()
                            .id("reset-viewer-image")
                            .flex()
                            .items_center()
                            .h(px(28.0))
                            .px_2()
                            .rounded_md()
                            .cursor_pointer()
                            .text_size(theme::font(9.0))
                            .text_color(theme::text_secondary())
                            .hover(|style| style.bg(theme::accent_soft()))
                            .tooltip(delayed_tooltip("恢复适合窗口 (F / 0)"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.reset_transform(window, cx)),
                            )
                            .child("适合窗口"),
                    )
                    .child(
                        div()
                            .id("toggle-viewer-metadata")
                            .flex()
                            .items_center()
                            .h(px(28.0))
                            .px_2()
                            .ml_1()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(if self.state.metadata_visible {
                                theme::accent_soft()
                            } else {
                                gpui::transparent_black()
                            })
                            .text_size(theme::font(9.0))
                            .text_color(if self.state.metadata_visible {
                                theme::accent()
                            } else {
                                theme::text_secondary()
                            })
                            .hover(|style| style.bg(theme::accent_soft()))
                            .tooltip(delayed_tooltip("显示或隐藏图片元信息 (I)"))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_metadata(cx)))
                            .child(if self.state.metadata_visible {
                                "隐藏信息"
                            } else {
                                "显示信息"
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .min_h_0()
                    .flex_1()
                    .child(content)
                    .when_some(metadata_panel, |body, panel| body.child(panel)),
            )
    }
}

fn next_image_index(current: usize, count: usize, delta: isize) -> Option<usize> {
    if count == 0 || current >= count {
        return None;
    }
    let next = current.checked_add_signed(delta)?;
    (next < count && next != current).then_some(next)
}

fn fit_image_zoom_to_viewport(
    width: u32,
    height: u32,
    metadata_visible: bool,
    window_width: f32,
    window_height: f32,
) -> f32 {
    let viewport_width = (window_width
        - if metadata_visible {
            METADATA_WIDTH
        } else {
            0.0
        }
        - VIEWPORT_PADDING)
        .max(1.0);
    let viewport_height = (window_height - TOOLBAR_HEIGHT - VIEWPORT_PADDING).max(1.0);
    let horizontal = viewport_width / width.max(1) as f32;
    let vertical = viewport_height / height.max(1) as f32;
    horizontal
        .min(vertical)
        .clamp(MIN_IMAGE_ZOOM, MAX_IMAGE_ZOOM)
}

#[cfg(test)]
mod tests {
    use super::{fit_image_zoom_to_viewport, next_image_index};
    use crate::views::workspace::MIN_IMAGE_ZOOM;

    #[test]
    fn navigation_stops_at_window_sequence_edges() {
        assert_eq!(next_image_index(1, 3, -1), Some(0));
        assert_eq!(next_image_index(1, 3, 1), Some(2));
        assert_eq!(next_image_index(0, 3, -1), None);
        assert_eq!(next_image_index(2, 3, 1), None);
    }

    #[test]
    fn fit_zoom_uses_resizable_window_viewport() {
        let compact = fit_image_zoom_to_viewport(1600, 1200, false, 800.0, 600.0);
        let maximized = fit_image_zoom_to_viewport(1600, 1200, false, 1600.0, 1200.0);
        assert!(maximized > compact);
        assert_eq!(
            fit_image_zoom_to_viewport(1_000_000, 1_000_000, true, 640.0, 420.0),
            MIN_IMAGE_ZOOM
        );
    }
}
