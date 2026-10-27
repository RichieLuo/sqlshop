use super::*;

pub(super) fn configure_style(style: &mut egui::Style) {
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.interact_size.y = 32.0;
    style.spacing.window_margin = egui::Margin::symmetric(20, 14);
    style.spacing.menu_margin = egui::Margin::same(8);
    for widgets in [
        &mut style.visuals.widgets.noninteractive,
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        widgets.corner_radius = egui::CornerRadius::same(6);
    }
    style.visuals.window_corner_radius = egui::CornerRadius::same(12);
    style.visuals.menu_corner_radius = egui::CornerRadius::same(8);
}

/// Keep toolbars comfortable even inside dense data workspaces.
pub(super) fn toolbar_style(ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing = egui::vec2(10.0, 8.0);
    ui.spacing_mut().button_padding = egui::vec2(12.0, 6.0);
    ui.spacing_mut().interact_size.y = 32.0;
}

pub(super) fn dialog_input(text: &mut dyn egui::TextBuffer) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text)
        .margin(egui::vec2(8.0, 6.0))
        .min_size(egui::vec2(0.0, 32.0))
}

pub(super) fn primary_button(ui: &egui::Ui, label: impl Into<String>) -> egui::Button<'static> {
    let text = if ui.visuals().dark_mode {
        Color32::from_rgb(15, 40, 33)
    } else {
        Color32::WHITE
    };
    egui::Button::new(RichText::new(label.into()).color(text))
        .fill(accent(ui))
        .stroke(egui::Stroke::NONE)
        .corner_radius(6)
        .min_size(egui::vec2(80.0, 36.0))
}

pub(super) fn danger_button(ui: &egui::Ui, label: impl Into<String>) -> egui::Button<'static> {
    let (fill, text) = if ui.visuals().dark_mode {
        (
            Color32::from_rgb(242, 153, 153),
            Color32::from_rgb(59, 20, 20),
        )
    } else {
        (Color32::from_rgb(187, 55, 55), Color32::WHITE)
    };
    egui::Button::new(RichText::new(label.into()).color(text))
        .fill(fill)
        .stroke(egui::Stroke::NONE)
        .corner_radius(6)
        .min_size(egui::vec2(80.0, 36.0))
}

/// Place the main action at the trailing edge, with room above the footer.
pub(super) fn dialog_actions<R>(
    ui: &mut egui::Ui,
    actions: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    ui.add_space(14.0);
    ui.separator();
    ui.add_space(8.0);
    ui.spacing_mut().button_padding = egui::vec2(16.0, 8.0);
    ui.spacing_mut().interact_size.y = 36.0;
    // A plain with_layout uses all remaining window height and centers the
    // buttons inside it. Reserve only a single row so dialogs fit their content.
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), ui.spacing().interact_size.y),
        egui::Layout::right_to_left(egui::Align::Center),
        actions,
    )
}

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
        configure_style(style);
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
            Color32::from_rgb(250, 252, 252)
        };
        style.visuals.selection.bg_fill = accent.gamma_multiply(0.38);
        style.visuals.selection.stroke.color = accent;
        style.visuals.hyperlink_color = accent;
        style.visuals.widgets.inactive.bg_fill = control;
        style.visuals.widgets.inactive.weak_bg_fill = control;
        style.visuals.extreme_bg_color = if dark {
            Color32::from_rgb(17, 25, 32)
        } else {
            Color32::WHITE
        };
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
                Color32::from_rgb(216, 224, 228)
            },
        );
        style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(
            1.0,
            if dark {
                Color32::from_rgb(58, 70, 79)
            } else {
                Color32::from_rgb(206, 217, 222)
            },
        );
        style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, accent);
        style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, accent);
        style.visuals.window_fill = if dark {
            Color32::from_rgb(25, 34, 42)
        } else {
            Color32::WHITE
        };
        style.visuals.window_stroke = style.visuals.widgets.noninteractive.bg_stroke;
        style.visuals.window_highlight_topmost = false;
        style.visuals.window_shadow = egui::epaint::Shadow {
            offset: [0, 8],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha(if dark { 100 } else { 36 }),
        };
        style.visuals.popup_shadow = egui::epaint::Shadow {
            offset: [0, 4],
            blur: 16,
            spread: 0,
            color: Color32::from_black_alpha(if dark { 80 } else { 28 }),
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
