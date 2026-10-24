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
            || self.confirm_sql.is_some()
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
        let tab = &self.tabs[self.active_tab];
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
                            if ui.button("关闭当前标签").clicked() {
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
                            if ui.button("已保存查询…").clicked() {
                                self.library_open = true;
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
                ui.spacing_mut().button_padding = egui::vec2(10.0, 5.0);
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 4.0);
                ui.spacing_mut().interact_size.y = 30.0;
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
                        if ui.button("已保存查询").clicked() {
                            self.library_open = true;
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

    pub(super) fn saved_queries_sidebar(&mut self, ui: &mut egui::Ui) {
        let mut selected = None;
        egui::CollapsingHeader::new(format!("已保存查询 ({})", self.catalog.queries.len()))
            .id_salt("saved-queries-sidebar")
            .default_open(true)
            .show(ui, |ui| {
                if ui.button("打开查询库…").clicked() {
                    self.library_open = true;
                }
                if self.catalog.queries.is_empty() {
                    ui.label(
                        RichText::new("在查询页点击“保存查询”即可添加")
                            .small()
                            .weak(),
                    );
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("saved-queries-sidebar-list")
                        .max_height(140.0)
                        .show(ui, |ui| {
                            for query in &self.catalog.queries {
                                let active = !self.browser_visible
                                    && self.tabs[self.active_tab].saved_query.as_ref()
                                        == Some(query);
                                if ui
                                    .selectable_label(active, &query.name)
                                    .on_hover_text("打开命名查询，保留执行目标")
                                    .clicked()
                                {
                                    selected = Some(query.clone());
                                }
                            }
                        });
                }
            });
        if let Some(query) = selected {
            self.open_saved_query(query);
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
            let mut selected_query = None;
            dialog_window("已保存查询", ctx)
                .open(&mut open).default_width(740.0).default_height(480.0)
                .show(ctx, |ui| {
                    ui.add_enabled_ui(self.catalog_dialog.is_none(), |ui| {
                        if self.catalog_store.is_none() { ui.colored_label(Color32::from_rgb(230, 130, 90), "查询库不可用，修改已禁用；请检查状态栏错误并重启。"); }
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.library_search).hint_text("搜索查询名称 / SQL"));
                            if ui.add_enabled(self.catalog_store.is_some() && self.can_save_query(), egui::Button::new("保存当前查询…")).on_hover_text("命名查询独立于草稿和历史；SQL 以明文保存在本机，可能含敏感信息").clicked() { self.save_to_library(None); }
                        });
                        let needle = self.library_search.to_lowercase();
                        egui::ScrollArea::vertical().id_salt("saved-query-list").max_height(280.0).show(ui, |ui| {
                            if self.catalog.queries.is_empty() { ui.label("暂无命名查询"); }
                            for query in self.catalog.queries.clone() {
                                if !format!("{} {}", query.name, query.sql).to_lowercase().contains(&needle) { continue; }
                                ui.push_id(query.id, |ui| { ui.group(|ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.strong(&query.name);
                                        if ui.button("打开").clicked() { selected_query = Some(query.clone()); }
                                        ui.add_enabled_ui(self.catalog_store.is_some(), |ui| {
                                            if ui.button("重命名…").clicked() { self.catalog_dialog = Some(CatalogDialog::Query { id: Some(query.id), name: query.name.clone(), sql: query.sql.clone(), target: query.target.clone(), source_tab: None, expected: Some(Box::new(query.clone())) }); }
                                            if ui.add_enabled(self.can_save_query(), egui::Button::new("用当前查询更新…")).on_hover_text("确认后覆盖此命名查询的 SQL 和执行目标，不修改 SQL 文件").clicked() { self.save_to_library(Some(query.id)); }
                                            if ui.button("删除…").clicked() { self.catalog_dialog = Some(CatalogDialog::DeleteQuery(query.id, query.name.clone())); }
                                        });
                                    });
                                    let target = query.target.as_ref().map(|target| {
                                        let connection = self.profiles.iter().find(|p| p.id == target.connection).map(|p| p.name.clone()).unwrap_or_else(|| format!("已删除 / 不可用连接 {}", target.connection));
                                        format!("{} / {}", connection, target.database.as_deref().unwrap_or("服务器"))
                                    }).unwrap_or_else(|| "未绑定（首次执行时绑定当前连接）".into());
                                    ui.label(RichText::new(target).small().weak());
                                    ui.label(RichText::new(query.sql.chars().take(150).collect::<String>()).monospace().small());
                                }); });
                            }
                        });
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.strong("连接分组").on_hover_text("右键侧栏连接可移动到分组");
                            if ui.add_enabled(self.catalog_store.is_some(), egui::Button::new("新建分组…")).clicked() { self.catalog_dialog = Some(CatalogDialog::Group { id: None, name: String::new() }); }
                        });
                        egui::ScrollArea::vertical().id_salt("group-list").max_height(150.0).show(ui, |ui| {
                            for group in self.catalog.groups.clone() {
                                ui.push_id(group.id, |ui| { ui.horizontal(|ui| {
                                    ui.label(&group.name);
                                    ui.add_enabled_ui(self.catalog_store.is_some(), |ui| {
                                        if ui.button("重命名…").clicked() { self.catalog_dialog = Some(CatalogDialog::Group { id: Some(group.id), name: group.name.clone() }); }
                                        if ui.button("删除分组…").clicked() { self.catalog_dialog = Some(CatalogDialog::DeleteGroup(group.id, group.name.clone())); }
                                    });
                                }); });
                            }
                        });
                    });
                });
            self.library_open = open;
            if let Some(query) = selected_query {
                self.open_saved_query(query);
                self.library_open = false;
            }
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
                            ui.text_edit_singleline(name);
                        }
                        CatalogDialog::Query {
                            id, name, target, ..
                        } => {
                            ui.label(if id.is_some() {
                                "更新已保存查询（将替换该条目的名称、SQL 和目标）"
                            } else {
                                "新建命名查询（不会覆盖同名条目）"
                            });
                            ui.text_edit_singleline(name);
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
                    ui.horizontal(|ui| {
                        cancel = ui.button("取消").clicked();
                        confirm = ui
                            .add_enabled(
                                self.catalog_store.is_some(),
                                egui::Button::new("确认保存 / 删除"),
                            )
                            .on_hover_text("仅修改本机查询库，不执行 SQL，也不更改 SQL 文件")
                            .clicked();
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
mod tests {
    use super::*;

    fn library_app(directory: &std::path::Path) -> App {
        let mut app = super::super::tests::session_test_app();
        app.browser_visible = false;
        app.catalog_store = Some(CatalogStore::new(directory.into()));
        app
    }

    fn save_new_query(app: &mut App, name: &str) -> SavedQuery {
        app.save_query_command(false);
        let mut dialog = app
            .catalog_dialog
            .take()
            .expect("name dialog for new query");
        let CatalogDialog::Query { name: title, .. } = &mut dialog else {
            panic!("query dialog")
        };
        *title = name.into();
        assert!(app.apply_catalog_dialog(&dialog));
        app.catalog.queries.last().unwrap().clone()
    }

    #[test]
    fn named_query_save_update_and_restart_preserve_identity_sql_and_target() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = library_app(dir.path());
        let target = Some(QueryTarget {
            connection: Uuid::new_v4(),
            database: Some("业务库".into()),
            schema: Some("sales".into()),
        });
        app.tabs[0].sql = "SELECT '查询内容';".into();
        app.tabs[0].target = target.clone();
        let original = save_new_query(&mut app, "每日报表");
        assert!(!app.tabs[0].dirty());
        assert!(app.tabs[0].saved_sql.is_empty() && app.tabs[0].file_path.is_none());
        app.tabs[0].sql = "SELECT '更新内容';".into();
        assert!(app.tabs[0].dirty());
        app.save_query_command(false);
        assert!(app.catalog_dialog.is_none());
        assert_eq!(app.catalog.queries.len(), 1);
        assert_eq!(app.catalog.queries[0].id, original.id);
        assert!(!app.tabs[0].dirty());
        let stored = CatalogStore::new(dir.path().into()).load().unwrap();
        assert_eq!(stored.queries[0].sql, "SELECT '更新内容';");
        assert_eq!(stored.queries[0].target, target);

        WorkspaceStore::new(dir.path().into())
            .save(&app.workspace_snapshot())
            .unwrap();
        let mut restored = library_app(dir.path());
        restored.store = Some(Store {
            directory: dir.path().into(),
        });
        restored.restore_workspace();
        restored.restore_catalog();
        assert_eq!(
            restored.tabs[0].saved_query.as_ref().unwrap().id,
            original.id
        );
        assert_eq!(restored.tabs[0].target, target);
        assert!(!restored.tabs[0].dirty());
        assert!(restored.query_sessions.is_empty());
        assert!(!restored.tabs[0].query_started);
        restored.tabs[0].sql = "SELECT '重启后修改';".into();
        restored.save_query_command(false);
        assert_eq!(restored.catalog.queries.len(), 1);
        assert_eq!(restored.catalog.queries[0].sql, "SELECT '重启后修改';");
    }

    #[test]
    fn opening_named_query_reuses_its_editor_and_keeps_unsaved_changes() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = library_app(dir.path());
        app.tabs[0].sql = "SELECT 1".into();
        let saved = save_new_query(&mut app, "常用查询");
        app.tabs[0].sql = "SELECT 2".into();
        app.open_saved_query(saved);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].sql, "SELECT 2");
        assert!(app.tabs[0].dirty());
        assert!(app.query_sessions.is_empty());
    }

    #[test]
    fn opening_updated_query_uses_current_contents_and_keeps_old_target_as_a_draft() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = library_app(dir.path());
        app.tabs[0].sql = "SELECT 'old'".into();
        app.tabs[0].target = Some(QueryTarget {
            connection: Uuid::new_v4(),
            database: Some("old_db".into()),
            schema: None,
        });
        let old_target = app.tabs[0].target.clone();
        let saved = save_new_query(&mut app, "目标保护");
        let mut newer = QueryTab::new(2);
        newer.sql = "SELECT 'new'".into();
        newer.target = Some(QueryTarget {
            connection: Uuid::new_v4(),
            database: Some("new_db".into()),
            schema: None,
        });
        app.tabs.push(newer);
        app.active_tab = 1;
        app.save_to_library(Some(saved.id));
        let dialog = app.catalog_dialog.take().unwrap();
        assert!(app.apply_catalog_dialog(&dialog));
        assert!(app.tabs[0].saved_query_stale && app.tabs[0].dirty());
        assert_eq!(app.tabs[0].target, old_target);
        app.active_tab = 0;
        app.open_saved_query(app.catalog.queries[0].clone());
        assert_eq!(app.active_tab, 1);
        assert_eq!(app.tabs[1].sql, "SELECT 'new'");
        assert_eq!(
            app.tabs[1].target.as_ref().unwrap().database.as_deref(),
            Some("new_db")
        );
        assert_eq!(app.tabs[0].sql, "SELECT 'old'");
    }

    #[test]
    fn file_save_shortcut_writes_the_file_even_when_also_saved_in_the_library() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = library_app(dir.path());
        app.tabs[0].sql = "SELECT 1".into();
        let named = save_new_query(&mut app, "文件和查询");
        let file = dir.path().join("query.sql");
        app.tabs[0].file_path = Some(file.clone());
        app.tabs[0].sql = "SELECT 2".into();
        app.save_query_command(false);
        assert_eq!(std::fs::read_to_string(file).unwrap(), "SELECT 2");
        assert_eq!(app.catalog.queries[0], named);
        assert!(!app.tabs[0].dirty());
    }

    #[test]
    fn editor_save_shortcut_opens_naming_for_an_untitled_query() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = library_app(dir.path());
        app.tabs[0].sql = "SELECT 1".into();
        let ctx = app.context.clone();
        let modifiers = egui::Modifiers::COMMAND;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 800.0),
            )),
            modifiers,
            events: vec![egui::Event::Key {
                key: egui::Key::S,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.workspace_tools(ui));
        });
        assert!(matches!(
            app.catalog_dialog,
            Some(CatalogDialog::Query { id: None, .. })
        ));
        assert!(app.catalog.queries.is_empty());
        assert!(!app.tabs[0].query_started);
    }

    #[test]
    fn stale_named_query_never_overwrites_another_editors_newer_save() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = library_app(dir.path());
        app.tabs[0].sql = "SELECT 1".into();
        let saved = save_new_query(&mut app, "并发编辑");
        app.tabs.push(query_tab_from_saved(saved, Uuid::new_v4()));
        app.tabs[0].sql = "SELECT 2".into();
        app.save_query_command(false);
        app.active_tab = 1;
        app.tabs[1].sql = "SELECT 3".into();
        app.save_query_command(false);
        assert!(app.error);
        assert_eq!(app.catalog.queries[0].sql, "SELECT 2");
        assert_eq!(
            CatalogStore::new(dir.path().into()).load().unwrap().queries[0].sql,
            "SELECT 2"
        );
        assert_eq!(app.tabs[1].sql, "SELECT 3");
        assert!(app.tabs[1].dirty());
        app.save_query_command(true);
        let mut dialog = app.catalog_dialog.take().unwrap();
        let CatalogDialog::Query { name, .. } = &mut dialog else {
            panic!()
        };
        *name = "并发编辑副本".into();
        assert!(app.apply_catalog_dialog(&dialog));
        assert_eq!(app.catalog.queries.len(), 2);
        assert!(!app.tabs[1].dirty());
    }

    #[test]
    fn save_dialog_binds_its_source_and_failed_write_keeps_the_draft_dirty() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = library_app(dir.path());
        app.tabs[0].sql = "SELECT 1".into();
        app.save_query_command(false);
        let dialog = app.catalog_dialog.take().unwrap();
        app.tabs.push(QueryTab::new(2));
        app.active_tab = 1;
        assert!(app.apply_catalog_dialog(&dialog));
        assert!(app.tabs[0].saved_query.is_some());
        assert!(app.tabs[1].saved_query.is_none());
        app.tabs[1].sql = "SELECT 2".into();
        app.save_query_command(false);
        let dialog = app.catalog_dialog.take().unwrap();
        std::fs::write(dir.path().join("catalog.json"), b"{corrupt").unwrap();
        assert!(!app.apply_catalog_dialog(&dialog));
        assert_eq!(app.catalog.queries.len(), 1);
        assert!(app.tabs[1].saved_query.is_none());
        assert!(app.tabs[1].dirty());
    }

    #[test]
    fn library_save_never_marks_an_existing_sql_file_as_saved() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = library_app(dir.path());
        app.tabs[0].sql = "SELECT 2".into();
        app.tabs[0].saved_sql = "SELECT 1".into();
        app.tabs[0].file_path = Some(dir.path().join("query.sql"));
        app.save_to_library(None);
        let dialog = app.catalog_dialog.take().unwrap();
        assert!(app.apply_catalog_dialog(&dialog));
        assert!(app.tabs[0].dirty());
        assert_eq!(app.tabs[0].saved_sql, "SELECT 1");
        assert!(!dir.path().join("query.sql").exists());
    }

    #[test]
    fn library_open_preserves_missing_target_without_executing_or_claiming_a_file_save() {
        let target = Some(QueryTarget {
            schema: None,
            connection: Uuid::new_v4(),
            database: Some("生产".into()),
        });
        let tab = query_tab_from_saved(
            SavedQuery {
                id: Uuid::new_v4(),
                name: "危险查询".into(),
                sql: "DELETE FROM items".into(),
                target: target.clone(),
            },
            Uuid::new_v4(),
        );
        assert_eq!(tab.target, target);
        assert!(!tab.query_started);
        assert!(tab.output.is_none() && tab.file_path.is_none() && tab.table_target.is_none());
        assert!(!tab.dirty());
        assert!(tab.saved_sql.is_empty());
        assert!(tab.selection.is_empty());
    }
}
