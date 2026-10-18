mod keys;
mod term;
mod theme;
mod ui;

use eframe::egui;

fn main() -> eframe::Result {
    let debug_keys = std::env::args().any(|a| a == "--keys");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("omniterm")
            .with_inner_size([960.0, 600.0])
            .with_min_inner_size([320.0, 200.0])
            .with_decorations(true),
        clipboard_shortcuts: false,
        ..Default::default()
    };
    eframe::run_native(
        "omniterm",
        options,
        Box::new(|cc| ui::App::new(cc, debug_keys).map(|app| Box::new(app) as Box<dyn eframe::App>).map_err(Into::into)),
    )
}
