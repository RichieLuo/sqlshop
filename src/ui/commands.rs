use super::*;

impl App {
    pub(super) fn fail(&mut self, message: String) {
        self.status = message;
        self.error = true;
    }

    pub(super) fn selected_session(&self) -> Option<&ConnectionSession> {
        self.selected.and_then(|id| self.sessions.get(&id))
    }

    pub(super) fn session_busy(&self, id: Uuid) -> bool {
        self.sessions.get(&id).is_some_and(|s| s.busy)
    }

    pub(super) fn target_ready(&self, target: &QueryTarget) -> bool {
        self.sessions
            .get(&target.connection)
            .is_some_and(|s| s.connected.is_some() && s.active_database == target.database)
    }

    pub(super) fn execution_target(&self) -> Option<QueryTarget> {
        self.tabs[self.active_tab]
            .target
            .clone()
            .or_else(|| self.current_target())
    }

    pub(super) fn can_execute(&self) -> bool {
        !self.tabs[self.active_tab].management_document()
            && self.tabs[self.active_tab].definition.is_none()
            && self.execution_target().is_some_and(|t| {
                self.query_target_ready(self.tabs[self.active_tab].id, &t)
                    && !self.tab_busy(self.active_tab)
            })
    }

    pub(super) fn completion_columns(&self) -> Vec<String> {
        self.tabs[self.active_tab]
            .target
            .as_ref()
            .and_then(|target| {
                self.sessions
                    .get(&target.connection)
                    .map(|session| session.columns_for(target))
            })
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(super) fn completion_tables(&self) -> Vec<String> {
        self.tabs[self.active_tab]
            .target
            .as_ref()
            .and_then(|target| {
                self.sessions
                    .get(&target.connection)
                    .map(|session| session.tables_for(target))
            })
            .unwrap_or_default()
    }

    pub(super) fn semantic_completion_tables(&self) -> Vec<sqlshop::sql_editor::CompletionTable> {
        let Some(target) = self.tabs[self.active_tab].target.as_ref() else {
            return Vec::new();
        };
        let Some(session) = self
            .sessions
            .get(&target.connection)
            .filter(|session| session.target_matches(target))
        else {
            return Vec::new();
        };
        session
            .tables
            .iter()
            .map(|table| sqlshop::sql_editor::CompletionTable {
                schema: table.schema.clone(),
                name: table.name.clone(),
                columns: session
                    .column_cache
                    .get(&(table.schema.clone(), table.name.clone()))
                    .cloned()
                    .unwrap_or_default(),
            })
            .collect()
    }

    pub(super) fn prefetch_completion_columns(&mut self) {
        if self.screenshot_path.is_some() {
            return;
        }
        let Some(target) = self.tabs[self.active_tab].target.clone() else {
            return;
        };
        let Some(session) = self.sessions.get_mut(&target.connection) else {
            return;
        };
        if session.busy
            || session.connected.as_ref().map(|p| p.id) != Some(target.connection)
            || session.active_database != target.database
            || target.database.is_none()
            || session.completion_pending.is_some()
            || session.completion_pause
        {
            return;
        }
        let Some(table) = session
            .tables
            .iter()
            .find(|table| {
                !session
                    .column_cache
                    .contains_key(&(table.schema.clone(), table.name.clone()))
            })
            .cloned()
        else {
            return;
        };
        let generation = session.completion_generation;
        if session
            .worker
            .send(Request::CompleteColumns {
                target: target.clone(),
                table: table.clone(),
                generation,
            })
            .is_ok()
        {
            session.completion_pending = Some((target, table, generation));
        }
    }

    pub(super) fn tab_busy(&self, index: usize) -> bool {
        let tab = &self.tabs[index];
        if tab.oracle.as_ref().is_some_and(|doc| doc.busy)
            || tab.redis.as_ref().is_some_and(|doc| doc.busy)
        {
            return true;
        }
        if tab.advanced.as_ref().is_some_and(|doc| doc.busy)
            || tab
                .compare
                .as_ref()
                .is_some_and(|doc| doc.pending.is_some())
            || tab.backup.as_ref().is_some_and(|doc| doc.pending.is_some())
            || tab
                .local_database
                .as_ref()
                .is_some_and(|doc| doc.pending.is_some())
        {
            return true;
        }
        if self.tabs[index]
            .recovery
            .as_ref()
            .is_some_and(|doc| doc.busy)
        {
            return true;
        }
        if let Some(document) = &self.tabs[index].definition {
            return self
                .sessions
                .get(&document.connection)
                .is_some_and(|session| {
                    session.busy && session.running_tab == Some(self.tabs[index].id)
                });
        }
        if self.tabs[index].table_target.is_none() {
            return self
                .query_sessions
                .get(&self.tabs[index].id)
                .is_some_and(|s| s.busy);
        }
        self.tabs[index]
            .target
            .as_ref()
            .is_some_and(|t| self.session_busy(t.connection))
    }

    pub(super) fn any_busy(&self) -> bool {
        if self.object_design_busy() {
            return true;
        }
        self.test_busy
            || self.tabs.iter().any(|tab| {
                tab.oracle.as_ref().is_some_and(|doc| doc.busy)
                    || tab.redis.as_ref().is_some_and(|doc| doc.busy)
                    || tab.advanced.as_ref().is_some_and(|doc| doc.busy)
                    || tab
                        .compare
                        .as_ref()
                        .is_some_and(|doc| doc.pending.is_some())
                    || tab.backup.as_ref().is_some_and(|doc| doc.pending.is_some())
                    || tab
                        .local_database
                        .as_ref()
                        .is_some_and(|doc| doc.pending.is_some())
            })
            || self.job_scheduler.as_ref().is_some_and(|scheduler| {
                scheduler
                    .store
                    .records()
                    .iter()
                    .any(|record| record.state == sqlshop::jobs::JobState::Running)
            })
            || self
                .tabs
                .iter()
                .any(|tab| tab.recovery.as_ref().is_some_and(|doc| doc.busy))
            || self.sessions.values().any(|s| s.busy)
            || self.query_sessions.values().any(|s| s.busy)
    }

    pub(super) fn request(&mut self, request: Request, status: &str) {
        if self.screenshot_path.is_some() {
            return;
        }
        if matches!(&request, Request::Test(..)) {
            if self.test_busy {
                return;
            }
            match self.test_worker.send(request) {
                Ok(()) => {
                    self.test_busy = true;
                    self.status = status.into();
                    self.error = false;
                }
                Err(error) => self.fail(error.to_string()),
            }
            return;
        }
        let connection = match &request {
            Request::Connect(profile, _) => Some(profile.id),
            Request::ObjectDefinition { target, .. }
            | Request::ReconnectTableManagement { target, .. }
            | Request::RunQuery { target, .. }
            | Request::ReadPage { target, .. }
            | Request::ReadLastPage { target, .. }
            | Request::DescribeTable { target, .. }
            | Request::ChangeStructure { target, .. }
            | Request::SaveCell { target, .. }
            | Request::DeleteRows { target, .. }
            | Request::InsertDefaultRow { target, .. }
            | Request::InsertRows { target, .. }
            | Request::SaveTableDraft { target, .. }
            | Request::SaveCells { target, .. }
            | Request::ExportTable { target, .. } => Some(target.connection),
            _ => self.selected,
        };
        if let Some(id) = connection {
            self.request_on(id, request, status);
        }
    }

    pub(super) fn request_on(&mut self, id: Uuid, request: Request, status: &str) {
        self.request_on_impl(id, request, status, false);
    }

    pub(super) fn request_on_impl(
        &mut self,
        id: Uuid,
        request: Request,
        status: &str,
        confirmed: bool,
    ) {
        if self.screenshot_path.is_some() {
            return;
        }
        if self.screenshot_path.is_some() {
            return;
        }
        if self.session_busy(id) {
            return;
        }
        if matches!(&request, Request::ChangeStructure { .. })
            && self.tabs.iter().any(|tab| {
                tab.has_table_drafts()
                    && tab
                        .target
                        .as_ref()
                        .is_some_and(|target| target.connection == id)
            })
        {
            self.fail("此连接有暂存修改或新增行；请先提交或撤销草稿，再修改表结构".into());
            return;
        }
        let table_write_tab = match &request {
            Request::SaveCell { tab, .. }
            | Request::SaveCells { tab, .. }
            | Request::DeleteRows { tab, .. }
            | Request::InsertDefaultRow { tab, .. }
            | Request::InsertRows { tab, .. } => Some(*tab),
            Request::SaveTableDraft { tab, .. } => Some(*tab),
            _ => None,
        };
        if table_write_tab.is_some_and(|id| {
            self.tabs
                .iter()
                .find(|tab| tab.id == id)
                .is_none_or(|tab| tab.table_snapshot_stale || self.cell_editor.is_some())
        }) {
            self.fail("表数据已过期或单元格编辑窗尚未关闭；请先刷新或结束编辑再写入".into());
            return;
        }
        if self.tabs.iter().any(|tab| {
            tab.has_table_drafts() && tab.target.as_ref().is_some_and(|t| t.connection == id)
        }) && matches!(
            &request,
            Request::Connect(..) | Request::Disconnect | Request::SelectDatabase(..)
        ) {
            self.fail("此连接有暂存修改；请先提交或丢弃后再切库/断开".into());
            return;
        }
        if self
            .cell_editor
            .as_ref()
            .is_some_and(|e| e.connection == id)
            && matches!(
                &request,
                Request::Connect(..) | Request::Disconnect | Request::SelectDatabase(..)
            )
        {
            self.fail("请先保存或取消此连接的单元格修改".into());
            return;
        }
        if matches!(&request, Request::Connect(..) | Request::Disconnect) {
            if self.query_connection_busy(id) {
                self.fail("此连接的查询正在执行，请等待执行结束后再重连或断开".into());
                return;
            }
            if !confirmed && self.query_connection_used(id) {
                self.confirm_request = Some((id, request, status.into()));
                return;
            }
            self.close_query_sessions(id);
        }
        let session = self
            .sessions
            .entry(id)
            .or_insert_with(|| ConnectionSession::new(&self.context));
        if !confirmed
            && session.raw_session_used
            && matches!(
                &request,
                Request::Connect(..) | Request::Disconnect | Request::SelectDatabase(..)
            )
        {
            self.confirm_request = Some((id, request, status.into()));
            return;
        }
        if matches!(
            &request,
            Request::Connect(..) | Request::Disconnect | Request::SelectDatabase(..)
        ) {
            session.clear_columns();
            session.object_catalog = None;
            session.catalog_loading = false;
            session.catalog_error = None;
        }
        let tab = match &request {
            Request::ObjectDefinition { tab, .. }
            | Request::ReconnectTableManagement { tab, .. }
            | Request::RunQuery { tab, .. }
            | Request::ReadPage { tab, .. }
            | Request::ReadLastPage { tab, .. }
            | Request::ReadTable { tab, .. }
            | Request::DescribeTable { tab, .. }
            | Request::ChangeStructure { tab, .. }
            | Request::SaveCell { tab, .. }
            | Request::DeleteRows { tab, .. }
            | Request::InsertDefaultRow { tab, .. }
            | Request::InsertRows { tab, .. }
            | Request::SaveCells { tab, .. }
            | Request::SaveTableDraft { tab, .. }
            | Request::ExportTable { tab, .. } => Some(*tab),
            _ => None,
        };
        if matches!(&request, Request::ObjectCatalog { .. }) {
            session.catalog_loading = true;
            session.catalog_error = None;
        }
        let raw = matches!(&request, Request::RunQuery { .. } | Request::Query(..));
        let recovery_reconnect_request =
            matches!(&request, Request::ReconnectTableManagement { .. });
        let table_read = matches!(
            &request,
            Request::ReadPage { .. } | Request::ReadLastPage { .. } | Request::ReadTable { .. }
        );
        let connection_reset = matches!(&request, Request::Connect(..) | Request::Disconnect);
        match session.worker.send(request) {
            Ok(()) => {
                if connection_reset {
                    session.connected = None;
                    session.active_database = None;
                    for tab in &mut self.tabs {
                        if tab
                            .table_target
                            .as_ref()
                            .is_some_and(|target| target.connection == id)
                        {
                            tab.table_snapshot_stale = true;
                        }
                    }
                }
                if table_read && let Some(tab) = self.tabs.iter_mut().find(|t| Some(t.id) == tab) {
                    tab.table_read_pending = true;
                    tab.table_snapshot_stale = true;
                }
                session.busy = true;
                if let Some(source) =
                    table_write_tab.and_then(|id| self.tabs.iter_mut().find(|tab| tab.id == id))
                {
                    source.table_write_pending = true;
                    if source.recovery_profile.is_none() {
                        source.recovery_profile = session.connected.clone();
                    }
                }
                session.epoch = session.epoch.wrapping_add(1);
                if recovery_reconnect_request
                    && let Some(source) =
                        tab.and_then(|id| self.tabs.iter_mut().find(|tab| tab.id == id))
                {
                    source.recovery_manager_epoch = Some(session.epoch);
                }
                session.running_tab = tab;
                session.raw_session_used |= raw;
                session.error = false;
                session.status = status.into();
                self.error = false;
                self.status = status.into();
            }
            Err(error) => {
                session.catalog_loading = false;
                if let Some(source) = tab
                    .and_then(|id| self.tabs.iter_mut().find(|tab| tab.id == id))
                    .and_then(|tab| tab.definition.as_mut())
                {
                    source.error = Some(error.to_string());
                }
                self.fail(error.to_string());
            }
        }
    }
}
