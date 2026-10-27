use super::*;
use sqlshop::catalog::{Catalog, CatalogStore, SavedQuery};

pub(super) enum CatalogDialog {
    Group {
        id: Option<Uuid>,
        name: String,
    },
    Query {
        id: Option<Uuid>,
        name: String,
        sql: String,
        target: Option<QueryTarget>,
        source_tab: Option<Uuid>,
        expected: Option<Box<SavedQuery>>,
    },
    DeleteGroup(Uuid, String),
    DeleteQuery(Uuid, String),
}

impl App {
    pub(super) fn saved_query_fixture(&mut self) {
        if self.screenshot_path.is_none() {
            return;
        }
        let mode = std::env::var("SQLSHOP_SCREENSHOT_VIEW").unwrap_or_default();
        if !matches!(mode.as_str(), "saved-queries" | "saved-queries-offline") {
            return;
        }
        let connection = self.profiles[0].id;
        for (name, database, schema, sql) in [
            (
                "每日订单汇总",
                "app_dev",
                "public",
                "SELECT date_trunc('day', created_at) AS day, count(*) AS orders\nFROM public.orders\nGROUP BY 1\nORDER BY 1 DESC;",
            ),
            (
                "待发货订单",
                "app_dev",
                "public",
                "SELECT * FROM public.orders WHERE status = 'pending';",
            ),
            (
                "活跃用户",
                "app_dev",
                "public",
                "SELECT * FROM public.users WHERE active = true;",
            ),
            (
                "审计记录",
                "app_dev",
                "audit",
                "SELECT * FROM audit.events ORDER BY created_at DESC;",
            ),
            (
                "月度营收",
                "analytics",
                "public",
                "SELECT * FROM public.monthly_revenue;",
            ),
        ] {
            self.catalog.queries.push(SavedQuery {
                id: Uuid::new_v4(),
                name: name.into(),
                sql: sql.into(),
                target: Some(QueryTarget {
                    connection,
                    database: Some(database.into()),
                    schema: Some(schema.into()),
                }),
            });
        }
        self.catalog.queries.push(SavedQuery {
            id: Uuid::new_v4(),
            name: "未绑定查询示例".into(),
            sql: "SELECT 1;".into(),
            target: None,
        });
        self.show_object_scope(
            object_browser_ui::BrowserScope {
                connection,
                database: Some("app_dev".into()),
                schema: None,
            },
            None,
        );
        self.browser_state.category = object_browser_ui::BrowserCategory::Queries;
        self.browser_state.selected_query = Some(self.catalog.queries[0].id);
        self.tree_selection.selected =
            Some(TreeNode::Queries(connection, Some("app_dev".into()), None));
        self.status = "查询列表布局示例 · 未连接数据库".into();
        if mode == "saved-queries-offline" {
            self.sessions.get_mut(&connection).unwrap().connected = None;
        }
    }

    pub(super) fn show_saved_queries(&mut self) {
        let unbound_document = !self.browser_visible && self.tabs[self.active_tab].target.is_none();
        if !self.browser_visible {
            if let Some(target) = self.tabs[self.active_tab].target.clone() {
                self.show_object_scope(
                    object_browser_ui::BrowserScope {
                        connection: target.connection,
                        database: target.database,
                        schema: target.schema,
                    },
                    None,
                );
            } else {
                self.show_selected_objects();
            }
        } else {
            self.show_selected_objects();
        }
        if self.browser_visible {
            if self
                .browser_scope
                .as_ref()
                .is_some_and(|scope| self.is_redis_connection(scope.connection))
            {
                self.browser_scope = None;
                self.browser_state.set_scope(None);
            }
            self.browser_state.category = object_browser_ui::BrowserCategory::Queries;
            self.browser_state.selected = None;
            self.browser_state.selected_query = None;
            self.browser_state.search.clear();
            self.browser_state.all_queries = unbound_document || self.browser_scope.is_none();
        }
    }

    pub(super) fn restore_catalog(&mut self) {
        let Some(store) = &self.store else { return };
        let store = CatalogStore::new(store.directory.clone());
        match store.load() {
            Ok(catalog) => {
                self.catalog = catalog;
                self.refresh_saved_query_states();
                self.catalog_store = Some(store);
            }
            Err(error) => self.fail(format!(
                "查询库 / 分组读取失败，已停用修改以保护原文件：{error:#}"
            )),
        }
    }

    fn commit_catalog(&mut self, catalog: Catalog) -> bool {
        let result = self
            .catalog_store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("查询库不可写，请检查配置目录后重启"))
            .and_then(|store| store.save(&catalog));
        match result {
            Ok(()) => {
                self.catalog = catalog;
                self.refresh_saved_query_states();
                self.status = "查询库 / 连接分组已保存".into();
                self.error = false;
                true
            }
            Err(error) => {
                self.fail(format!("保存失败，未更改内存中的查询库：{error:#}"));
                false
            }
        }
    }

    fn refresh_saved_query_states(&mut self) {
        for tab in &mut self.tabs {
            tab.saved_query_stale = tab.saved_query.as_ref().is_some_and(|saved| {
                self.catalog
                    .queries
                    .iter()
                    .find(|query| query.id == saved.id)
                    != Some(saved)
            });
        }
    }

    pub(super) fn modal_open(&self) -> bool {
        self.form.is_some()
            || self.confirm_delete.is_some()
            || self.schema_dialog.is_some()
            || self.delete_rows.is_some()
            || self.export_path.is_some()
            || self.csv_import.is_some()
            || self.confirm_tab_action.is_some()
            || self.confirm_rebind.is_some()
            || self.confirm_request.is_some()
            || self.confirm_exit
            || self.history_open
            || self.library_open
            || self.catalog_dialog.is_some()
    }

    pub(super) fn new_connection(&mut self) {
        self.form = Some(ConnectionForm::sql(
            Profile::default(),
            Credentials::default(),
            false,
        ));
    }

    pub(super) fn can_save_query(&self) -> bool {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return false;
        };
        !self.browser_visible
            && !tab.management_document()
            && self.table_create.is_none()
            && tab.table_target.is_none()
            && tab.definition.is_none()
    }

    pub(super) fn save_query_command(&mut self, save_as: bool) {
        if !self.can_save_query() {
            return;
        }
        let tab = &self.tabs[self.active_tab];
        if !save_as && tab.file_path.is_some() {
            self.save_sql(false);
            return;
        }
        if !save_as && let Some(saved) = tab.saved_query.clone() {
            let dialog = CatalogDialog::Query {
                id: Some(saved.id),
                name: saved.name.clone(),
                sql: tab.sql.clone(),
                target: tab.target.clone(),
                source_tab: Some(tab.id),
                expected: Some(Box::new(saved)),
            };
            self.apply_catalog_dialog(&dialog);
        } else {
            self.save_to_library(None);
        }
    }

    fn save_to_library(&mut self, id: Option<Uuid>) {
        if !self.can_save_query() {
            return;
        }
        let tab = &self.tabs[self.active_tab];
        let expected = id.and_then(|id| {
            tab.saved_query
                .as_ref()
                .filter(|q| q.id == id)
                .or_else(|| self.catalog.queries.iter().find(|q| q.id == id))
                .cloned()
        });
        let name = id
            .and_then(|id| {
                self.catalog
                    .queries
                    .iter()
                    .find(|q| q.id == id)
                    .map(|q| q.name.clone())
            })
            .unwrap_or_else(|| tab.name.clone());
        self.catalog_dialog = Some(CatalogDialog::Query {
            id,
            name,
            sql: tab.sql.clone(),
            target: tab.target.clone(),
            source_tab: Some(tab.id),
            expected: expected.map(Box::new),
        });
    }

    fn apply_catalog_dialog(&mut self, dialog: &CatalogDialog) -> bool {
        let mut catalog = self.catalog.clone();
        let result = match dialog {
            CatalogDialog::Group { id, name } => catalog.save_group(*id, name).map(|_| None),
            CatalogDialog::Query {
                id,
                name,
                sql,
                target,
                expected,
                ..
            } => {
                if id.is_some()
                    && catalog.queries.iter().find(|q| Some(q.id) == *id) != expected.as_deref()
                {
                    self.fail(
                        "此查询已被修改或删除，未覆盖保存。请重新打开核对，或另存为查询。".into(),
                    );
                    return false;
                }
                catalog
                    .save_query(*id, name, sql.clone(), target.clone())
                    .map(Some)
            }
            CatalogDialog::DeleteGroup(id, _) => {
                catalog.delete_group(*id);
                Ok(None)
            }
            CatalogDialog::DeleteQuery(id, _) => {
                catalog.queries.retain(|q| q.id != *id);
                Ok(None)
            }
        };
        let saved_id = match result {
            Ok(id) => id,
            Err(error) => {
                self.fail(error.to_string());
                return false;
            }
        };
        if !self.commit_catalog(catalog) {
            return false;
        }
        if let Some(id) = saved_id {
            let query = self
                .catalog
                .queries
                .iter()
                .find(|q| q.id == id)
                .unwrap()
                .clone();
            let source_tab = match dialog {
                CatalogDialog::Query { source_tab, .. } => *source_tab,
                _ => None,
            };
            for tab in &mut self.tabs {
                let source = Some(tab.id) == source_tab;
                let renamed = tab.saved_query.as_ref().is_some_and(|old| {
                    old.id == query.id && old.sql == query.sql && old.target == query.target
                });
                if source || renamed {
                    if tab.file_path.is_none() {
                        tab.name = query.name.clone();
                    }
                    tab.saved_query = Some(query.clone());
                    tab.saved_query_stale = false;
                }
            }
            self.status = format!("查询“{}”已保存", query.name);
        }
        true
    }

    pub(super) fn command_bar(&mut self, ctx: &egui::Context) {
        let available = !self.modal_open();
        let selected_sql_ready = self
            .selected_session()
            .is_some_and(|s| s.connected.is_some() && !s.busy);
        let selected_ready = self
            .selected
            .map(|id| {
                if self.is_redis_connection(id) {
                    self.redis_ready(id)
                } else {
                    selected_sql_ready
                }
            })
            .unwrap_or(false);
        let selected_database = self
            .selected_session()
            .is_some_and(|s| s.active_database.is_some());
        let writable = self.catalog_store.is_some();
        let sql_tab = self.can_save_query();
        let dark = ctx.style().visuals.dark_mode;
        let divider = egui::Stroke::new(
            1.0,
            if dark {
                Color32::from_rgb(48, 61, 72)
            } else {
                Color32::from_rgb(215, 225, 229)
            },
        );
        egui::TopBottomPanel::top("main-menu")
            .frame(
                egui::Frame::new()
                    .fill(chrome(ctx))
                    .inner_margin(egui::Margin::symmetric(16, 4)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().button_padding = egui::vec2(8.0, 3.0);
                ui.spacing_mut().interact_size.y = 24.0;
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
                ui.add_enabled_ui(available, |ui| {
                    egui::MenuBar::new().ui(ui, |ui| {
                        ui.spacing_mut().button_padding = egui::vec2(10.0, 3.0);
                        ui.spacing_mut().item_spacing.x = 14.0;
                        ui.menu_button("文件", |ui| {
                            if ui.button("新建连接…").clicked() {
                                self.new_connection();
                                ui.close();
                            }
                            if ui.button("新建查询").clicked() {
                                self.change_tab(TabAction::New);
                                ui.close();
                            }
                            ui.separator();
                            if ui
                                .add_enabled(
                                    writable && sql_tab,
                                    egui::Button::new("保存查询  Ctrl+S"),
                                )
                                .clicked()
                            {
                                self.save_query_command(false);
                                ui.close();
                            }
                            if ui.button("打开 SQL 文件…  Ctrl+O").clicked() {
                                self.open_sql();
                                ui.close();
                            }
                            if ui
                                .add_enabled(sql_tab, egui::Button::new("保存 SQL 文件…"))
                                .clicked()
                            {
                                self.save_sql(false);
                                ui.close();
                            }
                            if ui
                                .add_enabled(sql_tab, egui::Button::new("SQL 文件另存为…"))
                                .clicked()
                            {
                                self.save_sql(true);
                                ui.close();
                            }
                            ui.separator();
                            if ui
                                .add_enabled(
                                    !self.tabs.is_empty(),
                                    egui::Button::new("关闭当前标签"),
                                )
                                .clicked()
                            {
                                self.change_tab(TabAction::Close(self.active_tab));
                                ui.close();
                            }
                            if ui.button("退出").clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                ui.close();
                            }
                        });
                        ui.menu_button("连接", |ui| {
                            if ui
                                .add_enabled(
                                    self.selected.is_some(),
                                    egui::Button::new("连接选中服务器"),
                                )
                                .clicked()
                            {
                                self.connect_selected();
                                ui.close();
                            }
                            if ui
                                .add_enabled(
                                    self.selected.is_some(),
                                    egui::Button::new("编辑选中连接…"),
                                )
                                .clicked()
                            {
                                if let Some(id) = self.selected {
                                    self.edit_connection(id);
                                }
                                ui.close();
                            }
                            if ui
                                .add_enabled(
                                    self.selected.is_some(),
                                    egui::Button::new("删除选中连接…"),
                                )
                                .clicked()
                            {
                                self.confirm_delete = self.selected;
                                ui.close();
                            }
                            if ui
                                .add_enabled(selected_ready, egui::Button::new("断开活动连接…"))
                                .clicked()
                            {
                                if let Some(id) = self.selected {
                                    self.disconnect_connection(id);
                                }
                                ui.close();
                            }
                            if ui
                                .add_enabled(
                                    selected_sql_ready,
                                    egui::Button::new("刷新数据库列表"),
                                )
                                .clicked()
                            {
                                self.request(Request::Databases, "正在刷新数据库列表…");
                                ui.close();
                            }
                            if ui
                                .add_enabled(
                                    selected_database && selected_sql_ready,
                                    egui::Button::new("刷新表和视图"),
                                )
                                .clicked()
                            {
                                self.request(Request::Tables, "正在刷新表目录…");
                                ui.close();
                            }
                            ui.separator();
                            if ui
                                .add_enabled(writable, egui::Button::new("新建连接分组…"))
                                .clicked()
                            {
                                self.catalog_dialog = Some(CatalogDialog::Group {
                                    id: None,
                                    name: String::new(),
                                });
                                ui.close();
                            }
                            if ui
                                .add_enabled(writable, egui::Button::new("管理连接分组…"))
                                .clicked()
                            {
                                self.library_open = true;
                                ui.close();
                            }
                        });
                        ui.menu_button("查询", |ui| {
                            if ui.button("新建查询").clicked() {
                                self.change_tab(TabAction::New);
                                ui.close();
                            }
                            if ui
                                .add_enabled(
                                    sql_tab && self.can_execute(),
                                    egui::Button::new("执行选中 / 全部 SQL  Ctrl+Enter"),
                                )
                                .clicked()
                            {
                                self.execute();
                                ui.close();
                            }
                            if ui
                                .add_enabled(
                                    sql_tab && self.can_execute(),
                                    egui::Button::new("执行当前语句  Ctrl/⌘+Shift+Enter"),
                                )
                                .clicked()
                            {
                                self.execute_scope(SqlExecutionScope::CurrentStatement);
                                ui.close();
                            }
                            if ui
                                .add_enabled(
                                    sql_tab && self.can_execute(),
                                    egui::Button::new("执行全文"),
                                )
                                .clicked()
                            {
                                self.execute_scope(SqlExecutionScope::All);
                                ui.close();
                            }
                            ui.separator();
                            if ui
                                .add_enabled(
                                    writable && sql_tab,
                                    egui::Button::new("保存查询  Ctrl+S"),
                                )
                                .clicked()
                            {
                                self.save_query_command(false);
                                ui.close();
                            }
                            if ui
                                .add_enabled(
                                    writable && sql_tab,
                                    egui::Button::new("查询另存为…  Ctrl+Shift+S"),
                                )
                                .clicked()
                            {
                                self.save_query_command(true);
                                ui.close();
                            }
                            if ui.button("查询列表").clicked() {
                                self.show_saved_queries();
                                ui.close();
                            }
                            if ui.button("查询历史…").clicked() {
                                self.history_open = true;
                                ui.close();
                            }
                        });
                        ui.menu_button("编辑", |ui| {
                            if ui
                                .add_enabled(sql_tab, egui::Button::new("查找 / 替换  Ctrl+F"))
                                .clicked()
                            {
                                self.tabs[self.active_tab].find_open = true;
                                ui.close();
                            }
                        });
                        self.management_menu(ui);
                        ui.menu_button("视图", |ui| {
                            ui.checkbox(&mut self.view.navigation, "导航栏  ⌘/Ctrl+1");
                            if ui
                                .checkbox(&mut self.inspector_visible, "信息栏  ⌘/Ctrl+2")
                                .changed()
                            {
                                self.view.information_force = self.inspector_visible;
                                if self.inspector_visible {
                                    self.view.focus = false;
                                }
                            }
                            ui.checkbox(&mut self.view.toolbar, "工具栏");
                            ui.checkbox(&mut self.view.status, "状态栏");
                            ui.checkbox(&mut self.view.focus, "专注模式");
                            ui.separator();
                            for (theme, label) in [
                                (egui::ThemePreference::System, "跟随系统"),
                                (egui::ThemePreference::Light, "浅色"),
                                (egui::ThemePreference::Dark, "深色"),
                            ] {
                                if ui.selectable_label(self.theme == theme, label).clicked() {
                                    self.theme = theme;
                                    ctx.set_theme(theme);
                                    ui.close();
                                }
                            }
                        });
                    });
                });
            });
        if !self.view.toolbar || self.view.focus {
            return;
        }
        egui::TopBottomPanel::top("main-toolbar")
            .frame(
                egui::Frame::new()
                    .fill(canvas(ctx))
                    .stroke(divider)
                    .inner_margin(egui::Margin::symmetric(16, 8)),
            )
            .show(ctx, |ui| {
                toolbar_style(ui);
                ui.visuals_mut().widgets.noninteractive.bg_stroke = divider;
                ui.add_enabled_ui(available, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("＋ 新建连接").clicked() {
                            self.new_connection();
                        }
                        if ui
                            .add_enabled(self.selected.is_some(), egui::Button::new("连接服务器"))
                            .clicked()
                        {
                            self.connect_selected();
                        }
                        if ui
                            .add_enabled(selected_ready, egui::Button::new("断开"))
                            .clicked()
                            && let Some(id) = self.selected
                        {
                            self.disconnect_connection(id);
                        }
                        ui.separator();
                        if ui.button("＋ 新建查询").clicked() {
                            self.change_tab(TabAction::New);
                        }
                        if ui.button("打开 SQL").clicked() {
                            self.open_sql();
                        }
                        if ui
                            .add_enabled(writable && sql_tab, egui::Button::new("保存查询…"))
                            .on_hover_text(
                                "命名保存查询；已打开的命名查询直接更新，SQL 文件继续保存到原文件",
                            )
                            .clicked()
                        {
                            self.save_query_command(false);
                        }
                        if ui
                            .button("查询")
                            .on_hover_text("在对象页浏览当前连接和数据库的查询")
                            .clicked()
                        {
                            self.show_saved_queries();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(selected_sql_ready, egui::Button::new("刷新目录"))
                            .clicked()
                        {
                            self.request(
                                if selected_database {
                                    Request::Tables
                                } else {
                                    Request::Databases
                                },
                                "正在刷新目录…",
                            );
                        }
                    });
                });
            });
    }

    pub(super) fn move_to_group(&mut self, connection: Uuid, group: Option<Uuid>) {
        let mut catalog = self.catalog.clone();
        match catalog.move_connection(connection, group) {
            Ok(()) => {
                self.commit_catalog(catalog);
            }
            Err(error) => self.fail(error.to_string()),
        }
    }

    pub(super) fn open_saved_query(&mut self, query: SavedQuery) {
        // Reuse the named document without replacing unsaved edits.
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.saved_query.as_ref() == Some(&query))
        {
            self.active_tab = index;
            self.browser_visible = false;
            self.tabs[index].focus_editor = true;
            return;
        }
        if !self.apply_ui_tab_action(TabAction::New) {
            return;
        }
        self.tabs[self.active_tab] = query_tab_from_saved(query, self.tabs[self.active_tab].id);
        self.status = "已在新标签打开保存的查询，保留原执行目标；未连接、未执行".into();
        self.error = false;
    }

    pub(super) fn catalog_windows(&mut self, ctx: &egui::Context) {
        if self.library_open {
            let mut open = true;
            dialog_window("连接分组", ctx)
                .open(&mut open)
                .default_width(440.0)
                .show(ctx, |ui| {
                    ui.add_enabled_ui(self.catalog_dialog.is_none(), |ui| {
                        if self.catalog_store.is_none() {
                            ui.label("连接分组不可写，请检查配置目录。");
                        }
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.strong("连接分组")
                                .on_hover_text("右键侧栏连接可移动到分组");
                            if ui
                                .add_enabled(
                                    self.catalog_store.is_some(),
                                    egui::Button::new("新建分组…"),
                                )
                                .clicked()
                            {
                                self.catalog_dialog = Some(CatalogDialog::Group {
                                    id: None,
                                    name: String::new(),
                                });
                            }
                        });
                        egui::ScrollArea::vertical()
                            .id_salt("group-list")
                            .max_height(150.0)
                            .show(ui, |ui| {
                                for group in self.catalog.groups.clone() {
                                    ui.push_id(group.id, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(&group.name);
                                            ui.add_enabled_ui(self.catalog_store.is_some(), |ui| {
                                                if ui.button("重命名…").clicked() {
                                                    self.catalog_dialog =
                                                        Some(CatalogDialog::Group {
                                                            id: Some(group.id),
                                                            name: group.name.clone(),
                                                        });
                                                }
                                                if ui.button("删除分组…").clicked() {
                                                    self.catalog_dialog =
                                                        Some(CatalogDialog::DeleteGroup(
                                                            group.id,
                                                            group.name.clone(),
                                                        ));
                                                }
                                            });
                                        });
                                    });
                                }
                            });
                    });
                });
            self.library_open = open;
        }
        if let Some(mut dialog) = self.catalog_dialog.take() {
            let mut cancel = false;
            let mut confirm = false;
            dialog_window("确认查询库修改", ctx)
                .id(egui::Id::new("catalog-dialog"))
                .show(ctx, |ui| {
                    match &mut dialog {
                        CatalogDialog::Group { id, name } => {
                            ui.label(if id.is_some() {
                                "重命名连接分组"
                            } else {
                                "新建连接分组"
                            });
                            ui.add(dialog_input(name).desired_width(f32::INFINITY));
                        }
                        CatalogDialog::Query {
                            id, name, target, ..
                        } => {
                            ui.label(if id.is_some() {
                                "更新已保存查询（将替换该条目的名称、SQL 和目标）"
                            } else {
                                "新建命名查询（不会覆盖同名条目）"
                            });
                            ui.add(dialog_input(name).desired_width(f32::INFINITY));
                            ui.label(format!(
                                "执行目标：{}",
                                target
                                    .as_ref()
                                    .map(|t| format!(
                                        "{} / {}",
                                        t.connection,
                                        t.database.as_deref().unwrap_or("服务器")
                                    ))
                                    .unwrap_or_else(|| "未绑定".into())
                            ));
                        }
                        CatalogDialog::DeleteGroup(_, name) => {
                            ui.label(format!(
                                "删除分组“{name}”？组内连接移入“未分组”，保留连接配置及凭据。"
                            ));
                        }
                        CatalogDialog::DeleteQuery(_, name) => {
                            ui.label(format!(
                                "永久删除命名查询“{name}”？已打开的标签、SQL 文件和历史不受影响。"
                            ));
                        }
                    }
                    dialog_actions(ui, |ui| {
                        let button = match dialog {
                            CatalogDialog::DeleteGroup(..) | CatalogDialog::DeleteQuery(..) => {
                                danger_button(ui, "确认删除")
                            }
                            _ => primary_button(ui, "保存"),
                        };
                        confirm = ui
                            .add_enabled(self.catalog_store.is_some(), button)
                            .on_hover_text("仅修改本机查询库，不执行 SQL，也不更改 SQL 文件")
                            .clicked();
                        cancel = ui.button("取消").clicked();
                    });
                });
            let completed = confirm && self.apply_catalog_dialog(&dialog);
            if !cancel && !completed {
                self.catalog_dialog = Some(dialog);
            }
        }
    }
}

fn query_tab_from_saved(query: SavedQuery, id: Uuid) -> QueryTab {
    let mut tab = QueryTab::new(1);
    tab.id = id;
    tab.name = query.name.clone();
    tab.sql = query.sql.clone();
    // Library snapshots are not file saves. Keep a separate file-dirty baseline.
    tab.saved_sql.clear();
    tab.target = query.target.clone();
    tab.saved_query = Some(query);
    tab
}

#[cfg(test)]
mod tests;
