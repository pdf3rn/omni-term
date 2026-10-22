// En Windows el binario de release no abre una consola detrás de la ventana.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod adb;
mod cursor;
mod containers;
mod docker;
mod dialogs;
mod explorer;
mod fonts;
mod ftp;
mod icons;
mod known_hosts;
mod keys;
mod layout;
mod local;
mod mouse;
mod palette;
mod pane;
mod remotefs;
mod search;
mod settings;
mod secrets;
mod sftp;
mod ssh;
mod snippet_vars;
mod smb;
mod sshconfig;
mod socks;
mod store;
mod transfer;
#[cfg(test)]
mod testutil;
mod telnet;
mod serial;
mod term;
mod theme;
mod ui;
mod ui_kit;

use eframe::egui;

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("omni-term {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    fonts::preload();
    let debug_keys = std::env::args().any(|a| a == "--keys");
    let size = saved_window_size().unwrap_or([960.0, 600.0]);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("omni-term")
        .with_app_id("omni-term")
        .with_inner_size(size)
        .with_min_inner_size([320.0, 200.0])
        .with_decorations(system_titlebar())
        .with_maximized(saved_flag("window_maximized"));
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

/// Si el usuario eligió la barra de título del sistema; por defecto se dibuja la integrada.
fn system_titlebar() -> bool {
    let read = || -> Option<bool> {
        let store = store::Store::open(&store::Store::default_path().ok()?).ok()?;
        Some(store.meta("setting.system_titlebar").ok()??.as_slice() == b"1")
    };
    read().unwrap_or(false)
}

/// Un ajuste guardado como «1» (verdadero) en la tabla `meta`.
fn saved_flag(key: &str) -> bool {
    let read = || -> Option<bool> {
        let store = store::Store::open(&store::Store::default_path().ok()?).ok()?;
        Some(store.meta(key).ok()??.as_slice() == b"1")
    };
    read().unwrap_or(false)
}

/// Tamaño de ventana de la última sesión, si se guardó uno razonable.
fn saved_window_size() -> Option<[f32; 2]> {
    let store = store::Store::open(&store::Store::default_path().ok()?).ok()?;
    let text = String::from_utf8(store.meta("window_size").ok()??).ok()?;
    let (width, height) = text.split_once('x')?;
    let (width, height) = (width.parse::<f32>().ok()?, height.parse::<f32>().ok()?);
    ((320.0..=10_000.0).contains(&width) && (200.0..=10_000.0).contains(&height)).then_some([width, height])
}
