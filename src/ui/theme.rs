use super::*;

pub(super) fn accent(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(96, 216, 179)
    } else {
        Color32::from_rgb(13, 130, 105)
    }
}

pub(super) fn chrome(ctx: &egui::Context) -> Color32 {
    if ctx.style().visuals.dark_mode {
        Color32::from_rgb(21, 28, 35)
    } else {
        Color32::from_rgb(250, 251, 252)
    }
}

pub(super) fn canvas(ctx: &egui::Context) -> Color32 {
    if ctx.style().visuals.dark_mode {
        Color32::from_rgb(15, 21, 27)
    } else {
        Color32::from_rgb(234, 239, 242)
    }
}

pub(super) fn surface(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(25, 34, 42)
    } else {
        Color32::from_rgb(255, 255, 255)
    }
}

pub(super) fn apply_theme(ctx: &egui::Context) {
    ctx.style_mut(|style| {
        let dark = style.visuals.dark_mode;
        let text = if dark {
            Color32::from_rgb(230, 236, 240)
        } else {
            Color32::from_rgb(30, 40, 48)
        };
        style.visuals.weak_text_color = Some(if dark {
            Color32::from_rgb(170, 184, 194)
        } else {
            Color32::from_rgb(71, 84, 94)
        });
        for widgets in [
            &mut style.visuals.widgets.noninteractive,
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
            &mut style.visuals.widgets.open,
        ] {
            widgets.fg_stroke.color = text;
        }
        let accent = if dark {
            Color32::from_rgb(96, 216, 179)
        } else {
            Color32::from_rgb(13, 130, 105)
        };
        let control = if dark {
            Color32::from_rgb(35, 46, 55)
        } else {
            Color32::from_rgb(229, 235, 238)
        };
        style.visuals.selection.bg_fill = accent.gamma_multiply(0.38);
        style.visuals.selection.stroke.color = accent;
        style.visuals.hyperlink_color = accent;
        style.visuals.widgets.inactive.bg_fill = control;
        style.visuals.widgets.inactive.weak_bg_fill = control;
        style.visuals.widgets.noninteractive.bg_fill = if dark {
            Color32::from_rgb(25, 34, 42)
        } else {
            Color32::WHITE
        };
        style.visuals.widgets.noninteractive.weak_bg_fill =
            style.visuals.widgets.noninteractive.bg_fill;
        style.visuals.widgets.hovered.bg_fill = if dark {
            Color32::from_rgb(46, 61, 68)
        } else {
            Color32::from_rgb(211, 226, 224)
        };
        style.visuals.widgets.active.bg_fill = accent.gamma_multiply(if dark { 0.5 } else { 0.25 });
        style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(
            1.0,
            if dark {
                Color32::from_rgb(58, 70, 79)
            } else {
                Color32::from_rgb(190, 201, 207)
            },
        );
        style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(
            1.0,
            if dark {
                Color32::from_rgb(58, 70, 79)
            } else {
                Color32::from_rgb(184, 196, 202)
            },
        );
        style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, accent);
        style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, accent);
        style.visuals.window_fill = if dark {
            Color32::from_rgb(25, 34, 42)
        } else {
            Color32::from_rgb(250, 251, 252)
        };
        style.visuals.panel_fill = if dark {
            Color32::from_rgb(15, 21, 27)
        } else {
            Color32::from_rgb(234, 239, 242)
        };
    });
}

pub(super) fn install_font(ctx: &egui::Context) {
    let custom = std::env::var("SQLSHOP_FONT").ok();
    let candidates = [
        custom.as_deref().unwrap_or(""),
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "C:/Windows/Fonts/msyh.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("cjk".into(), egui::FontData::from_owned(bytes).into());
            // Use the system's regular UI font instead of egui's light Latin font.
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "cjk".into());
            // Keep SQL and other monospace text aligned, with CJK as a fallback.
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("cjk".into());
            ctx.set_fonts(fonts);
            break;
        }
    }
}
