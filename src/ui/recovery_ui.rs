mod resolution;

use eframe::egui::{self, RichText};
use sqlshop::{
    db::TablePage,
    model::{CellUpdate, InsertValue},
    recovery::{DraftVerification, RowVerification, VerificationState},
};

#[derive(Default)]
pub(super) struct RecoveryDocument {
    pub report: Option<DraftVerification>,
    pub fresh_page: Option<TablePage>,
    pub error: Option<String>,
    pub busy: bool,
    /// The document still has unresolved local drafts after a failed submission.
    pub pending: bool,
    pub manager_reconnect_required: bool,
    pub manager_reset_required: bool,
}

#[derive(Clone)]
pub(super) enum RecoveryAction {
    CheckAgain,
    KeepOriginalDrafts,
    /// Explicitly selected indexes in report.rows, each currently Intended.
    DiscardMatchingDrafts(Vec<usize>),
    DiscardAll,
    /// Explicitly selected indexes in report.rows; no primary key is rewritten.
    RebaseConflicts(Vec<usize>),
}

/// Draw only. The host validates source identity, epoch, immutable draft
/// equality, and each requested resolution again before changing local state.
pub(super) fn show_recovery(
    ui: &mut egui::Ui,
    document: &RecoveryDocument,
    staged_changes: &[CellUpdate],
    inserts: &[Vec<InsertValue>],
) -> Option<RecoveryAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.strong("提交结果核实");
        if document.busy {
            ui.spinner();
            ui.label("正在核实…");
        } else if document.pending {
            ui.label("草稿已保留，等待处理");
        }
    });
    ui.label("提交未确认时需先核实；草稿处理不撤销服务器提交。");
    if let Some(error) = &document.error {
        let color = ui.visuals().error_fg_color;
        ui.label(RichText::new(error).color(color));
    }
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(!document.busy, egui::Button::new("重新核实"))
            .on_hover_text("使用新连接只读核实；处理按钮仅修改本地草稿。")
            .clicked()
        {
            action = Some(RecoveryAction::CheckAgain);
        }
        if ui
            .add_enabled(
                !document.busy && (!staged_changes.is_empty() || !inserts.is_empty()),
                egui::Button::new("放弃全部本地草稿"),
            )
            .on_hover_text("仅放弃本地编辑，不会撤销可能已在服务器提交的事务")
            .clicked()
        {
            action = Some(RecoveryAction::DiscardAll);
        }
    });
    let Some(report) = &document.report else {
        ui.separator();
        ui.label(format!(
            "保留 {} 个字段修改、{} 条新增草稿。",
            staged_changes.len(),
            inserts.len()
        ));
        ui.label("尚无核实结果。");
        if !inserts.is_empty() {
            ui.label("新增记录无可靠回执，无法自动判定是否已提交。");
        }
        return action;
    };
    let current_report = report_matches(report, staged_changes, inserts);
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "{}.{} · {}",
            report.table.schema, report.table.name, report.database
        ));
        ui.weak(format!("核实时间：{}", report.checked_at));
    });
    ui.horizontal_wrapped(|ui| {
        for state in [
            VerificationState::Original,
            VerificationState::Intended,
            VerificationState::Conflict,
            VerificationState::Missing,
            VerificationState::Unknown,
        ] {
            ui.label(format!("{} {}", state.label(), report.count(state)));
        }
    });
    ui.label(&report.note);
    if let Some(page) = &document.fresh_page {
        let count = page
            .data
            .output
            .sets
            .first()
            .map_or(0, |set| set.rows.len());
        ui.weak(format!("已读取当前页 {count} 条记录"))
            .on_hover_text("原草稿和主键快照未替换。");
    }
    if !current_report {
        ui.label(RichText::new("草稿已变化；需重新核实后处理。").color(ui.visuals().warn_fg_color));
    }
    let selection_id = ui
        .id()
        .with(("table-recovery-selection", &report.checked_at));
    let mut selected = ui
        .data(|data| data.get_temp::<Vec<usize>>(selection_id))
        .unwrap_or_default();
    selected.retain(|index| report.rows.get(*index).is_some_and(selectable));
    egui::ScrollArea::both()
        .id_salt("table-recovery-evidence")
        .max_height((ui.available_height() - 155.0).max(40.0))
        .show(ui, |ui| {
            for (index, row) in report.rows.iter().enumerate() {
                ui.push_id(index, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let mut checked = selected.contains(&index);
                        if ui
                            .add_enabled(
                                !document.busy && current_report && selectable(row),
                                egui::Checkbox::new(&mut checked, "选择"),
                            )
                            .changed()
                        {
                            if checked {
                                selected.push(index);
                            } else {
                                selected.retain(|selected| *selected != index);
                            }
                        }
                        ui.strong(format!("记录 {} · {}", index + 1, row.state.label()));
                        ui.label(key_text(row));
                    });
                    ui.label(&row.reason);
                    egui::Grid::new("recovery-fields")
                        .striped(true)
                        .min_col_width(105.0)
                        .show(ui, |ui| {
                            for heading in ["字段", "原值", "当前值（按查询键）", "拟提交值"]
                            {
                                ui.strong(heading);
                            }
                            ui.end_row();
                            for change in &row.changes {
                                ui.label(&change.column);
                                cell(ui, &change.original);
                                ui.label(current_values(row, &change.column));
                                cell(ui, &change.value);
                                ui.end_row();
                            }
                        });
                    if !row.evidence.is_empty() {
                        ui.collapsing("查看按原键／目标键读取的完整记录", |ui| {
                            for (lookup, evidence) in row.evidence.iter().enumerate() {
                                ui.push_id(lookup, |ui| {
                                    ui.weak(format!(
                                        "查询键：{} · {} 条匹配",
                                        format_key(&evidence.lookup_key),
                                        evidence.rows.len()
                                    ));
                                    egui::Grid::new("recovery-server-row").striped(true).show(
                                        ui,
                                        |ui| {
                                            for name in &evidence.columns {
                                                ui.strong(name);
                                            }
                                            ui.end_row();
                                            for values in &evidence.rows {
                                                for value in values {
                                                    cell(ui, value);
                                                }
                                                ui.end_row();
                                            }
                                        },
                                    );
                                });
                            }
                        });
                    }
                    ui.separator();
                });
            }
            for insert in &report.inserts {
                ui.push_id(("insert", insert.draft_index), |ui| {
                    ui.strong(format!(
                        "新增草稿 {} · {}",
                        insert.draft_index + 1,
                        insert.state.label()
                    ));
                    ui.label(&insert.reason);
                    egui::Grid::new("recovery-insert-values")
                        .striped(true)
                        .show(ui, |ui| {
                            for name in &report.insert_columns {
                                ui.strong(name);
                            }
                            ui.end_row();
                            for value in &insert.values {
                                match value {
                                    InsertValue::Default => {
                                        ui.weak("DEFAULT");
                                    }
                                    InsertValue::Null => {
                                        ui.weak("NULL");
                                    }
                                    InsertValue::Text(text) => {
                                        ui.label(display(text));
                                    }
                                }
                            }
                            ui.end_row();
                        });
                });
            }
        });
    ui.data_mut(|data| data.insert_temp(selection_id, selected.clone()));
    let intended = selected
        .iter()
        .copied()
        .filter(|index| report.rows[*index].state == VerificationState::Intended)
        .collect::<Vec<_>>();
    let conflicts = selected
        .iter()
        .copied()
        .filter(|index| can_rebase(&report.rows[*index]))
        .collect::<Vec<_>>();
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        let original = !report.rows.is_empty() && report.rows.iter().all(|row| row.state == VerificationState::Original) && report.inserts.is_empty();
        if original && ui.add_enabled(!document.busy && current_report, egui::Button::new(resolution_label(document, "保留原草稿继续编辑"))).on_hover_text("需要重新建立可用管理连接；此操作不提交草稿，也不认定上次事务未提交").clicked() {
            action = Some(RecoveryAction::KeepOriginalDrafts);
        }
        if report.rows.iter().any(|row| row.state == VerificationState::Intended) && ui.add_enabled(!document.busy && current_report && !intended.is_empty(), egui::Button::new(resolution_label(document, &format!("移除选中匹配草稿 ({})", intended.len())))).on_hover_text("只移除当前值符合拟提交值的选中字段草稿，不执行数据库写入").clicked() {
            action = Some(RecoveryAction::DiscardMatchingDrafts(intended));
        }
        if report.rows.iter().any(can_rebase) && ui.add_enabled(!document.busy && current_report && !conflicts.is_empty(), egui::Button::new(resolution_label(document, &format!("以当前值重建选中冲突草稿 ({})", conflicts.len())))).on_hover_text("只更新选中记录的本地原值快照，保留拟提交值；原主键必须仍唯一且未暂存修改主键。服务器不会被写入").clicked() {
            action = Some(RecoveryAction::RebaseConflicts(conflicts));
        }
    });
    action
}

pub(super) fn report_matches(
    report: &DraftVerification,
    changes: &[CellUpdate],
    inserts: &[Vec<InsertValue>],
) -> bool {
    if report.inserts.len() != inserts.len()
        || report
            .inserts
            .iter()
            .zip(inserts)
            .any(|(saved, current)| saved.values != *current)
    {
        return false;
    }
    let saved = report
        .rows
        .iter()
        .flat_map(|row| &row.changes)
        .collect::<Vec<_>>();
    if saved.len() != changes.len() {
        return false;
    }
    let mut used = vec![false; saved.len()];
    changes.iter().all(|current| {
        if let Some(index) = saved
            .iter()
            .enumerate()
            .position(|(index, saved)| !used[index] && same_change(saved, current))
        {
            used[index] = true;
            true
        } else {
            false
        }
    })
}

fn same_change(left: &CellUpdate, right: &CellUpdate) -> bool {
    left.database == right.database
        && left.table == right.table
        && left.primary_keys == right.primary_keys
        && left.key_values == right.key_values
        && left.column == right.column
        && left.original == right.original
        && left.value == right.value
}

fn selectable(row: &RowVerification) -> bool {
    row.state == VerificationState::Intended || can_rebase(row)
}

pub(super) fn can_rebase(row: &RowVerification) -> bool {
    row.state == VerificationState::Conflict
        && row.original_key == row.intended_key
        && row
            .evidence
            .iter()
            .any(|evidence| evidence.lookup_key == row.original_key && evidence.rows.len() == 1)
}

fn key_text(row: &RowVerification) -> String {
    let original = format_key(&row.original_key);
    if row.original_key == row.intended_key {
        original
    } else {
        format!("{original} → {}", format_key(&row.intended_key))
    }
}

fn format_key(values: &[Option<String>]) -> String {
    values
        .iter()
        .map(|value| value.as_deref().map_or_else(|| "NULL".into(), display))
        .collect::<Vec<_>>()
        .join(" / ")
}

fn current_values(row: &RowVerification, column: &str) -> String {
    let mut values = Vec::new();
    for evidence in &row.evidence {
        let Some(index) = evidence.columns.iter().position(|name| name == column) else {
            continue;
        };
        if evidence.rows.is_empty() {
            values.push(format!("{}：未找到", format_key(&evidence.lookup_key)));
        }
        for server_row in &evidence.rows {
            if let Some(value) = server_row.get(index) {
                values.push(format!(
                    "{}：{}",
                    format_key(&evidence.lookup_key),
                    value.as_deref().map_or_else(|| "NULL".into(), display)
                ));
            }
        }
    }
    if values.is_empty() {
        "未读取／无法判断".into()
    } else {
        values.join("\n")
    }
}

fn cell(ui: &mut egui::Ui, value: &Option<String>) {
    match value {
        Some(value) => {
            ui.label(display(value)).on_hover_text(value);
        }
        None => {
            ui.weak("NULL");
        }
    }
}

fn display(value: &str) -> String {
    if value.is_empty() {
        return "（空字符串）".into();
    }
    if value == "NULL" {
        return "\"NULL\"（文本）".into();
    }
    if value.chars().count() > 220 {
        format!("{}…", value.chars().take(220).collect::<String>())
    } else {
        value.to_owned()
    }
}

fn resolution_label(document: &RecoveryDocument, action: &str) -> String {
    if document.manager_reset_required {
        format!("断开管理会话（未提交事务会回滚）并重连：{action}")
    } else if document.manager_reconnect_required {
        format!("重连管理连接并{action}")
    } else {
        action.to_owned()
    }
}

use super::App;
use sqlshop::{
    model::{Profile, Table, TableColumn},
    recovery_worker::{RecoveryCheck, RecoveryResponse},
    worker::Request,
    workspace::QueryTarget,
};
use uuid::Uuid;

impl App {
    pub(super) fn table_recovery_workspace(&mut self, ui: &mut egui::Ui) -> bool {
        let index = self.active_tab;
        let input_pending = self
            .cell_editor
            .as_ref()
            .is_some_and(|editor| editor.tab == self.tabs[index].id);
        if (self.tabs[index].has_table_drafts() || input_pending)
            && self.tabs[index].table_snapshot_stale
        {
            self.tabs[index]
                .recovery
                .get_or_insert_with(RecoveryDocument::default)
                .pending = true;
        }
        if !self.tabs[index].has_table_drafts()
            && !input_pending
            && self.tabs[index]
                .recovery
                .as_ref()
                .is_some_and(|doc| !doc.busy)
        {
            self.tabs[index].recovery = None;
        }
        let Some(target) = self.tabs[index].target.clone() else {
            return false;
        };
        let Some(document) = self.tabs[index].recovery.as_mut() else {
            return false;
        };
        let management = self.sessions.get(&target.connection);
        let reported_live = document
            .error
            .as_ref()
            .is_some_and(|error| error.contains("原管理连接仍在线"));
        document.manager_reset_required = reported_live
            || management.is_some_and(|session| {
                session.connected.is_some()
                    && (session.raw_session_used || session.active_database != target.database)
            });
        document.manager_reconnect_required = management
            .is_none_or(|session| session.connected.is_none())
            || document.manager_reset_required;
        ui.spacing_mut().item_spacing.y = 5.0;
        ui.spacing_mut().button_padding.y = 4.0;
        if input_pending {
            let busy = self.tabs[index]
                .recovery
                .as_ref()
                .is_some_and(|doc| doc.busy);
            let mut preserve = false;
            let mut discard = false;
            ui.add_enabled_ui(!busy, |ui| {
                if let Some(editor) = &mut self.cell_editor {
                    ui.strong(format!("{} · 尚未暂存的输入已保留", editor.change.column));
                    ui.checkbox(&mut editor.is_null, "NULL");
                    ui.add_enabled(
                        !editor.is_null,
                        egui::TextEdit::multiline(&mut editor.text)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                    ui.horizontal(|ui| {
                        preserve = ui.button("保留为本地草稿").clicked();
                        discard = ui.button("放弃这段输入").clicked();
                    });
                }
            });
            if preserve && let Some(mut editor) = self.cell_editor.take() {
                editor.change.value = if editor.is_null {
                    None
                } else {
                    Some(editor.text)
                };
                self.tabs[index].stage_cell(editor.change);
                self.status =
                    "已将保留输入加入旧快照的本地草稿；请重新核实后再处理，没有写入数据库".into();
            } else if discard {
                self.cell_editor = None;
            }
            ui.separator();
        }
        let tab = &self.tabs[index];
        let inserts = tab
            .insert_drafts
            .iter()
            .map(|draft| draft.values.clone())
            .collect::<Vec<_>>();
        let action = show_recovery(
            ui,
            tab.recovery.as_ref().expect("recovery document"),
            &tab.staged_cells,
            &inserts,
        );
        if let Some(action) = action {
            self.apply_recovery_action(index, action);
        }
        true
    }

    fn recovery_identity(&self, index: usize) -> Result<(QueryTarget, Table, Profile), String> {
        let tab = &self.tabs[index];
        let target = tab.target.clone().ok_or("草稿缺少原查询目标")?;
        let identity = tab.table_target.as_ref().ok_or("草稿缺少原表身份")?;
        if target.connection != identity.connection
            || target.database.as_deref() != Some(identity.database.as_str())
        {
            return Err("草稿目标与表身份不一致，已保留草稿".into());
        }
        let profile = tab
            .recovery_profile
            .clone()
            .ok_or("缺少原连接配置快照，不能自动核实；请人工核对并处理本地草稿")?;
        let current = self
            .profiles
            .iter()
            .find(|profile| profile.id == target.connection)
            .ok_or("原连接配置已被移除，草稿仍保留")?;
        if profile.id != target.connection || !same_profile(&profile, current) {
            return Err("原连接配置已变化，不能使用另一服务器或账号核实旧草稿；请先恢复原配置或人工核对后放弃草稿".into());
        }
        Ok((
            target,
            Table {
                schema: identity.schema.clone(),
                name: identity.name.clone(),
                kind: tab
                    .table_kind
                    .clone()
                    .unwrap_or_else(|| "BASE TABLE".into()),
            },
            profile,
        ))
    }

    fn check_table_recovery(&mut self, index: usize) {
        if self.screenshot_path.is_some()
            || self.tabs[index]
                .recovery
                .as_ref()
                .is_some_and(|doc| doc.busy)
        {
            return;
        }
        let result = (|| {
            if self
                .cell_editor
                .as_ref()
                .is_some_and(|editor| editor.tab == self.tabs[index].id)
            {
                return Err("请先将保留输入加入本地草稿，或明确放弃该输入，再进行核实".into());
            }
            let (target, table, profile) = self.recovery_identity(index)?;
            if self.session_busy(target.connection) {
                return Err("原管理连接正在操作，请等待结束后核实".into());
            }
            let credentials = self
                .credentials_for(&profile)
                .map_err(|error| error.to_string())?
                .ok_or("缺少原连接凭据，请在连接设置中补充后重新核实")?;
            let tab = &self.tabs[index];
            let data = tab.table_data.as_ref().ok_or("缺少草稿的原表结构快照")?;
            let epoch = tab.recovery_epoch.wrapping_add(1);
            let manager_epoch = self
                .sessions
                .get(&target.connection)
                .map(|session| session.epoch);
            let request = RecoveryCheck {
                tab: tab.id,
                epoch,
                profile,
                credentials,
                target,
                table,
                browse: tab.browse.clone(),
                changes: tab.staged_cells.clone(),
                insert_columns: data
                    .columns
                    .iter()
                    .map(|column| column.name.clone())
                    .collect(),
                inserts: tab
                    .insert_drafts
                    .iter()
                    .map(|draft| draft.values.clone())
                    .collect(),
                expected_columns: data.columns.clone(),
            };
            self.recovery_worker
                .check(request)
                .map_err(|error| error.to_string())?;
            let tab = &mut self.tabs[index];
            tab.recovery_epoch = epoch;
            tab.recovery_manager_epoch = manager_epoch;
            tab.recovery_reconnect = None;
            let document = tab.recovery.get_or_insert_with(RecoveryDocument::default);
            document.busy = true;
            document.error = None;
            Ok::<_, String>(())
        })();
        if let Err(message) = result {
            let id = self.tabs[index].id;
            self.table_recovery_failed(id, &message);
            self.fail(message);
        } else {
            self.status = "正在通过新连接按原主键核实，原草稿和失效会话均未重放…".into();
            self.error = false;
        }
    }

    pub(super) fn poll_table_recovery(&mut self) {
        while let Ok(response) = self.recovery_worker.receiver.try_recv() {
            let (id, epoch, target, profile) = match &response {
                RecoveryResponse::Checked {
                    tab,
                    epoch,
                    target,
                    profile_snapshot,
                    ..
                }
                | RecoveryResponse::Failed {
                    tab,
                    epoch,
                    target,
                    profile_snapshot,
                    ..
                } => (*tab, *epoch, target, profile_snapshot),
            };
            let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
                continue;
            };
            if self.tabs[index].recovery_epoch != epoch {
                continue;
            }
            let valid = self
                .recovery_identity(index)
                .is_ok_and(|(bound, _, source)| bound == *target && same_profile(&source, profile))
                && self.tabs[index].recovery_manager_epoch
                    == self
                        .sessions
                        .get(&target.connection)
                        .map(|session| session.epoch);
            if !valid {
                self.table_recovery_failed(
                    id,
                    "核实期间连接配置或管理上下文已变化，此结果未接入；请重新核实",
                );
                continue;
            }
            match response {
                RecoveryResponse::Checked {
                    mut report, page, ..
                } => {
                    let tab = &mut self.tabs[index];
                    let inserts = tab
                        .insert_drafts
                        .iter()
                        .map(|draft| draft.values.clone())
                        .collect::<Vec<_>>();
                    if !report_matches(&report, &tab.staged_cells, &inserts) {
                        self.table_recovery_failed(
                            id,
                            "核实期间本地草稿已变化，此结果未接入；请重新核实",
                        );
                        continue;
                    }
                    if tab
                        .table_data
                        .as_ref()
                        .is_none_or(|data| !same_columns(&data.columns, &page.data.columns))
                    {
                        for row in &mut report.rows {
                            row.state = VerificationState::Unknown;
                            row.reason =
                                "核实后读取当前页时表结构再次变化，不能继续解释旧草稿；请人工核对"
                                    .into();
                        }
                    }
                    let document = tab.recovery.get_or_insert_with(RecoveryDocument::default);
                    document.busy = false;
                    document.report = Some(report);
                    document.fresh_page = Some(page);
                    document.error = None;
                    self.status = "当前值核实完成；请选择本地草稿处理方式，未执行写入".into();
                    self.error = false;
                }
                RecoveryResponse::Failed { message, .. } => {
                    self.table_recovery_failed(id, &message);
                    self.fail(message);
                }
            }
        }
    }

    pub(super) fn table_recovery_failed(&mut self, id: Uuid, message: &str) {
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
            tab.recovery_reconnect = None;
            tab.recovery_disconnect_pending = false;
            let document = tab.recovery.get_or_insert_with(RecoveryDocument::default);
            document.busy = false;
            document.error = Some(message.to_owned());
        }
    }
}

fn same_profile(left: &Profile, right: &Profile) -> bool {
    matches!((serde_json::to_value(left), serde_json::to_value(right)), (Ok(left),Ok(right)) if left == right)
}

fn same_columns(left: &[TableColumn], right: &[TableColumn]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.name == right.name
                && left.declared_type == right.declared_type
                && left.sql_type == right.sql_type
                && left.kind == right.kind
                && left.primary_key == right.primary_key
                && left.editable == right.editable
        })
}
