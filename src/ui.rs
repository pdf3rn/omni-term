use eframe::egui::{
    self, Button, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Frame, Margin, Panel,
    RichText, ViewportCommand,
};

use crate::pane::{bold_family, Pane, PADDING};
use crate::term::Session;
use crate::theme;

const SIDEBAR_WIDTH: f32 = 220.0;
const TAB_BAR_HEIGHT: f32 = 38.0;

struct Tab {
    label: String,
    pane: Pane,
}

pub struct App {
    tabs: Vec<Tab>,
    active: usize,
    debug_keys: bool,
    window_title: String,
    ctx: egui::Context,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, debug_keys: bool) -> anyhow::Result<Self> {
        install_fonts(&cc.egui_ctx);
        install_visuals(&cc.egui_ctx);
        let mut app = Self {
            tabs: Vec::new(),
            active: 0,
            debug_keys,
            window_title: String::new(),
            ctx: cc.egui_ctx.clone(),
        };
        app.open_local()?;
        Ok(app)
    }

    fn open_local(&mut self) -> anyhow::Result<()> {
        let ctx = self.ctx.clone();
        let session = Session::start(
            80,
            24,
            move || ctx.request_repaint(),
            |feed| crate::local::spawn(80, 24, feed),
        )?;
        self.tabs.push(Tab { label: "Terminal local".into(), pane: Pane::new(session, self.debug_keys) });
        self.active = self.tabs.len() - 1;
        Ok(())
    }

    fn close_tab(&mut self, index: usize) {
        self.tabs.remove(index);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        } else if index < self.active {
            self.active -= 1;
        }
    }
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "jb-regular".into(),
        FontData::from_static(include_bytes!("../assets/JetBrainsMono-Regular.ttf")).into(),
    );
    fonts.font_data.insert(
        "jb-bold".into(),
        FontData::from_static(include_bytes!("../assets/JetBrainsMono-Bold.ttf")).into(),
    );
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "jb-regular".into());
    fonts.families.insert(bold_family(), vec!["jb-bold".into()]);
    ctx.set_fonts(fonts);
}

fn install_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = theme::BASE;
    visuals.window_fill = theme::BASE;
    visuals.override_text_color = Some(theme::TEXT);
    visuals.widgets.inactive.weak_bg_fill = theme::SURFACE;
    visuals.widgets.hovered.weak_bg_fill = theme::SELECTION;
    visuals.widgets.active.weak_bg_fill = theme::SELECTION;
    ctx.set_visuals(visuals);
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        let exited: Vec<usize> =
            (0..self.tabs.len()).rev().filter(|&i| self.tabs[i].pane.session.has_exited()).collect();
        for index in exited {
            self.close_tab(index);
        }
        if self.tabs.is_empty() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }

        let mut new_local = false;
        Panel::left("sidebar")
            .default_size(SIDEBAR_WIDTH)
            .min_size(160.0)
            .frame(Frame::NONE.fill(theme::MANTLE).inner_margin(Margin::same(12)))
            .show(ui, |ui| {
                ui.label(RichText::new("OMNITERM").small().color(theme::SUBTEXT));
                ui.add_space(8.0);
                if ui.add(Button::new("+ Terminal local").corner_radius(6).min_size(egui::vec2(ui.available_width(), 28.0))).clicked() {
                    new_local = true;
                }
                ui.add_space(16.0);
                ui.label(RichText::new("CONEXIONES").small().color(theme::SUBTEXT));
                ui.add_space(4.0);
                ui.label(RichText::new("Sin conexiones guardadas").color(theme::SUBTEXT));
            });

        let mut activate = None;
        let mut close = None;
        Panel::top("tabs")
            .exact_size(TAB_BAR_HEIGHT)
            .frame(Frame::NONE.fill(theme::MANTLE).inner_margin(Margin::symmetric(8, 5)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, tab) in self.tabs.iter().enumerate() {
                        let selected = i == self.active;
                        let color = if selected { theme::TEXT } else { theme::SUBTEXT };
                        let fill = if selected { theme::BASE } else { Color32::TRANSPARENT };
                        let title = self.tabs[i].pane.title();
                        let label = if title.is_empty() { tab.label.clone() } else { title };
                        if ui.add(Button::new(RichText::new(label).color(color)).fill(fill).corner_radius(6)).clicked() {
                            activate = Some(i);
                        }
                        if ui.add(Button::new("×").frame(false)).clicked() {
                            close = Some(i);
                        }
                        ui.add_space(4.0);
                    }
                    if ui.add(Button::new("+").frame(false)).clicked() {
                        new_local = true;
                    }
                });
            });

        if let Some(i) = activate {
            self.active = i;
        }
        if new_local {
            if let Err(error) = self.open_local() {
                eprintln!("no se pudo abrir el terminal local: {error:#}");
            }
        }
        if let Some(i) = close {
            self.close_tab(i);
            if self.tabs.is_empty() {
                ctx.send_viewport_cmd(ViewportCommand::Close);
                return;
            }
        }

        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, CornerRadius::ZERO, theme::BASE);
        let active = self.active;
        self.tabs[active].pane.show(ui, area.shrink(PADDING));

        let title = self.tabs[active].pane.title();
        let title = if title.is_empty() { "omniterm".to_string() } else { title };
        if title != self.window_title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }
}
