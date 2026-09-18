#[cfg(target_os = "macos")]
mod accessibility;
mod actions;
mod distribution;
mod icons;
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
    Application::new()
        .with_assets(icons::IconAssets)
        .run(|cx: &mut App| {
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
                    app_id: Some("cc.bso.flowfile".to_string()),
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

#[cfg(test)]
mod dependency_regression_tests {
    #[test]
    fn patched_grid_rejects_dimension_overflow_without_corrupting_state() {
        let mut grid = grid::Grid::from_vec(vec![1_u8, 2_u8], 2);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            grid.expand_rows(usize::MAX / 2);
        }));

        assert!(result.is_err());
        assert_eq!(grid.size(), (1, 2));
        assert_eq!(grid.get(0, 0), Some(&1));
        assert_eq!(grid.get(1, 0), None);
    }
}
