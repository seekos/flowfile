use crate::theme;
use gpui::{AnyView, App, AppContext, Context, Render, SharedString, Window, div, prelude::*, px};

/// Builds the shared macOS-style tooltip used by every interactive button.
/// GPUI displays tooltips after its built-in 500 ms hover delay and paints them
/// above the window contents, so scroll views and pane clipping cannot hide them.
pub fn delayed_tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| {
        let text = text.clone();
        cx.new(|_| ButtonTooltip { text }).into()
    }
}

struct ButtonTooltip {
    text: SharedString,
}

impl Render for ButtonTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(320.0))
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(theme::border_strong())
            .bg(theme::surface())
            .shadow_md()
            .font_family(".SystemUIFont")
            .text_size(theme::font(9.0))
            .text_color(theme::text_primary())
            .child(self.text.clone())
    }
}
