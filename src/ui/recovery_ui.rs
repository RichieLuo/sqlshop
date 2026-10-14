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
            ui.label("正在使用新连接读取当前值…");
        } else if document.pending {
            ui.label("草稿已保留，等待处理");
        }
    });
    ui.label("提交未获确认时，请先核实记录。核实只读取数据；下面的处理按钮只修改本地草稿。");
    if let Some(error) = &document.error {
        let color = ui.visuals().error_fg_color;
        ui.label(RichText::new(error).color(color));
    }
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(!document.busy, egui::Button::new("重新核实"))
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
        ui.label(
            "尚无核实结果，请点击“重新核实”。新增记录没有可靠回执时，无法自动判断是否已提交。",
        );
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
        ui.weak(format!(
            "已读取当前页 {count} 条记录；原草稿和主键快照尚未替换。"
        ));
    }
    if !current_report {
        ui.label(
            RichText::new("本地草稿已变化，此核实结果仅供查看。请重新核实后再处理。")
                .color(ui.visuals().warn_fg_color),
        );
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

    fn apply_recovery_action(&mut self, index: usize, action: RecoveryAction) {
        if self.tabs[index]
            .recovery
            .as_ref()
            .is_some_and(|doc| doc.busy)
        {
            return;
        }
        match action {
            RecoveryAction::CheckAgain => self.check_table_recovery(index),
            RecoveryAction::DiscardAll => {
                if self
                    .cell_editor
                    .as_ref()
                    .is_some_and(|editor| editor.tab == self.tabs[index].id)
                {
                    self.cell_editor = None;
                }
                let tab = &mut self.tabs[index];
                tab.clear_table_drafts();
                tab.recovery_epoch = tab.recovery_epoch.wrapping_add(1);
                tab.recovery = None;
                tab.recovery_reconnect = None;
                tab.table_snapshot_stale = true;
                self.status =
                    "已放弃本地草稿；没有回滚、撤销或重试服务器写入。请重新连接并刷新实际数据。"
                        .into();
                self.error = false;
            }
            action => {
                let result = self.resolve_table_recovery(index, action);
                if let Err(message) = result {
                    self.table_recovery_failed(self.tabs[index].id, &message);
                    self.fail(message);
                }
            }
        }
    }

    fn validate_recovery_resolution(
        &self,
        index: usize,
        action: &RecoveryAction,
    ) -> Result<(), String> {
        if self
            .cell_editor
            .as_ref()
            .is_some_and(|editor| editor.tab == self.tabs[index].id)
        {
            return Err("仍有尚未暂存的输入，请先保留为草稿或明确放弃".into());
        }
        let (target, table, _) = self.recovery_identity(index)?;
        let tab = &self.tabs[index];
        if tab.recovery_manager_epoch
            != self
                .sessions
                .get(&target.connection)
                .map(|session| session.epoch)
        {
            return Err("管理上下文已变化，请重新核实；草稿保留".into());
        }
        let document = tab.recovery.as_ref().ok_or("缺少核实文档")?;
        let report = document.report.as_ref().ok_or("请先核实当前值")?;
        let page = document.fresh_page.as_ref().ok_or("缺少核实后的当前页")?;
        let data = tab.table_data.as_ref().ok_or("缺少原表结构快照")?;
        let inserts = tab
            .insert_drafts
            .iter()
            .map(|draft| draft.values.clone())
            .collect::<Vec<_>>();
        if report.database != target.database.as_deref().unwrap_or_default()
            || report.table != table
            || !report_matches(report, &tab.staged_cells, &inserts)
            || !same_columns(&data.columns, &page.data.columns)
        {
            return Err("草稿、表身份或结构与核实快照不一致，请重新核实".into());
        }
        match action {
            RecoveryAction::KeepOriginalDrafts
                if !report.rows.is_empty()
                    && report.inserts.is_empty()
                    && report
                        .rows
                        .iter()
                        .all(|row| row.state == VerificationState::Original) =>
            {
                Ok(())
            }
            RecoveryAction::DiscardMatchingDrafts(selected)
                if !selected.is_empty()
                    && selected.iter().enumerate().all(|(position, index)| {
                        !selected[..position].contains(index)
                            && report
                                .rows
                                .get(*index)
                                .is_some_and(|row| row.state == VerificationState::Intended)
                    }) =>
            {
                Ok(())
            }
            RecoveryAction::RebaseConflicts(selected)
                if !selected.is_empty()
                    && selected.iter().enumerate().all(|(position, index)| {
                        !selected[..position].contains(index)
                            && report.rows.get(*index).is_some_and(can_rebase)
                    }) =>
            {
                Ok(())
            }
            _ => Err("所选记录不满足此处理方式，草稿未改变".into()),
        }
    }

    fn resolve_table_recovery(
        &mut self,
        index: usize,
        action: RecoveryAction,
    ) -> Result<(), String> {
        self.validate_recovery_resolution(index, &action)?;
        let (target, _, _) = self.recovery_identity(index)?;
        if self.session_busy(target.connection) {
            return Err("管理连接正在操作，请等待后重新核实".into());
        }
        let ready = self
            .sessions
            .get(&target.connection)
            .is_some_and(|session| {
                session.connected.is_some()
                    && session.active_database == target.database
                    && !session.raw_session_used
            });
        if ready {
            return self.finish_local_recovery(index, action);
        }
        self.tabs[index].recovery_reconnect = Some(action);
        self.tabs[index]
            .recovery
            .as_mut()
            .expect("recovery document")
            .busy = true;
        let live = self
            .sessions
            .get(&target.connection)
            .is_some_and(|session| session.connected.is_some())
            || self.tabs[index].recovery.as_ref().is_some_and(|doc| {
                doc.error
                    .as_ref()
                    .is_some_and(|error| error.contains("原管理连接仍在线"))
            });
        if live {
            // The clicked button explicitly says the old management transaction
            // may roll back. Independent query actors remain alive.
            let session = self
                .sessions
                .get_mut(&target.connection)
                .ok_or("缺少原管理工作线程，请重新核实；草稿仍保留")?;
            session
                .worker
                .send(Request::Disconnect)
                .map_err(|error| error.to_string())?;
            session.busy = true;
            session.epoch = session.epoch.wrapping_add(1);
            session.running_tab = Some(self.tabs[index].id);
            self.tabs[index].recovery_disconnect_pending = true;
            self.tabs[index].recovery_manager_epoch = Some(session.epoch);
            self.status = "按明确选择断开旧管理会话，然后重连原数据库；原草稿仍保留".into();
        } else {
            let request = self.recovery_reconnect_request(self.tabs[index].id)?;
            self.request_on(
                target.connection,
                request,
                "正在重建原数据库的表管理连接；草稿保留，不重放 SQL…",
            );
            if !self.session_busy(target.connection) {
                return Err("恢复连接请求未发出，草稿仍保留".into());
            }
        }
        Ok(())
    }

    pub(super) fn recovery_reconnect_request(&mut self, id: Uuid) -> Result<Request, String> {
        let index = self
            .tabs
            .iter()
            .position(|tab| tab.id == id)
            .ok_or("原表标签已关闭")?;
        let (target, _, profile) = self.recovery_identity(index)?;
        let credentials = self
            .credentials_for(&profile)
            .map_err(|error| error.to_string())?
            .ok_or("缺少原连接凭据，请补充后重新核实")?;
        Ok(Request::ReconnectTableManagement {
            tab: id,
            epoch: self.tabs[index].recovery_epoch,
            target,
            profile,
            credentials,
        })
    }

    pub(super) fn finish_recovery_reconnect(
        &mut self,
        id: Uuid,
        epoch: u64,
        target: &QueryTarget,
        profile: &Profile,
    ) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        if self.tabs[index].recovery_epoch != epoch {
            return;
        }
        if !self
            .recovery_identity(index)
            .is_ok_and(|(bound, _, source)| bound == *target && same_profile(&source, profile))
        {
            self.table_recovery_failed(id, "恢复期间目标或配置变化；已保留草稿，不接入当前页");
            return;
        }
        if let Some(document) = &mut self.tabs[index].recovery {
            document.busy = false;
        } else {
            return;
        }
        let Some(action) = self.tabs[index].recovery_reconnect.take() else {
            return;
        };
        if let Err(message) = self
            .validate_recovery_resolution(index, &action)
            .and_then(|_| self.finish_local_recovery(index, action))
        {
            self.table_recovery_failed(id, &message);
            self.fail(message);
        }
    }

    fn finish_local_recovery(
        &mut self,
        index: usize,
        action: RecoveryAction,
    ) -> Result<(), String> {
        self.validate_recovery_resolution(index, &action)?;
        let tab = &mut self.tabs[index];
        let report = tab
            .recovery
            .as_ref()
            .and_then(|doc| doc.report.as_ref())
            .ok_or("缺少核实报告")?
            .clone();
        let rebased_indices = match &action {
            RecoveryAction::RebaseConflicts(indices) => indices.clone(),
            _ => Vec::new(),
        };
        match action {
            RecoveryAction::KeepOriginalDrafts => {}
            RecoveryAction::DiscardMatchingDrafts(selected) => {
                let remove = selected
                    .iter()
                    .flat_map(|index| &report.rows[*index].changes)
                    .collect::<Vec<_>>();
                tab.staged_cells
                    .retain(|change| !remove.iter().any(|saved| same_change(change, saved)));
            }
            RecoveryAction::RebaseConflicts(selected) => {
                let mut rebased = Vec::new();
                for index in selected {
                    let row = &report.rows[index];
                    let evidence = row
                        .evidence
                        .iter()
                        .find(|evidence| {
                            evidence.lookup_key == row.original_key && evidence.rows.len() == 1
                        })
                        .ok_or("原主键已不能唯一定位")?;
                    for saved in &row.changes {
                        let column = evidence
                            .columns
                            .iter()
                            .position(|column| column == &saved.column)
                            .ok_or("核实证据缺少待改字段")?;
                        let original = evidence.rows[0]
                            .get(column)
                            .ok_or("核实证据缺少当前值")?
                            .clone();
                        rebased.push((saved.clone(), original));
                    }
                }
                for (saved, original) in rebased {
                    let change = tab
                        .staged_cells
                        .iter_mut()
                        .find(|change| same_change(change, &saved))
                        .ok_or("原草稿已变化")?;
                    change.original = original;
                }
                tab.staged_cells
                    .retain(|change| change.original != change.value);
            }
            _ => return Err("无效的草稿解决方式".into()),
        }
        // Unknown/conflicting or unacknowledged INSERT drafts remain guarded.
        let remaining_safe = tab.insert_drafts.is_empty()
            && tab.staged_cells.iter().all(|change| {
                report.rows.iter().any(|row| {
                    row.original_key == change.key_values
                        && row
                            .changes
                            .iter()
                            .any(|saved| saved.column == change.column)
                        && (row.state == VerificationState::Original
                            || (rebased_indices
                                .iter()
                                .any(|index| std::ptr::eq(&report.rows[*index], row))
                                && can_rebase(row)))
                })
            });
        if remaining_safe {
            let mut page = tab
                .recovery
                .as_mut()
                .and_then(|doc| doc.fresh_page.take())
                .ok_or("缺少新页面快照")?;
            tab.last_executed_sql = Some(page.execution_sql());
            tab.output = Some(std::mem::take(&mut page.data.output));
            tab.table_data = Some(page.data);
            tab.has_more = page.has_more;
            tab.result_index = 0;
            tab.selected_rows = None;
            tab.selected_cell = None;
            tab.cell_anchor = None;
            tab.table_snapshot_stale = false;
            tab.table_read_pending = false;
            tab.table_write_pending = false;
            tab.recovery = None;
            tab.recovery_reconnect = None;
            self.status =
                "已按选择处理本地草稿并接入当前页；没有提交或重放写入，继续编辑后需手动提交".into();
        } else {
            tab.table_snapshot_stale = true;
            let document = tab.recovery.as_mut().expect("recovery document");
            document.busy = false;
            document.error = Some("已处理所选本地字段草稿，其余未知、新增或冲突草稿仍保留；请再次核实或人工处理后放弃。".into());
            // A changed local draft invalidates the old report's action proof.
            self.status = "所选草稿已处理；其余草稿仍受保护，未写入数据库".into();
        }
        self.error = false;
        Ok(())
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
