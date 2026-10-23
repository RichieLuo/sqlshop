use super::*;

#[derive(Clone)]
pub(super) struct InsertDraft {
    pub id: Uuid,
    pub values: Vec<InsertValue>,
}

impl InsertDraft {
    pub(super) fn new(columns: usize) -> Self {
        Self {
            id: Uuid::new_v4(),
            values: vec![InsertValue::Default; columns],
        }
    }
}

impl QueryTab {
    pub(super) fn has_table_drafts(&self) -> bool {
        !self.staged_cells.is_empty() || !self.insert_drafts.is_empty()
    }

    pub(super) fn clear_table_drafts(&mut self) {
        self.staged_cells.clear();
        self.insert_drafts.clear();
        self.selected_insert = None;
        self.insert_focus = None;
    }

    pub(super) fn draft_summary(&self) -> String {
        match (self.staged_cells.len(), self.insert_drafts.len()) {
            (0, rows) => format!("新增 {rows} 行"),
            (cells, 0) => format!("暂存 {cells} 格"),
            (cells, rows) => format!("暂存 {cells} 格 · 新增 {rows} 行"),
        }
    }
}

impl App {
    pub(super) fn add_inline_row(&mut self) {
        let index = self.active_tab;
        if !self.can_add_table_row(index) {
            return;
        }
        let tab = &mut self.tabs[index];
        let Some(data) = tab.table_data.as_ref() else {
            return;
        };
        if !data.writable || tab.insert_drafts.len() >= 1000 {
            return;
        }
        let draft = InsertDraft::new(data.columns.len());
        tab.selected_insert = Some(draft.id);
        tab.insert_focus = Some(draft.id);
        tab.selected_cell = None;
        tab.cell_anchor = None;
        tab.selected_rows = None;
        tab.insert_drafts.push(draft);
        self.status = "已在表格底部追加新增草稿 · 直接填写，提交全部后写入数据库".into();
        self.error = false;
        self.context.request_repaint();
    }
}

pub(super) struct DraftColumnLayout<'a> {
    pub visible_columns: &'a [usize],
    pub frozen_count: usize,
    pub horizontal_offset: f32,
    pub frozen_boundary: f32,
    pub viewport_rect: egui::Rect,
}

pub(super) fn render_draft_row(
    row: &mut egui_extras::TableRow<'_, '_>,
    draft: &mut InsertDraft,
    columns: &[sqlshop::model::TableColumn],
    layout: DraftColumnLayout<'_>,
    enabled: bool,
    selected: &mut Option<Uuid>,
    focus: &mut Option<Uuid>,
) -> bool {
    let DraftColumnLayout {
        visible_columns,
        frozen_count,
        horizontal_offset,
        frozen_boundary,
        viewport_rect,
    } = layout;
    let mut remove = false;
    row.set_selected(*selected == Some(draft.id));
    row.col(|ui| {
        grid_columns_ui::cell(
            ui,
            frozen_count > 0,
            horizontal_offset,
            frozen_boundary,
            viewport_rect,
            |ui| {
                let response = grid_cell(ui, RichText::new("＋").color(accent(ui)), false, false);
                if response.clicked() {
                    *selected = Some(draft.id);
                }
                response
                    .on_hover_text("新增草稿，尚未写入数据库；填写后提交全部，或撤销暂存")
                    .context_menu(|ui| {
                        if ui
                            .add_enabled(enabled, egui::Button::new("移除此新增行"))
                            .clicked()
                        {
                            remove = true;
                            ui.close();
                        }
                    });
            },
        )
    });
    let first_editable = visible_columns.iter().copied().find(|&index| {
        columns[index].editable && columns[index].kind != sqlshop::model::CellKind::Unsupported
    });
    for (display_index, &index) in visible_columns.iter().enumerate() {
        let column = &columns[index];
        let value = &mut draft.values[index];
        row.col(|ui| grid_columns_ui::cell(ui, display_index < frozen_count, horizontal_offset, frozen_boundary, viewport_rect, |ui| {
            if !column.editable || column.kind == sqlshop::model::CellKind::Unsupported {
                grid_cell(ui, RichText::new("默认值").italics().weak(), false, false)
                    .on_hover_text("自增、生成或不支持编辑的列，由数据库处理默认值");
                return;
            }
            let rect = ui.available_rect_before_wrap();
            ui.painter().rect_filled(rect, 0, accent(ui).gamma_multiply(0.08));
            ui.allocate_rect(rect, egui::Sense::hover());
            let mut text = match value { InsertValue::Text(text) => text.clone(), _ => String::new() };
            let hint = match value { InsertValue::Default => "默认值", InsertValue::Null => "NULL", InsertValue::Text(text) if text.is_empty() => "空字符串", InsertValue::Text(_) => "" };
            let response = ui.add_enabled_ui(enabled, |ui| {
                ui.put(rect.shrink2(egui::vec2(8.0, 4.0)), egui::TextEdit::singleline(&mut text)
                    .id_salt(("insert-cell", draft.id, index)).frame(false).hint_text(hint))
            }).inner;
            if *focus == Some(draft.id) && first_editable == Some(index) && enabled {
                response.request_focus();
                *focus = None;
            }
            if response.changed() { *value = InsertValue::Text(text); }
            if response.clicked() || response.has_focus() { *selected = Some(draft.id); }
            response.on_hover_text(format!("{} · {}\n右键设置默认值、NULL 或空字符串；直接输入 NULL 为字符串", column.name, column.declared_type))
                .context_menu(|ui| {
                    ui.add_enabled_ui(enabled, |ui| {
                        for (label, mode) in [("使用默认值", InsertValue::Default), ("设为 NULL", InsertValue::Null)] {
                            if ui.button(label).clicked() { *value = mode; ui.close(); }
                        }
                        if column.kind == sqlshop::model::CellKind::Text && ui.button("设为空字符串").clicked() {
                            *value = InsertValue::Text(String::new());
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("移除此新增行").clicked() { remove = true; ui.close(); }
                    });
                });
            grid_cell_border(ui, rect);
        }));
    }
    // A table containing only generated columns still accepts DEFAULT VALUES.
    if *focus == Some(draft.id) && first_editable.is_none() {
        *focus = None;
    }
    remove
}
