use super::{context_menu::ContextMenuView, pane::PaneView};
use crate::{
    models::{FileOperationController, LayoutMode, Model, MultiPaneModel},
    services::ThumbnailEngine,
    theme,
};
use gpui::{AnyElement, Context, Entity, IntoElement, Render, Window, div, prelude::*, px};

pub struct MultiPaneContainerView {
    model: Model<MultiPaneModel>,
    panes: Vec<Entity<PaneView>>,
}

impl MultiPaneContainerView {
    pub fn new(
        model: Model<MultiPaneModel>,
        operations: Entity<FileOperationController>,
        thumbnails: Entity<ThumbnailEngine>,
        context_menu: Entity<ContextMenuView>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();

        let pane_models = model.read(cx).panes.clone();
        let panes = pane_models
            .into_iter()
            .enumerate()
            .map(|(index, pane)| {
                let model = model.clone();
                let operations = operations.clone();
                let thumbnails = thumbnails.clone();
                let context_menu = context_menu.clone();
                cx.new(|cx| {
                    PaneView::new(index, pane, model, operations, thumbnails, context_menu, cx)
                })
            })
            .collect();

        Self { model, panes }
    }

    fn pane(&self, index: usize) -> Entity<PaneView> {
        self.panes[index].clone()
    }

    fn pair(&self, first: usize, second: usize) -> impl IntoElement {
        div()
            .flex()
            .min_w_0()
            .min_h_0()
            .size_full()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .min_h_0()
                    .flex_1()
                    .child(self.pane(first)),
            )
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .min_h_0()
                    .flex_1()
                    .child(self.pane(second)),
            )
    }

    fn layout(&self, mode: LayoutMode) -> AnyElement {
        match mode {
            LayoutMode::Single => div()
                .flex()
                .size_full()
                .child(self.pane(0))
                .into_any_element(),
            LayoutMode::DualVertical => self.pair(0, 1).into_any_element(),
            LayoutMode::DualHorizontal => div()
                .flex()
                .flex_col()
                .min_w_0()
                .min_h_0()
                .size_full()
                .gap(px(6.0))
                .child(div().flex().min_h_0().flex_1().child(self.pane(0)))
                .child(div().flex().min_h_0().flex_1().child(self.pane(1)))
                .into_any_element(),
            LayoutMode::Quad => div()
                .flex()
                .flex_col()
                .min_w_0()
                .min_h_0()
                .size_full()
                .gap(px(6.0))
                .child(div().flex().min_h_0().flex_1().child(self.pair(0, 1)))
                .child(div().flex().min_h_0().flex_1().child(self.pair(2, 3)))
                .into_any_element(),
        }
    }
}

impl Render for MultiPaneContainerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.model.read(cx).layout_mode;

        div()
            .flex()
            .min_w_0()
            .min_h_0()
            .size_full()
            .p(px(6.0))
            .bg(theme::canvas())
            .child(self.layout(mode))
    }
}
