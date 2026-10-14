use super::*;

#[derive(Clone)]
struct FilterDraft {
    enabled: bool,
    column: String,
    operator: FilterOperator,
    value: String,
}

impl FilterDraft {
    fn new(column: String) -> Self {
        Self {
            enabled: true,
            column,
            operator: FilterOperator::Eq,
            value: String::new(),
        }
    }

    fn needs_value(&self) -> bool {
        !matches!(
            self.operator,
            FilterOperator::IsNull | FilterOperator::IsNotNull
        )
    }

    fn applied(&self) -> ColumnFilter {
        ColumnFilter {
            column: self.column.clone(),
            operator: self.operator,
            value: self.needs_value().then(|| self.value.clone()),
        }
    }
}

/// Editable conditions are local to the tab; paging and refresh use only `browse`.
#[derive(Clone, Default)]
pub(super) struct TableBrowseDraft {
    filters: Vec<FilterDraft>,
    sorts: Vec<ColumnSort>,
}

impl TableBrowseDraft {
    pub(super) fn add_filter(
        &mut self,
        column: String,
        operator: FilterOperator,
        value: Option<String>,
    ) -> bool {
        if self.filters.len() < 100 {
            self.filters.push(FilterDraft {
                enabled: true,
                column,
                operator,
                value: value.unwrap_or_default(),
            });
            true
        } else {
            false
        }
    }

    pub(super) fn sync_sorts(&mut self, sorts: &[ColumnSort]) {
        self.sorts = sorts.to_vec();
    }

    fn matches(&self, browse: &TableBrowseOptions) -> bool {
        self.sorts == browse.sorts
            && self
                .filters
                .iter()
                .filter(|row| row.enabled)
                .map(FilterDraft::applied)
                .collect::<Vec<_>>()
                == browse.filters
    }

    fn validation_error(&self, columns: &[String]) -> Option<&'static str> {
        let filters: Vec<_> = self.filters.iter().filter(|row| row.enabled).collect();
        if filters.len() > 100 {
            return Some("最多支持 100 个启用的筛选条件");
        }
        if filters.iter().any(|row| !columns.contains(&row.column)) {
            return Some("请为每个启用的筛选条件选择有效列");
        }
        if self.sorts.iter().any(|row| !columns.contains(&row.column)) {
            return Some("请为每个排序条件选择有效列");
        }
        for (index, row) in self.sorts.iter().enumerate() {
            if self.sorts[..index]
                .iter()
                .any(|previous| previous.column == row.column)
            {
                return Some("排序列不能重复");
            }
        }
        None
    }

    fn apply_to(&self, browse: &mut TableBrowseOptions) {
        browse.filters = self
            .filters
            .iter()
            .filter(|row| row.enabled)
            .map(FilterDraft::applied)
            .collect();
        browse.sorts = self.sorts.clone();
        browse.offset = 0;
    }
}

impl QueryTab {
    fn apply_browse_draft(&mut self) {
        self.browse_draft.apply_to(&mut self.browse);
    }

    fn reset_browse(&mut self) {
        self.browse_draft = TableBrowseDraft::default();
        self.apply_browse_draft();
    }

    pub(super) fn bind_for_execution(&mut self, current: &QueryTarget) -> bool {
        if self.target.as_ref().is_some_and(|target| target != current) {
            return false;
        }
        self.target = Some(current.clone());
        true
    }

    pub(super) fn dirty(&self) -> bool {
        self.sql != self.saved_sql
            || self.local_database.as_ref().is_some_and(|doc| doc.dirty())
            || self
                .oracle
                .as_ref()
                .is_some_and(|doc| doc.is_dirty() || doc.may_have_transaction())
            || self.jobs.as_ref().is_some_and(|doc| doc.dirty())
            || self
                .advanced
                .as_ref()
                .is_some_and(|doc| doc.preview.is_some() || doc.account_preview.is_some())
    }
}

#[derive(Clone, Copy)]
struct TableActionState {
    idle: bool,
    ready: bool,
    stale: bool,
    has_drafts: bool,
}

impl TableActionState {
    fn can_read(self) -> bool {
        self.idle && self.ready && !self.has_drafts
    }

    fn can_write(self) -> bool {
        self.can_read() && !self.stale
    }

    fn can_submit(self) -> bool {
        self.idle && self.ready && !self.stale && self.has_drafts
    }

    fn can_revert(self) -> bool {
        self.idle && self.has_drafts
    }
}

impl App {
    pub(super) fn can_add_table_row(&self, index: usize) -> bool {
        let state = self.table_action_state(index);
        let tab = &self.tabs[index];
        state.idle
            && state.ready
            && !state.stale
            && tab.table_target.is_some()
            && tab.table_data.as_ref().is_some_and(|data| data.writable)
            && tab.insert_drafts.len() < 1000
    }

    pub(super) fn can_browse_grid(&self, index: usize) -> bool {
        let state = self.table_action_state(index);
        state.can_read() && !state.stale
    }

    fn table_action_state(&self, index: usize) -> TableActionState {
        let tab = &self.tabs[index];
        TableActionState {
            idle: !self.tab_busy(index) && self.cell_editor.is_none(),
            ready: tab
                .target
                .as_ref()
                .is_some_and(|target| self.target_ready(target)),
            stale: tab.table_snapshot_stale,
            has_drafts: tab.has_table_drafts(),
        }
    }

    pub(super) fn current_target(&self) -> Option<QueryTarget> {
        self.selected_session().and_then(|s| {
            s.connected.as_ref().map(|p| QueryTarget {
                connection: p.id,
                database: s.active_database.clone(),
                schema: None,
            })
        })
    }

    pub(super) fn restore_workspace(&mut self) {
        let Some(store) = &self.store else { return };
        let store = WorkspaceStore::new(store.directory.clone());
        match store.load() {
            Ok(workspace) => {
                self.tabs = workspace
                    .tabs
                    .into_iter()
                    .map(|saved| {
                        let mut tab = QueryTab::new(1);
                        tab.id = saved.id;
                        tab.name = saved.name;
                        tab.sql = saved.sql;
                        tab.saved_sql = saved.saved_sql;
                        tab.file_path = saved.file_path;
                        tab.target = saved.target;
                        tab
                    })
                    .collect();
                if self.tabs.is_empty() {
                    self.tabs.push(QueryTab::new(1));
                }
                self.active_tab = workspace.active_tab.min(self.tabs.len() - 1);
                self.next_tab = workspace.next_tab.max(self.tabs.len() + 1);
                self.editor_height = workspace.editor_height;
                self.history = workspace.history;
                if let Some(layout) = workspace.layout {
                    self.view.navigation = layout.navigation;
                    self.inspector_visible = layout.information;
                    self.view.information_force = layout.information_force;
                    self.view.toolbar = layout.toolbar;
                    self.view.status = layout.status;
                    self.view.focus = layout.focus;
                    self.view.connected_only = layout.connected_only;
                    self.theme = match layout.theme.as_str() {
                        "light" => egui::ThemePreference::Light,
                        "dark" => egui::ThemePreference::Dark,
                        _ => egui::ThemePreference::System,
                    };
                    self.context.set_theme(self.theme);
                }
                self.workspace_store = Some(store);
                self.status =
                    "草稿已恢复 · 未连接、未执行 SQL · 草稿和历史以明文保存在本机，可能含敏感信息"
                        .into();
            }
            Err(error) => {
                // Do not overwrite an unreadable workspace with an empty one.
                self.fail(format!(
                    "工作区恢复失败，已停用自动保存以保护原文件：{error:#}"
                ));
            }
        }
    }

    pub(super) fn workspace_snapshot(&self) -> Workspace {
        let tabs: Vec<_> = self
            .tabs
            .iter()
            .filter(|t| {
                !t.management_document()
                    && t.definition.is_none()
                    && (t.table_target.is_none() || t.dirty())
            })
            .map(|t| SavedTab {
                id: t.id,
                name: t.name.clone(),
                sql: t.sql.clone(),
                file_path: t.file_path.clone(),
                saved_sql: t.saved_sql.clone(),
                target: t.target.clone(),
            })
            .collect();
        let active_id = self.tabs[self.active_tab].id;
        let active_tab = tabs.iter().position(|t| t.id == active_id).unwrap_or(0);
        Workspace {
            tabs,
            active_tab,
            next_tab: self.next_tab,
            history: self.history.clone(),
            editor_height: self.editor_height,
            layout: Some(sqlshop::workspace::WorkspaceLayout {
                navigation: self.view.navigation,
                information: self.inspector_visible,
                information_force: self.view.information_force,
                toolbar: self.view.toolbar,
                status: self.view.status,
                focus: self.view.focus,
                connected_only: self.view.connected_only,
                theme: match self.theme {
                    egui::ThemePreference::Light => "light",
                    egui::ThemePreference::Dark => "dark",
                    _ => "system",
                }
                .into(),
            }),
            ..Workspace::default()
        }
    }

    pub(super) fn autosave_workspace(&mut self, force: bool) -> bool {
        if self.screenshot_path.is_some() {
            return true;
        }
        if !force && self.last_autosave.elapsed() < std::time::Duration::from_secs(2) {
            return true;
        }
        self.last_autosave = std::time::Instant::now();
        let Some(store) = &self.workspace_store else {
            return false;
        };
        let snapshot = self.workspace_snapshot();
        let serialized = match serde_json::to_string(&snapshot) {
            Ok(value) => value,
            Err(error) => {
                self.fail(format!("草稿序列化失败：{error}"));
                return false;
            }
        };
        if serialized == self.last_workspace {
            return true;
        }
        match store.save(&snapshot) {
            Ok(()) => {
                self.last_workspace = serialized;
                true
            }
            Err(error) => {
                self.fail(format!("草稿自动保存失败：{error:#}"));
                false
            }
        }
    }

    pub(super) fn finish_history(&mut self, tab: Uuid, success: bool) {
        if let Some(mut entry) = self.pending_history.remove(&tab) {
            entry.success = success;
            let mut workspace = Workspace {
                history: std::mem::take(&mut self.history),
                ..Workspace::default()
            };
            workspace.push_history(entry);
            self.history = workspace.history;
        }
    }

    pub(super) fn change_tab(&mut self, action: TabAction) {
        if self.cell_editor.is_some() {
            self.finish_inline_cell(false, CellMovement::Stay);
            if self.cell_editor.is_some() {
                return;
            }
        }
        let discards = |index: usize| match action {
            TabAction::Close(i) => index == i,
            TabAction::CloseOthers(i) => index != i,
            TabAction::CloseRight(i) => index > i,
            _ => false,
        };
        if self
            .tabs
            .iter()
            .enumerate()
            .any(|(i, tab)| discards(i) && tab.has_table_drafts())
        {
            self.fail("待关闭表标签含暂存修改；请先提交或丢弃".into());
            return;
        }
        if self.tabs.iter().enumerate().any(|(i, tab)| {
            discards(i)
                && self
                    .sessions
                    .values()
                    .any(|s| s.running_tab == Some(tab.id))
                || (discards(i) && self.query_sessions.get(&tab.id).is_some_and(|s| s.busy))
                || (discards(i) && self.tab_busy(i))
        }) {
            self.fail("正在操作的标签不能关闭；可切换或新建其他连接的标签".into());
            return;
        }
        if self.tabs.iter().enumerate().any(|(i, tab)| {
            discards(i)
                && (tab.document_dirty()
                    || self
                        .query_sessions
                        .get(&tab.id)
                        .is_some_and(|s| s.connected && s.user_sql))
        }) {
            self.confirm_tab_action = Some(action);
            return;
        }
        self.apply_ui_tab_action(action);
    }

    pub(super) fn apply_ui_tab_action(&mut self, action: TabAction) -> bool {
        if self.cell_editor.is_some() {
            self.finish_inline_cell(false, CellMovement::Stay);
            if self.cell_editor.is_some() {
                return false;
            }
        }
        let new = matches!(action, TabAction::New);
        let new_target = if self.browser_visible {
            self.browser_scope
                .as_ref()
                .map(|scope| QueryTarget {
                    connection: scope.connection,
                    database: scope.database.clone(),
                    schema: self
                        .profiles
                        .iter()
                        .find(|p| p.id == scope.connection)
                        .filter(|p| p.engine == Engine::Postgres)
                        .and(scope.schema.clone()),
                })
                .or_else(|| self.current_target())
        } else {
            let mut target = self.tabs[self.active_tab]
                .target
                .clone()
                .or_else(|| self.current_target());
            if let Some(target) = &mut target
                && self
                    .profiles
                    .iter()
                    .any(|p| p.id == target.connection && p.engine == Engine::Postgres)
                && let Some(table) = &self.tabs[self.active_tab].table_target
            {
                target.schema = Some(table.schema.clone());
            }
            target
        };
        let advanced_before: Vec<_> = self
            .tabs
            .iter()
            .filter_map(|tab| tab.advanced.as_ref().map(|doc| doc.id))
            .collect();
        apply_tab_action(
            &mut self.tabs,
            &mut self.active_tab,
            &mut self.next_tab,
            action,
        );
        if new {
            self.tabs[self.active_tab].target = new_target;
        }
        self.prune_query_sessions();
        for id in advanced_before {
            if !self.tabs.iter().any(|tab| tab.id == id) {
                self.advanced_worker.close(id);
            }
        }
        self.browser_visible = false;
        self.tabs[self.active_tab].focus_editor = true;
        self.context.data_mut(|data| {
            data.insert_temp(
                egui::Id::new("grid-keyboard-owner"),
                self.tabs[self.active_tab].id,
            )
        });
        true
    }

    pub(super) fn open_sql(&mut self) {
        if self.cell_editor.is_some() {
            self.finish_inline_cell(false, CellMovement::Stay);
            if self.cell_editor.is_some() {
                return;
            }
        }
        if self.screenshot_path.is_some() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("SQL", &["sql"])
            .pick_file()
        else {
            return;
        };
        let identity = sql_file_identity(&path);
        if let Some(index) = self.tabs.iter().position(|tab| {
            tab.table_target.is_none()
                && tab
                    .file_path
                    .as_ref()
                    .is_some_and(|open| sql_file_identity(open) == identity)
        }) {
            self.active_tab = index;
            self.browser_visible = false;
            self.tabs[index].focus_editor = true;
            self.status = "已切换到打开的 SQL 文档，保留当前草稿与查询目标".into();
            self.error = false;
            return;
        }
        match read_sql_file(&path) {
            Ok(sql) => {
                if !self.apply_ui_tab_action(TabAction::New) {
                    return;
                }
                let tab = &mut self.tabs[self.active_tab];
                tab.name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                tab.sql = sql.clone();
                tab.saved_sql = sql;
                tab.file_path = Some(path);
            }
            Err(error) => self.fail(format!("打开 SQL 失败：{error:#}")),
        }
    }

    pub(super) fn save_sql(&mut self, save_as: bool) -> bool {
        if self.tabs[self.active_tab].management_document()
            || self.tabs[self.active_tab].definition.is_some()
        {
            return false;
        }
        if self.screenshot_path.is_some() {
            return false;
        }
        let tab = &self.tabs[self.active_tab];
        let path = if !save_as && tab.file_path.is_some() {
            tab.file_path.clone()
        } else {
            rfd::FileDialog::new()
                .add_filter("SQL", &["sql"])
                .set_file_name(format!("{}.sql", tab.name))
                .save_file()
        };
        let Some(path) = path else { return false };
        let identity = sql_file_identity(&path);
        if let Some(other) = self.tabs.iter().enumerate().find_map(|(index, open)| {
            (index != self.active_tab
                && open
                    .file_path
                    .as_ref()
                    .is_some_and(|path| sql_file_identity(path) == identity))
            .then(|| open.name.clone())
        }) {
            self.fail(format!(
                "此文件已由“{other}”标签打开。请切换到该标签保存，或选择其他文件名。"
            ));
            return false;
        }
        match write_sql_file(&path, &tab.sql) {
            Ok(()) => {
                let tab = &mut self.tabs[self.active_tab];
                tab.saved_sql = tab.sql.clone();
                if tab.table_target.is_none() {
                    tab.name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                }
                tab.file_path = Some(path);
                self.status = "SQL 文件已保存（明文，可能含敏感信息）".into();
                true
            }
            Err(error) => {
                self.fail(format!("保存 SQL 失败：{error:#}"));
                false
            }
        }
    }

    pub(super) fn workspace_tools(&mut self, ui: &mut egui::Ui) {
        if self.tabs[self.active_tab].table_target.is_some() {
            self.table_controls(ui);
            return;
        }
        self.query_target_controls(ui);
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            self.query_execution_controls(ui);
            ui.separator();
            ui.add_enabled_ui(self.screenshot_path.is_none(), |ui| {
                if ui.button("打开 SQL…").clicked() {
                    self.open_sql();
                }
                if ui.button("保存 SQL").clicked() {
                    self.save_sql(false);
                }
                if ui.button("另存为…").clicked() {
                    self.save_sql(true);
                }
            });
            if ui.button("查找 / 替换").clicked() {
                self.tabs[self.active_tab].find_open ^= true;
            }
            if ui
                .button("查询历史")
                .on_hover_text("历史保存在本机，恢复到编辑器时不会执行 SQL")
                .clicked()
            {
                self.history_open = true;
            }
            let tab = &self.tabs[self.active_tab];
            if self
                .query_sessions
                .get(&tab.id)
                .is_some_and(|session| session.connected)
            {
                ui.add_enabled_ui(!self.tab_busy(self.active_tab), |ui| {
                    ui.menu_button("会话", |ui| {
                        if ui
                            .button("重建会话…")
                            .on_hover_text("结束此标签当前会话，下一次运行建立新连接")
                            .clicked()
                            && let Some(target) = self.tabs[self.active_tab].target.clone()
                        {
                            self.confirm_rebind = Some((self.tabs[self.active_tab].id, target));
                            ui.close();
                        }
                    });
                });
            }
        });
        let shortcuts = ui.is_enabled()
            && self.cell_editor.is_none()
            && self.form.is_none()
            && self.export_path.is_none()
            && self.confirm_delete.is_none()
            && !self.history_open;
        if shortcuts {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
                self.tabs[self.active_tab].find_open = true;
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) {
                self.save_sql(false);
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::O)) {
                self.open_sql();
            }
        }
        find_controls(ui, &mut self.tabs[self.active_tab]);
    }

    pub(super) fn select_query_target(&mut self, target: QueryTarget) {
        if self.tab_busy(self.active_tab)
            || self.cell_editor.is_some()
            || self.tabs[self.active_tab].table_target.is_some()
        {
            return;
        }
        let id = self.tabs[self.active_tab].id;
        if self.tabs[self.active_tab].target.as_ref() == Some(&target) {
            return;
        }
        if self
            .query_sessions
            .get(&id)
            .is_some_and(|s| s.connected && s.user_sql)
        {
            self.confirm_rebind = Some((id, target));
        } else {
            self.apply_query_target(id, target);
        }
    }

    fn query_target_controls(&mut self, ui: &mut egui::Ui) {
        let target = self.execution_target();
        let tab_id = self.tabs[self.active_tab].id;
        let mut connection = target.as_ref().map(|t| t.connection);
        let previous_connection = connection;
        let enabled = !self.tab_busy(self.active_tab)
            && self.cell_editor.is_none()
            && self.confirm_request.is_none()
            && self.confirm_rebind.is_none();
        let manager_connected = connection
            .and_then(|id| self.sessions.get(&id))
            .is_some_and(|session| session.connected.is_some());
        let private_connected = self
            .query_sessions
            .get(&tab_id)
            .is_some_and(|session| session.connected);
        let postgres = connection
            .and_then(|id| self.profiles.iter().find(|p| p.id == id))
            .is_some_and(|profile| profile.engine == Engine::Postgres);
        let extra = (if manager_connected || private_connected {
            180.0
        } else {
            284.0
        }) + if postgres { 150.0 } else { 0.0 };
        let combo_width =
            ((ui.available_width() - extra) / if postgres { 3.0 } else { 2.0 }).clamp(84.0, 180.0);
        ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            ui.add_enabled_ui(enabled, |ui| {
                ui.label("连接");
                let name = connection
                    .and_then(|id| self.profiles.iter().find(|p| p.id == id))
                    .map(|p| p.name.as_str())
                    .unwrap_or(if connection.is_some() {
                        "原连接已删除"
                    } else {
                        "请选择连接"
                    });
                egui::ComboBox::from_id_salt(("query-connection", tab_id))
                    .width(combo_width)
                    .wrap_mode(egui::TextWrapMode::Truncate)
                    .selected_text(name)
                    .show_ui(ui, |ui| {
                        for profile in &self.profiles {
                            ui.selectable_value(&mut connection, Some(profile.id), &profile.name)
                                .on_hover_text(format!("{}:{}", profile.host, profile.port));
                        }
                    });
                if connection != previous_connection
                    && let Some(id) = connection
                {
                    let database = self
                        .sessions
                        .get(&id)
                        .and_then(|s| s.active_database.clone());
                    self.select_query_target(QueryTarget {
                        connection: id,
                        database,
                        schema: None,
                    });
                }
                let target = self.tabs[self.active_tab].target.clone().or(target.clone());
                connection = target.as_ref().map(|target| target.connection);
                let session = connection.and_then(|id| self.sessions.get(&id));
                let connected = session.is_some_and(|s| s.connected.is_some())
                    || self
                        .query_sessions
                        .get(&tab_id)
                        .is_some_and(|s| s.connected && Some(s.target.connection) == connection);
                let busy = session.is_some_and(|s| s.busy);
                let mut databases = session.map(|s| s.databases.clone()).unwrap_or_default();
                if let Some(name) = target.as_ref().and_then(|t| t.database.as_ref())
                    && !databases.contains(name)
                {
                    databases.push(name.clone());
                }
                let active_database = session.and_then(|s| s.active_database.clone());
                let mut database = target.as_ref().and_then(|t| t.database.clone());
                let previous_database = database.clone();
                ui.label("数据库");
                ui.add_enabled_ui(connected, |ui| {
                    egui::ComboBox::from_id_salt(("query-database", tab_id))
                        .width(combo_width)
                        .wrap_mode(egui::TextWrapMode::Truncate)
                        .selected_text(database.as_deref().unwrap_or(if connected {
                            "请选择数据库"
                        } else {
                            "请先连接"
                        }))
                        .show_ui(ui, |ui| {
                            if active_database.is_none() {
                                ui.selectable_value(&mut database, None, "服务器（未选库）");
                            }
                            for name in &databases {
                                ui.selectable_value(&mut database, Some(name.clone()), name);
                            }
                        });
                });
                if database != previous_database
                    && let Some(id) = connection
                {
                    self.select_query_target(QueryTarget {
                        connection: id,
                        database: database.clone(),
                        schema: None,
                    });
                }
                let target = self.tabs[self.active_tab].target.clone().or(target.clone());
                let postgres = connection.and_then(|id| self.profiles.iter().find(|p| p.id == id))
                    .is_some_and(|profile| profile.engine == Engine::Postgres);
                if postgres {
                    if let Some(target) = &target
                        && connected
                        && target.database.is_some()
                    {
                        self.request_query_schemas(target, false);
                    }
                    let state = self.query_sessions.get(&tab_id);
                    let loading = state.is_some_and(|s| s.schemas_loading);
                    let schema_error = state.and_then(|s| s.schemas_error.clone());
                    let mut schemas = state.map(|s| s.schemas.clone()).unwrap_or_default();
                    let mut schema = target.as_ref().and_then(|t| t.schema.clone());
                    if let Some(schema) = &schema
                        && !schemas.contains(schema)
                    {
                        schemas.push(schema.clone());
                    }
                    let previous_schema = schema.clone();
                    ui.label("Schema");
                    ui.add_enabled_ui(connected && database.is_some() && !loading, |ui| {
                        egui::ComboBox::from_id_salt(("query-schema", tab_id))
                            .width(combo_width)
                            .wrap_mode(egui::TextWrapMode::Truncate)
                            .selected_text(if loading { "正在读取…" } else { schema.as_deref().unwrap_or("连接默认") })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut schema, None, "连接默认");
                                for name in &schemas {
                                    ui.selectable_value(&mut schema, Some(name.clone()), name);
                                }
                            })
                            .response
                            .on_hover_text("为本查询设置 search_path；更改已使用的会话会先确认结束其事务。连接默认使用数据库角色的默认搜索路径。");
                    });
                    if schema != previous_schema
                        && let Some(mut target) = target.clone()
                    {
                        target.schema = schema;
                        self.select_query_target(target);
                    }
                    if let Some(error) = schema_error {
                        ui.label(RichText::new("Schema 读取失败").color(ui.visuals().error_fg_color))
                            .on_hover_text(error);
                    }
                    if connected && !loading
                        && ui.small_button("↻").on_hover_text("刷新 Schema 列表").clicked()
                        && let Some(target) = &target
                    {
                        self.request_query_schemas(target, true);
                    }
                } else if connection.and_then(|id| self.profiles.iter().find(|p| p.id == id))
                    .is_some_and(|profile| profile.engine == Engine::SqlServer)
                {
                    let actual = target.as_ref().and_then(|target| target.schema.as_deref());
                    let label = actual.map(|schema| format!("绑定 Schema {schema}"))
                        .unwrap_or_else(|| "默认 Schema".into());
                    ui.label(RichText::new(label).small().weak())
                        .on_hover_text("SQL Server 默认 Schema 在连接或主动检查上下文时读取；访问其他 Schema 请写 schema.object，查询后不自动探测，以保留 @@ROWCOUNT。");
                }
                if !connected
                    && ui
                        .add_enabled(
                            connection.is_some_and(|id| self.profiles.iter().any(|p| p.id == id))
                                && !busy,
                            egui::Button::new("连接服务器"),
                        )
                        .clicked()
                {
                    self.selected = connection;
                    self.connect_selected();
                }
            });
            let state = self.query_sessions.get(&tab_id);
            ui.label(
                RichText::new(if state.is_some_and(|session| session.busy) {
                    "执行中"
                } else if state.is_some_and(|session| session.connected) {
                    "会话已连接"
                } else {
                    "未建立会话"
                })
                .small()
                .weak(),
            )
            .on_hover_text("每个 SQL 标签使用独立数据库会话；首次运行时建立连接");
        });
    }

    fn query_execution_controls(&mut self, ui: &mut egui::Ui) {
        let enabled = !self.tab_busy(self.active_tab)
            && self.cell_editor.is_none()
            && self.confirm_request.is_none()
            && self.confirm_rebind.is_none();
        if ui
            .add_enabled(
                enabled && self.can_execute(),
                egui::Button::new(
                    RichText::new(if self.tabs[self.active_tab].selection.trim().is_empty() {
                        "▶  运行全文"
                    } else {
                        "▶  运行选区"
                    })
                    .color(Color32::WHITE),
                )
                .fill(accent(ui)),
            )
            .on_hover_text("Ctrl / ⌘ + Enter · 执行选中内容，无选区时执行整段 SQL")
            .clicked()
        {
            self.execute();
        }
        ui.add_enabled_ui(enabled && self.can_execute(), |ui| {
            ui.menu_button("执行范围", |ui| {
                if ui
                    .button("执行当前语句  Ctrl/⌘+Shift+Enter")
                    .on_hover_text("只执行光标所在语句；忽略选区")
                    .clicked()
                {
                    self.execute_scope(SqlExecutionScope::CurrentStatement);
                    ui.close();
                }
                if ui.button("执行全文").clicked() {
                    self.execute_scope(SqlExecutionScope::All);
                    ui.close();
                }
            });
        });
        if self.tab_busy(self.active_tab) {
            ui.spinner();
        }
        self.query_control_toolbar(ui);
    }

    pub(super) fn executed_sql_footer(&self, ui: &mut egui::Ui) {
        let sql = self.tabs[self.active_tab].last_executed_sql.as_deref();
        let size = egui::vec2(ui.available_width().max(0.0), 22.0);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
        let hovered = ui.rect_contains_pointer(rect);
        let text_rect = egui::Rect::from_min_max(rect.min, rect.max - egui::vec2(28.0, 0.0));
        let preview = sql
            .map(|sql| sql.replace(['\r', '\n'], " "))
            .unwrap_or_else(|| "尚无成功执行的 SQL".into());
        ui.scope_builder(egui::UiBuilder::new().max_rect(text_rect), |ui| {
            ui.add_sized(
                text_rect.size(),
                egui::Label::new(RichText::new(preview).monospace().small().weak()).truncate(),
            );
        });
        if let Some(sql) = sql {
            response.on_hover_text(sql);
            if hovered {
                let icon_rect = egui::Rect::from_center_size(
                    egui::pos2(rect.right() - 12.0, rect.center().y),
                    egui::vec2(22.0, 22.0),
                );
                let copy = ui
                    .interact(
                        icon_rect,
                        ui.id().with("copy-executed-sql"),
                        egui::Sense::click(),
                    )
                    .on_hover_text("复制完整 SQL");
                let stroke = egui::Stroke::new(
                    1.2,
                    if copy.hovered() {
                        accent(ui)
                    } else {
                        ui.visuals().text_color()
                    },
                );
                let back = egui::Rect::from_min_size(
                    icon_rect.min + egui::vec2(4.0, 4.0),
                    egui::vec2(10.0, 12.0),
                );
                ui.painter()
                    .rect_stroke(back, 1.0, stroke, egui::StrokeKind::Inside);
                ui.painter().rect(
                    back.translate(egui::vec2(4.0, 3.0)),
                    1.0,
                    chrome(ui.ctx()),
                    stroke,
                    egui::StrokeKind::Inside,
                );
                if copy.clicked() {
                    ui.ctx().copy_text(sql.to_owned());
                }
            }
        }
    }

    pub(super) fn table_pagination(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.spacing_mut().button_padding.y = 4.0;
        ui.spacing_mut().interact_size.y = 24.0;
        let actions = self.table_action_state(self.active_tab);
        let two_rows = table_footer_two_rows(ui.available_width())
            || self.tabs[self.active_tab]
                .output
                .as_ref()
                .and_then(|output| output.sets.get(self.tabs[self.active_tab].result_index))
                .is_some_and(|set| !set.rows.is_empty());
        let can_insert_row = self.can_add_table_row(self.active_tab);
        let inline_editing = self
            .cell_editor
            .as_ref()
            .is_some_and(|editor| editor.tab == self.tabs[self.active_tab].id);
        let inline_idle = inline_editing && !self.tab_busy(self.active_tab);
        let can_submit = actions.can_submit() || (inline_idle && actions.ready && !actions.stale);
        let can_revert = actions.can_revert() || inline_idle;
        let tab = &mut self.tabs[self.active_tab];
        let mut status = tab
            .target
            .as_ref()
            .map(|target| {
                let name = self
                    .profiles
                    .iter()
                    .find(|p| p.id == target.connection)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| target.connection.to_string());
                format!(
                    "{} / {}",
                    name,
                    target.database.as_deref().unwrap_or("服务器")
                )
            })
            .unwrap_or_else(|| "未绑定执行目标".into());
        if let Some(output) = &tab.output
            && let Some(set) = output.sets.get(tab.result_index)
        {
            status.push_str(&format!(
                " · {} 行 · {} 列 · {} ms",
                set.rows.len(),
                set.columns.len(),
                output.elapsed_ms
            ));
        }
        if !actions.ready {
            status.push_str(" · 绑定会话未连接或数据库不符");
        }
        if tab.table_snapshot_stale {
            status.push_str(" · 过期快照（刷新成功后恢复编辑）");
        }
        let mut reload = false;
        let mut last_page = false;
        let mut insert_row = false;
        let mut delete_selected_rows = false;
        let selected_row_count = tab
            .selected_rows
            .map(|(start, end)| start.abs_diff(end) + 1)
            .unwrap_or(0);
        let selected_insert = tab.selected_insert;
        let can_remove_insert = actions.can_revert()
            && selected_insert
                .is_some_and(|id| tab.insert_drafts.iter().any(|draft| draft.id == id));
        let can_delete_rows = can_remove_insert
            || (actions.can_write()
                && selected_row_count > 0
                && tab.table_data.as_ref().is_some_and(|data| {
                    data.can_delete_rows()
                        && tab.selected_rows.is_some_and(|(start, end)| {
                            (start.min(end)..=start.max(end)).all(|index| {
                                data.keys
                                    .get(index)
                                    .is_some_and(|keys| keys.len() == data.primary_keys.len())
                            })
                        })
                }));
        let draft_count = tab.staged_cells.len() + tab.insert_drafts.len();
        let draft_label = tab.draft_summary();
        if draft_count > 0 {
            status.push_str(&format!(" · {draft_label}"));
        }
        let mut submit = false;
        let mut revert = false;
        let mut refresh = false;
        let mut edit_buttons = |ui: &mut egui::Ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.spacing_mut().button_padding.x = 8.0;
            insert_row = ui
                .add_enabled(can_insert_row, egui::Button::new("+"))
                .on_hover_text("在网格底部追加新增草稿；直接填写，提交全部后写入数据库")
                .clicked();
            delete_selected_rows = ui
                .add_enabled(can_delete_rows, egui::Button::new("−"))
                .on_hover_text(if can_remove_insert {
                    "移除选中的新增草稿；不会写入数据库".into()
                } else if selected_row_count > 0 {
                    format!("删除选中的 {selected_row_count} 行")
                } else {
                    "先选择要删除的行".into()
                })
                .clicked();
            ui.separator();
            submit = ui
                .add_enabled(can_submit, egui::Button::new("提交全部"))
                .on_hover_text(format!("在同一事务中提交本标签的{draft_label}"))
                .clicked();
            revert = ui
                .add_enabled(can_revert, egui::Button::new("撤销暂存"))
                .on_hover_text("仅清除本标签的本地草稿，恢复数据库快照；不会撤销已提交的数据库写入")
                .clicked();
            ui.separator();
            refresh = ui
                .add_enabled(actions.can_read(), egui::Button::new("刷新"))
                .on_hover_text("重新读取当前页；存在暂存时请先提交或撤销暂存")
                .clicked();
        };
        ui.separator();
        if two_rows {
            ui.horizontal_wrapped(|ui| {
                edit_buttons(ui);
                ui.separator();
                table_record_controls(ui, tab, actions.idle);
                if draft_count > 0 {
                    ui.add_sized(
                        [ui.available_width().max(0.0), 24.0],
                        egui::Label::new(RichText::new(&draft_label).small()).truncate(),
                    );
                }
            });
            ui.horizontal(|ui| {
                (reload, last_page) = table_page_controls(ui, tab, actions.can_read());
                table_footer_status(ui, &status, &tab.source, actions.ready);
            });
        } else {
            ui.horizontal(|ui| {
                edit_buttons(ui);
                ui.separator();
                table_record_controls(ui, tab, actions.idle);
                ui.separator();
                (reload, last_page) = table_page_controls(ui, tab, actions.can_read());
                table_footer_status(ui, &status, &tab.source, actions.ready);
            });
        }
        reload |= refresh;
        let mut delete_snapshot = None;
        if delete_selected_rows && can_remove_insert {
            tab.insert_drafts
                .retain(|draft| Some(draft.id) != selected_insert);
            tab.selected_insert = None;
            tab.insert_focus = None;
        }
        if delete_selected_rows
            && !can_remove_insert
            && let (Some(table), Some(data), Some((start, end))) = (
                tab.table_target.as_ref(),
                tab.table_data.as_ref(),
                tab.selected_rows,
            )
        {
            let indices: Vec<_> = (start.min(end)..=start.max(end)).collect();
            let key_values: Vec<_> = indices
                .iter()
                .filter_map(|index| data.keys.get(*index).cloned())
                .collect();
            if key_values.len() == indices.len() {
                delete_snapshot = Some((
                    tab.id,
                    TableTarget {
                        connection: table.connection,
                        database: table.database.clone(),
                        schema: table.schema.clone(),
                        name: table.name.clone(),
                    },
                    data.primary_keys.clone(),
                    key_values,
                ));
            }
        }
        let _ = tab;
        if insert_row {
            self.add_inline_row();
        }
        if let Some(snapshot) = delete_snapshot {
            self.delete_rows = Some(snapshot);
        }
        if revert {
            self.discard_staged_cells(self.active_tab);
        }
        if submit {
            self.submit_staged_cells(self.active_tab);
        }
        if last_page {
            self.load_last_table_page(self.active_tab);
        } else if reload {
            self.load_table(self.active_tab);
        }
    }

    pub(super) fn table_controls(&mut self, ui: &mut egui::Ui) {
        let actions = self.table_action_state(self.active_tab);
        let enabled = actions.idle && actions.ready;
        let mut reload = false;
        let mut describe = false;
        let mut export = false;
        let mut import = false;
        let tab = &mut self.tabs[self.active_tab];
        let columns = tab
            .table_data
            .as_ref()
            .map(|data| {
                data.columns
                    .iter()
                    .map(|c| c.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);
                describe = ui
                    .add(egui::Button::new("结构 / DDL").frame(false))
                    .clicked();
                ui.separator();
                let active = !tab.browse.filters.is_empty() || !tab.browse.sorts.is_empty();
                let label = if !tab.browse_draft.matches(&tab.browse) {
                    "筛选与排序 · 待应用"
                } else if active {
                    "筛选与排序 · 已应用"
                } else {
                    "筛选与排序"
                };
                if ui
                    .add_enabled(
                        actions.can_read(),
                        egui::Button::selectable(tab.filters_open, label),
                    )
                    .on_hover_text("展开筛选与排序；检查条件后点击应用，才会重新读取数据库")
                    .clicked()
                {
                    tab.filters_open ^= true;
                }
                ui.separator();
                import = ui
                    .add_enabled(actions.can_write(), egui::Button::new("导入 CSV…"))
                    .clicked();
                ui.separator();
                ui.add_enabled_ui(
                    tab.output
                        .as_ref()
                        .and_then(|o| o.sets.get(tab.result_index))
                        .is_some_and(|set| !set.columns.is_empty()),
                    |ui| {
                        ui.menu_button("复制", |ui| result_copy_controls(ui, tab, true));
                        ui.menu_button("导出", |ui| {
                            if ui.button("导出 CSV…").clicked() {
                                export = true;
                                ui.close();
                            }
                        });
                    },
                );
            });
            if tab.filters_open {
                ui.separator();
                ui.add_enabled_ui(actions.can_read(), |ui| {
                    match browse_draft_controls(ui, tab.id, &mut tab.browse_draft, &columns) {
                        Some(BrowseDraftAction::Apply) => {
                            tab.apply_browse_draft();
                            reload = true;
                        }
                        Some(BrowseDraftAction::Reset) => {
                            tab.reset_browse();
                            reload = true;
                        }
                        None => {}
                    }
                });
            }
        });
        if self.tabs[self.active_tab].has_table_drafts() {
            let tab = &self.tabs[self.active_tab];
            egui::CollapsingHeader::new(format!("{} · 查看草稿", tab.draft_summary()))
                .id_salt(("table-draft-details", tab.id))
                .default_open(false)
                .show(ui, |ui| {
                    ui.label(
                        "网格显示暂存值（●）和新增行（＋）；复制和导出使用上次读取的数据库快照。",
                    );
                    if !tab.insert_drafts.is_empty() {
                        ui.label(format!(
                            "新增 {} 行待提交；默认值由数据库处理，右键可选择 NULL 或空字符串。",
                            tab.insert_drafts.len()
                        ));
                    }
                    egui::ScrollArea::vertical()
                        .max_height(110.0)
                        .show(ui, |ui| {
                            for change in &tab.staged_cells {
                                let keys = change
                                    .primary_keys
                                    .iter()
                                    .zip(&change.key_values)
                                    .map(|(name, value)| {
                                        format!("{name}={}", draft_value_label(value))
                                    })
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                let detail = format!(
                                    "{keys} · {}：{} → {}",
                                    change.column,
                                    draft_value_label(&change.original),
                                    draft_value_label(&change.value)
                                );
                                ui.add(egui::Label::new(&detail).truncate())
                                    .on_hover_text(detail);
                            }
                        });
                })
                .header_response
                .on_hover_text("仅保存于本标签的本地草稿；在底部提交全部或撤销暂存");
        }
        if reload {
            self.load_table(self.active_tab);
        }
        if import {
            self.open_csv_import();
        }
        if export {
            self.export_path = Some(default_csv_export_path());
        }
        if describe {
            let tab = &self.tabs[self.active_tab];
            if let Some(table) = tab.table_target.clone() {
                self.open_structure_document(
                    table.connection,
                    table.database,
                    Table {
                        schema: table.schema,
                        name: table.name,
                        kind: tab
                            .table_kind
                            .clone()
                            .unwrap_or_else(|| "BASE TABLE".into()),
                    },
                );
            }
        }
    }

    pub(super) fn discard_staged_cells(&mut self, index: usize) -> bool {
        if self
            .cell_editor
            .as_ref()
            .is_some_and(|editor| editor.tab == self.tabs[index].id)
        {
            self.finish_inline_cell(true, CellMovement::Stay);
        }
        if !self.table_action_state(index).can_revert() {
            return false;
        }
        self.tabs[index].clear_table_drafts();
        self.status = "已撤销本标签的本地暂存；已提交的数据库写入不会撤销".into();
        self.error = false;
        if let Some(session) = self.tabs[index]
            .target
            .as_ref()
            .and_then(|target| self.sessions.get_mut(&target.connection))
        {
            session.status = self.status.clone();
            session.error = false;
        }
        true
    }

    pub(super) fn submit_staged_cells(&mut self, index: usize) -> bool {
        if self
            .cell_editor
            .as_ref()
            .is_some_and(|editor| editor.tab == self.tabs[index].id)
        {
            self.finish_inline_cell(false, CellMovement::Stay);
        }
        if !self.table_action_state(index).can_submit() {
            return false;
        }
        let tab = &self.tabs[index];
        let target = tab.target.clone().unwrap();
        let connection = target.connection;
        let request = if tab.insert_drafts.is_empty() {
            Request::SaveCells {
                tab: tab.id,
                target,
                changes: tab.staged_cells.clone(),
            }
        } else {
            let Some(table) = &tab.table_target else {
                return false;
            };
            let Some(data) = &tab.table_data else {
                return false;
            };
            let indices = data
                .columns
                .iter()
                .enumerate()
                .filter(|(_, column)| {
                    column.editable && column.kind != sqlshop::model::CellKind::Unsupported
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            Request::SaveTableDraft {
                tab: tab.id,
                target,
                table: Table {
                    schema: table.schema.clone(),
                    name: table.name.clone(),
                    kind: "BASE TABLE".into(),
                },
                changes: tab.staged_cells.clone(),
                columns: indices
                    .iter()
                    .map(|index| data.columns[*index].name.clone())
                    .collect(),
                rows: tab
                    .insert_drafts
                    .iter()
                    .map(|draft| {
                        indices
                            .iter()
                            .map(|index| draft.values[*index].clone())
                            .collect()
                    })
                    .collect(),
            }
        };
        self.request_on(connection, request, "正在事务提交修改…");
        self.session_busy(connection)
    }

    pub(super) fn workspace_windows(&mut self, ctx: &egui::Context) {
        if let Some(pending) = &self.confirm_sql {
            let mut confirm = false;
            let mut cancel = false;
            dialog_window("确认执行 SQL？", ctx)
                .default_width(720.0)
                .max_width((ctx.available_rect().width() - 32.0).max(200.0))
                .show(ctx, |ui| {
                    ui.label(format!(
                        "连接：{}  ·  数据库：{}{}",
                        pending.connection_name,
                        pending
                            .target
                            .database
                            .as_deref()
                            .unwrap_or("服务器（未选库）"),
                        pending.target.schema.as_ref()
                            .map(|schema| format!("  ·  Schema：{schema}"))
                            .unwrap_or_default()
                    ));
                    ui.label("此 SQL 可能修改数据或结构；原始 SQL 仍受数据库账号权限约束，批量语句中先前语句可能已提交。请核对目标与内容。");
                    let max_height = (ctx.available_rect().height() * 0.5).clamp(80.0, 400.0);
                    egui::ScrollArea::both()
                        .max_height(max_height)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.label(RichText::new(&pending.sql).monospace());
                        });
                    ui.horizontal(|ui| {
                        cancel = ui.button("取消").clicked();
                        confirm = ui.button("确认执行原始 SQL").clicked();
                    });
                });
            if cancel {
                self.confirm_sql = None;
            } else if confirm && let Some(pending) = self.confirm_sql.take() {
                self.send_sql(pending);
            }
        }
        if let Some(action) = self.confirm_tab_action {
            let mut discard = false;
            let mut cancel = false;
            dialog_window("关闭 SQL 标签？", ctx).show(ctx, |ui| {
                    ui.label("关闭将移除这些标签的草稿，并结束对应独立会话；未保存的 SQL 编辑会丢弃，未提交事务可能回滚。");
                    ui.label("需要保留时，请取消并保存文件或执行 COMMIT / ROLLBACK。");
                    ui.horizontal(|ui| {
                        cancel = ui.button("取消，返回保存").clicked();
                        discard = ui.button("确认关闭").clicked();
                    });
                });
            if cancel {
                self.confirm_tab_action = None;
            }
            if discard {
                self.confirm_tab_action = None;
                self.apply_ui_tab_action(action);
            }
        }
        if let Some((id, target)) = self.confirm_rebind.clone() {
            let mut confirm = false;
            let mut cancel = false;
            dialog_window("确认更改 SQL 标签目标", ctx).show(ctx, |ui| {
                    ui.label(format!(
                        "后续执行将发送到 {} / {}{}。这不会执行 SQL。",
                        target.connection,
                        target.database.as_deref().unwrap_or("服务器"),
                        target.schema.as_ref().map(|schema| format!(" / {schema}")).unwrap_or_default()
                    ));
                    ui.label("更改目标会结束此标签的独立会话，未提交事务可能回滚；旧结果清除，SQL 文本保留。");
                    ui.horizontal(|ui| {
                        cancel = ui.button("取消").clicked();
                        confirm = ui.button("确认改绑").clicked();
                    });
                });
            if cancel || confirm {
                self.confirm_rebind = None;
            }
            if confirm && self.query_sessions.get(&id).is_some_and(|s| s.busy) {
                self.fail("此标签正在执行，不能更改目标".into());
            } else if confirm {
                self.apply_query_target(id, target);
            }
        }
        if self.confirm_request.is_some() {
            let mut confirm = false;
            let mut cancel = false;
            dialog_window("切换 / 断开数据库会话？", ctx).show(ctx, |ui| {
                let connection = self.confirm_request.as_ref().map(|(id, _, _)| *id).unwrap();
                let name = self.profiles.iter().find(|p| p.id == connection).map(|p| p.name.as_str()).unwrap_or("原连接");
                ui.label(format!("连接 {name} 已执行用户 SQL。切库、重连或断开此连接可能回滚未提交事务；其他连接不受影响。应用不会自动提交或自动重连。"));
                ui.label("如需保留事务，请取消并先自行 COMMIT / ROLLBACK。");
                ui.horizontal(|ui| { cancel = ui.button("取消").clicked(); confirm = ui.button("确认继续").clicked(); });
            });
            if cancel {
                self.confirm_request = None;
            }
            if confirm && let Some((connection, request, status)) = self.confirm_request.take() {
                self.request_on_impl(connection, request, &status, true);
            }
        }
        if self.confirm_exit {
            let busy = self.any_busy();
            let mut exit = false;
            let mut discard_exit = false;
            let mut cancel = false;
            dialog_window("退出前确认", ctx).show(ctx, |ui| {
                ui.label("存在未保存到 SQL 文件的修改、未提交的表数据暂存，或正在执行数据库操作。");
                ui.label("表数据暂存不会自动保存；退出将丢弃暂存修改。未提交事务可能回滚。");
                ui.horizontal(|ui| {
                    cancel = ui.button("取消").clicked();
                    exit = ui
                        .add_enabled(!busy, egui::Button::new("保存草稿并退出"))
                        .clicked();
                    discard_exit = ui
                        .add_enabled(!busy, egui::Button::new("不保存草稿，直接退出"))
                        .clicked();
                });
                if busy {
                    ui.label("正在执行数据库操作，请返回并停止查询或等待任务结束后退出。");
                }
            });
            if cancel {
                self.confirm_exit = false;
            }
            if !self.any_busy() && (discard_exit || (exit && self.autosave_workspace(true))) {
                self.allow_exit = true;
                self.confirm_exit = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if self.history_open {
            let mut open = true;
            let mut restore = None;
            let mut clear = false;
            dialog_window("查询历史（仅恢复到编辑器，不执行）", ctx)
                .open(&mut open)
                .default_width(720.0)
                .show(ctx, |ui| {
                    ui.label("历史含原始 SQL，可能包含敏感内容；以明文保存在本机。");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.history_search)
                                .hint_text("搜索 SQL / 来源"),
                        );
                        clear = ui.button("清空历史").clicked();
                    });
                    let needle = self.history_search.to_lowercase();
                    egui::ScrollArea::vertical()
                        .max_height(440.0)
                        .show(ui, |ui| {
                            for (index, entry) in self.history.iter().enumerate().rev() {
                                if !format!("{} {}", entry.sql, entry.source)
                                    .to_lowercase()
                                    .contains(&needle)
                                {
                                    continue;
                                }
                                ui.push_id(index, |ui| {
                                    ui.group(|ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(format!(
                                                "{} · {} · {}",
                                                if entry.success { "成功" } else { "失败" },
                                                entry.source,
                                                chrono::DateTime::from_timestamp(
                                                    entry.timestamp,
                                                    0
                                                )
                                                .map(|t| t
                                                    .with_timezone(&chrono::Local)
                                                    .format("%Y-%m-%d %H:%M:%S")
                                                    .to_string())
                                                .unwrap_or_default()
                                            ));
                                            if ui.button("恢复为新标签").clicked() {
                                                restore = Some(entry.clone());
                                            }
                                        });
                                        ui.label(
                                            egui::RichText::new(
                                                entry.sql.chars().take(500).collect::<String>(),
                                            )
                                            .monospace(),
                                        );
                                    });
                                });
                            }
                        });
                });
            self.history_open = open;
            if clear {
                self.history.clear();
            }
            if let Some(entry) = restore {
                if !self.apply_ui_tab_action(TabAction::New) {
                    return;
                }
                let tab = &mut self.tabs[self.active_tab];
                tab.sql = entry.sql;
                tab.saved_sql.clear();
                tab.target = Some(entry.target);
            }
        }
    }
}

fn draft_value_label(value: &Option<String>) -> String {
    match value {
        None => "NULL".into(),
        Some(value) if value.is_empty() => "空字符串".into(),
        Some(value) => value.replace(['\n', '\r'], " "),
    }
}

/// Resolve aliases for document identity while retaining the user's file path
/// for I/O. New save-as destinations can resolve their existing parent folder.
fn sql_file_identity(path: &std::path::Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        path.parent()
            .and_then(|parent| parent.canonicalize().ok())
            .zip(path.file_name())
            .map(|(parent, name)| parent.join(name))
            .unwrap_or_else(|| path.to_path_buf())
    })
}

fn table_footer_status(ui: &mut egui::Ui, status: &str, source: &str, ready: bool) {
    let color = if ready {
        ui.visuals().weak_text_color()
    } else {
        Color32::from_rgb(235, 150, 70)
    };
    ui.add_sized(
        [ui.available_width().max(0.0), 24.0],
        egui::Label::new(RichText::new(status).small().color(color)).truncate(),
    )
    .on_hover_text(format!("{status}\n{source}"));
}

fn table_record_controls(ui: &mut egui::Ui, tab: &mut QueryTab, enabled: bool) {
    let Some(set) = tab
        .output
        .as_ref()
        .and_then(|output| output.sets.get(tab.result_index))
    else {
        return;
    };
    let rows = set.rows.len();
    let selected = if tab.selected_insert.is_some() {
        None
    } else {
        tab.selected_cell
            .map(|(row, _)| row)
            .or_else(|| tab.selected_rows.map(|(_, row)| row))
            .filter(|row| *row < rows)
    };
    let mut destination = None;
    ui.add_enabled_ui(enabled && rows > 0, |ui| {
        if ui
            .add_enabled(selected != Some(0), egui::Button::new("|‹"))
            .on_hover_text("当前页第一条记录")
            .clicked()
        {
            destination = Some(0);
        }
        if ui
            .add_enabled(selected.is_none_or(|row| row > 0), egui::Button::new("‹"))
            .on_hover_text("上一条记录（当前页）")
            .clicked()
        {
            destination = Some(selected.unwrap_or(0).saturating_sub(1));
        }
        ui.label(format!(
            "第 {} 条 / {} 条",
            selected.map_or(0, |row| row + 1),
            rows
        ))
        .on_hover_text("当前页的记录位置与总条数");
        if ui
            .add_enabled(
                selected.is_none_or(|row| row + 1 < rows),
                egui::Button::new("›"),
            )
            .on_hover_text("下一条记录（当前页）")
            .clicked()
        {
            destination = Some(selected.map_or(0, |row| row + 1));
        }
        if ui
            .add_enabled(selected != rows.checked_sub(1), egui::Button::new("›|"))
            .on_hover_text("当前页最后一条记录")
            .clicked()
        {
            destination = rows.checked_sub(1);
        }
    });
    if let Some(row) = destination {
        if let Some((_, column)) = tab.selected_cell {
            tab.selected_cell = Some((row, column));
            tab.cell_anchor = tab.selected_cell;
            tab.selected_rows = None;
        } else {
            tab.selected_rows = Some((row, row));
            tab.selected_cell = None;
            tab.cell_anchor = None;
        }
        tab.selected_insert = None;
        tab.insert_focus = None;
        tab.grid_scroll_row = Some(row);
    }
}

fn table_page_controls(ui: &mut egui::Ui, tab: &mut QueryTab, enabled: bool) -> (bool, bool) {
    let mut reload = false;
    let mut last_page = false;
    ui.add_enabled_ui(enabled, |ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.spacing_mut().button_padding.x = 8.0;
        let step = u64::from(tab.browse.page_size.max(1));
        if ui
            .add_enabled(tab.browse.offset > 0, egui::Button::new("首页"))
            .clicked()
        {
            tab.browse.offset = 0;
            reload = true;
        }
        if ui
            .add_enabled(tab.browse.offset > 0, egui::Button::new("上一页"))
            .clicked()
        {
            tab.browse.offset = tab.browse.offset.saturating_sub(step);
            reload = true;
        }
        ui.label("第");
        let mut page = tab.browse.offset / step + 1;
        ui.add(
            egui::DragValue::new(&mut page)
                .range(1..=((i64::MAX as u64 - step - 1) / step + 1))
                .speed(1)
                .update_while_editing(false),
        )
        .on_hover_text("输入页码并按 Enter 或离开输入框跳转");
        let page_changed = page != tab.browse.offset / step + 1;
        if page_changed && let Some(offset) = page.saturating_sub(1).checked_mul(step) {
            tab.browse.offset = offset;
            reload = true;
        }
        ui.label("页");
        if ui
            .add_enabled(
                tab.output.is_some()
                    && tab.has_more
                    && tab
                        .browse
                        .offset
                        .checked_add(step)
                        .is_some_and(|offset| offset < i64::MAX as u64 - step),
                egui::Button::new("下一页"),
            )
            .clicked()
        {
            tab.browse.offset = tab.browse.offset.saturating_add(step);
            reload = true;
        }
        last_page = ui
            .button("末页")
            .on_hover_text("按当前筛选计算总条数并读取末页；并发新增或删除记录时，末页位置可能变化")
            .clicked();
        ui.label("每页");
        let previous_size = tab.browse.page_size;
        egui::ComboBox::from_id_salt(("page-size", tab.id))
            .width(60.0)
            .selected_text(tab.browse.page_size.to_string())
            .show_ui(ui, |ui| {
                for size in [50, 100, 200, 500, 1000] {
                    ui.selectable_value(&mut tab.browse.page_size, size, size.to_string());
                }
            });
        if tab.browse.page_size != previous_size {
            tab.browse.offset = 0;
            reload = true;
        }
    });
    (reload, last_page)
}

enum BrowseDraftAction {
    Apply,
    Reset,
}

const FILTER_OPERATORS: [(FilterOperator, &str); 7] = [
    (FilterOperator::Eq, "="),
    (FilterOperator::NotEq, "≠"),
    (FilterOperator::Contains, "包含"),
    (FilterOperator::Gt, ">"),
    (FilterOperator::Lt, "<"),
    (FilterOperator::IsNull, "IS NULL"),
    (FilterOperator::IsNotNull, "IS NOT NULL"),
];

fn draft_column(ui: &mut egui::Ui, column: &mut String, columns: &[String]) {
    egui::ComboBox::from_id_salt("column")
        .width(150.0)
        .selected_text(if column.is_empty() {
            "选择列"
        } else {
            column.as_str()
        })
        .show_ui(ui, |ui| {
            for name in columns {
                ui.selectable_value(column, name.clone(), name);
            }
        });
}

fn browse_draft_controls(
    ui: &mut egui::Ui,
    tab: Uuid,
    draft: &mut TableBrowseDraft,
    columns: &[String],
) -> Option<BrowseDraftAction> {
    let mut action = None;
    ui.push_id(("table-filters", tab), |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
        egui::Frame::group(ui.style())
            .inner_margin(8)
            .show(ui, |ui| {
                // Bound the condition list so Apply and the data grid remain reachable.
                let max_height = (ui.available_height() * 0.45).clamp(80.0, 240.0);
                egui::ScrollArea::both()
                    .id_salt("draft-rows")
                    .max_height(max_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.strong("筛选");
                        let mut remove = None;
                        let filter_count = draft.filters.len();
                        for (index, row) in draft.filters.iter_mut().enumerate() {
                            ui.push_id(("filter", index), |ui| {
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut row.enabled, "")
                                        .on_hover_text("启用此条件；未启用的条件不会提交");
                                    draft_column(ui, &mut row.column, columns);
                                    let label = FILTER_OPERATORS
                                        .iter()
                                        .find(|(operator, _)| *operator == row.operator)
                                        .map(|(_, label)| *label)
                                        .unwrap_or("=");
                                    egui::ComboBox::from_id_salt("operator")
                                        .width(110.0)
                                        .selected_text(label)
                                        .show_ui(ui, |ui| {
                                            for (operator, label) in FILTER_OPERATORS {
                                                ui.selectable_value(
                                                    &mut row.operator,
                                                    operator,
                                                    label,
                                                );
                                            }
                                        });
                                    ui.add_enabled(
                                        row.needs_value(),
                                        egui::TextEdit::singleline(&mut row.value)
                                            .desired_width(170.0)
                                            .hint_text("筛选值"),
                                    )
                                    .on_hover_text("空文本是有效筛选值；查找 NULL 请使用 IS NULL");
                                    if ui
                                        .small_button("−")
                                        .on_hover_text("删除此筛选条件")
                                        .clicked()
                                    {
                                        remove = Some(index);
                                    }
                                    if index + 1 < filter_count {
                                        ui.label(RichText::new("AND").small().weak());
                                    }
                                });
                            });
                        }
                        if let Some(index) = remove {
                            draft.filters.remove(index);
                        }
                        if ui
                            .add_enabled(
                                draft.filters.len() < 100,
                                egui::Button::new("＋ 添加条件"),
                            )
                            .clicked()
                        {
                            draft.filters.push(FilterDraft::new(String::new()));
                        }
                        ui.separator();
                        ui.strong("排序方式");
                        let mut remove = None;
                        for (index, row) in draft.sorts.iter_mut().enumerate() {
                            ui.push_id(("sort", index), |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(format!("{}.", index + 1)).weak());
                                    draft_column(ui, &mut row.column, columns);
                                    egui::ComboBox::from_id_salt("direction")
                                        .width(110.0)
                                        .selected_text(if row.descending {
                                            "降序 DESC"
                                        } else {
                                            "升序 ASC"
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut row.descending,
                                                false,
                                                "升序 ASC",
                                            );
                                            ui.selectable_value(
                                                &mut row.descending,
                                                true,
                                                "降序 DESC",
                                            );
                                        });
                                    if ui
                                        .small_button("−")
                                        .on_hover_text("删除此排序条件")
                                        .clicked()
                                    {
                                        remove = Some(index);
                                    }
                                });
                            });
                        }
                        if let Some(index) = remove {
                            draft.sorts.remove(index);
                        }
                        if ui
                            .add_enabled(
                                draft.sorts.len() < columns.len(),
                                egui::Button::new("＋ 添加排序"),
                            )
                            .clicked()
                        {
                            draft.sorts.push(ColumnSort {
                                column: String::new(),
                                descending: false,
                            });
                        }
                    });
                ui.separator();
                let error = draft.validation_error(columns);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(error.is_none(), egui::Button::new("应用筛选与排序"))
                        .clicked()
                    {
                        action = Some(BrowseDraftAction::Apply);
                    }
                    if ui
                        .button("重置")
                        .on_hover_text("清空草稿和已应用条件，返回第一页")
                        .clicked()
                    {
                        action = Some(BrowseDraftAction::Reset);
                    }
                    ui.label(
                        RichText::new(
                            error.unwrap_or("编辑不会刷新；应用后生效 · 条件以 AND 组合"),
                        )
                        .small()
                        .weak(),
                    );
                });
            });
    });
    action
}

fn find_controls(ui: &mut egui::Ui, tab: &mut QueryTab) {
    if !tab.find_open {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        if ui
            .add(
                egui::TextEdit::singleline(&mut tab.find_text)
                    .hint_text("查找文本")
                    .desired_width(160.0),
            )
            .changed()
        {
            tab.find_index = usize::MAX;
        }
        ui.checkbox(&mut tab.find_case, "区分大小写");
        let matches = find_matches(&tab.sql, &tab.find_text, tab.find_case);
        ui.label(format!("{} 处匹配", matches.len()));
        if !matches.is_empty() {
            if ui.button("上一处").clicked() {
                tab.find_index = if tab.find_index >= matches.len() {
                    matches.len() - 1
                } else {
                    (tab.find_index + matches.len() - 1) % matches.len()
                };
                tab.select_range = Some(matches[tab.find_index].clone());
            }
            if ui.button("下一处").clicked() {
                tab.find_index = if tab.find_index >= matches.len() {
                    0
                } else {
                    (tab.find_index + 1) % matches.len()
                };
                tab.select_range = Some(matches[tab.find_index].clone());
            }
        }
        if ui.button("关闭查找").clicked() {
            tab.find_open = false;
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut tab.replace_text)
                .hint_text("替换为")
                .desired_width(160.0),
        );
        if ui.button("替换一处").clicked() {
            let matches = find_matches(&tab.sql, &tab.find_text, tab.find_case);
            if !matches.is_empty() {
                if tab.find_index >= matches.len() {
                    tab.find_index = 0;
                }
                let range = matches[tab.find_index].clone();
                replace_match(&mut tab.sql, range, &tab.replace_text);
                tab.selection.clear();
            }
        }
        if ui.button("全部替换").clicked() {
            tab.sql = replace_all(&tab.sql, &tab.find_text, &tab.replace_text, tab.find_case).0;
            tab.selection.clear();
            tab.find_index = 0;
        }
    });
}

#[cfg(test)]
mod browse_draft_tests {
    use super::*;

    fn columns() -> Vec<String> {
        vec!["id".into(), "name".into()]
    }

    #[test]
    fn draft_apply_excludes_disabled_rows_and_preserves_null_and_sort_order() {
        let mut tab = QueryTab::new(1);
        tab.browse.offset = 400;
        tab.browse.page_size = 100;
        tab.browse_draft.filters = vec![
            FilterDraft {
                enabled: true,
                column: "name".into(),
                operator: FilterOperator::Eq,
                value: String::new(),
            },
            FilterDraft {
                enabled: false,
                column: String::new(),
                operator: FilterOperator::Gt,
                value: "ignored".into(),
            },
            FilterDraft {
                enabled: true,
                column: "id".into(),
                operator: FilterOperator::IsNull,
                value: "stale".into(),
            },
            FilterDraft {
                enabled: true,
                column: "name".into(),
                operator: FilterOperator::IsNotNull,
                value: "stale".into(),
            },
        ];
        tab.browse_draft.sorts = vec![
            ColumnSort {
                column: "name".into(),
                descending: true,
            },
            ColumnSort {
                column: "id".into(),
                descending: false,
            },
        ];
        assert_eq!(tab.browse_draft.validation_error(&columns()), None);
        assert!(tab.browse.filters.is_empty());
        assert!(tab.browse.sorts.is_empty());
        assert_eq!(tab.browse.offset, 400);
        tab.apply_browse_draft();
        assert_eq!(tab.browse.filters.len(), 3);
        assert_eq!(tab.browse.filters[0].value, Some(String::new()));
        assert_eq!(tab.browse.filters[1].value, None);
        assert_eq!(tab.browse.filters[2].value, None);
        assert_eq!(tab.browse.sorts, tab.browse_draft.sorts);
        assert_eq!(tab.browse.offset, 0);
        assert_eq!(tab.browse.page_size, 100);
        assert_eq!(tab.browse_draft.filters.len(), 4);
    }

    #[test]
    fn incomplete_enabled_rows_and_duplicate_sorts_block_apply() {
        let mut draft = TableBrowseDraft::default();
        assert!(draft.validation_error(&columns()).is_none());
        draft.filters.push(FilterDraft::new(String::new()));
        assert!(draft.validation_error(&columns()).is_some());
        draft.filters[0].enabled = false;
        assert!(draft.validation_error(&columns()).is_none());
        draft.sorts.push(ColumnSort {
            column: String::new(),
            descending: false,
        });
        assert!(draft.validation_error(&columns()).is_some());
        draft.sorts[0].column = "id".into();
        assert!(draft.validation_error(&columns()).is_none());
        draft.sorts.push(draft.sorts[0].clone());
        assert!(draft.validation_error(&columns()).is_some());
    }

    #[test]
    fn reset_clears_applied_and_draft_conditions_without_affecting_other_tabs() {
        let mut tab = QueryTab::new(1);
        let other = QueryTab::new(2);
        tab.browse_draft
            .filters
            .push(FilterDraft::new("name".into()));
        tab.browse_draft.sorts.push(ColumnSort {
            column: "id".into(),
            descending: true,
        });
        tab.apply_browse_draft();
        tab.browse.offset = 200;
        tab.browse.page_size = 50;
        tab.reset_browse();
        assert!(tab.browse_draft.filters.is_empty());
        assert!(tab.browse_draft.sorts.is_empty());
        assert!(tab.browse.filters.is_empty());
        assert!(tab.browse.sorts.is_empty());
        assert_eq!(tab.browse.offset, 0);
        assert_eq!(tab.browse.page_size, 50);
        assert_eq!(other.browse, TableBrowseOptions::default());
        assert!(other.browse_draft.filters.is_empty());
    }
}
