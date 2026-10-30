// En Windows el binario de release no abre una consola detrás de la ventana.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod connect;
mod containers;
mod data;
mod files;
mod terminal;
#[cfg(test)]
mod testutil;
mod ui;

use eframe::egui;

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("omni-term {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    ui::fonts::preload();
    let debug_keys = std::env::args().any(|a| a == "--keys");
    let size = saved_window_size().unwrap_or([960.0, 600.0]);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("omni-term")
        .with_app_id("omni-term")
        .with_inner_size(size)
        .with_min_inner_size([320.0, 200.0])
        .with_decorations(data::settings::NATIVE_CHROME || system_titlebar())
        .with_maximized(saved_flag("window_maximized"));
    if data::settings::NATIVE_CHROME {
        // macOS: la barra de título es transparente y el contenido llega hasta arriba; los botones de la ventana quedan encima.
        viewport = viewport.with_titlebar_shown(false).with_title_shown(false).with_fullsize_content_view(true);
    }
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(icon);
    }
    let mut options = eframe::NativeOptions {
        viewport,
        clipboard_shortcuts: false,
        ..Default::default()
    };
    prefer_dx12(&mut options.wgpu_options);
    eframe::run_native(
        "omni-term",
        options,
        Box::new(|cc| ui::App::new(cc, debug_keys).map(|app| Box::new(app) as Box<dyn eframe::App>).map_err(Into::into)),
    )
}

/// En Windows, wgpu puede elegir Vulkan, y con algunos controladores la ventana parpadea al redibujarse.
/// Se usa DX12 (con GL de reserva) salvo que `WGPU_BACKEND` pida otro.
#[cfg(windows)]
fn prefer_dx12(config: &mut eframe::WgpuConfiguration) {
    use eframe::egui_wgpu::{wgpu::Backends, WgpuSetup};
    if let WgpuSetup::CreateNew(setup) = &mut config.wgpu_setup
        && Backends::from_env().is_none()
    {
        setup.instance_descriptor.backends = Backends::DX12 | Backends::GL;
    }
}

#[cfg(not(windows))]
fn prefer_dx12(_config: &mut eframe::WgpuConfiguration) {}

/// Si el usuario eligió la barra de título del sistema; por defecto se dibuja la integrada (en macOS nunca).
fn system_titlebar() -> bool {
    let read = || -> Option<bool> {
        let store = data::store::Store::open(&data::store::Store::default_path().ok()?).ok()?;
        Some(store.meta("setting.system_titlebar").ok()??.as_slice() == b"1")
    };
    !data::settings::NATIVE_CHROME && read().unwrap_or(false)
}

/// Un ajuste guardado como «1» (verdadero) en la tabla `meta`.
fn saved_flag(key: &str) -> bool {
    let read = || -> Option<bool> {
        let store = data::store::Store::open(&data::store::Store::default_path().ok()?).ok()?;
        Some(store.meta(key).ok()??.as_slice() == b"1")
    };
    read().unwrap_or(false)
}

/// Tamaño de ventana de la última sesión, si se guardó uno razonable.
fn saved_window_size() -> Option<[f32; 2]> {
    let store = data::store::Store::open(&data::store::Store::default_path().ok()?).ok()?;
    let text = String::from_utf8(store.meta("window_size").ok()??).ok()?;
    let (width, height) = text.split_once('x')?;
    let (width, height) = (width.parse::<f32>().ok()?, height.parse::<f32>().ok()?);
    ((320.0..=10_000.0).contains(&width) && (200.0..=10_000.0).contains(&height)).then_some([width, height])
}
