//! Página de ajustes: secciones, atajos y hosts conocidos.

use crate::i18n::t;
use super::*;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum SettingsTab {
    #[default]
    General,
    Atajos,
    Hosts,
    Passwords,
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
                self.error = Some(crate::tf!("settings-known-hosts-failed", "error" => error.to_string()));
            }
        }
    }
}

pub enum PasswordRequest {
    New,
    Edit(crate::data::store::SavedPassword),
    Delete(crate::data::store::SavedPassword),
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
        "settings-pestanas",
        &[
            ("Ctrl+Shift+P", "settings-paleta-de-comandos"),
            ("Ctrl+,", "settings-abrir-los-ajustes"),
            ("Ctrl+Shift+N", "dialogs-nueva-conexion"),
            ("Alt+1 … 9", "settings-ir-a-la-pestana-1"),
            ("Ctrl+Tab / Ctrl+Shift+Tab", "settings-pestana-siguiente-anterior"),
            ("settings-doble-clic-en-la-pestana", "settings-renombrarla"),
        ],
    ),
    (
        "conn-edit-terminal",
        &[
            ("Ctrl+Shift+C / V", "settings-copiar-pegar"),
            ("Ctrl+Shift+F", "settings-buscar-en-la-salida"),
            ("Ctrl+Alt+P", "settings-escribir-una-contrasena-guardada"),
            ("Ctrl+Shift+D / E", "settings-dividir-a-la-derecha-hacia"),
            ("Ctrl+Shift+W", "settings-cerrar-el-panel-o-la"),
            ("Alt+flechas", "settings-ir-al-panel-de-al"),
            ("Alt+Shift+flechas", "settings-mover-el-borde-del-panel"),
            ("Ctrl+Shift+Z", "settings-ampliar-o-restaurar-el-panel"),
            ("Ctrl+= / Ctrl+− / Ctrl+0", "settings-aumentar-reducir-restablecer-la-le"),
        ],
    ),
    (
        "settings-ventana",
        &[("Ctrl+Shift+B", "settings-mostrar-u-ocultar-la-barra"), ("Ctrl+Shift+K", "settings-buscar-en-la-barra-lateral"), ("F11", "settings-pantalla-completa")],
    ),
    (
        "settings-explorador-de-archivos",
        &[
            ("F2", "explorer-renombrar"),
            ("explorer-supr", "explorer-eliminar"),
            ("Enter", "settings-abrir-la-carpeta"),
            ("settings-retroceso", "settings-subir-un-nivel"),
            ("F5", "settings-actualizar"),
            ("Ctrl+A", "pane-seleccionar-todo"),
            ("Ctrl+L", "settings-escribir-una-ruta"),
        ],
    ),
];

/// Lista de secciones de la página de ajustes.
pub(super) const SETTINGS_SECTIONS: [(SettingsTab, &str); 4] = [
    (SettingsTab::General, "settings-general"),
    (SettingsTab::Atajos, "settings-atajos-de-teclado"),
    (SettingsTab::Passwords, "settings-contrasenas"),
    (SettingsTab::Hosts, "settings-hosts-conocidos"),
];

pub struct PasswordsPage<'a> {
    pub passwords: &'a [crate::data::store::SavedPassword],
    pub request: &'a mut Option<PasswordRequest>,
}

pub(super) const SETTINGS_MAX_WIDTH: f32 = 720.0;

/// Página de ajustes de una pestaña: lista de secciones a la izquierda y el contenido a la derecha.
pub fn settings_page(ui: &mut egui::Ui, tab: &mut SettingsTab, settings: &mut Settings, secrets: &mut SecretsChoice, hosts: &mut KnownHostsView, connections: &[Connection], passwords: PasswordsPage<'_>) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 4.0);
        ui.vertical(|ui| {
            ui.set_width(168.0);
            for (choice, label) in SETTINGS_SECTIONS {
                let selected = *tab == choice;
                let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::click());
                let response = response.hand();
                // La sección elegida lleva el tinte del acento y una marca a la izquierda; el hover, solo un gris
                // suave: así no se confunden al pasar el ratón por otra sección.
                if selected {
                    ui.painter().rect_filled(rect, egui::CornerRadius::same(theme::radius::CONTROL), theme::accent().gamma_multiply(0.16));
                    let mark = egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 8.0), egui::vec2(3.0, rect.height() - 16.0));
                    ui.painter().rect_filled(mark, egui::CornerRadius::same(theme::radius::TINY), theme::accent());
                } else if response.hovered() {
                    ui.painter().rect_filled(rect, egui::CornerRadius::same(theme::radius::CONTROL), theme::hover());
                }
                let font = FontId::new(theme::size::BODY, if selected { crate::ui::strong_family() } else { egui::FontFamily::Proportional });
                ui.painter().text(rect.min + egui::vec2(12.0, 16.0), egui::Align2::LEFT_CENTER, t(label), font, if selected { theme::text() } else { theme::subtext() });
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
                settings_content(ui, *tab, settings, secrets, hosts, connections, passwords);
            });
        });
    });
}

pub(super) fn settings_content(ui: &mut egui::Ui, tab: SettingsTab, settings: &mut Settings, secrets: &mut SecretsChoice, hosts: &mut KnownHostsView, connections: &[Connection], passwords: PasswordsPage<'_>) {
    if tab == SettingsTab::Passwords {
        passwords_body(ui, passwords);
        return;
    }
    if tab == SettingsTab::Hosts {
        known_hosts_body(ui, hosts, connections);
        return;
    }
    if tab == SettingsTab::Atajos {
        for (group, entries) in SHORTCUTS {
            crate::ui::kit::card(ui, t(group), |ui| {
                for (index, (keys, what)) in entries.iter().enumerate() {
                    crate::ui::kit::setting_row(ui, &crate::ui::shortcuts::label(t(what)), None, index + 1 < entries.len(), |ui| {
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
    crate::ui::kit::card(ui, t("settings-apariencia"), |ui| {
        crate::ui::kit::setting_row(ui, t("settings-tema"), None, true, |ui| {
            ui.selectable_value(&mut settings.theme, ThemeChoice::Dark, t("settings-oscuro"));
            ui.selectable_value(&mut settings.theme, ThemeChoice::Light, t("settings-claro"));
            ui.selectable_value(&mut settings.theme, ThemeChoice::System, t("settings-sistema"));
        });
        crate::ui::kit::setting_row(ui, t("settings-language"), Some(t("settings-language-hint")), true, |ui| {
            ui.selectable_value(&mut settings.language, None, t("settings-language-auto"));
            for lang in crate::i18n::Lang::ALL {
                ui.selectable_value(&mut settings.language, Some(lang), lang.native_name());
            }
        });
        crate::ui::kit::setting_row(ui, t("settings-acento"), Some(t("settings-color-de-lo-seleccionado")), true, |ui| {
            for (choice, label) in AccentChoice::ALL {
                ui.selectable_value(&mut settings.accent, choice, t(label));
            }
        });
        if !crate::data::settings::NATIVE_CHROME {
            crate::ui::kit::setting_row(ui, t("settings-usar-barra-de-titulo-del"), Some(t("settings-se-aplica-al-reiniciar")), true, |ui| {
                crate::ui::kit::toggle(ui, &mut settings.system_titlebar);
            });
        }
        if settings.system_titlebar {
            crate::ui::kit::setting_row(ui, t("settings-barra-de-menu"), Some(t("settings-si-esta-oculta-alt-la")), true, |ui| {
                crate::ui::kit::toggle(ui, &mut settings.menu_bar);
            });
        } else {
            if !crate::data::settings::NATIVE_CHROME {
                crate::ui::kit::setting_row(ui, t("settings-menu-en-la-barra-de"), Some(t("settings-si-no-queda-tras-el")), true, |ui| {
                    crate::ui::kit::toggle(ui, &mut settings.titlebar_menu);
                });
            }
            crate::ui::kit::setting_row(ui, t("settings-altura-de-la-barra-de"), None, true, |ui| {
                ui.selectable_value(&mut settings.titlebar_compact, false, t("settings-comoda"));
                ui.selectable_value(&mut settings.titlebar_compact, true, t("settings-compacta"));
            });
        }
        crate::ui::kit::setting_row(ui, t("settings-confirmar-al-cerrar-pestanas"), Some(t("settings-solo-si-tienen-algo-en")), true, |ui| {
            crate::ui::kit::toggle(ui, &mut settings.confirm_close);
        });
        crate::ui::kit::setting_row(ui, t("settings-confirmar-al-pegar-varias-lineas"), Some(t("settings-evita-ejecutar-comandos-sin-querer")), true, |ui| {
            crate::ui::kit::toggle(ui, &mut settings.confirm_paste);
        });
        crate::ui::kit::setting_row(ui, t("settings-al-iniciar"), Some(t("settings-pestanas-de-la-sesion-anterior")), false, |ui| {
            ui.selectable_value(&mut settings.on_start, StartupChoice::Ask, t("settings-preguntar"));
            ui.selectable_value(&mut settings.on_start, StartupChoice::Restore, t("dialogs-restaurar"));
            ui.selectable_value(&mut settings.on_start, StartupChoice::Fresh, t("settings-vacio"));
        });
    });

    crate::ui::kit::card(ui, t("conn-edit-terminal"), |ui| {
        crate::ui::kit::setting_row(ui, t("settings-terminal-local-predeterminado"), Some(&crate::ui::shortcuts::label(t("settings-para-el-boton-y-ctrl"))), true, |ui| {
            let profiles: Vec<&Connection> = connections.iter().filter(|c| c.is_local()).collect();
            let current = settings.local_profile.and_then(|id| profiles.iter().find(|c| c.id == Some(id)));
            let current_name = current.map_or(t("settings-shell-del-sistema").to_string(), |c| c.name.clone());
            crate::ui::kit::select(ui, "local-profile", 190.0, current_name, |options| {
                options.value(&mut settings.local_profile, None, t("settings-shell-del-sistema"));
                for profile in &profiles {
                    options.value(&mut settings.local_profile, profile.id, profile.name.as_str());
                }
            });
        });
        crate::ui::kit::setting_row(ui, t("settings-fuente"), Some(t("settings-monoespaciada-instalada-en-el-equi")), true, |ui| {
            font_picker(ui, &mut settings.font_family);
        });
        crate::ui::kit::setting_row(ui, t("settings-tamano-de-la-letra"), None, true, |ui| {
            ui.add(egui::Slider::new(&mut settings.font_size, MIN_FONT_SIZE..=MAX_FONT_SIZE).step_by(1.0).suffix(" pt"));
        });
        crate::ui::kit::setting_row(ui, t("settings-cursor"), None, true, |ui| {
            ui.selectable_value(&mut settings.cursor, CursorChoice::Block, t("settings-bloque"));
            ui.selectable_value(&mut settings.cursor, CursorChoice::Beam, t("settings-barra"));
            ui.selectable_value(&mut settings.cursor, CursorChoice::Underline, t("settings-subrayado"));
        });
        crate::ui::kit::setting_row(ui, t("settings-cursor-parpadeante"), None, true, |ui| {
            crate::ui::kit::toggle(ui, &mut settings.blink);
        });
        crate::ui::kit::setting_row(ui, t("settings-tema-del-terminal"), Some(t("settings-fondo-texto-cursor-y-los")), true, |ui| {
            let themes = &crate::data::term_theme::THEMES;
            let current = settings.ansi.index() as usize;
            crate::ui::kit::select(ui, "terminal-theme", 250.0, themes[current].name, |options| {
                for (index, theme) in themes.iter().enumerate() {
                    let preview = |painter: &egui::Painter, rect: egui::Rect| paint_theme_preview(painter, rect, index as u8);
                    if options.option_decorated(index == current, theme.name, preview) {
                        settings.ansi = AnsiChoice::new(index as u8);
                    }
                }
            });
        });
        crate::ui::kit::setting_row(ui, t("settings-negrita-brillante"), Some(t("settings-la-negrita-aclara-los-colores")), true, |ui| {
            crate::ui::kit::toggle(ui, &mut settings.bold_bright);
        });
        crate::ui::kit::setting_row(ui, t("settings-historial"), Some(t("settings-se-aplica-a-las-sesiones")), false, |ui| {
            ui.add(egui::DragValue::new(&mut settings.scrollback).range(MIN_SCROLLBACK..=MAX_SCROLLBACK).speed(100.0).suffix(format!(" {}", t("settings-lines-unit"))));
        });
    });

    crate::ui::kit::card(ui, t("settings-avisos"), |ui| {
        let mut notify = settings.notify_after > 0;
        crate::ui::kit::setting_row(ui, t("settings-avisar-al-terminar-la-actividad"), Some(t("settings-de-una-pestana-que-no")), notify, |ui| {
            if crate::ui::kit::toggle(ui, &mut notify).changed() {
                settings.notify_after = if notify { 15 } else { 0 };
            }
        });
        if notify {
            crate::ui::kit::setting_row(ui, t("settings-solo-si-duro-al-menos"), None, false, |ui| {
                ui.add(egui::DragValue::new(&mut settings.notify_after).range(1..=3600).suffix(" s"));
            });
        }
    });

    crate::ui::kit::card(ui, t("settings-secretos"), |ui| {
        crate::ui::kit::setting_row(ui, t("settings-modo-de-cifrado-de-credenciales"), Some(t("settings-donde-se-guardan-las-contrasenas")), false, |ui| {
            let name = |master: bool| if master { t("settings-clave-maestra") } else { t("settings-llavero-del-sistema") };
            let keyring_label = if secrets.keyring_available { t("settings-llavero-del-sistema") } else { t("settings-llavero-del-sistema-no-disponible") };
            crate::ui::kit::select(ui, "secrets-mode", 190.0, name(secrets.master), |options| {
                if options.option_enabled(!secrets.master, keyring_label, secrets.keyring_available) {
                    secrets.master = false;
                }
                options.value(&mut secrets.master, true, t("settings-clave-maestra"));
            });
        });
        if secrets.master != secrets.active_master {
            let text = crate::tf!("settings-in-use-now", "name" => name_of(secrets.active_master));
            ui.label(RichText::new(text).small().color(theme::subtext()));
        } else if !secrets.keyring_available {
            ui.label(RichText::new(t("settings-este-sistema-no-tiene-llavero")).small().color(theme::subtext()));
        }
    });
}

pub(super) fn name_of(master: bool) -> &'static str {
    if master { t("prompts-clave-maestra") } else { t("settings-llavero-del-sistema-2") }
}

/// Vista previa de un tema en su fila: un recuadro con su fondo y texto, y los seis colores principales.
pub(super) fn paint_theme_preview(painter: &egui::Painter, rect: egui::Rect, index: u8) {
    let (bg, fg, colors) = theme::term_theme_preview(index);
    let chip = egui::Rect::from_center_size(egui::pos2(rect.max.x - 8.0 - 6.0 * 11.0 - 14.0, rect.center().y), egui::vec2(26.0, 16.0));
    painter.rect_filled(chip, egui::CornerRadius::same(theme::radius::SMALL), bg);
    painter.rect_stroke(chip, egui::CornerRadius::same(theme::radius::SMALL), egui::Stroke::new(theme::stroke::HAIRLINE, theme::subtext()), egui::StrokeKind::Inside);
    painter.text(chip.center(), egui::Align2::CENTER_CENTER, "Aa", egui::FontId::new(theme::size::CAPTION, egui::FontFamily::Proportional), fg);
    for (i, color) in colors.iter().enumerate() {
        painter.circle_filled(egui::pos2(rect.max.x - 8.0 - 6.0 * 11.0 + 5.5 + i as f32 * 11.0, rect.center().y), 4.0, *color);
    }
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
        let field = crate::ui::kit::search_field(ui, &mut query, t("settings-buscar-fuente"), f32::INFINITY);
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
                ui.label(RichText::new(t("settings-no-se-encontro-ninguna-fuente")).color(theme::subtext()));
            }
        });
    });
    crate::ui::kit::return_focus_after_menu(ui.ctx(), &button);
}

/// Sección de contraseñas: las del gestor, para escribirlas en el terminal con Ctrl+Alt+P.
pub(super) fn passwords_body(ui: &mut egui::Ui, page: PasswordsPage<'_>) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = theme::space::XS;
        ui.label(RichText::new(t("settings-escribelas-en-el-terminal-con")).color(theme::subtext()));
        crate::ui::kit::keycap(ui, "Ctrl+Alt+P");
    });
    ui.add_space(theme::space::M);
    if crate::ui::kit::button(ui, t("dialogs-nueva-contrasena"), crate::ui::kit::Variant::PrimaryOutline).clicked() {
        *page.request = Some(PasswordRequest::New);
    }
    ui.add_space(theme::space::S);
    if page.passwords.is_empty() {
        ui.label(RichText::new(t("settings-aun-no-hay-contrasenas-guardadas")).color(theme::subtext()));
        return;
    }
    crate::ui::kit::card_frame().inner_margin(egui::Margin::symmetric(6, 4)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.set_width(ui.available_width());
        for password in page.passwords {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::click());
            let over = ui.rect_contains_pointer(rect);
            if over {
                ui.painter().rect_filled(rect, egui::CornerRadius::same(theme::radius::CONTROL), theme::hover());
            }
            ui.painter().text(rect.min + egui::vec2(18.0, 22.0), egui::Align2::CENTER_CENTER, crate::ui::icons::LOCK, FontId::proportional(theme::size::ICON), theme::subtext());
            let (title_y, has_user) = if password.user.is_empty() { (22.0, false) } else { (14.0, true) };
            let clip = ui.painter().with_clip_rect(rect.with_max_x(rect.max.x - 72.0));
            clip.text(rect.min + egui::vec2(36.0, title_y), egui::Align2::LEFT_CENTER, &password.name, FontId::new(theme::size::BODY, crate::ui::strong_family()), theme::text());
            if has_user {
                clip.text(rect.min + egui::vec2(36.0, 31.0), egui::Align2::LEFT_CENTER, &password.user, FontId::proportional(theme::size::SMALL), theme::subtext());
            }
            for (offset, glyph, tip) in [(52.0, crate::ui::icons::PENCIL, t("settings-editar")), (22.0, crate::ui::icons::TRASH, t("explorer-eliminar"))] {
                let slot = egui::Rect::from_center_size(egui::pos2(rect.max.x - offset, rect.center().y), egui::vec2(26.0, 26.0));
                let color = if glyph == crate::ui::icons::TRASH { theme::red() } else { theme::text() };
                let clicked = crate::ui::kit::IconButton::new(glyph).color(color).show_at(ui, slot, ("password", password.id, glyph)).on_hover_text(tip).clicked();
                if clicked {
                    *page.request = Some(if glyph == crate::ui::icons::TRASH { PasswordRequest::Delete(password.clone()) } else { PasswordRequest::Edit(password.clone()) });
                }
            }
            if response.hand().double_clicked() {
                *page.request = Some(PasswordRequest::Edit(password.clone()));
            }
        }
    });
}

pub(super) fn known_hosts_body(ui: &mut egui::Ui, view: &mut KnownHostsView, connections: &[Connection]) {
    view.ensure_loaded();
    crate::ui::kit::search_field(ui, &mut view.query, t("settings-buscar-por-host-conexion-o"), f32::INFINITY);
    if view.error.is_none() && !view.entries.is_empty() {
        ui.label(RichText::new(crate::tf!("settings-known-hosts-count", "n" => view.entries.len(), "path" => crate::connect::ssh::default_known_hosts().display().to_string())).small().color(theme::subtext()));
    }
    ui.add_space(theme::space::S);
    if let Some(error) = &view.error {
        ui.label(RichText::new(error).color(theme::red()));
        return;
    }
    if view.entries.is_empty() {
        ui.label(RichText::new(t("settings-aun-no-hay-hosts-conocidos")).color(theme::subtext()));
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
        for (offset, glyph, tip) in [(56.0, crate::ui::icons::COPY, t("settings-copiar-la-huella")), (24.0, crate::ui::icons::TRASH, t("settings-olvidar-este-host"))] {
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
        ui.label(RichText::new(t("settings-ningun-host-coincide-con-la")).color(theme::subtext()));
    }
}
