//! Shop dialog geometry. Keep these metrics consistent across the four apps.
use eframe::egui::{self, Color32};

pub(super) fn frame(ctx: &egui::Context) -> egui::Frame {
    let dark = ctx.style().visuals.dark_mode;
    egui::Frame::new()
        .fill(if dark {
            Color32::from_rgb(32, 34, 38)
        } else {
            Color32::WHITE
        })
        .stroke(egui::Stroke::new(
            1.0,
            if dark {
                Color32::from_white_alpha(28)
            } else {
                Color32::from_black_alpha(24)
            },
        ))
        .corner_radius(12)
        .inner_margin(24)
        .shadow(egui::epaint::Shadow {
            offset: [0, 12],
            blur: 36,
            spread: 0,
            color: Color32::from_black_alpha(if dark { 72 } else { 28 }),
        })
}
