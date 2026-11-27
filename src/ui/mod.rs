//! Application shell.

mod theme;

use eframe::egui::{self, Margin};
use theme::*;

pub struct App;

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        install_theme(&cc.egui_ctx);
        Self
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let bg = Palette::current(ui).bg;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(bg).inner_margin(Margin::same(40)))
            .show(ui, |ui| {
                ui.label(title("Video Studio"));
            });
    }
}
