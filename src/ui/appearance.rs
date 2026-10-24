//! Apariencia: tema, estilos, fuentes y opciones del terminal.

use super::*;

impl App {
    pub(super) fn term_options(&self) -> TermOptions {
        use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};
        let shape = match self.settings.cursor {
            CursorChoice::Block => CursorShape::Block,
            CursorChoice::Beam => CursorShape::Beam,
            CursorChoice::Underline => CursorShape::Underline,
        };
        TermOptions { history: self.settings.scrollback, cursor: CursorStyle { shape, blinking: self.settings.blink } }
    }

    /// Guarda los ajustes y los aplica a las sesiones abiertas.
    pub(super) fn apply_settings(&mut self) {
        theme::set_bold_bright(self.settings.bold_bright);
        theme::set_ansi_scheme(self.settings.ansi.index());
        if self.settings.font_family != self.loaded_font {
            if let Some(problem) = install_fonts(&self.ctx, self.settings.font_family.as_deref()) {
                self.settings.font_family = None;
                self.notify(problem);
            }
            self.loaded_font = self.settings.font_family.clone();
        }
        let _ = self.settings.save(&self.store.lock().unwrap());
        let options = self.term_options();
        for tab in &self.tabs {
            if let Body::Terminal { panes, .. } = &tab.body {
                panes.for_each(&mut |_, pane| pane.session.set_options(options));
            }
        }
    }

    /// Cambia entre tema claro y oscuro según el ajuste (o el del sistema) y reinstala los estilos.
    pub(super) fn sync_theme(&mut self, ctx: &egui::Context) {
        let light = match self.settings.theme {
            ThemeChoice::Light => true,
            ThemeChoice::Dark => false,
            ThemeChoice::System => ctx.system_theme() == Some(egui::Theme::Light),
        };
        let (accent, compact) = (self.settings.accent.index(), self.settings.compact);
        if light != theme::is_light() || accent != theme::accent_index() || compact != theme::is_compact() {
            theme::set_light(light);
            theme::set_accent(accent);
            theme::set_compact(compact);
            install_visuals(ctx);
        }
    }

    pub(super) fn set_font_size(&mut self, size: f32) {
        let size = size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        if (size - self.settings.font_size).abs() > f32::EPSILON {
            self.settings.font_size = size;
            let _ = self.settings.save(&self.store.lock().unwrap());
        }
    }
}

/// Instala las fuentes; con `family` (una familia instalada en el equipo) el terminal usa esa en lugar de JetBrains Mono.
/// Si no se puede usar, se queda JetBrains Mono y se devuelve el motivo.
pub(super) fn install_fonts(ctx: &egui::Context, family: Option<&str>) -> Option<String> {
    let mut problem = None;
    let system = family.and_then(|name| match crate::ui::fonts::load_family(name) {
        Ok(faces) if faces.iter().all(|f| ab_glyph::FontRef::try_from_slice_and_index(&f.data, f.index).is_ok()) => Some(faces),
        Ok(_) => {
            problem = Some(format!("La fuente «{name}» no se puede usar en el terminal"));
            None
        }
        Err(error) => {
            problem = Some(error);
            None
        }
    });
    let mut fonts = FontDefinitions::default();
    let bundled: [(&str, &'static [u8]); 4] = [
        ("jb-regular", include_bytes!("../../assets/JetBrainsMono-Regular.ttf")),
        ("jb-bold", include_bytes!("../../assets/JetBrainsMono-Bold.ttf")),
        ("jb-italic", include_bytes!("../../assets/JetBrainsMono-Italic.ttf")),
        ("jb-bolditalic", include_bytes!("../../assets/JetBrainsMono-BoldItalic.ttf")),
    ];
    match system {
        Some(faces) => {
            for ((name, _), face) in bundled.iter().zip(faces) {
                let mut data = FontData::from_owned(face.data);
                data.index = face.index;
                fonts.font_data.insert((*name).into(), data.into());
            }
        }
        None => {
            for (name, bytes) in bundled {
                fonts.font_data.insert(name.into(), FontData::from_static(bytes).into());
            }
        }
    }
    let load = |name: &str, bytes: &'static [u8], fonts: &mut FontDefinitions| {
        fonts.font_data.insert(name.into(), FontData::from_static(bytes).into());
    };
    // La interfaz usa una sans neutra con buena cobertura de acentos; el terminal, JetBrains Mono.
    load("ui-regular", include_bytes!("../../assets/NotoSans-Regular.ttf"), &mut fonts);
    load("ui-semibold", include_bytes!("../../assets/NotoSans-SemiBold.ttf"), &mut fonts);
    load("icons", include_bytes!("../../assets/Phosphor.ttf"), &mut fonts);
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "jb-regular".into());
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "ui-regular".into());
    fonts.families.insert(bold_family(), vec!["jb-bold".into()]);
    fonts.families.insert(pane::italic_family(), vec!["jb-italic".into()]);
    fonts.families.insert(pane::bold_italic_family(), vec!["jb-bolditalic".into()]);
    fonts.families.insert(strong_family(), vec!["ui-semibold".into(), "ui-regular".into()]);
    // Los iconos van de último recurso: cualquier texto puede llevar uno sin cambiar de familia.
    for family in [FontFamily::Proportional, strong_family()] {
        fonts.families.entry(family).or_default().push("icons".into());
    }
    ctx.set_fonts(fonts);
    problem
}

pub fn strong_family() -> FontFamily {
    FontFamily::Name("ui-strong".into())
}

/// Escala de espaciado (4/8/12/16) y radios (controles 6, diálogos 10) de toda la interfaz.
pub(super) fn install_visuals(ctx: &egui::Context) {
    ctx.set_theme(if theme::is_light() { egui::Theme::Light } else { egui::Theme::Dark });
    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (TextStyle::Small, FontId::new(theme::size::SMALL, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(theme::size::BODY, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(theme::size::BODY, FontFamily::Proportional)),
        (TextStyle::Heading, FontId::new(theme::size::DISPLAY, strong_family())),
        (TextStyle::Monospace, FontId::new(theme::size::BODY, FontFamily::Monospace)),
    ]
    .into();
    style.animation_time = 0.12;
    // Barras finas que solo aparecen al acercar el ratón o al desplazar.
    style.spacing.scroll = egui::style::ScrollStyle::floating();
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.interact_size.y = 30.0;

    let radius = CornerRadius::same(theme::radius::CONTROL);
    style.visuals = if theme::is_light() { egui::Visuals::light() } else { egui::Visuals::dark() };
    let v = &mut style.visuals;
    v.panel_fill = theme::base();
    v.window_fill = theme::mantle();
    v.window_corner_radius = CornerRadius::same(theme::radius::DIALOG);
    v.window_stroke = Stroke::new(theme::stroke::HAIRLINE, theme::surface());
    v.window_shadow = egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: theme::shadow(true) };
    v.popup_shadow = egui::Shadow { offset: [0, 6], blur: 16, spread: 0, color: theme::shadow(false) };
    v.menu_corner_radius = CornerRadius::same(theme::radius::CARD);
    v.override_text_color = Some(theme::text());
    v.extreme_bg_color = theme::base();
    v.selection.bg_fill = theme::accent().gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(theme::stroke::HAIRLINE, theme::accent());

    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    v.widgets.noninteractive.bg_stroke = Stroke::new(theme::stroke::HAIRLINE, theme::surface());
    for (widget, fill) in [
        (&mut v.widgets.inactive, theme::surface()),
        (&mut v.widgets.hovered, theme::selection()),
        (&mut v.widgets.active, theme::selection()),
    ] {
        widget.weak_bg_fill = fill;
        widget.bg_fill = fill;
        widget.bg_stroke = Stroke::NONE;
        widget.corner_radius = radius;
    }
    v.widgets.hovered.fg_stroke = Stroke::new(theme::stroke::HAIRLINE, theme::text());
    ctx.set_global_style(style);
}
