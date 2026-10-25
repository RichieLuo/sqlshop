use super::*;

impl App {
    pub(in crate::ui) fn workspace_tools(&mut self, ui: &mut egui::Ui) {
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
                if ui.button("保存查询").clicked() {
                    self.save_query_command(false);
                }
                if ui.button("已保存查询").clicked() {
                    self.library_open = true;
                }
                if ui.button("保存 SQL 文件").clicked() {
                    self.save_sql(false);
                }
                if ui.button("SQL 文件另存为…").clicked() {
                    self.save_sql(true);
                }
            });
            if ui.button("查找 / 替换").clicked() {
                self.tabs[self.active_tab].find_open ^= true;
            }
            if ui
                .button("查询历史")
                .on_hover_text("历史 SQL 以明文保存在本机，可能含敏感信息；恢复到编辑器不执行")
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
        if self.tabs[self.active_tab].saved_query_stale {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "查询库已有新版；此标签保留旧内容和目标。可从侧栏打开新版，或另存为查询。",
            );
        }
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
            if ui.input_mut(|i| {
                i.consume_key(
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                    egui::Key::S,
                )
            }) {
                self.save_query_command(true);
            } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) {
                self.save_query_command(false);
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::O)) {
                self.open_sql();
            }
        }
        find_controls(ui, &mut self.tabs[self.active_tab]);
    }

    pub(in crate::ui) fn select_query_target(&mut self, target: QueryTarget) {
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

    pub(super) fn query_target_controls(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn query_execution_controls(&mut self, ui: &mut egui::Ui) {
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

    pub(in crate::ui) fn executed_sql_footer(&self, ui: &mut egui::Ui) {
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
}
