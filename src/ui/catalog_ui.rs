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

    fn new_connection(&mut self) {
        self.form = Some(ConnectionForm {
            profile: Profile::default(),
            password: String::new(),
            ssh_secret: String::new(),
        });
    }

    fn save_to_library(&mut self, id: Option<Uuid>) {
        let tab = &self.tabs[self.active_tab];
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
        });
    }

    pub(super) fn command_bar(&mut self, ctx: &egui::Context) {
        let available = !self.modal_open();
        let selected_ready = self
            .selected_session()
            .is_some_and(|s| s.connected.is_some() && !s.busy);
        let selected_database = self
            .selected_session()
            .is_some_and(|s| s.active_database.is_some());
        let writable = self.catalog_store.is_some();
        let sql_tab = !self.browser_visible
            && !self.tabs[self.active_tab].management_document()
            && self.table_create.is_none()
            && (self.tabs[self.active_tab].table_target.is_none()
                && self.tabs[self.active_tab].definition.is_none());
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
                            if ui.button("打开 SQL 文件…  Ctrl+O").clicked() {
                                self.open_sql();
                                ui.close();
                            }
                            if ui
                                .add_enabled(sql_tab, egui::Button::new("保存 SQL 文件…  Ctrl+S"))
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
                                self.request(Request::Disconnect, "正在断开连接…");
                                ui.close();
                            }
                            if ui
                                .add_enabled(selected_ready, egui::Button::new("刷新数据库列表"))
                                .clicked()
                            {
                                self.request(Request::Databases, "正在刷新数据库列表…");
                                ui.close();
                            }
                            if ui
                                .add_enabled(
                                    selected_database && selected_ready,
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
                                    egui::Button::new("保存到查询库…"),
                                )
                                .clicked()
                            {
                                self.save_to_library(None);
                                ui.close();
                            }
                            if ui.button("已保存查询 / 连接分组…").clicked() {
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
                        {
                            self.request(Request::Disconnect, "正在断开连接…");
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
                            .on_hover_text("命名保存到本机查询库，与 SQL 文件 / 自动草稿独立")
                            .clicked()
                        {
                            self.save_to_library(None);
                        }
                        if ui.button("查询库").clicked() {
                            self.library_open = true;
                        }
                        ui.separator();
                        if ui
                            .add_enabled(selected_ready, egui::Button::new("刷新目录"))
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
        // Always create a new editor, never overwrite a dirty draft, connect or run.
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
            dialog_window("已保存查询与连接分组", ctx)
                .open(&mut open).default_width(740.0).default_height(480.0)
                .show(ctx, |ui| {
                    ui.add_enabled_ui(self.catalog_dialog.is_none(), |ui| {
                        ui.label("命名查询独立于草稿和历史；SQL 明文保存在本机，可能含敏感信息。");
                        if self.catalog_store.is_none() { ui.colored_label(Color32::from_rgb(230, 130, 90), "查询库不可用，修改已禁用；请检查状态栏错误并重启。"); }
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.library_search).hint_text("搜索查询名称 / SQL"));
                            if ui.add_enabled(self.catalog_store.is_some() && (self.tabs[self.active_tab].table_target.is_none() && self.tabs[self.active_tab].definition.is_none()), egui::Button::new("保存当前查询…")).clicked() { self.save_to_library(None); }
                        });
                        let needle = self.library_search.to_lowercase();
                        egui::ScrollArea::vertical().id_salt("saved-query-list").max_height(280.0).show(ui, |ui| {
                            if self.catalog.queries.is_empty() { ui.label("还没有命名查询。先编写 SQL，再点击“保存当前查询”。"); }
                            for query in self.catalog.queries.clone() {
                                if !format!("{} {}", query.name, query.sql).to_lowercase().contains(&needle) { continue; }
                                ui.push_id(query.id, |ui| { ui.group(|ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.strong(&query.name);
                                        if ui.button("打开").clicked() { selected_query = Some(query.clone()); }
                                        ui.add_enabled_ui(self.catalog_store.is_some(), |ui| {
                                            if ui.button("重命名…").clicked() { self.catalog_dialog = Some(CatalogDialog::Query { id: Some(query.id), name: query.name.clone(), sql: query.sql.clone(), target: query.target.clone() }); }
                                            if ui.add_enabled(self.tabs[self.active_tab].table_target.is_none() && self.tabs[self.active_tab].definition.is_none(), egui::Button::new("用当前查询更新…")).on_hover_text("确认后覆盖此命名查询的 SQL 和执行目标，不修改 SQL 文件").clicked() { self.save_to_library(Some(query.id)); }
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
                            ui.strong("连接分组");
                            if ui.add_enabled(self.catalog_store.is_some(), egui::Button::new("新建分组…")).clicked() { self.catalog_dialog = Some(CatalogDialog::Group { id: None, name: String::new() }); }
                        });
                        ui.label("右键侧栏连接 → 移动到分组。删除分组只会将连接移到“未分组”。");
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
                            ui.label("只保存快照，不执行；不会更改 SQL 文件或其未保存状态。");
                        }
                        CatalogDialog::DeleteGroup(_, name) => {
                            ui.label(format!("删除分组“{name}”？"));
                            ui.label("组内连接移入“未分组”，连接配置及凭据不会删除。");
                        }
                        CatalogDialog::DeleteQuery(_, name) => {
                            ui.label(format!("删除命名查询“{name}”？此操作不可撤销。"));
                            ui.label("已打开的标签、SQL 文件和历史不会删除。");
                        }
                    }
                    ui.horizontal(|ui| {
                        cancel = ui.button("取消").clicked();
                        confirm = ui
                            .add_enabled(
                                self.catalog_store.is_some(),
                                egui::Button::new("确认保存 / 删除"),
                            )
                            .clicked();
                    });
                });
            let mut completed = false;
            if confirm {
                let mut catalog = self.catalog.clone();
                let result = match &dialog {
                    CatalogDialog::Group { id, name } => catalog.save_group(*id, name).map(|_| ()),
                    CatalogDialog::Query {
                        id,
                        name,
                        sql,
                        target,
                    } => catalog
                        .save_query(*id, name, sql.clone(), target.clone())
                        .map(|_| ()),
                    CatalogDialog::DeleteGroup(id, _) => {
                        catalog.delete_group(*id);
                        Ok(())
                    }
                    CatalogDialog::DeleteQuery(id, _) => {
                        catalog.queries.retain(|q| q.id != *id);
                        Ok(())
                    }
                };
                match result {
                    Ok(()) => completed = self.commit_catalog(catalog),
                    Err(error) => self.fail(error.to_string()),
                }
            }
            if !cancel && !completed {
                self.catalog_dialog = Some(dialog);
            }
        }
    }
}

fn query_tab_from_saved(query: SavedQuery, id: Uuid) -> QueryTab {
    let mut tab = QueryTab::new(1);
    tab.id = id;
    tab.name = query.name;
    tab.sql = query.sql;
    // Library snapshots are not file saves. Keep a separate file-dirty baseline.
    tab.saved_sql.clear();
    tab.target = query.target;
    tab
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(tab.dirty());
        assert!(tab.selection.is_empty());
    }
}
