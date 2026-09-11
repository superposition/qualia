//! The console binary; all logic lives in the library so tests drive the same
//! surface the window does.

fn main() -> eframe::Result<()> {
    qualia_console::run()
}
