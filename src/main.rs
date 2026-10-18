// En Windows el binario de release no abre una consola detrás de la ventana.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod cursor;
mod dialogs;
mod explorer;
mod icons;
mod keys;
mod layout;
mod local;
mod mouse;
mod pane;
mod search;
mod settings;
mod secrets;
mod sftp;
mod ssh;
mod snippet_vars;
mod sshconfig;
mod socks;
mod store;
mod transfer;
#[cfg(test)]
mod testutil;
mod term;
mod theme;
mod ui;

use eframe::egui;

fn main() -> eframe::Result {
    let debug_keys = std::env::args().any(|a| a == "--keys");
    let size = saved_window_size().unwrap_or([960.0, 600.0]);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("omni-term")
        .with_app_id("omni-term")
        .with_inner_size(size)
        .with_min_inner_size([320.0, 200.0])
        .with_decorations(true);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        clipboard_shortcuts: false,
        ..Default::default()
    };
    eframe::run_native(
        "omni-term",
        options,
        Box::new(|cc| ui::App::new(cc, debug_keys).map(|app| Box::new(app) as Box<dyn eframe::App>).map_err(Into::into)),
    )
}

/// Tamaño de ventana de la última sesión, si se guardó uno razonable.
fn saved_window_size() -> Option<[f32; 2]> {
    let store = store::Store::open(&store::Store::default_path().ok()?).ok()?;
    let text = String::from_utf8(store.meta("window_size").ok()??).ok()?;
    let (width, height) = text.split_once('x')?;
    let (width, height) = (width.parse::<f32>().ok()?, height.parse::<f32>().ok()?);
    ((320.0..=10_000.0).contains(&width) && (200.0..=10_000.0).contains(&height)).then_some([width, height])
}
