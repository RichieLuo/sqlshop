use super::*;

const LINE_HEIGHT_ID: &str = "sql-editor-line-height";

pub(in crate::ui) fn line_height(context: &egui::Context) -> f32 {
    context.data_mut(|data| {
        data.get_temp::<f32>(egui::Id::new(LINE_HEIGHT_ID))
            .unwrap_or(sqlshop::workspace::DEFAULT_SQL_LINE_HEIGHT)
    })
}

pub(in crate::ui) fn set_line_height(context: &egui::Context, value: f32) {
    let value = if value.is_finite() {
        value.clamp(1.0, 2.0)
    } else {
        sqlshop::workspace::DEFAULT_SQL_LINE_HEIGHT
    };
    context.data_mut(|data| data.insert_temp(egui::Id::new(LINE_HEIGHT_ID), value));
}

pub(super) fn line_height_control(ui: &mut egui::Ui) -> f32 {
    let mut value = line_height(ui.ctx());
    ui.label("行高");
    egui::ComboBox::from_id_salt(LINE_HEIGHT_ID)
        .width(60.0)
        .selected_text(format!("{value}×"))
        .show_ui(ui, |ui| {
            for (height, label) in [
                (1.0, "1.0×（紧凑）"),
                (1.25, "1.25×"),
                (1.5, "1.5×（默认）"),
                (1.75, "1.75×"),
                (2.0, "2.0×（宽松）"),
            ] {
                ui.selectable_value(&mut value, height, label);
            }
        })
        .response
        .on_hover_text("调整 SQL 行间距；所有查询标签共用，自动保存");
    set_line_height(ui.ctx(), value);
    value
}

pub(super) fn layout_sql(ui: &egui::Ui, sql: &str, line_height: f32) -> egui::text::LayoutJob {
    let mut job = highlight_sql(sql, ui.visuals().dark_mode);
    let font = job
        .sections
        .first()
        .map(|section| section.format.font_id.clone())
        .unwrap_or_else(|| egui::FontId::monospace(14.0));
    let height = ui.fonts_mut(|fonts| fonts.row_height(&font)) * line_height;
    job.first_row_min_height = height;
    for section in &mut job.sections {
        section.format.line_height = Some(height);
        section.format.valign = egui::Align::Center;
    }
    job
}
