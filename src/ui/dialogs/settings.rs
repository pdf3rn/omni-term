//! Página de ajustes: secciones, atajos y hosts conocidos.

use super::*;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum SettingsTab {
    #[default]
    General,
    Atajos,
    Hosts,
}

/// Lista de `known_hosts` de la página de ajustes, leída una vez y recargada al cambiar.
#[derive(Default)]
pub struct KnownHostsView {
    entries: Vec<crate::connect::known_hosts::KnownHost>,
    pub query: String,
    pub loaded: bool,
    error: Option<String>,
    /// Entrada que el usuario pidió olvidar; la procesa la aplicación.
    pub delete: Option<crate::connect::known_hosts::KnownHost>,
}

impl KnownHostsView {
    pub(super) fn ensure_loaded(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        match crate::connect::known_hosts::load(&crate::connect::ssh::default_known_hosts()) {
            Ok(entries) => {
                self.entries = entries;
                self.error = None;
            }
            Err(error) => {
                self.entries.clear();
                self.error = Some(format!("No se pudo leer known_hosts: {error}"));
            }
        }
    }
}

/// Estado del selector de almacén de secretos en Ajustes.
pub struct SecretsChoice {
    /// Elegido: clave maestra (si no, llavero).
    pub master: bool,
    pub keyring_available: bool,
    /// Almacén en uso en esta ejecución.
    pub active_master: bool,
}

pub(super) const SHORTCUTS: [(&str, &[(&str, &str)]); 4] = [
    (
        "Pestañas",
        &[
            ("Ctrl+Shift+P", "Paleta de comandos"),
            ("Ctrl+,", "Abrir los ajustes"),
            ("Alt+1 … 9", "Ir a la pestaña 1 … 9 (Alt+9, la última)"),
            ("Ctrl+Tab / Ctrl+Mayús+Tab", "Pestaña siguiente / anterior"),
            ("Doble clic en la pestaña", "Renombrarla"),
        ],
    ),
    (
        "Terminal",
        &[
            ("Ctrl+Shift+C / V", "Copiar / pegar"),
            ("Ctrl+Shift+F", "Buscar en la salida"),
            ("Ctrl+Shift+D / E", "Dividir a la derecha / hacia abajo"),
            ("Ctrl+Shift+W", "Cerrar el panel"),
            ("Ctrl+= / Ctrl+− / Ctrl+0", "Aumentar / reducir / restablecer la letra"),
        ],
    ),
    (
        "Ventana",
        &[("Ctrl+Shift+B", "Mostrar u ocultar la barra lateral"), ("Ctrl+Shift+K", "Buscar en la barra lateral"), ("F11", "Pantalla completa")],
    ),
    (
        "Explorador de archivos",
        &[
            ("F2", "Renombrar"),
            ("Supr", "Eliminar"),
            ("Enter", "Abrir la carpeta"),
            ("Retroceso", "Subir un nivel"),
            ("F5", "Actualizar"),
            ("Ctrl+A", "Seleccionar todo"),
            ("Ctrl+L", "Escribir una ruta"),
        ],
    ),
];

/// Lista de secciones de la página de ajustes.
pub(super) const SETTINGS_SECTIONS: [(SettingsTab, &str); 3] = [(SettingsTab::General, "General"), (SettingsTab::Atajos, "Atajos de teclado"), (SettingsTab::Hosts, "Hosts conocidos")];

pub(super) const SETTINGS_MAX_WIDTH: f32 = 720.0;

/// Página de ajustes de una pestaña: lista de secciones a la izquierda y el contenido a la derecha.
pub fn settings_page(ui: &mut egui::Ui, tab: &mut SettingsTab, settings: &mut Settings, secrets: &mut SecretsChoice, hosts: &mut KnownHostsView, connections: &[Connection]) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 4.0);
        ui.vertical(|ui| {
            ui.set_width(168.0);
            for (choice, label) in SETTINGS_SECTIONS {
                let selected = *tab == choice;
                let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::click());
                let response = response.hand();
                if selected {
                    ui.painter().rect_filled(rect, egui::CornerRadius::same(theme::radius::CONTROL), theme::selection());
                } else if response.hovered() {
                    ui.painter().rect_filled(rect, egui::CornerRadius::same(theme::radius::CONTROL), theme::surface());
                }
                let font = FontId::new(theme::size::BODY, if selected { crate::ui::strong_family() } else { egui::FontFamily::Proportional });
                ui.painter().text(rect.min + egui::vec2(12.0, 16.0), egui::Align2::LEFT_CENTER, label, font, if selected { theme::text() } else { theme::subtext() });
                if response.clicked() && !selected {
                    *tab = choice;
                    hosts.loaded = false;
                }
            }
        });
        ui.add_space(theme::space::XL);
        ui.vertical(|ui| {
            // Hasta 720 px, pero nunca más de lo que cabe: en ventanas estrechas el contenido se encoge.
            let width = ui.available_width().min(SETTINGS_MAX_WIDTH);
            ui.set_max_width(width);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.set_max_width(width - 12.0);
                settings_content(ui, *tab, settings, secrets, hosts, connections);
            });
        });
    });
}

pub(super) fn settings_content(ui: &mut egui::Ui, tab: SettingsTab, settings: &mut Settings, secrets: &mut SecretsChoice, hosts: &mut KnownHostsView, connections: &[Connection]) {
    if tab == SettingsTab::Hosts {
        known_hosts_body(ui, hosts, connections);
        return;
    }
    if tab == SettingsTab::Atajos {
        for (group, entries) in SHORTCUTS {
            crate::ui::kit::card(ui, group, |ui| {
                for (index, (keys, what)) in entries.iter().enumerate() {
                    crate::ui::kit::setting_row(ui, what, None, index + 1 < entries.len(), |ui| {
                        for (n, combo) in keys.split(" / ").enumerate() {
                            if n > 0 {
                                ui.label(RichText::new("/").color(theme::subtext()));
                            }
                            crate::ui::kit::keycap(ui, combo);
                        }
                    });
                }
            });
        }
        return;
    }
    crate::ui::kit::card(ui, "Apariencia", |ui| {
        crate::ui::kit::setting_row(ui, "Tema", None, true, |ui| {
            ui.selectable_value(&mut settings.theme, ThemeChoice::Dark, "Oscuro");
            ui.selectable_value(&mut settings.theme, ThemeChoice::Light, "Claro");
            ui.selectable_value(&mut settings.theme, ThemeChoice::System, "Sistema");
        });
        crate::ui::kit::setting_row(ui, "Acento", Some("Color de lo seleccionado"), true, |ui| {
            for (choice, label) in AccentChoice::ALL {
                ui.selectable_value(&mut settings.accent, choice, label);
            }
        });
        crate::ui::kit::setting_row(ui, "Densidad", Some("Altura de las filas de las listas"), true, |ui| {
            ui.selectable_value(&mut settings.compact, false, "Cómoda");
            ui.selectable_value(&mut settings.compact, true, "Compacta");
        });
        crate::ui::kit::setting_row(ui, "Usar barra de título del sistema", Some("Se aplica al reiniciar"), true, |ui| {
            crate::ui::kit::toggle(ui, &mut settings.system_titlebar);
        });
        if settings.system_titlebar {
            crate::ui::kit::setting_row(ui, "Barra de menú", Some("Si está oculta, Alt la muestra"), true, |ui| {
                crate::ui::kit::toggle(ui, &mut settings.menu_bar);
            });
        } else {
            crate::ui::kit::setting_row(ui, "Menú en la barra de título", Some("Si no, queda tras el botón ⋯"), true, |ui| {
                crate::ui::kit::toggle(ui, &mut settings.titlebar_menu);
            });
            crate::ui::kit::setting_row(ui, "Altura de la barra de título", None, true, |ui| {
                ui.selectable_value(&mut settings.titlebar_compact, false, "Cómoda");
                ui.selectable_value(&mut settings.titlebar_compact, true, "Compacta");
            });
        }
        crate::ui::kit::setting_row(ui, "Confirmar al cerrar pestañas", Some("Solo si tienen algo en ejecución"), true, |ui| {
            crate::ui::kit::toggle(ui, &mut settings.confirm_close);
        });
        crate::ui::kit::setting_row(ui, "Confirmar al pegar varias líneas", Some("Evita ejecutar comandos sin querer"), true, |ui| {
            crate::ui::kit::toggle(ui, &mut settings.confirm_paste);
        });
        crate::ui::kit::setting_row(ui, "Al iniciar", Some("Pestañas de la sesión anterior"), false, |ui| {
            ui.selectable_value(&mut settings.on_start, StartupChoice::Ask, "Preguntar");
            ui.selectable_value(&mut settings.on_start, StartupChoice::Restore, "Restaurar");
            ui.selectable_value(&mut settings.on_start, StartupChoice::Fresh, "Vacío");
        });
    });

    crate::ui::kit::card(ui, "Terminal", |ui| {
        crate::ui::kit::setting_row(ui, "Terminal local predeterminado", Some("Para el botón + y Ctrl+Shift+T"), true, |ui| {
            let profiles: Vec<&Connection> = connections.iter().filter(|c| c.is_local()).collect();
            let current = settings.local_profile.and_then(|id| profiles.iter().find(|c| c.id == Some(id)));
            egui::ComboBox::from_id_salt("local-profile")
                .width(190.0)
                .selected_text(current.map_or("Shell del sistema".to_string(), |c| c.name.clone()))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut settings.local_profile, None, "Shell del sistema");
                    for profile in &profiles {
                        ui.selectable_value(&mut settings.local_profile, profile.id, profile.name.as_str());
                    }
                })
                .response
                .hand();
        });
        crate::ui::kit::setting_row(ui, "Fuente", Some("Monoespaciada, instalada en el equipo"), true, |ui| {
            font_picker(ui, &mut settings.font_family);
        });
        crate::ui::kit::setting_row(ui, "Tamaño de la letra", None, true, |ui| {
            ui.add(egui::Slider::new(&mut settings.font_size, MIN_FONT_SIZE..=MAX_FONT_SIZE).step_by(1.0).suffix(" pt"));
        });
        crate::ui::kit::setting_row(ui, "Cursor", None, true, |ui| {
            ui.selectable_value(&mut settings.cursor, CursorChoice::Block, "Bloque");
            ui.selectable_value(&mut settings.cursor, CursorChoice::Beam, "Barra");
            ui.selectable_value(&mut settings.cursor, CursorChoice::Underline, "Subrayado");
        });
        crate::ui::kit::setting_row(ui, "Cursor parpadeante", None, true, |ui| {
            crate::ui::kit::toggle(ui, &mut settings.blink);
        });
        crate::ui::kit::setting_row(ui, "Paleta de colores", Some("Los 16 colores ANSI de los programas"), true, |ui| {
            for (choice, label) in AnsiChoice::ALL {
                ui.selectable_value(&mut settings.ansi, choice, label);
            }
        });
        crate::ui::kit::setting_row(ui, "Negrita brillante", Some("La negrita aclara los colores ANSI 0-7"), true, |ui| {
            crate::ui::kit::toggle(ui, &mut settings.bold_bright);
        });
        crate::ui::kit::setting_row(ui, "Historial", Some("Se aplica a las sesiones nuevas"), false, |ui| {
            ui.add(egui::DragValue::new(&mut settings.scrollback).range(MIN_SCROLLBACK..=MAX_SCROLLBACK).speed(100.0).suffix(" líneas"));
        });
    });

    crate::ui::kit::card(ui, "Avisos", |ui| {
        let mut notify = settings.notify_after > 0;
        crate::ui::kit::setting_row(ui, "Avisar al terminar la actividad", Some("De una pestaña que no está a la vista"), notify, |ui| {
            if crate::ui::kit::toggle(ui, &mut notify).changed() {
                settings.notify_after = if notify { 15 } else { 0 };
            }
        });
        if notify {
            crate::ui::kit::setting_row(ui, "Solo si duró al menos", None, false, |ui| {
                ui.add(egui::DragValue::new(&mut settings.notify_after).range(1..=3600).suffix(" s"));
            });
        }
    });

    crate::ui::kit::card(ui, "Secretos", |ui| {
        crate::ui::kit::setting_row(ui, "Modo de cifrado de credenciales", Some("Dónde se guardan las contraseñas. Se aplica al reiniciar"), false, |ui| {
            let name = |master: bool| if master { "Clave maestra" } else { "Llavero del sistema" };
            egui::ComboBox::from_id_salt("secrets-mode")
                .width(190.0)
                .selected_text(name(secrets.master))
                .show_ui(ui, |ui| {
                    let keyring_label = if secrets.keyring_available { "Llavero del sistema" } else { "Llavero del sistema (no disponible)" };
                    ui.add_enabled_ui(secrets.keyring_available, |ui| ui.selectable_value(&mut secrets.master, false, keyring_label));
                    ui.selectable_value(&mut secrets.master, true, "Clave maestra");
                })
                .response
                .hand();
        });
        if secrets.master != secrets.active_master {
            let text = format!("En uso ahora: {}. El cambio se aplica al reiniciar.", name_of(secrets.active_master));
            ui.label(RichText::new(text).small().color(theme::subtext()));
        } else if !secrets.keyring_available {
            ui.label(RichText::new("Este sistema no tiene llavero disponible.").small().color(theme::subtext()));
        }
    });
}

pub(super) fn name_of(master: bool) -> &'static str {
    if master { "clave maestra" } else { "llavero del sistema" }
}

pub(super) const DEFAULT_FONT: &str = "JetBrains Mono (incluida)";

/// Botón con la fuente actual que abre una lista con búsqueda de las monoespaciadas instaladas.
pub(super) fn font_picker(ui: &mut egui::Ui, family: &mut Option<String>) {
    let current = family.clone().unwrap_or_else(|| DEFAULT_FONT.to_string());
    let button = crate::ui::kit::button_with(ui, egui::Button::new(format!("{current}  {}", crate::ui::icons::CARET_DOWN)).min_size(egui::vec2(0.0, theme::height::FIELD)), crate::ui::kit::Variant::Secondary);
    egui::Popup::menu(&button).show(|ui| {
        ui.set_width(300.0);
        // Filas compactas y con el mismo margen que el buscador (los valores por defecto de egui son para botones).
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.spacing_mut().button_padding = egui::vec2(8.0, 3.0);
        ui.spacing_mut().interact_size.y = 26.0;
        let search_id = ui.make_persistent_id("font-search");
        let mut query: String = ui.data_mut(|d| d.get_temp(search_id)).unwrap_or_default();
        let field = crate::ui::kit::search_field(ui, &mut query, "Buscar fuente", f32::INFINITY);
        if button.clicked() {
            field.request_focus();
        }
        ui.data_mut(|d| d.insert_temp(search_id, query.clone()));
        ui.add_space(theme::space::S);
        let needle = query.trim().to_lowercase();
        let families = crate::ui::fonts::monospace_families();
        egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
            // Deja sitio a la barra de desplazamiento flotante para que no tape el final de las filas.
            ui.set_max_width(ui.available_width() - 10.0);
            if needle.is_empty() || DEFAULT_FONT.to_lowercase().contains(&needle) {
                if ui.selectable_label(family.is_none(), DEFAULT_FONT).hand().clicked() {
                    *family = None;
                    ui.close();
                }
            }
            for name in families.iter().filter(|n| needle.is_empty() || n.to_lowercase().contains(&needle)) {
                if ui.selectable_label(family.as_deref() == Some(name.as_str()), name).hand().clicked() {
                    *family = Some(name.clone());
                    ui.close();
                }
            }
            if families.is_empty() {
                ui.label(RichText::new("No se encontró ninguna fuente monoespaciada instalada.").color(theme::subtext()));
            }
        });
    });
}

pub(super) fn known_hosts_body(ui: &mut egui::Ui, view: &mut KnownHostsView, connections: &[Connection]) {
    view.ensure_loaded();
    crate::ui::kit::search_field(ui, &mut view.query, "Buscar por host, conexión o huella", f32::INFINITY);
    if view.error.is_none() && !view.entries.is_empty() {
        ui.label(RichText::new(format!("{} entradas en {}", view.entries.len(), crate::connect::ssh::default_known_hosts().display())).small().color(theme::subtext()));
    }
    ui.add_space(theme::space::S);
    if let Some(error) = &view.error {
        ui.label(RichText::new(error).color(theme::red()));
        return;
    }
    if view.entries.is_empty() {
        ui.label(RichText::new("Aún no hay hosts conocidos. Aparecen aquí cuando confías en un servidor al conectar.").color(theme::subtext()));
        return;
    }
    let query = view.query.trim().to_lowercase();
    let mut shown = 0;
    for entry in &view.entries {
        let name = entry.display();
        let endpoints = entry.endpoints();
        let connection = connections.iter().find(|c| endpoints.iter().any(|(h, p)| h.eq_ignore_ascii_case(&c.host) && *p == c.port));
        let haystack = format!("{} {} {}", entry.hosts.join(" "), connection.map_or("", |c| c.name.as_str()), entry.fingerprint).to_lowercase();
        if !query.is_empty() && !haystack.contains(&query) {
            continue;
        }
        shown += 1;
        let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 48.0), egui::Sense::hover());
        if response.hovered() {
            ui.painter().rect_filled(rect, egui::CornerRadius::same(theme::radius::CONTROL), theme::surface());
        }
        let title = ui.painter().text(
            rect.min + egui::vec2(10.0, 15.0),
            egui::Align2::LEFT_CENTER,
            &name,
            FontId::new(theme::size::BODY, crate::ui::strong_family()),
            if entry.hashed { theme::subtext() } else { theme::text() },
        );
        if let Some(connection) = connection {
            ui.painter().text(egui::pos2(title.max.x + 10.0, rect.min.y + 15.0), egui::Align2::LEFT_CENTER, &connection.name, FontId::proportional(theme::size::SMALL), theme::accent());
        }
        let short: String = entry.fingerprint.chars().take(28).collect();
        ui.painter().text(
            rect.min + egui::vec2(10.0, 34.0),
            egui::Align2::LEFT_CENTER,
            format!("{}  ·  {short}…", entry.key_type),
            FontId::new(theme::size::SMALL, egui::FontFamily::Monospace),
            theme::subtext(),
        );
        for (offset, glyph, tip) in [(56.0, crate::ui::icons::COPY, "Copiar la huella"), (24.0, crate::ui::icons::TRASH, "Olvidar este host")] {
            // Cada botón se coloca a la derecha de la fila, con una identidad propia para que no se mezclen entre filas.
            let slot = egui::Rect::from_center_size(egui::pos2(rect.max.x - offset, rect.center().y), egui::vec2(26.0, 26.0));
            let clicked = crate::ui::kit::IconButton::new(glyph).show_at(ui, slot, ("known-host", entry.index, glyph)).on_hover_text(tip).clicked();
            if clicked {
                if glyph == crate::ui::icons::COPY {
                    ui.ctx().copy_text(entry.fingerprint.clone());
                } else {
                    view.delete = Some(entry.clone());
                }
            }
        }
    }
    if shown == 0 {
        ui.label(RichText::new("Ningún host coincide con la búsqueda.").color(theme::subtext()));
    }
}
