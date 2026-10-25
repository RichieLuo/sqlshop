mod cells;
pub(super) use cells::{
    grid_cell, grid_cell_border, grid_cell_input, grid_header_cell, grid_header_cell_marked,
};

mod editing;
#[cfg(test)]
pub(super) use editing::{apply_grid_cell_draft, selected_grid_cells};
pub(super) use editing::{
    apply_grid_selection_draft, grid_cell_editor, grid_cell_eligible, paste_grid_values,
};

use super::*;

#[derive(Clone, Debug)]
pub(super) enum GridAction {
    ExportRows(Vec<usize>),
    DeleteRows(Vec<usize>),
    Browse(grid_browse_ui::BrowseAction),
    FinishCell {
        cancel: bool,
        movement: CellMovement,
    },
    PasteValues(String),
    SubmitDrafts,
    RevertDrafts,
    DraftError(String),
}

#[derive(Clone, Copy, Debug)]
pub(super) enum CellMovement {
    Stay,
    Across(i32),
    Down(i32),
}

#[derive(Clone, Copy)]
pub(super) enum CellDraftAction {
    SetNull,
    SetEmptyString,
    Revert,
}

pub(super) fn row_context_menu(
    response: egui::Response,
    selected_rows: &mut Option<(usize, usize)>,
    set: &ResultSet,
    visible_columns: &[usize],
    deletable: bool,
    row_index: usize,
) -> Option<GridAction> {
    if response.secondary_clicked() && !row_in_selection(*selected_rows, row_index) {
        *selected_rows = Some((row_index, row_index));
    }
    let mut action = None;
    response.context_menu(|ui| {
        let rows = selected_rows
            .map(|(a, b)| (a.min(b)..=a.max(b)).collect::<Vec<_>>())
            .unwrap_or_else(|| vec![row_index]);
        if ui
            .button(format!("复制选中行 ({} 行, TSV)", rows.len()))
            .clicked()
        {
            ui.ctx()
                .copy_text(grid_columns_ui::rows_tsv(set, visible_columns, &rows));
            ui.close();
        }
        if ui
            .button(format!("导出选中行 CSV… ({} 行)", rows.len()))
            .clicked()
        {
            action = Some(GridAction::ExportRows(rows.clone()));
            ui.close();
        }
        if ui
            .add_enabled(
                deletable,
                egui::Button::new(format!("删除选中行… ({} 行)", rows.len())),
            )
            .clicked()
        {
            action = Some(GridAction::DeleteRows(rows));
            ui.close();
        }
    });
    action
}

#[cfg(test)]
pub(super) fn result_grid(
    ui: &mut egui::Ui,
    tab: &mut QueryTab,
    can_edit: bool,
) -> (Option<CellEditor>, Option<GridAction>) {
    let draft_actions = (can_edit && !tab.table_snapshot_stale, true);
    result_grid_inline(ui, tab, can_edit, None, draft_actions)
}

pub(super) fn result_grid_inline(
    ui: &mut egui::Ui,
    tab: &mut QueryTab,
    can_edit: bool,
    mut inline_editor: Option<&mut CellEditor>,
    draft_actions: (bool, bool),
) -> (Option<CellEditor>, Option<GridAction>) {
    let can_edit = can_edit && !tab.table_snapshot_stale;
    let has_drafts = tab.has_table_drafts();
    let Some(set) = tab
        .output
        .as_ref()
        .and_then(|output| output.sets.get(tab.result_index))
    else {
        return (None, None);
    };
    let mut column_layout = tab
        .grid_layouts
        .get(&tab.result_index)
        .cloned()
        .unwrap_or_default();
    grid_columns_ui::reconcile(&mut column_layout, &set.columns);
    if grid_columns_ui::controls(ui, set, &mut column_layout, inline_editor.is_none()) {
        tab.selected_cell = None;
        tab.cell_anchor = None;
    }
    let visible_columns = grid_columns_ui::visible(&column_layout, set.columns.len());
    let frozen_count = grid_columns_ui::frozen_count(&column_layout, &visible_columns);
    tab.grid_layouts
        .insert(tab.result_index, column_layout.clone());
    let mut edit_cell = None;
    let mut draft_action = None;
    let mut remove_insert = None;
    let mut row_action = None;
    let mut editor_controls = None;
    let mut editor_input = None;
    let mut editor_opening = inline_editor.as_ref().is_some_and(|editor| editor.focus);
    let target = tab.table_target.as_ref();
    let metadata = tab.table_data.as_ref();
    let editing_location = inline_editor
        .as_ref()
        .filter(|editor| editor.tab == tab.id)
        .and_then(|editor| {
            let data = metadata?;
            Some((
                data.keys
                    .iter()
                    .position(|keys| keys == &editor.change.key_values)?,
                set.columns
                    .iter()
                    .position(|name| name == &editor.change.column)?,
            ))
        });
    let draft_summary = tab.draft_summary();
    let draft_buttons = |ui: &mut egui::Ui| {
        let mut action = None;
        if ui
            .add_enabled(draft_actions.0, egui::Button::new("提交全部"))
            .on_hover_text(format!(
                "先暂存当前编辑，再在同一事务中提交本标签的全部修改；{draft_summary}"
            ))
            .clicked()
        {
            action = Some(GridAction::SubmitDrafts);
        }
        if ui
            .add_enabled(draft_actions.1, egui::Button::new("撤销暂存"))
            .on_hover_text("取消当前编辑并清除本标签的本地草稿；不会撤销已提交的数据库写入")
            .clicked()
        {
            action = Some(GridAction::RevertDrafts);
        }
        action
    };
    if let Some(editor) = inline_editor.as_mut().filter(|editor| editor.tab == tab.id) {
        let controls = ui.horizontal(|ui| {
            ui.label(RichText::new(format!("编辑 {}", editor.change.column)).small());
            if ui
                .add_enabled(can_edit, egui::Checkbox::new(&mut editor.is_null, "NULL"))
                .changed()
            {
                editor.focus = true;
            }
            if ui.selectable_label(editor.expanded, "展开编辑").clicked() {
                editor.expanded = !editor.expanded;
                editor.focus = true;
            }
            if ui
                .add_enabled(can_edit, egui::Button::new("暂存"))
                .clicked()
            {
                row_action = Some(GridAction::FinishCell {
                    cancel: false,
                    movement: CellMovement::Stay,
                });
            }
            if ui.button("取消").clicked() {
                row_action = Some(GridAction::FinishCell {
                    cancel: true,
                    movement: CellMovement::Stay,
                });
            }
            ui.separator();
            if let Some(action) = draft_buttons(ui) {
                row_action = Some(action);
            }
            ui.label(
                RichText::new(if editor.expanded {
                    "⌘/Ctrl+Enter 暂存 · Esc 取消"
                } else {
                    "Tab 下一格 · Enter 下一行 · Esc 取消"
                })
                .small()
                .weak(),
            );
        });
        editor_controls = Some(controls.response);
        editor_opening |= editor.focus;
        if editor.expanded {
            let was_focus = editor.focus;
            let expanded = egui::Frame::group(ui.style())
                .inner_margin(6)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(("expanded-cell", tab.id))
                        .max_height(140.0)
                        .show(ui, |ui| {
                            let response = ui.add_enabled(
                                can_edit && !editor.is_null,
                                egui::TextEdit::multiline(&mut editor.text)
                                    .id(egui::Id::new(("inline-cell", tab.id)))
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(4),
                            );
                            if was_focus {
                                if !editor.is_null {
                                    response.request_focus();
                                }
                                editor.focus = false;
                            }
                            let (cancel, save) = ui.input_mut(|input| {
                                (
                                    input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                                    input.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter),
                                )
                            });
                            if row_action.is_none() && (cancel || (can_edit && save)) {
                                row_action = Some(GridAction::FinishCell {
                                    cancel,
                                    movement: CellMovement::Stay,
                                });
                            }
                            editor_input = Some(response);
                        });
                });
            editor_controls = editor_controls.map(|controls| controls | expanded.response);
        }
    }
    if has_drafts
        && inline_editor
            .as_ref()
            .is_none_or(|editor| editor.tab != tab.id)
    {
        ui.horizontal(|ui| {
            row_action = draft_buttons(ui);
            ui.label(RichText::new(&draft_summary).small().weak());
        });
    }
    let can_browse = can_edit
        && inline_editor.is_none()
        && target.is_some()
        && metadata.is_some()
        && !has_drafts;
    let staged_values: HashMap<_, _> = tab
        .staged_cells
        .iter()
        .map(|change| {
            (
                (change.key_values.as_slice(), change.column.as_str()),
                &change.value,
            )
        })
        .collect();
    let mut pointer_selection = grid_selection_ui::PointerSelection::new(ui, tab);
    let grid_response = egui::Frame::new()
        .fill(surface(ui))
        .corner_radius(6)
        .stroke(egui::Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            // Keep scrolled rows from painting over the fixed header.
            ui.visuals_mut().clip_rect_margin = 0.0;
            ui.visuals_mut().selection.bg_fill = accent(ui).gamma_multiply(0.18);
            let height = ui.available_height().max(0.0);
            let body_height =
                (height - GRID_HEADER_HEIGHT - ui.spacing().scroll.bar_width).max(0.0);
            let viewport_rect = ui.available_rect_before_wrap().intersect(ui.clip_rect());
            let horizontal_scroll = egui::ScrollArea::horizontal()
                .id_salt(("result-grid-scroll", tab.id, tab.result_index))
                .scroll_source(egui::scroll_area::ScrollSource { drag: false, ..Default::default() })
                .auto_shrink([false, false])
                .show_viewport(ui, |ui, _viewport| {
                    // ScrollArea rounds its content origin to pixels; derive the exact
                    // displacement to keep pinned text steady during smooth scrolling.
                    let horizontal_offset = viewport_rect.left() - ui.max_rect().left();
                    let widths_id = ui
                        .id()
                        .with(("grid-column-widths", tab.id, tab.result_index));
                    let mut widths: Vec<_> = visible_columns.iter().map(|&index| column_layout.columns[index].width).collect();
                    let frozen_boundary = if frozen_count > 0 {
                        (viewport_rect.left() + 44.0 + widths.iter().take(frozen_count).sum::<f32>()).min(viewport_rect.right())
                    } else { viewport_rect.left() };
                    let mut header_rects = Vec::new();
                    let mut builder = TableBuilder::new(ui)
                        .id_salt(("data-grid", tab.id, tab.result_index))
                        .drag_to_scroll(false)
                        .striped(true)
                        .resizable(false)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .min_scrolled_height(body_height)
                        .max_scroll_height(body_height)
                        .column(Column::exact(44.0));
                    for width in &widths {
                        builder = builder.column(Column::exact(*width).clip(true));
                    }
                    if let Some(row) = tab.grid_scroll_row.take() {
                        builder = builder.scroll_to_row(row, Some(egui::Align::Center));
                    }
                    if let Some((row, _)) = editing_location.filter(|_| inline_editor.as_ref().is_some_and(|editor| editor.focus)) {
                        builder = builder.scroll_to_row(row, None);
                    }
                    if let Some(index) = tab.insert_focus.and_then(|id| tab.insert_drafts.iter().position(|draft| draft.id == id)) {
                        builder = builder.scroll_to_row(set.rows.len() + index, Some(egui::Align::BOTTOM));
                    }
                    let vertical_scroll = builder
                        .header(GRID_HEADER_HEIGHT, |mut header| {
                            header.col(|ui| grid_columns_ui::cell(ui, frozen_count > 0, horizontal_offset, frozen_boundary, viewport_rect, |ui| {
                                grid_header_cell(ui, "#", "行号");
                            }));
                            for (display_index, &index) in visible_columns.iter().enumerate() {
                                let name = &set.columns[index];
                                header.col(|ui| grid_columns_ui::cell(ui, display_index < frozen_count, horizontal_offset, frozen_boundary, viewport_rect, |ui| {
                                    let column = metadata.and_then(|data| data.columns.get(index));
                                    let sql_type = column
                                        .map(|column| column.declared_type.as_str())
                                        .or_else(|| set.column_types.get(index).map(String::as_str))
                                        .filter(|sql_type| !sql_type.is_empty())
                                        .unwrap_or("类型未知");
                                    let sort = tab.browse.sorts.iter().enumerate().find(|(_, sort)| sort.column == *name);
                                    let filtered = tab.browse.filters.iter().any(|filter| filter.column == *name);
                                    let marker = sort.map(|(index, sort)| {
                                        format!("{}{}{}", if sort.descending { "↓" } else { "↑" },
                                            if tab.browse.sorts.len() > 1 { (index + 1).to_string() } else { String::new() },
                                            if filtered { " •" } else { "" })
                                    }).or_else(|| filtered.then(|| "•".into()));
                                    let response = grid_header_cell_marked(ui, name, sql_type, marker.as_deref());
                                    header_rects.push(response.rect);
                                    let key = if column.is_some_and(|column| column.primary_key) {
                                        " · 主键"
                                    } else {
                                        ""
                                    };
                                    let hint = if target.is_some() {
                                        "单击：升序 / 降序 / 清除 · Shift+单击追加排序\n右键：排序或筛选 · 拖动分隔线调整列宽\n排序在数据库执行，返回第一页；有暂存修改时禁用"
                                    } else {
                                        "拖动表头分隔线调整列宽"
                                    };
                                    let response = response.on_hover_text(format!("{name} · {sql_type}{key}\n{hint}"));
                                    if target.is_some()
                                        && let Some(action) = grid_browse_ui::header_controls(response, name, &tab.browse, can_browse) {
                                        row_action = Some(GridAction::Browse(action));
                                    }
                                }));
                            }
                        })
                        .body(|body| {
                            body.rows(32.0, set.rows.len() + tab.insert_drafts.len(), |mut row| {
                                let row_index = row.index();
                                if row_index >= set.rows.len() {
                                    let draft = &mut tab.insert_drafts[row_index - set.rows.len()];
                                    if table_insert_ui::render_draft_row(&mut row, draft, metadata.map_or(&[], |data| data.columns.as_slice()), table_insert_ui::DraftColumnLayout { visible_columns: &visible_columns, frozen_count, horizontal_offset, frozen_boundary, viewport_rect }, can_edit, &mut tab.selected_insert, &mut tab.insert_focus) {
                                        remove_insert = Some(draft.id);
                                    }
                                    if tab.selected_insert == Some(draft.id) {
                                        tab.selected_rows = None;
                                        tab.selected_cell = None;
                                        tab.cell_anchor = None;
                                    }
                                    return;
                                }
                                row.set_selected(row_in_selection(tab.selected_rows, row_index));
                                row.col(|ui| grid_columns_ui::cell(ui, frozen_count > 0, horizontal_offset, frozen_boundary, viewport_rect, |ui| {
                                    let response = grid_cell(
                                        ui,
                                        RichText::new((row_index + 1).to_string()).weak(),
                                        false,
                                        false,
                                    );
                                    if response.secondary_clicked() { tab.selected_insert = None; }
                                    if inline_editor.is_none() {
                                        pointer_selection.cell(ui, &response, grid_selection_ui::SelectionRefs { rows: &mut tab.selected_rows, cell: &mut tab.selected_cell, anchor: &mut tab.cell_anchor, insert: &mut tab.selected_insert }, (row_index, 0), true);
                                    }
                                    let response = response.on_hover_text(
                                        "单击行号选中整行 · 拖动行号选择多行 · Shift+单击扩展选择 · 右键打开行操作 · ⌘/Ctrl+A 全选 · ⌘/Ctrl+C 复制",
                                    );
                                    let deletable = can_edit
                                        && !has_drafts
                                        && tab.table_target.is_some()
                                        && tab
                                            .table_data
                                            .as_ref()
                                            .is_some_and(TableData::can_delete_rows);
                                    if let Some(action) = row_context_menu(
                                        response,
                                        &mut tab.selected_rows,
                                        set,
                                        &visible_columns,
                                        deletable,
                                        row_index,
                                    ) {
                                        row_action = Some(action);
                                    }
                                }));
                                for (display_index, &column_index) in visible_columns.iter().enumerate() {
                                    let cell = &set.rows[row_index][column_index];
                                    row.col(|ui| grid_columns_ui::cell(ui, display_index < frozen_count, horizontal_offset, frozen_boundary, viewport_rect, |ui| {
                                        if editing_location != Some((row_index, column_index))
                                            && !ui.is_sizing_pass()
                                            && !ui.is_rect_visible(ui.available_rect_before_wrap()) {
                                            ui.allocate_space(ui.available_rect_before_wrap().size());
                                            return;
                                        }
                                        let staged_value = metadata.and_then(|data| data.keys.get(row_index))
                                            .and_then(|keys| staged_values.get(&(keys.as_slice(), set.columns[column_index].as_str())))
                                            .copied();
                                        if editing_location == Some((row_index, column_index))
                                            && let Some(editor) = inline_editor.as_mut()
                                            && !editor.expanded {
                                            let was_focus = editor.focus;
                                            let response = grid_cell_input(ui, tab.id, editor, can_edit);
                                            if was_focus {
                                                if !editor.is_null { response.request_focus(); }
                                                editor.focus = false;
                                            }
                                            let (escape, enter, next, shift) = ui.input_mut(|input| {
                                                let shift = input.modifiers.shift;
                                                let escape = input.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
                                                let modifiers = if shift { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE };
                                                let enter = input.consume_key(modifiers, egui::Key::Enter);
                                                let next = input.consume_key(modifiers, egui::Key::Tab);
                                                (escape, enter, next, shift)
                                            });
                                            if row_action.is_none() && (escape || (can_edit && (enter || next))) {
                                                    row_action = Some(GridAction::FinishCell { cancel: escape, movement: if next {
                                                        CellMovement::Across(if shift { -1 } else { 1 })
                                                    } else if enter { CellMovement::Down(if shift { -1 } else { 1 }) }
                                                    else { CellMovement::Stay } });
                                            }
                                            editor_input = Some(response);
                                            return;
                                        }
                                        let displayed = staged_value.unwrap_or(cell);
                                        let value = displayed.as_deref().unwrap_or("NULL");
                                        let text = if staged_value.is_some() {
                                            RichText::new(format!("● {value}"))
                                                .strong().color(accent(ui))
                                        } else if displayed.is_none() {
                                            RichText::new(value).italics().weak()
                                        } else {
                                            RichText::new(value)
                                        };
                                        let response = grid_cell(
                                            ui,
                                            text,
                                            cell_in_selection(
                                                tab.cell_anchor,
                                                tab.selected_cell,
                                                row_index,
                                                column_index,
                                            ),
                                            false,
                                        );
                                        if inline_editor.is_none() {
                                            pointer_selection.cell(ui, &response, grid_selection_ui::SelectionRefs { rows: &mut tab.selected_rows, cell: &mut tab.selected_cell, anchor: &mut tab.cell_anchor, insert: &mut tab.selected_insert }, (row_index, column_index), false);
                                        }
                                        let editable = inline_editor.is_none() && grid_cell_eligible(can_edit, target, metadata, set, row_index, column_index);
                                        let begin_edit = response.double_clicked() && editable && inline_editor.is_none();
                                        let mut menu_edit = false;
                                        if response.secondary_clicked() { tab.selected_insert = None; }
                                        if response.secondary_clicked()
                                            && !cell_in_selection(tab.cell_anchor, tab.selected_cell, row_index, column_index)
                                            && !row_in_selection(tab.selected_rows, row_index)
                                        {
                                            tab.selected_insert = None;
                                            tab.selected_rows = Some((row_index, row_index));
                                            tab.selected_cell = None;
                                            tab.cell_anchor = None;
                                        }
                                        let response = response.on_hover_ui(|ui| {
                                            if staged_value.is_some() {
                                                ui.label(format!("暂存值：{displayed:?}\n数据库快照：{cell:?}\n复制和导出使用数据库快照；双击可继续编辑"));
                                            } else {
                                                ui.label(value);
                                                if editable {
                                                    ui.label("双击 / F2 编辑");
                                                } else if target.is_none() {
                                                    ui.label("查询结果只读");
                                                }
                                            }
                                        });
                                        response.context_menu(|ui| {
                                            if cell_in_selection(tab.cell_anchor, tab.selected_cell, row_index, column_index)
                                                && let Some((end_row, end_column)) = tab.selected_cell {
                                                let (start_row, start_column) = tab.cell_anchor.unwrap_or((end_row, end_column));
                                                let count = (start_row.abs_diff(end_row) + 1) * visible_columns.iter().filter(|&&column| (start_column.min(end_column)..=start_column.max(end_column)).contains(&column)).count();
                                                if count > 1 { ui.label(format!("操作选中 {count} 格")); }
                                            }
                                            if target.is_some() {
                                                let filterable = metadata.and_then(|data| data.columns.get(column_index))
                                                    .is_some_and(|column| column.kind != sqlshop::model::CellKind::Unsupported);
                                                let text_column = metadata.and_then(|data| data.columns.get(column_index))
                                                    .is_some_and(|column| column.kind == sqlshop::model::CellKind::Text);
                                                if let Some(action) = grid_browse_ui::cell_filter_controls(ui, &set.columns[column_index], cell, text_column, can_browse && filterable) {
                                                    row_action = Some(GridAction::Browse(action));
                                                }
                                                ui.separator();
                                            }
                                            if ui.add_enabled(editable, egui::Button::new("设为 NULL"))
                                                .on_hover_text("仅暂存此格为 NULL，提交全部修改后写入数据库").clicked() {
                                                draft_action = Some((row_index, column_index, CellDraftAction::SetNull));
                                                ui.close();
                                            }
                                            let text_column = metadata.and_then(|data| data.columns.get(column_index))
                                                .is_some_and(|column| column.kind == sqlshop::model::CellKind::Text);
                                            if ui.add_enabled(editable && text_column, egui::Button::new("设为空字符串"))
                                                .on_hover_text("仅暂存此文本格为空字符串；空字符串与 NULL 不同").clicked() {
                                                draft_action = Some((row_index, column_index, CellDraftAction::SetEmptyString));
                                                ui.close();
                                            }
                                            if ui.add_enabled(editable && staged_value.is_some(), egui::Button::new("撤销此格修改"))
                                                .on_hover_text("恢复此格的数据库快照值，保留其他格的暂存修改").clicked() {
                                                draft_action = Some((row_index, column_index, CellDraftAction::Revert));
                                                ui.close();
                                            }
                                            ui.separator();
                                            let rows = tab
                                                .selected_rows
                                                .map(|(a, b)| {
                                                    (a.min(b)..=a.max(b)).collect::<Vec<_>>()
                                                })
                                                .unwrap_or_else(|| vec![row_index]);
                                            if ui
                                                .button(format!(
                                                    "复制选中行 ({} 行, TSV)",
                                                    rows.len()
                                                ))
                                                .clicked()
                                            {
                                                ui.ctx().copy_text(grid_columns_ui::rows_tsv(set, &visible_columns, &rows));
                                                ui.close();
                                            }
                                            if ui
                                                .button(format!(
                                                    "导出选中行 CSV… ({} 行)",
                                                    rows.len()
                                                ))
                                                .clicked()
                                            {
                                                row_action =
                                                    Some(GridAction::ExportRows(rows.clone()));
                                                ui.close();
                                            }
                                            let deletable = can_edit
                                        && !has_drafts
                                                && tab.table_target.is_some()
                                                && tab.table_data.as_ref().is_some_and(|data| {
                                                    data.can_delete_rows()
                                                });
                                            if ui
                                                .add_enabled(
                                                    deletable,
                                                    egui::Button::new(format!(
                                                        "删除选中行… ({} 行)",
                                                        rows.len()
                                                    )),
                                                )
                                                .clicked()
                                            {
                                                row_action = Some(GridAction::DeleteRows(rows));
                                                ui.close();
                                            }
                                            if ui.button("复制单元格").clicked() {
                                                ui.ctx().copy_text(cell.as_deref().unwrap_or("NULL").into());
                                                ui.close();
                                            }
                                            if ui
                                                .add_enabled(
                                                    editable && inline_editor.is_none(),
                                                    egui::Button::new("编辑单元格"),
                                                )
                                                .clicked()
                                            {
                                                menu_edit = true;
                                                ui.close();
                                            }
                                        });
                                        if begin_edit || menu_edit {
                                            edit_cell = Some((row_index, column_index));
                                        }
                                    }));
                                }
                            });
                        });
                    pointer_selection.exclude_scrollbar(ui, &vertical_scroll, 1);
                    resize_grid_columns(ui, widths_id, &header_rects, &mut widths);
                    for (&index, width) in visible_columns.iter().zip(widths) { column_layout.columns[index].width = width; }
                });
            pointer_selection.exclude_scrollbar(ui, &horizontal_scroll, 0);
        });
    if row_action.is_none()
        && can_edit
        && !editor_opening
        && !egui::Popup::is_any_open(ui.ctx())
        && let (Some(controls), Some(input)) = (editor_controls, editor_input)
    {
        // NULL inputs are disabled and never gain focus, so also check outside clicks.
        let outside_click = controls.clicked_elsewhere() && input.clicked_elsewhere();
        let (pressed, position) =
            ui.input(|state| (state.pointer.any_pressed(), state.pointer.interact_pos()));
        let controls_interaction = pressed
            && position.is_some_and(|position| {
                controls.interact_rect.contains(position)
                    && ui.ctx().layer_id_at(position) == Some(controls.layer_id)
            });
        if outside_click || (input.lost_focus() && !controls_interaction) {
            row_action = Some(GridAction::FinishCell {
                cancel: false,
                movement: CellMovement::Stay,
            });
        }
    }
    pointer_selection.finish(
        ui,
        grid_selection_ui::SelectionRefs {
            rows: &mut tab.selected_rows,
            cell: &mut tab.selected_cell,
            anchor: &mut tab.cell_anchor,
            insert: &mut tab.selected_insert,
        },
    );
    let pointer_over_grid = ui
        .ctx()
        .pointer_latest_pos()
        .is_some_and(|position| grid_response.response.rect.contains(position));
    let owner_id = egui::Id::new("grid-keyboard-owner");
    if pointer_over_grid && ui.input(|input| input.pointer.any_pressed()) {
        ui.data_mut(|data| data.insert_temp(owner_id, tab.id));
    }
    let keyboard_shortcuts = !ui.ctx().wants_keyboard_input()
        && (pointer_over_grid || ui.data(|data| data.get_temp::<Uuid>(owner_id)) == Some(tab.id));
    let (select_all, copy) = ui.ctx().input(|input| {
        (
            keyboard_shortcuts && input.modifiers.command && input.key_pressed(egui::Key::A),
            keyboard_shortcuts && input.modifiers.command && input.key_pressed(egui::Key::C),
        )
    });
    if keyboard_shortcuts
        && can_edit
        && inline_editor.is_none()
        && tab.selected_cell.is_some()
        && let Some(text) = ui.input(|input| {
            input.events.iter().find_map(|event| {
                if let egui::Event::Paste(text) = event {
                    Some(text.clone())
                } else {
                    None
                }
            })
        })
    {
        row_action = Some(GridAction::PasteValues(text));
    }
    if select_all && !set.rows.is_empty() {
        tab.selected_insert = None;
        tab.selected_rows = Some((0, set.rows.len() - 1));
        tab.selected_cell = None;
        tab.cell_anchor = None;
    }
    if copy {
        let rows = tab
            .selected_rows
            .map(|(anchor, end)| (anchor.min(end)..=anchor.max(end)).collect::<Vec<_>>());
        let text = grid_columns_ui::selected_tsv(tab, set, rows.as_deref());
        if !text.is_empty() {
            ui.ctx().copy_text(text);
        }
    }
    if ui.is_enabled()
        && keyboard_shortcuts
        && !egui::Popup::is_any_open(ui.ctx())
        && inline_editor.is_none()
        && let Some((row, column)) = tab.selected_cell
    {
        let (horizontal, vertical, home, end, extend) = ui.input_mut(|input| {
            let modifiers = if input.modifiers.shift {
                egui::Modifiers::SHIFT
            } else {
                egui::Modifiers::NONE
            };
            (
                i32::from(input.consume_key(modifiers, egui::Key::ArrowRight))
                    - i32::from(input.consume_key(modifiers, egui::Key::ArrowLeft)),
                i32::from(input.consume_key(modifiers, egui::Key::ArrowDown))
                    - i32::from(input.consume_key(modifiers, egui::Key::ArrowUp)),
                input.consume_key(modifiers, egui::Key::Home),
                input.consume_key(modifiers, egui::Key::End),
                input.modifiers.shift,
            )
        });
        if horizontal != 0 || vertical != 0 || home || end {
            let next_row = if home {
                0
            } else if end {
                set.rows.len().saturating_sub(1)
            } else {
                row.saturating_add_signed(vertical as isize)
                    .min(set.rows.len().saturating_sub(1))
            };
            let display_index = visible_columns
                .iter()
                .position(|&index| index == column)
                .unwrap_or(0);
            let next_column = visible_columns
                .get(
                    display_index
                        .saturating_add_signed(horizontal as isize)
                        .min(visible_columns.len().saturating_sub(1)),
                )
                .copied()
                .unwrap_or(column);
            tab.selected_cell = Some((next_row, next_column));
            tab.grid_scroll_row = Some(next_row);
            if !extend {
                tab.cell_anchor = tab.selected_cell;
            }
            tab.selected_rows = None;
        }
    }
    if ui.is_enabled()
        && !ui.ctx().wants_keyboard_input()
        && !egui::Popup::is_any_open(ui.ctx())
        && can_edit
        && inline_editor.is_none()
        && let Some(selected) = tab.selected_cell
        && ui.input_mut(|input| {
            input.consume_key(egui::Modifiers::NONE, egui::Key::F2)
                || input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
        })
    {
        edit_cell = Some(selected);
    }
    if let Some((row, column, action)) = draft_action
        && let Err(message) = apply_grid_selection_draft(tab, row, column, action, can_edit)
    {
        row_action = Some(GridAction::DraftError(message));
    }
    if let Some(id) = remove_insert {
        tab.insert_drafts.retain(|draft| draft.id != id);
        if tab.selected_insert == Some(id) {
            tab.selected_insert = None;
        }
        if tab.insert_focus == Some(id) {
            tab.insert_focus = None;
        }
    }
    tab.grid_layouts.insert(tab.result_index, column_layout);
    let editor = edit_cell.and_then(|(row, column)| grid_cell_editor(tab, row, column, can_edit));
    (editor, row_action)
}

pub(super) fn resize_grid_columns(
    ui: &mut egui::Ui,
    id: egui::Id,
    headers: &[egui::Rect],
    widths: &mut [f32],
) {
    // Keep resize hitboxes in the header, away from the body scroll bar.
    let radius = ui.style().interaction.resize_grab_radius_side;
    for (index, (header, width)) in headers.iter().zip(widths).enumerate() {
        let rect = egui::Rect::from_min_max(
            egui::pos2(header.right() - radius, header.top()),
            egui::pos2(header.right() + radius, header.bottom()),
        );
        let response = ui
            .interact(rect, id.with(index), egui::Sense::drag())
            .on_hover_and_drag_cursor(egui::CursorIcon::ResizeColumn);
        if response.dragged() {
            *width = (*width + response.drag_delta().x).max(110.0);
        }
    }
}

pub(super) fn row_selection(
    previous: Option<(usize, usize)>,
    row: usize,
    extend: bool,
) -> (usize, usize) {
    let anchor = if extend {
        previous.map_or(row, |(anchor, _)| anchor)
    } else {
        row
    };
    (anchor, row)
}

pub(super) fn row_in_selection(selection: Option<(usize, usize)>, row: usize) -> bool {
    selection.is_some_and(|(anchor, end)| (anchor.min(end)..=anchor.max(end)).contains(&row))
}

pub(super) fn cell_in_selection(
    anchor: Option<(usize, usize)>,
    end: Option<(usize, usize)>,
    row: usize,
    column: usize,
) -> bool {
    let Some((r1, c1)) = end else { return false };
    let (r0, c0) = anchor.unwrap_or((r1, c1));
    (r0.min(r1)..=r0.max(r1)).contains(&row) && (c0.min(c1)..=c0.max(c1)).contains(&column)
}

pub(super) fn selected_tsv(
    set: &sqlshop::model::ResultSet,
    anchor: Option<(usize, usize)>,
    end: Option<(usize, usize)>,
) -> String {
    let Some((r1, c1)) = end else {
        return String::new();
    };
    let (r0, c0) = anchor.unwrap_or((r1, c1));
    let width = set.rows.iter().map(Vec::len).max().unwrap_or(0);
    borrowed_rows_tsv(
        set.rows
            .iter()
            .skip(r0.min(r1))
            .take(r0.max(r1).saturating_sub(r0.min(r1)).saturating_add(1)),
        c0.min(c1),
        c0.max(c1),
        width,
    )
}

#[cfg(test)]
pub(super) fn selected_rows_tsv(set: &ResultSet, rows: &[usize]) -> String {
    let width = rows
        .iter()
        .filter_map(|index| set.rows.get(*index))
        .map(Vec::len)
        .max()
        .unwrap_or(0);
    borrowed_rows_tsv(
        rows.iter().filter_map(|index| set.rows.get(*index)),
        0,
        set.columns.len().saturating_sub(1),
        width,
    )
}

pub(super) fn borrowed_rows_tsv<'a>(
    rows: impl Iterator<Item = &'a Vec<Option<String>>>,
    start_column: usize,
    end_column: usize,
    width: usize,
) -> String {
    if start_column >= width {
        return String::new();
    }
    let mut output = String::new();
    for (index, row) in rows.enumerate() {
        if index > 0 {
            output.push('\n');
        }
        for column in start_column..=end_column.min(width - 1) {
            if column > start_column {
                output.push('\t');
            }
            let value = row
                .get(column)
                .map(|value| value.as_deref().unwrap_or("NULL"))
                .unwrap_or("");
            if value.contains(['\t', '\n', '\r', '"']) {
                output.push('"');
                for character in value.chars() {
                    if character == '"' {
                        output.push('"');
                    }
                    output.push(character);
                }
                output.push('"');
            } else {
                output.push_str(value);
            }
        }
    }
    output
}
