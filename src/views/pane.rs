use super::{
    address_bar::AddressBar, context_menu::ContextMenuView, main_list::MainListView,
    search_bar::SearchBar, tooltip::delayed_tooltip,
};
use crate::{
    models::{FileDragPayload, FileOperationController, Model, MultiPaneModel, Pane, ViewMode},
    services::{ThumbnailEngine, TransferMode},
    theme,
};
use gpui::{Context, Entity, IntoElement, Render, Window, div, prelude::*, px};

pub struct PaneView {
    index: usize,
    pane: Model<Pane>,
    multi_pane: Model<MultiPaneModel>,
    operations: Entity<FileOperationController>,
    address_bar: Entity<AddressBar>,
    search_bar: Entity<SearchBar>,
    main_list: Entity<MainListView>,
}

impl PaneView {
    pub fn new(
        index: usize,
        pane: Model<Pane>,
        multi_pane: Model<MultiPaneModel>,
        operations: Entity<FileOperationController>,
        thumbnails: Entity<ThumbnailEngine>,
        context_menu: Entity<ContextMenuView>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&multi_pane, |_, _, cx| cx.notify()).detach();
        cx.observe(&pane, |_, _, cx| cx.notify()).detach();

        let address_bar = cx.new(|cx| AddressBar::new(pane.clone(), cx));
        let search_bar = cx.new(|cx| SearchBar::new(pane.clone(), cx));
        let main_list = cx.new(|cx| {
            MainListView::new(
                index,
                pane.clone(),
                operations.clone(),
                thumbnails,
                context_menu,
                cx,
            )
        });

        Self {
            index,
            pane,
            multi_pane,
            operations,
            address_bar,
            search_bar,
            main_list,
        }
    }
}

impl Render for PaneView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_active = self.multi_pane.read(cx).active_pane_index == self.index;
        let pane = self.pane.read(cx);
        let active_tab = pane
            .tabs
            .get(pane.active_tab_index)
            .map(|tab| tab.title.clone())
            .unwrap_or_else(|| "Files".to_string());
        let is_loading = pane.is_loading;
        let view_mode = pane.view_mode;
        let multi_pane = self.multi_pane.clone();
        let pane_model = self.pane.clone();
        let operations = self.operations.clone();
        let index = self.index;

        div()
            .id(("pane", self.index))
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .size_full()
            .overflow_hidden()
            .rounded_sm()
            .border(px(if is_active { 2.0 } else { 1.0 }))
            .border_color(if is_active {
                theme::accent()
            } else {
                theme::border_strong()
            })
            .bg(theme::surface())
            .on_click(move |_, _, cx| {
                multi_pane.update(cx, |model, cx| {
                    model.set_active_pane(index);
                    cx.notify();
                });
            })
            .drag_over::<FileDragPayload>(|style, _, _, _| {
                style
                    .border_color(theme::accent())
                    .bg(theme::accent_soft().opacity(0.35))
            })
            .on_drop(move |payload: &FileDragPayload, window, cx| {
                if payload.source_pane_index == index && !window.modifiers().alt {
                    return;
                }
                let destination = pane_model.read(cx).current_path.clone();
                let mode = if window.modifiers().alt {
                    TransferMode::Copy
                } else {
                    TransferMode::Move
                };
                operations.update(cx, |operations, cx| {
                    operations.transfer_to_path(payload.paths.clone(), destination, mode, cx);
                });
            })
            .child(
                div()
                    .flex()
                    .items_end()
                    .h(px(34.0))
                    .px_2()
                    .pt_1()
                    .bg(if is_active {
                        theme::accent_soft()
                    } else {
                        theme::surface_subtle()
                    })
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .h(px(29.0))
                            .max_w(px(196.0))
                            .px_3()
                            .rounded_t_md()
                            .bg(theme::surface())
                            .text_size(theme::font(11.0))
                            .text_color(if is_active {
                                theme::accent()
                            } else {
                                theme::text_secondary()
                            })
                            .child(if is_loading { "◌" } else { "●" })
                            .child(div().min_w_0().flex_1().truncate().child(active_tab)),
                    )
                    .child(div().flex_1())
                    .child(view_mode_button(
                        self.index,
                        "pane-details",
                        "≡",
                        ViewMode::Details,
                        view_mode,
                        self.pane.clone(),
                        "详细列表",
                    ))
                    .child(view_mode_button(
                        self.index,
                        "pane-grid",
                        "▦",
                        ViewMode::Grid,
                        view_mode,
                        self.pane.clone(),
                        "大图标网格",
                    ))
                    .child(
                        div()
                            .ml_1()
                            .mb_1()
                            .text_size(theme::font(16.0))
                            .text_color(theme::text_tertiary())
                            .child("+"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .flex_shrink_0()
                    .child(
                        div()
                            .flex()
                            .min_w_0()
                            .flex_1()
                            .child(self.address_bar.clone()),
                    )
                    .child(self.search_bar.clone()),
            )
            .child(
                div()
                    .flex()
                    .min_h_0()
                    .flex_1()
                    .child(self.main_list.clone()),
            )
    }
}

fn view_mode_button(
    pane_index: usize,
    id: &'static str,
    glyph: &'static str,
    target: ViewMode,
    current: ViewMode,
    pane: Model<Pane>,
    tooltip: &'static str,
) -> impl IntoElement {
    let active = target == current;
    div()
        .id((id, pane_index))
        .flex()
        .items_center()
        .justify_center()
        .size(px(28.0))
        .mb_1()
        .ml_1()
        .rounded_sm()
        .bg(if active {
            theme::accent_soft()
        } else {
            theme::surface()
        })
        .text_size(theme::font(12.0))
        .text_color(if active {
            theme::accent()
        } else {
            theme::text_tertiary()
        })
        .hover(|style| style.bg(theme::accent_soft()))
        .tooltip(delayed_tooltip(tooltip))
        .on_click(move |_, _, cx| {
            pane.update(cx, |pane, cx| pane.set_view_mode(target, cx));
        })
        .child(glyph)
}
