use eframe::egui::{self, Color32, Margin, RichText, Rounding, Stroke};

pub const ACCENT: Color32 = Color32::from_rgb(99, 140, 255);
pub const CARD: Color32 = Color32::from_rgb(33, 35, 42);
pub const CARD_STROKE: Color32 = Color32::from_rgb(48, 51, 60);
pub const GREEN: Color32 = Color32::from_rgb(110, 200, 140);
pub const ORANGE: Color32 = Color32::from_rgb(235, 170, 80);
pub const RED: Color32 = Color32::from_rgb(235, 110, 110);
pub const GRAY: Color32 = Color32::from_rgb(140, 144, 156);

pub fn setup(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::from_rgb(24, 25, 30);
    v.window_fill = Color32::from_rgb(30, 32, 38);
    v.selection.bg_fill = ACCENT;
    let r = Rounding::same(8.0);
    v.widgets.noninteractive.rounding = r;
    v.widgets.inactive.rounding = r;
    v.widgets.hovered.rounding = r;
    v.widgets.active.rounding = r;
    v.widgets.open.rounding = r;
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(45, 48, 58);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(58, 62, 75);
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(10.0, 8.0);
        s.spacing.button_padding = egui::vec2(12.0, 6.0);
        // reserve space for the scrollbar instead of drawing it over the cards
        s.spacing.scroll = egui::style::ScrollStyle::solid();
    });
}

/// Card frame used for list rows.
pub fn card() -> egui::Frame {
    egui::Frame::none()
        .fill(CARD)
        .stroke(Stroke::new(1.0, CARD_STROKE))
        .rounding(10.0)
        .inner_margin(Margin::symmetric(14.0, 10.0))
}

/// Small colored label with a tinted background. Drawn manually (not as a nested
/// frame) so that it wraps to the next row as a whole inside `horizontal_wrapped`.
pub fn chip(ui: &mut egui::Ui, text: impl Into<String>, color: Color32) {
    let font = egui::TextStyle::Small.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(text.into(), font, color);
    let pad = egui::vec2(8.0, 3.0);
    let size = galley.size() + pad * 2.0;
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 6.0, color.gamma_multiply(0.18));
        ui.painter().galley(rect.min + pad, galley, color);
    }
}

pub fn primary(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).color(Color32::WHITE)).fill(ACCENT)
}
