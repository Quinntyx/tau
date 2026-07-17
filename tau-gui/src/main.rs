use std::path::PathBuf;
fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut socket = None;
    while let Some(arg) = args.next() {
        if arg == "--socket" {
            socket = args.next().map(PathBuf::from);
        }
    }
    let socket = socket.unwrap_or(tau_core::default_socket_path()?);
    tau_gui::run(socket)
}
