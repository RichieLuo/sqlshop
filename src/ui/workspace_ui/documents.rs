use super::*;

impl App {
    pub(in crate::ui) fn change_tab(&mut self, action: TabAction) {
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

    pub(in crate::ui) fn apply_ui_tab_action(&mut self, action: TabAction) -> bool {
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

    pub(in crate::ui) fn open_sql(&mut self) {
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

    pub(in crate::ui) fn save_sql(&mut self, save_as: bool) -> bool {
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
}
