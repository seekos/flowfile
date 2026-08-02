mod actions;
mod models;
mod services;
mod theme;
mod views;

use anyhow::Result;
use gpui::{
    App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, px,
    size,
};
use views::WorkspaceView;

fn main() -> Result<()> {
    Application::new().run(|cx: &mut App| {
        actions::register_keybindings(cx);
        cx.on_action(|_: &actions::Quit, cx| cx.quit());
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);

        cx.open_window(
            WindowOptions {
                focus: true,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(860.0), px(560.0))),
                app_id: Some("com.flowfile.app".to_string()),
                titlebar: Some(TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(14.0), px(18.0))),
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| WorkspaceView::new(window, cx)),
        )
        .expect("failed to open FlowFile window");

        cx.activate(true);
    });

    Ok(())
}
