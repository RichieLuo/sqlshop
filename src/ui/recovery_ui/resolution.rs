use super::*;

impl App {
    pub(super) fn apply_recovery_action(&mut self, index: usize, action: RecoveryAction) {
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

    pub(super) fn validate_recovery_resolution(
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

    pub(super) fn resolve_table_recovery(
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

    pub(in crate::ui) fn recovery_reconnect_request(
        &mut self,
        id: Uuid,
    ) -> Result<Request, String> {
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

    pub(in crate::ui) fn finish_recovery_reconnect(
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

    pub(super) fn finish_local_recovery(
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
