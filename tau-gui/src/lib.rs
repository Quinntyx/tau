//! Native gpui client for tau model turns.

pub mod backend;
pub mod chat;
pub mod feed;
pub mod input;
pub mod picker;
pub mod preferences;
pub mod view;

use std::path::PathBuf;

use anyhow::{Context, Result};
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
    // Keep daemon startup in the normal Result path so a missing or stale socket
    // can be reported without panicking before the recovery UI is available.
    let (daemon, auto_started, cwd, model) = if let Some(runtime) = owned_runtime.as_ref() {
        runtime
            .block_on(Backend::prepare(&socket))
            .context("starting tau daemon")?
    } else {
        tokio::task::block_in_place(|| handle.block_on(Backend::prepare(&socket)))
            .context("starting tau daemon")?
    };
    let backend =
        Backend::from_parts_with_startup(socket, handle, daemon, auto_started, cwd, model);
    Application::new().run(move |cx: &mut App| {
        input::bind_keys(cx);
        view::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1_100.), px(760.)), cx);
        let window = cx
            .open_window(
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
            .context("opening tau window");
        if let Err(error) = window {
            eprintln!("{error:#}");
            return;
        }
        cx.activate(true);
    });
    Ok(())
}
