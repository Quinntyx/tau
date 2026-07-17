//! Native gpui client for tau model turns.

mod backend;
mod input;
mod view;

use std::path::PathBuf;

use anyhow::Result;
use backend::Backend;
use gpui::{App, AppContext, Application, Bounds, WindowBounds, WindowOptions, px, size};
use tokio::runtime::Runtime;
use view::TauView;

pub fn run(socket: PathBuf) -> Result<()> {
    let runtime = Runtime::new()?;
    let backend = Backend::new(socket, &runtime)?;
    Application::new().run(move |cx: &mut App| {
        input::bind_keys(cx);
        view::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1_100.), px(760.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("tau".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |_, cx| cx.new(|cx| TauView::new(backend, cx)),
        )
        .expect("open tau window");
        cx.activate(true);
    });
    Ok(())
}
