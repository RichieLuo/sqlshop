use super::*;

pub(in crate::ui) fn grid_cell_eligible(
    can_edit: bool,
    target: Option<&TableTarget>,
    data: Option<&TableData>,
    set: &ResultSet,
    row: usize,
    column: usize,
) -> bool {
    can_edit
        && target.is_some()
        && data.is_some_and(|data| {
            data.writable
                && !data.primary_keys.is_empty()
                && data.keys.get(row).is_some_and(|keys| {
                    keys.len() == data.primary_keys.len() && keys.iter().all(Option::is_some)
                })
                && data.columns.get(column).is_some_and(|metadata| {
                    metadata.editable
                        && metadata.kind != sqlshop::model::CellKind::Unsupported
                        && set.columns.get(column) == Some(&metadata.name)
                })
                && data
                    .columns
                    .iter()
                    .filter(|column| column.primary_key)
                    .all(|column| column.kind != sqlshop::model::CellKind::Unsupported)
                && data
                    .columns
                    .iter()
                    .filter(|column| column.primary_key)
                    .map(|column| &column.name)
                    .eq(data.primary_keys.iter())
                && set
                    .rows
                    .get(row)
                    .is_some_and(|values| values.get(column).is_some())
        })
}

pub(in crate::ui) fn grid_cell_editor(
    tab: &QueryTab,
    row: usize,
    column: usize,
    can_edit: bool,
) -> Option<CellEditor> {
    let set = tab.output.as_ref()?.sets.get(tab.result_index)?;
    let target = tab.table_target.as_ref()?;
    let data = tab.table_data.as_ref()?;
    if !grid_cell_eligible(
        can_edit && !tab.table_snapshot_stale,
        Some(target),
        Some(data),
        set,
        row,
        column,
    ) {
        return None;
    }
    let original = set.rows[row][column].clone();
    let value = tab
        .staged_cells
        .iter()
        .find(|change| change.key_values == data.keys[row] && change.column == set.columns[column])
        .map_or_else(|| original.clone(), |change| change.value.clone());
    Some(CellEditor {
        tab: tab.id,
        connection: target.connection,
        change: CellUpdate {
            database: target.database.clone(),
            table: Table {
                schema: target.schema.clone(),
                name: target.name.clone(),
                kind: "BASE TABLE".into(),
            },
            primary_keys: data.primary_keys.clone(),
            key_values: data.keys[row].clone(),
            column: set.columns[column].clone(),
            original,
            value: value.clone(),
        },
        text: value.clone().unwrap_or_default(),
        is_null: value.is_none(),
        focus: true,
        expanded: value
            .as_deref()
            .is_some_and(|text| text.contains('\n') || text.chars().count() > 140),
    })
}

pub(in crate::ui) fn apply_grid_cell_draft(
    tab: &mut QueryTab,
    row: usize,
    column: usize,
    action: CellDraftAction,
    can_edit: bool,
) -> bool {
    let Some(editor) = grid_cell_editor(tab, row, column, can_edit) else {
        return false;
    };
    match action {
        CellDraftAction::Revert => {
            tab.staged_cells.retain(|change| {
                change.key_values != editor.change.key_values
                    || change.column != editor.change.column
            });
        }
        CellDraftAction::SetNull => tab.stage_cell(CellUpdate {
            value: None,
            ..editor.change
        }),
        CellDraftAction::SetEmptyString => {
            if tab.table_data.as_ref().unwrap().columns[column].kind
                != sqlshop::model::CellKind::Text
            {
                return false;
            }
            tab.stage_cell(CellUpdate {
                value: Some(String::new()),
                ..editor.change
            });
        }
    }
    true
}

pub(in crate::ui) fn selected_grid_cells(
    tab: &QueryTab,
    row: usize,
    column: usize,
) -> Vec<(usize, usize)> {
    if !cell_in_selection(tab.cell_anchor, tab.selected_cell, row, column) {
        return vec![(row, column)];
    }
    let (end_row, end_column) = tab.selected_cell.unwrap();
    let (start_row, start_column) = tab.cell_anchor.unwrap_or((end_row, end_column));
    let count = tab
        .output
        .as_ref()
        .and_then(|output| output.sets.get(tab.result_index))
        .map_or(0, |set| set.columns.len());
    let columns = grid_columns_ui::tab_visible(tab, count);
    (start_row.min(end_row)..=start_row.max(end_row))
        .flat_map(|row| {
            columns
                .iter()
                .copied()
                .filter(move |column| {
                    (start_column.min(end_column)..=start_column.max(end_column)).contains(column)
                })
                .map(move |column| (row, column))
        })
        .collect()
}

pub(in crate::ui) fn apply_grid_selection_draft(
    tab: &mut QueryTab,
    row: usize,
    column: usize,
    action: CellDraftAction,
    can_edit: bool,
) -> Result<usize, String> {
    let cells = selected_grid_cells(tab, row, column);
    // Validate the complete block before changing any draft.
    for &(row, column) in &cells {
        if grid_cell_editor(tab, row, column, can_edit).is_none() {
            return Err("选中区域包含只读或无效单元格，未修改草稿。".into());
        }
        if matches!(action, CellDraftAction::SetEmptyString)
            && tab.table_data.as_ref().unwrap().columns[column].kind
                != sqlshop::model::CellKind::Text
        {
            return Err("空字符串操作仅支持文本列，未修改草稿。".into());
        }
    }
    let count = cells.len();
    for (row, column) in cells {
        apply_grid_cell_draft(tab, row, column, action, can_edit);
    }
    Ok(count)
}

pub(in crate::ui) fn paste_grid_values(
    tab: &mut QueryTab,
    text: &str,
    can_edit: bool,
) -> Result<usize, String> {
    let (row, column) = tab.selected_cell.ok_or("请先选择粘贴的起始单元格")?;
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .delimiter(b'\t')
        .flexible(false)
        .from_reader(text.as_bytes());
    let records = reader
        .records()
        .map(|record| record.map(|record| record.iter().map(str::to_owned).collect::<Vec<_>>()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法解析粘贴内容：{error}"))?;
    if records.is_empty() {
        return Ok(0);
    }
    let destinations: Vec<_> = if records.len() == 1 && records[0].len() == 1 {
        selected_grid_cells(tab, row, column)
            .into_iter()
            .map(|(row, column)| (row, column, records[0][0].clone()))
            .collect()
    } else {
        let anchor = tab.cell_anchor.unwrap_or((row, column));
        let start_row = anchor.0.min(row);
        let start_column = anchor.1.min(column);
        let count = tab
            .output
            .as_ref()
            .and_then(|output| output.sets.get(tab.result_index))
            .map_or(0, |set| set.columns.len());
        let columns = grid_columns_ui::tab_visible(tab, count);
        let first = columns
            .iter()
            .position(|&index| index == start_column)
            .ok_or("粘贴起点已隐藏")?;
        if records
            .iter()
            .any(|values| first + values.len() > columns.len())
        {
            return Err("粘贴区域超出可见列，未修改草稿。".into());
        }
        let columns = &columns;
        records
            .iter()
            .enumerate()
            .flat_map(|(r, values)| {
                values
                    .iter()
                    .enumerate()
                    .map(move |(c, value)| (start_row + r, columns[first + c], value.clone()))
            })
            .collect()
    };
    if destinations.len() > 10_000 {
        return Err("一次最多暂存 10000 格，请缩小粘贴范围。".into());
    }
    let mut changes = Vec::with_capacity(destinations.len());
    for (row, column, value) in destinations {
        let mut editor = grid_cell_editor(tab, row, column, can_edit)
            .ok_or("粘贴区域超出当前页或包含只读单元格，未修改草稿。")?;
        // Pasted text stays text, including the literal NULL and empty string.
        editor.change.value = Some(value);
        changes.push(editor.change);
    }
    let count = changes.len();
    for change in changes {
        tab.stage_cell(change);
    }
    Ok(count)
}
