use super::*;

pub(in crate::ui) fn grid_cell_border(ui: &egui::Ui, rect: egui::Rect) {
    let color = if ui.visuals().dark_mode {
        Color32::from_rgb(70, 84, 94)
    } else {
        Color32::from_rgb(210, 220, 224)
    };
    let stroke = egui::Stroke::new(1.0, color);
    let right = rect.right() - 0.5;
    let bottom = rect.bottom() - 0.5;
    ui.painter().line_segment(
        [
            egui::pos2(right, rect.top()),
            egui::pos2(right, rect.bottom()),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            egui::pos2(rect.left(), bottom),
            egui::pos2(rect.right(), bottom),
        ],
        stroke,
    );
}

pub(in crate::ui) fn grid_header_cell(
    ui: &mut egui::Ui,
    name: &str,
    sql_type: &str,
) -> egui::Response {
    grid_header_cell_marked(ui, name, sql_type, None)
}

pub(in crate::ui) fn grid_header_cell_marked(
    ui: &mut egui::Ui,
    name: &str,
    sql_type: &str,
    marker: Option<&str>,
) -> egui::Response {
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .rect_filled(rect, 0, accent(ui).gamma_multiply(0.07));
    let response = ui.allocate_rect(rect, egui::Sense::click());
    let mut text_rect = rect.shrink2(egui::vec2(8.0, 5.0));
    if let Some(marker) = marker {
        let font = egui::FontId::proportional(12.0);
        let width = ui
            .painter()
            .layout_no_wrap(marker.into(), font.clone(), accent(ui))
            .size()
            .x;
        text_rect.max.x = (text_rect.max.x - width - 6.0).max(text_rect.min.x);
        ui.painter().text(
            rect.right_top() + egui::vec2(-8.0, 7.0),
            egui::Align2::RIGHT_TOP,
            marker,
            font,
            accent(ui),
        );
    }
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
        |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(
                egui::Label::new(RichText::new(name).strong())
                    .selectable(false)
                    .truncate(),
            );
            ui.add(
                egui::Label::new(RichText::new(sql_type).size(11.0).weak())
                    .selectable(false)
                    .truncate(),
            );
        },
    );
    grid_cell_border(ui, rect);
    response
}

pub(in crate::ui) fn grid_cell_input(
    ui: &mut egui::Ui,
    tab: Uuid,
    editor: &mut CellEditor,
    can_edit: bool,
) -> egui::Response {
    let rect = ui.available_rect_before_wrap();
    ui.painter().rect_filled(rect, 0, surface(ui));
    let font = egui::TextStyle::Body.resolve(ui.style());
    let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let vertical_margin = ((rect.height() - row_height) * 0.5).max(0.0).floor() as i8;
    let response = ui
        .scope(|ui| {
            if !can_edit || editor.is_null {
                ui.disable();
            }
            ui.put(
                rect,
                egui::TextEdit::singleline(&mut editor.text)
                    .id(egui::Id::new(("inline-cell", tab)))
                    .frame(false)
                    .margin(egui::Margin::symmetric(8, vertical_margin))
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Body),
            )
        })
        .inner;
    grid_cell_border(ui, rect);
    ui.painter().rect_stroke(
        rect,
        0,
        egui::Stroke::new(1.0, accent(ui)),
        egui::StrokeKind::Inside,
    );
    response
}

pub(in crate::ui) fn grid_cell(
    ui: &mut egui::Ui,
    text: RichText,
    selected: bool,
    header: bool,
) -> egui::Response {
    let rect = ui.available_rect_before_wrap();
    if header || selected {
        ui.painter().rect_filled(
            rect,
            0,
            accent(ui).gamma_multiply(if selected { 0.16 } else { 0.07 }),
        );
    }
    let (_, response) = ui.allocate_exact_size(
        rect.size(),
        if header {
            egui::Sense::click()
        } else {
            egui::Sense::click_and_drag()
        },
    );
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (rect.width() - 16.0).max(1.0),
        egui::TextStyle::Body,
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y * 0.5),
        galley,
        ui.visuals().text_color(),
    );
    grid_cell_border(ui, rect);
    if selected {
        ui.painter().rect_stroke(
            rect,
            0,
            egui::Stroke::new(1.0, accent(ui)),
            egui::StrokeKind::Inside,
        );
    }
    response
}
