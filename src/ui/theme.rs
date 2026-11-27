//! Windows 11 (Fluent) look: Segoe UI fonts, Segoe Fluent Icons, Mica-like neutral
//! surfaces, 4px control / 8px card corners, accent-coloured primary buttons.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, RichText,
    Stroke, TextStyle, Vec2,
};
use std::sync::Arc;

// --- Segoe Fluent Icons code points (same values in Segoe MDL2 Assets on Windows 10) ---
pub const I_ADD: &str = "\u{E710}";
pub const I_CLOSE: &str = "\u{E711}";
pub const I_DELETE: &str = "\u{E74D}";
pub const I_UP: &str = "\u{E74A}";
pub const I_DOWN: &str = "\u{E74B}";
pub const I_RIGHT: &str = "\u{E76C}";
pub const I_ROTATE: &str = "\u{E7AD}";
pub const I_ROTATE_LEFT: &str = "\u{E7A7}";
pub const I_COPY: &str = "\u{E8C8}";
pub const I_OPEN: &str = "\u{E8E5}";
pub const I_FOLDER: &str = "\u{E838}";
pub const I_SAVE: &str = "\u{E74E}";
pub const I_INFO: &str = "\u{E946}";
pub const I_PHOTO: &str = "\u{EB9F}";
pub const I_CUT: &str = "\u{E8C6}";
pub const I_PACKAGE: &str = "\u{E7B8}";
pub const I_MERGE: &str = "\u{E8F1}";
pub const I_SUN: &str = "\u{E706}";
pub const I_MOON: &str = "\u{E708}";
pub const I_CHECK: &str = "\u{E73E}";
pub const I_ERROR: &str = "\u{E783}";
pub const I_CLEAR: &str = "\u{E894}";
pub const I_UPLOAD: &str = "\u{E898}";
pub const I_DOWNLOAD: &str = "\u{E896}";
pub const I_VIDEO: &str = "\u{E714}";
pub const I_PLAY: &str = "\u{E768}";
pub const I_PAUSE: &str = "\u{E769}";
pub const I_PREV: &str = "\u{E892}";
pub const I_NEXT: &str = "\u{E893}";
pub const I_LEFT: &str = "\u{E76B}";
pub const I_IMPORT: &str = "\u{E8B5}";
pub const I_AUDIO: &str = "\u{E8D6}";
pub const I_CAMERA: &str = "\u{E722}";
pub const I_CONVERT: &str = "\u{E8AB}";
pub const I_RESIZE: &str = "\u{E740}";
pub const I_SPEED: &str = "\u{EC4A}";
pub const I_CLOCK: &str = "\u{E916}";
pub const I_FLASH: &str = "\u{E945}";

pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

/// Load Windows fonts when present; otherwise keep egui's built-in fonts.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let dir = std::env::var("WINDIR")
        .map(|w| format!("{w}\\Fonts\\"))
        .unwrap_or_else(|_| "C:\\Windows\\Fonts\\".into());
    let mut load = |name: &str, files: &[&str]| -> bool {
        for f in files {
            if let Ok(bytes) = std::fs::read(format!("{dir}{f}")) {
                fonts
                    .font_data
                    .insert(name.into(), Arc::new(FontData::from_owned(bytes)));
                return true;
            }
        }
        false
    };
    let ui = load("segoe", &["segoeui.ttf"]);
    let sb = load("segoe-sb", &["seguisb.ttf", "segoeuib.ttf"]);
    let icons = load("fluent", &["SegoeIcons.ttf", "segmdl2.ttf"]);

    let defaults = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut body = Vec::new();
    if ui {
        body.push("segoe".to_string());
    }
    if icons {
        body.push("fluent".to_string());
    }
    body.extend(defaults.iter().cloned());

    let mut heading = Vec::new();
    if sb {
        heading.push("segoe-sb".to_string());
    }
    heading.extend(body.iter().cloned());

    fonts.families.insert(FontFamily::Proportional, body);
    fonts.families.insert(semibold(), heading);
    ctx.set_fonts(fonts);
}

/// Colour palette for the current mode.
#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    /// Side panels, top bar, timeline, status bar.
    pub panel: Color32,
    /// The deep area around the viewer.
    pub well: Color32,
    /// Raised surfaces: cards, inputs, unselected controls.
    pub card: Color32,
    pub card_hover: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text2: Color32,
    /// Honey: actions, selection, the In–Out range.
    pub accent: Color32,
    pub on_accent: Color32,
    /// The playhead (cream, distinct from the honey range).
    pub playhead: Color32,
    pub success: Color32,
    pub error: Color32,
}

impl Palette {
    pub fn of(dark: bool) -> Self {
        if dark {
            Self {
                dark,
                panel: Color32::from_rgb(0x1C, 0x1B, 0x19),
                well: Color32::from_rgb(0x12, 0x11, 0x10),
                card: Color32::from_rgb(0x27, 0x25, 0x22),
                card_hover: Color32::from_rgb(0x30, 0x2D, 0x29),
                border: Color32::from_rgb(0x36, 0x32, 0x2D),
                text: Color32::from_rgb(0xED, 0xE9, 0xE3),
                text2: Color32::from_rgb(0xA6, 0x9F, 0x96),
                accent: Color32::from_rgb(0xF2, 0xA9, 0x3B),
                on_accent: Color32::from_rgb(0x22, 0x16, 0x05),
                playhead: Color32::from_rgb(0xFF, 0xF3, 0xDC),
                success: Color32::from_rgb(0x5F, 0xCB, 0x8A),
                error: Color32::from_rgb(0xF2, 0x84, 0x7C),
            }
        } else {
            Self {
                dark,
                panel: Color32::from_rgb(0xF4, 0xF3, 0xF0),
                well: Color32::from_rgb(0xDC, 0xD9, 0xD4),
                card: Color32::WHITE,
                card_hover: Color32::from_rgb(0xFA, 0xF8, 0xF4),
                border: Color32::from_rgb(0xD8, 0xD4, 0xCD),
                text: Color32::from_rgb(0x1D, 0x1B, 0x18),
                text2: Color32::from_rgb(0x5F, 0x59, 0x51),
                accent: Color32::from_rgb(0xA8, 0x64, 0x00),
                on_accent: Color32::WHITE,
                playhead: Color32::from_rgb(0x1D, 0x1B, 0x18),
                success: Color32::from_rgb(0x1B, 0x7F, 0x46),
                error: Color32::from_rgb(0xB4, 0x2A, 0x1E),
            }
        }
    }

    pub fn current(ui: &egui::Ui) -> Self {
        Self::of(ui.visuals().dark_mode)
    }
}

/// Install Fluent visuals for both light and dark mode, plus spacing and text sizes.
pub fn install_theme(ctx: &egui::Context) {
    ctx.set_visuals_of(egui::Theme::Dark, visuals(true));
    ctx.set_visuals_of(egui::Theme::Light, visuals(false));
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(12.0, 5.0);
        s.spacing.interact_size = Vec2::new(40.0, 30.0);
        s.spacing.slider_width = 220.0;
        s.spacing.combo_width = 220.0;
        s.spacing.text_edit_width = 280.0;
        s.spacing.window_margin = Margin::same(16);
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
            (TextStyle::Heading, FontId::new(28.0, semibold())),
        ]
        .into();
    });
}

fn visuals(dark: bool) -> egui::Visuals {
    let p = Palette::of(dark);
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let r4 = CornerRadius::same(4);

    v.panel_fill = p.panel;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(8);
    v.faint_bg_color = p.card;
    // Text fields and other "wells".
    v.extreme_bg_color = if dark {
        Color32::from_rgb(0x14, 0x13, 0x11)
    } else {
        Color32::WHITE
    };
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent.gamma_multiply(if dark { 0.35 } else { 0.22 });
    v.selection.stroke = Stroke::new(1.0, p.text);

    let control = p.card;
    let control_hover = p.card_hover;
    let control_press = if dark {
        Color32::from_rgb(0x22, 0x20, 0x1D)
    } else {
        Color32::from_rgb(0xF0, 0xEE, 0xEA)
    };
    let stroke = p.border;

    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r4;
    for (wv, bg) in [
        (&mut w.inactive, control),
        (&mut w.hovered, control_hover),
        (&mut w.active, control_press),
    ] {
        wv.bg_fill = bg;
        wv.weak_bg_fill = bg;
        wv.bg_stroke = Stroke::new(1.0, stroke);
        wv.fg_stroke = Stroke::new(1.0, p.text);
        wv.corner_radius = r4;
        wv.expansion = 0.0;
    }
    w.open = w.active;
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v
}

// ---------------------------------------------------------------------------------------
// Widgets
// ---------------------------------------------------------------------------------------

pub fn subtitle(text: &str) -> RichText {
    RichText::new(text).family(semibold()).size(20.0)
}

pub fn strong(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).family(semibold())
}

pub fn secondary(ui: &egui::Ui, text: impl Into<String>) -> RichText {
    RichText::new(text.into()).color(Palette::current(ui).text2)
}

pub fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(secondary(ui, text).size(12.0));
}

/// Icon + label, e.g. `labeled(I_ADD, "Add files")`.
pub fn labeled(icon: &str, text: &str) -> String {
    format!("{icon}   {text}")
}

/// Filled accent button (the "primary action").
pub fn accent_button(ui: &mut egui::Ui, enabled: bool, text: &str) -> bool {
    let p = Palette::current(ui);
    let fill = if enabled {
        p.accent
    } else {
        p.accent.gamma_multiply(0.4)
    };
    let btn = egui::Button::new(strong(text).color(p.on_accent))
        .fill(fill)
        .stroke(Stroke::NONE)
        .corner_radius(4.0)
        .min_size(Vec2::new(140.0, 34.0));
    ui.add_enabled(enabled, btn).clicked()
}

/// Flat command-bar button (no frame until hovered).
pub fn command(ui: &mut egui::Ui, enabled: bool, icon: &str, text: &str) -> bool {
    let btn = egui::Button::new(format!("{icon}  {text}"))
        .frame_when_inactive(false)
        .min_size(Vec2::new(0.0, 32.0));
    ui.scope(|ui| {
        ui.spacing_mut().button_padding.x = 8.0;
        ui.add_enabled(enabled, btn).clicked()
    })
    .inner
}

/// Small square icon-only button with tooltip.
pub fn icon_button(ui: &mut egui::Ui, enabled: bool, icon: &str, tip: &str) -> bool {
    let btn = egui::Button::new(RichText::new(icon).size(13.0))
        .frame_when_inactive(false)
        .min_size(Vec2::splat(30.0));
    ui.add_enabled(enabled, btn).on_hover_text(tip).clicked()
}

/// "Drop files here or click to browse" area, like on the Home screen. It lights up while
/// files are dragged over the window. Returns true when clicked (open a file dialog then).
/// Dropped files are routed by the app shell to the current tool.
pub fn drop_zone(ui: &mut egui::Ui, height: f32, title: &str, subtitle: &str) -> bool {
    let p = Palette::current(ui);
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height),
        egui::Sense::click(),
    );
    let dragging_files = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
    let active = resp.hovered() || dragging_files;
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        8.0,
        if dragging_files {
            p.accent.gamma_multiply(0.12)
        } else if active {
            p.card_hover
        } else {
            p.card
        },
    );
    // Dashed outline.
    let r = rect.shrink(1.0);
    let outline = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    let color = if active {
        p.accent
    } else {
        p.text2.gamma_multiply(0.55)
    };
    painter.extend(egui::Shape::dashed_line(
        &outline,
        Stroke::new(1.5, color),
        8.0,
        6.0,
    ));

    let compact = height < 100.0;
    if compact {
        // One line: icon, title and subtitle side by side.
        let text = format!("{I_UPLOAD}   {title}");
        let g1 = painter.layout_no_wrap(
            text,
            FontId::new(14.0, semibold()),
            if active { p.accent } else { p.text },
        );
        let g2 = painter.layout_no_wrap(subtitle.to_string(), FontId::proportional(12.0), p.text2);
        let total = g1.size().x + 14.0 + g2.size().x;
        let x = rect.center().x - total / 2.0;
        painter.galley(
            egui::pos2(x, rect.center().y - g1.size().y / 2.0),
            g1.clone(),
            p.text,
        );
        painter.galley(
            egui::pos2(x + g1.size().x + 14.0, rect.center().y - g2.size().y / 2.0),
            g2,
            p.text2,
        );
    } else {
        let c = rect.center();
        painter.circle_filled(
            c - Vec2::new(0.0, 26.0),
            24.0,
            p.accent.gamma_multiply(if p.dark { 0.2 } else { 0.12 }),
        );
        painter.text(
            c - Vec2::new(0.0, 26.0),
            egui::Align2::CENTER_CENTER,
            I_UPLOAD,
            FontId::proportional(22.0),
            p.accent,
        );
        painter.text(
            c + Vec2::new(0.0, 16.0),
            egui::Align2::CENTER_CENTER,
            title,
            FontId::new(16.0, semibold()),
            p.text,
        );
        painter.text(
            c + Vec2::new(0.0, 38.0),
            egui::Align2::CENTER_CENTER,
            subtitle,
            FontId::proportional(12.0),
            p.text2,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

// ---------------------------------------------------------------------------------------
// Compact inspector widgets (narrow side panels)
// ---------------------------------------------------------------------------------------

/// A labelled control stacked vertically: small bold label, the control at full width,
/// and an optional hint underneath.
pub fn field<R>(
    ui: &mut egui::Ui,
    label: &str,
    hint: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.label(strong(label).size(13.0));
    let r = add(ui);
    if !hint.is_empty() {
        ui.add(egui::Label::new(secondary(ui, hint).size(11.5)).wrap());
    }
    ui.add_space(8.0);
    r
}

/// A small uppercase group heading inside a side panel.
pub fn group(ui: &mut egui::Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(strong(text).size(13.5).color(Palette::current(ui).text2));
    ui.add_space(2.0);
}

/// Full-width filled accent button (the inspector's main action).
pub fn action_button(ui: &mut egui::Ui, enabled: bool, text: &str) -> bool {
    let p = Palette::current(ui);
    let fill = if enabled {
        p.accent
    } else {
        p.accent.gamma_multiply(0.4)
    };
    let btn = egui::Button::new(strong(text).color(p.on_accent))
        .fill(fill)
        .stroke(Stroke::NONE)
        .corner_radius(4.0)
        .min_size(Vec2::new(ui.available_width(), 38.0));
    ui.add_enabled(enabled, btn).clicked()
}

/// Background of the editor's panels.
pub fn panel_fill(ui: &egui::Ui) -> Color32 {
    Palette::current(ui).panel
}

/// A framed choice button: honey fill with dark text when selected, a normal framed
/// control otherwise (so every option visibly reads as clickable).
pub fn toggle(ui: &mut egui::Ui, selected: bool, text: &str, width: f32) -> bool {
    let p = Palette::current(ui);
    let label = if selected {
        strong(text).color(p.on_accent)
    } else {
        RichText::new(text).color(p.text)
    };
    let mut btn = egui::Button::new(label)
        .corner_radius(4.0)
        .min_size(Vec2::new(width, 30.0));
    if selected {
        btn = btn.fill(p.accent).stroke(Stroke::NONE);
    }
    ui.add(btn).clicked()
}
