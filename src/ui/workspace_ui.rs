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
    }
}

impl App {
    pub(super) fn current_target(&self) -> Option<QueryTarget> {
        self.selected_session().and_then(|s| {
            s.connected.as_ref().map(|p| QueryTarget {
                connection: p.id,
                database: s.active_database.clone(),
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
            .filter(|t| t.table_target.is_none() || t.dirty())
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
        let discards = |index: usize| match action {
            TabAction::Close(i) => index == i,
            TabAction::CloseOthers(i) => index != i,
            TabAction::CloseRight(i) => index > i,
            _ => false,
        };
        if self.tabs.iter().enumerate().any(|(i, tab)| {
            discards(i)
                && self
                    .sessions
                    .values()
                    .any(|s| s.running_tab == Some(tab.id))
        }) {
            self.fail("正在操作的标签不能关闭；可切换或新建其他连接的标签".into());
            return;
        }
        if self
            .tabs
            .iter()
            .enumerate()
            .any(|(i, tab)| discards(i) && tab.dirty())
        {
            self.confirm_tab_action = Some(action);
            return;
        }
        self.apply_ui_tab_action(action);
    }

    pub(super) fn apply_ui_tab_action(&mut self, action: TabAction) {
        let new = matches!(action, TabAction::New);
        apply_tab_action(
            &mut self.tabs,
            &mut self.active_tab,
            &mut self.next_tab,
            action,
        );
        if new {
            self.tabs[self.active_tab].target = self.current_target();
        }
    }

    pub(super) fn open_sql(&mut self) {
        if self.screenshot_path.is_some() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("SQL", &["sql"])
            .pick_file()
        else {
            return;
        };
        match read_sql_file(&path) {
            Ok(sql) => {
                self.apply_ui_tab_action(TabAction::New);
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
        if self.tabs[self.active_tab].table_target.is_none() {
            ui.horizontal_wrapped(|ui| {
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
            if ui.button("查询历史").on_hover_text("草稿和历史以明文保存在本机，SQL 可能含敏感数据；恢复不执行。工作区不保存查询结果，表数据标签不保存；查询草稿恢复不自动连接数据库。").clicked() {
                self.history_open = true;
            }
        });
        }
        self.query_target_controls(ui);
        let shortcuts = ui.is_enabled()
            && self.cell_editor.is_none()
            && self.form.is_none()
            && self.export_path.is_none()
            && self.confirm_delete.is_none()
            && !self.history_open
            && self.structure.is_none();
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
        let tab = &mut self.tabs[self.active_tab];
        if tab.target.as_ref() != Some(&target) {
            tab.target = Some(target);
            tab.output = None;
            tab.last_executed_sql = None;
            tab.query_started = false;
            tab.result_index = 0;
            tab.selection.clear();
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
        ui.horizontal_wrapped(|ui| {
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
                    .width(180.0)
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
                    });
                }
                let target = self.tabs[self.active_tab].target.clone().or(target.clone());
                let session = connection.and_then(|id| self.sessions.get(&id));
                let connected = session.is_some_and(|s| s.connected.is_some());
                let busy = session.is_some_and(|s| s.busy);
                let databases = session.map(|s| s.databases.clone()).unwrap_or_default();
                let active_database = session.and_then(|s| s.active_database.clone());
                let mut database = target.as_ref().and_then(|t| t.database.clone());
                let previous_database = database.clone();
                ui.label("数据库");
                ui.add_enabled_ui(connected && !busy, |ui| {
                    egui::ComboBox::from_id_salt(("query-database", tab_id))
                        .width(180.0)
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
                    });
                    if database != active_database
                        && let Some(name) = &database
                    {
                        self.request_on(
                            id,
                            Request::SelectDatabase(name.clone()),
                            "正在切换数据库…",
                        );
                    }
                }
                if !connected {
                    if ui
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
                } else if database != active_database
                    && let Some(name) = &database
                {
                    if ui
                        .add_enabled(!busy, egui::Button::new("切换到此数据库"))
                        .on_hover_text("此连接当前使用其他数据库；切换成功后即可运行本查询。")
                        .clicked()
                        && let Some(id) = connection
                    {
                        self.request_on(
                            id,
                            Request::SelectDatabase(name.clone()),
                            "正在切换数据库…",
                        );
                    }
                }
            });
            ui.separator();
            if ui
                .add_enabled(
                    enabled && self.can_execute(),
                    egui::Button::new(RichText::new("▶  运行查询").color(Color32::WHITE))
                        .fill(accent(ui)),
                )
                .on_hover_text("Ctrl / ⌘ + Enter · 执行选中内容，无选区时执行整段 SQL")
                .clicked()
            {
                self.execute();
            }
            if connection.is_some_and(|id| self.session_busy(id)) {
                ui.spinner();
            }
        });
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
        let context_matches = self.tabs[self.active_tab]
            .target
            .as_ref()
            .is_some_and(|t| self.target_ready(t));
        let enabled =
            !self.tab_busy(self.active_tab) && self.cell_editor.is_none() && context_matches;
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
        if !context_matches {
            status.push_str(" · 绑定会话未连接或数据库不符");
        }
        let mut reload = false;
        ui.separator();
        ui.horizontal(|ui| {
            let status_width = (ui.available_width() - 350.0).max(0.0);
            let color = if context_matches {
                ui.visuals().weak_text_color()
            } else {
                Color32::from_rgb(235, 150, 70)
            };
            ui.add_sized(
                [status_width, 24.0],
                egui::Label::new(RichText::new(&status).small().color(color)).truncate(),
            )
            .on_hover_text(format!("{status}\n{}", tab.source));
            ui.add_enabled_ui(enabled, |ui| {
                ui.horizontal(|ui| {
                    let step = u64::from(tab.browse.page_size.max(1));
                    if ui
                        .add_enabled(tab.browse.offset > 0, egui::Button::new("上一页"))
                        .clicked()
                    {
                        tab.browse.offset = tab.browse.offset.saturating_sub(step);
                        reload = true;
                    }
                    ui.label(format!("第 {} 页", tab.browse.offset / step + 1));
                    if ui
                        .add_enabled(
                            tab.output.is_some() && tab.has_more,
                            egui::Button::new("下一页"),
                        )
                        .clicked()
                    {
                        tab.browse.offset = tab.browse.offset.saturating_add(step);
                        reload = true;
                    }
                    ui.label("每页");
                    let previous_size = tab.browse.page_size;
                    egui::ComboBox::from_id_salt(("page-size", tab.id))
                        .width(60.0)
                        .selected_text(tab.browse.page_size.to_string())
                        .show_ui(ui, |ui| {
                            for size in [50, 100, 200, 500, 1000] {
                                ui.selectable_value(
                                    &mut tab.browse.page_size,
                                    size,
                                    size.to_string(),
                                );
                            }
                        });
                    if tab.browse.page_size != previous_size {
                        tab.browse.offset = 0;
                        reload = true;
                    }
                });
            });
        });
        if reload {
            self.load_table(self.active_tab);
        }
    }

    pub(super) fn table_controls(&mut self, ui: &mut egui::Ui) {
        let enabled = !self.tab_busy(self.active_tab)
            && self.tabs[self.active_tab]
                .target
                .as_ref()
                .is_some_and(|t| self.target_ready(t));
        let mut reload = false;
        let mut describe = false;
        let mut export = false;
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
        ui.add_enabled_ui(enabled && self.cell_editor.is_none(), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);
                describe = ui
                    .add(egui::Button::new("结构 / DDL").frame(false))
                    .clicked();
                ui.separator();
                let active = !tab.browse.filters.is_empty() || !tab.browse.sorts.is_empty();
                let label = if active {
                    "筛选与排序 · 已应用"
                } else {
                    "筛选与排序"
                };
                if ui
                    .selectable_label(tab.filters_open, label)
                    .on_hover_text("展开参数化筛选与排序，不接受 SQL 表达式")
                    .clicked()
                {
                    tab.filters_open ^= true;
                }
                ui.separator();
                reload = ui
                    .add(egui::Button::new("刷新").frame(false))
                    .on_hover_text("重新读取当前页")
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
            }
        });
        if reload {
            self.load_table(self.active_tab);
        }
        if export {
            self.export_path = Some(default_csv_export_path());
        }
        if describe {
            let tab = &self.tabs[self.active_tab];
            if !tab.target.as_ref().is_some_and(|t| self.target_ready(t)) {
                self.fail("请先选择此表绑定的连接和数据库，再读取结构".into());
                return;
            }
            if let (Some(target), Some(table)) = (tab.target.clone(), tab.table_target.as_ref()) {
                let request = Request::DescribeTable {
                    tab: tab.id,
                    target,
                    table: Table {
                        schema: table.schema.clone(),
                        name: table.name.clone(),
                        kind: "BASE TABLE".into(),
                    },
                };
                self.request(request, "正在读取表结构…");
            }
        }
    }

    pub(super) fn workspace_windows(&mut self, ctx: &egui::Context) {
        if let Some(action) = self.confirm_tab_action {
            let mut discard = false;
            let mut cancel = false;
            egui::Window::new("关闭未保存的 SQL？")
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label("待关闭标签含未保存到 SQL 文件的修改；关闭后其自动草稿也会移除。");
                    ui.label("选择取消可返回标签，先保存 SQL 文件。");
                    ui.horizontal(|ui| {
                        cancel = ui.button("取消，返回保存").clicked();
                        discard = ui.button("丢弃并关闭").clicked();
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
            egui::Window::new("确认更改 SQL 标签目标")
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label(format!(
                        "后续执行将发送到 {} / {}。这不会执行 SQL。",
                        target.connection,
                        target.database.as_deref().unwrap_or("服务器")
                    ));
                    ui.label("请确认不是生产库；旧结果将清除，SQL 文本保留。");
                    ui.horizontal(|ui| {
                        cancel = ui.button("取消").clicked();
                        confirm = ui.button("确认改绑").clicked();
                    });
                });
            if cancel || confirm {
                self.confirm_rebind = None;
            }
            if confirm && self.sessions.values().any(|s| s.running_tab == Some(id)) {
                self.fail("此标签正在执行，不能更改目标".into());
            } else if confirm && let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
                tab.target = Some(target);
                tab.output = None;
                tab.query_started = false;
                tab.selection.clear();
            }
        }
        if self.confirm_request.is_some() {
            let mut confirm = false;
            let mut cancel = false;
            egui::Window::new("切换 / 断开数据库会话？").collapsible(false).show(ctx, |ui| {
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
            let mut exit = false;
            let mut discard_exit = false;
            let mut cancel = false;
            egui::Window::new("退出前确认")
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label("存在未保存到 SQL 文件的修改，或正在执行数据库操作。");
                    ui.label(
                        "退出会尝试保留本机草稿；未提交事务可能回滚。自动草稿不等于保存 SQL 文件。",
                    );
                    ui.horizontal(|ui| {
                        cancel = ui.button("取消").clicked();
                        exit = ui.button("保存草稿并退出").clicked();
                        discard_exit = ui.button("不保存草稿，直接退出").clicked();
                    });
                });
            if cancel {
                self.confirm_exit = false;
            }
            if discard_exit || (exit && self.autosave_workspace(true)) {
                self.allow_exit = true;
                self.confirm_exit = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if self.history_open {
            let mut open = true;
            let mut restore = None;
            let mut clear = false;
            egui::Window::new("查询历史（仅恢复到编辑器，不执行）")
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
                self.apply_ui_tab_action(TabAction::New);
                let tab = &mut self.tabs[self.active_tab];
                tab.sql = entry.sql;
                tab.saved_sql.clear();
                tab.target = Some(entry.target);
            }
        }
        if let Some((id, structure)) = &self.structure {
            let mut open = true;
            egui::Window::new("表结构 / 建表 DDL")
                .id(egui::Id::new(("structure", id)))
                .open(&mut open)
                .default_width(820.0)
                .default_height(560.0)
                .resizable(true)
                .show(ctx, |ui| {
                    ui.label("结构来自请求时的表标签目标，不会随左侧数据库切换。");
                    if ui.button("复制 DDL").clicked() {
                        ui.ctx().copy_text(structure.ddl.clone());
                    }
                    egui::ScrollArea::both().show(ui, |ui| {
                        for (title, set) in [
                            ("列", &structure.columns),
                            ("索引", &structure.indexes),
                            ("外键", &structure.foreign_keys),
                        ] {
                            egui::CollapsingHeader::new(title)
                                .default_open(true)
                                .show(ui, |ui| {
                                    egui::Grid::new(("structure-grid", title))
                                        .striped(true)
                                        .show(ui, |ui| {
                                            for column in &set.columns {
                                                ui.strong(column);
                                            }
                                            ui.end_row();
                                            for row in &set.rows {
                                                for cell in row {
                                                    ui.label(cell.as_deref().unwrap_or("NULL"));
                                                }
                                                ui.end_row();
                                            }
                                        });
                                });
                        }
                        ui.separator();
                        ui.label(RichText::new(&structure.ddl).monospace());
                    });
                });
            if !open {
                self.structure = None;
            }
        }
    }
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
