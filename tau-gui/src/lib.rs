//! Native gpui client for tau model turns.

pub mod backend;
pub mod chat;
pub mod input;
pub mod picker;
pub mod preferences;
mod view;

use std::path::PathBuf;

use anyhow::Result;
use backend::Backend;
use gpui::{App, AppContext, Application, Bounds, WindowBounds, WindowOptions, px, size};
use tokio::runtime::{Handle, Runtime};
use view::TauView;

pub fn run(socket: PathBuf) -> Result<()> {
    let owned_runtime = Handle::try_current()
        .is_err()
        .then(Runtime::new)
        .transpose()?;
    let handle = match Handle::try_current() {
        Ok(handle) => handle,
        Err(_) => owned_runtime
            .as_ref()
            .expect("owned runtime")
            .handle()
            .clone(),
    };
    let (daemon, auto_started, cwd, model) = if let Some(runtime) = owned_runtime.as_ref() {
        runtime.block_on(Backend::prepare(&socket))?
    } else {
        tokio::task::block_in_place(|| handle.block_on(Backend::prepare(&socket)))?
    };
    let backend =
        Backend::from_parts_with_startup(socket, handle, daemon, auto_started, cwd, model);
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
