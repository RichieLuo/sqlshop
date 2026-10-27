use super::*;

impl App {
    pub(super) fn workspace(&mut self, ui: &mut egui::Ui) {
        self.workspace_navigation(ui);
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                self.workspace_body(ui);
            });
    }

    pub(super) fn workspace_body(&mut self, ui: &mut egui::Ui) {
        if self.table_create.is_some() {
            self.workspace_content(ui);
            return;
        }
        if !self.browser_visible
            && self.tabs[self.active_tab].management_document()
            && self.table_create.is_none()
        {
            self.management_workspace(ui);
            return;
        }
        if !self.browser_visible
            && self.tabs[self.active_tab].definition.is_some()
            && self.table_create.is_none()
        {
            self.object_definition_workspace(ui);
            return;
        }
        if self.browser_visible && self.table_create.is_none() {
            self.object_browser_workspace(ui);
            return;
        }
        if self.tabs[self.active_tab].design.is_some() && self.table_create.is_none() {
            self.structure_workspace(ui);
            return;
        }
        if self.tabs[self.active_tab].table_target.is_none() || self.table_create.is_some() {
            self.workspace_content(ui);
            return;
        }
        if self.table_recovery_workspace(ui) {
            return;
        }
        // Reserve pagination before rendering content, including failed or empty queries.
        let (content, footer) = table_workspace_rects(ui.available_rect_before_wrap());
        ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(content));
            self.workspace_content(ui);
        });
        ui.scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(footer));
            self.table_pagination(ui);
        });
    }

    pub(super) fn workspace_content(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.spacing_mut().button_padding.y = 4.0;
        ui.spacing_mut().interact_size.y = 24.0;
        if self.table_create.is_some() {
            self.table_create_workspace(ui);
            return;
        }
        let busy = self.tab_busy(self.active_tab);
        let table_tab = self.tabs[self.active_tab].table_target.is_some();
        self.workspace_tools(ui);
        ui.add_space(4.0);
        let show_results = table_tab
            || self.tabs[self.active_tab].query_started
            || self.tabs[self.active_tab].output.is_some();
        if !table_tab {
            let columns = self.completion_columns();
            let tables = self.semantic_completion_tables();
            let engine = self.tabs[self.active_tab]
                .target
                .as_ref()
                .and_then(|target| self.sessions.get(&target.connection))
                .and_then(|session| session.connected.as_ref())
                .map_or(Engine::MySql, |profile| profile.engine);
            let (_, needs_columns) = sql_editor(
                ui,
                &mut self.tabs[self.active_tab],
                &mut self.editor_height,
                show_results,
                &columns,
                &tables,
                engine,
            );
            if needs_columns && self.tabs[self.active_tab].target.is_some() {
                self.prefetch_completion_columns();
            }
            if self.cell_editor.is_none()
                && self.form.is_none()
                && self.confirm_delete.is_none()
                && self.export_path.is_none()
                && self.confirm_rebind.is_none()
                && self.confirm_tab_action.is_none()
                && self.confirm_request.is_none()
                && !self.confirm_exit
                && !self.history_open
                && ui.is_enabled()
            {
                let current = ui.input_mut(|i| {
                    i.consume_key(
                        egui::Modifiers {
                            shift: true,
                            ..egui::Modifiers::COMMAND
                        },
                        egui::Key::Enter,
                    )
                });
                if current {
                    self.execute_scope(SqlExecutionScope::CurrentStatement);
                } else if ui
                    .input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter))
                {
                    self.execute();
                }
            }
        }
        if !show_results {
            return;
        }
        ui.add_space(4.0);
        let draft_actions = self.table_draft_action_state();
        let tab = &mut self.tabs[self.active_tab];
        if !table_tab {
            let can_locate_error = tab.last_executed_document.as_ref() == Some(&tab.sql)
                && tab.last_executed_char_start.is_some();
            let action = query_result_ui::show_results(
                ui,
                &mut tab.result_index,
                query_result_ui::QueryResultContext {
                    tab: tab.id,
                    output: tab.output.as_ref(),
                    error: tab.query_error.as_deref(),
                    diagnostic: tab.query_diagnostic.as_ref(),
                    elapsed_ms: tab.query_elapsed_ms,
                    busy,
                    can_locate_error,
                },
            );
            if let Some(offset) = action.locate_error.filter(|_| can_locate_error) {
                let offset = tab.last_executed_char_start.unwrap().saturating_add(offset);
                let start = char_to_byte_index(&tab.sql, offset);
                let end = char_to_byte_index(&tab.sql, offset.saturating_add(1));
                tab.select_range = Some(start..end);
            }
            if !action.show_grid {
                return;
            }
        }
        let Some(output) = &tab.output else {
            if let Some(message) = &tab.query_error {
                ui.colored_label(ui.visuals().error_fg_color, message);
                return;
            }
            ui.label(
                RichText::new(
                    if self
                        .sessions
                        .values()
                        .any(|s| s.running_tab == Some(tab.id))
                        || self.query_sessions.get(&tab.id).is_some_and(|s| s.busy)
                    {
                        "正在执行查询…"
                    } else {
                        "暂无结果"
                    },
                )
                .weak(),
            );
            return;
        };
        if table_tab && output.sets.len() > 1 {
            ui.horizontal(|ui| {
                for index in 0..output.sets.len() {
                    ui.selectable_value(&mut tab.result_index, index, format!("#{}", index + 1));
                }
            });
        }
        let Some(set) = output.sets.get(tab.result_index) else {
            ui.label("执行完成，无结果集");
            return;
        };
        if !table_tab {
            let mut column_layout = tab
                .grid_layouts
                .get(&tab.result_index)
                .cloned()
                .unwrap_or_default();
            let mut columns_changed = false;
            egui::ScrollArea::horizontal()
                .id_salt(("query-result-toolbar", tab.id))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.strong("查询结果");
                        ui.label(format!(
                            "{} 行 · {} 列 · {} ms",
                            set.rows.len(),
                            set.columns.len(),
                            output.elapsed_ms
                        ))
                        .on_hover_text(&tab.source);
                        if let Some(affected) = set.affected {
                            ui.label(RichText::new(format!("数据库报告行数: {affected}")).weak());
                        }
                        if !set.columns.is_empty() {
                            ui.menu_button("复制", |ui| result_copy_controls(ui, tab, true));
                            if ui.button("导出 CSV").clicked() {
                                self.export_path = Some(default_csv_export_path());
                            }
                            columns_changed = grid_columns_ui::controls(
                                ui,
                                set,
                                &mut column_layout,
                                self.cell_editor.is_none(),
                            );
                        }
                    });
                });
            if !set.columns.is_empty() {
                tab.grid_layouts.insert(tab.result_index, column_layout);
            }
            if columns_changed {
                tab.selected_cell = None;
                tab.cell_anchor = None;
            }
        }
        if set.truncated {
            ui.colored_label(
                Color32::from_rgb(235, 180, 80),
                "结果已截断，仅保留前 1000 行。",
            );
        }
        if set.columns.is_empty() {
            ui.label("执行完成，无行数据");
            return;
        }
        let context_matches = tab.table_target.as_ref().is_some_and(|target| {
            self.sessions.get(&target.connection).is_some_and(|s| {
                s.connected.is_some() && s.active_database.as_ref() == Some(&target.database)
            })
        });
        if let Some(data) = &tab.table_data {
            let reason = if tab.table_snapshot_stale {
                Some("表数据已过期；刷新成功前暂不能编辑、新增、删除或导入。")
            } else if !context_matches {
                Some("此表来自其他连接或数据库，选择对应数据库后可编辑。")
            } else if data.primary_keys.is_empty() {
                Some("已有记录只读：此表没有主键，无法准确定位修改；普通表仍可新增记录。")
            } else if !data.columns.iter().any(|column| column.editable) {
                Some("只读：视图、生成列或当前表类型不支持编辑。")
            } else {
                None
            };
            if let Some(reason) = reason {
                ui.label(RichText::new(reason).small().weak());
            }
            if let Some(notice) = &tab.table_write_notice {
                ui.label(RichText::new(format!("{notice} · 等待刷新确认最新数据")).small());
            }
        }
        grid_columns_ui::load_table_layout(tab, &self.table_grid_layouts);
        let (editor, grid_action) = result_grid_inline(
            ui,
            tab,
            context_matches && !busy && !tab.table_snapshot_stale,
            self.cell_editor.as_mut(),
            draft_actions,
        );
        grid_columns_ui::save_table_layout(tab, &mut self.table_grid_layouts);
        if let Some(editor) = editor {
            self.cell_editor = Some(editor);
        }
        if let Some(action) = grid_action {
            let tab = &self.tabs[self.active_tab];
            match action {
                GridAction::FinishCell { cancel, movement } => {
                    self.finish_inline_cell(cancel, movement)
                }
                GridAction::PasteValues(text) => {
                    match paste_grid_values(
                        &mut self.tabs[self.active_tab],
                        &text,
                        context_matches && !busy,
                    ) {
                        Ok(count) => {
                            self.status = format!("已暂存 {count} 格粘贴内容；提交后写入数据库");
                            self.error = false;
                        }
                        Err(message) => self.fail(message),
                    }
                }
                GridAction::SubmitDrafts => {
                    self.submit_staged_cells(self.active_tab);
                }
                GridAction::RevertDrafts => {
                    self.discard_staged_cells(self.active_tab);
                }
                GridAction::DraftError(message) => self.fail(message),
                GridAction::Browse(action) => self.apply_grid_browse(action),
                GridAction::ExportRows(rows) => {
                    self.export_rows = Some((tab.id, rows));
                    self.export_path = Some(default_csv_export_path());
                }
                GridAction::DeleteRows(rows) => {
                    if let (Some(target), Some(_table), Some(data)) =
                        (&tab.table_target, &tab.target, &tab.table_data)
                    {
                        let key_values = rows
                            .iter()
                            .filter_map(|index| data.keys.get(*index).cloned())
                            .collect();
                        self.delete_rows = Some((
                            tab.id,
                            target.clone(),
                            data.primary_keys.clone(),
                            key_values,
                        ));
                    }
                }
            }
        }
    }

    pub(super) fn table_create_workspace(&mut self, ui: &mut egui::Ui) {
        let connection = self.table_create.as_ref().map(|form| form.connection);
        let busy = connection.is_some_and(|id| self.session_busy(id))
            || self.table_create_pending.is_some();
        let Some(form) = self.table_create.as_mut() else {
            return;
        };
        let mut cancel = false;
        let mut create = false;
        let mut remove_column = None;
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.heading("新建数据表");
            ui.label(RichText::new(format!("{}  /  {}", form.database, form.schema)).weak());
        });
        ui.add_space(8.0);
        ui.label("表名称");
        ui.add(
            egui::TextEdit::singleline(&mut form.name)
                .hint_text("例如 users")
                .desired_width(360.0),
        );
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.heading("字段定义");
        });
        ui.add_space(6.0);
        let column_area_height = (ui.available_height() - 72.0).max(140.0);
        egui::ScrollArea::both()
            .id_salt("table-create-columns")
            .max_height(column_area_height)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_sized([150.0, 26.0], egui::Label::new("字段名"));
                    ui.add_sized([165.0, 26.0], egui::Label::new("数据类型"));
                    ui.add_sized([100.0, 26.0], egui::Label::new("长度 / 精度"));
                    ui.add_sized([70.0, 26.0], egui::Label::new("可空"));
                    ui.add_sized([70.0, 26.0], egui::Label::new("主键"));
                    ui.add_sized([70.0, 26.0], egui::Label::new("自增"));
                    ui.add_sized([180.0, 26.0], egui::Label::new("默认值"));
                    ui.add_sized([50.0, 26.0], egui::Label::new("操作"));
                });
                ui.separator();
                for (index, column) in form.columns.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.add_sized(
                                [150.0, 26.0],
                                egui::TextEdit::singleline(&mut column.name)
                                    .hint_text("column_name"),
                            );
                        });
                        egui::ComboBox::from_id_salt(("column-type", index))
                            .width(165.0)
                            .selected_text(&column.data_type)
                            .show_ui(ui, |ui| {
                                for data_type in column_type_options(form.engine) {
                                    ui.selectable_value(
                                        &mut column.data_type,
                                        (*data_type).to_owned(),
                                        *data_type,
                                    );
                                }
                            });
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.add_sized(
                                [100.0, 26.0],
                                egui::TextEdit::singleline(&mut column.size).hint_text("可选"),
                            );
                        });
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.add_sized(
                                [70.0, 26.0],
                                egui::Checkbox::new(&mut column.nullable, ""),
                            );
                        });
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.add_sized(
                                [70.0, 26.0],
                                egui::Checkbox::new(&mut column.primary_key, ""),
                            );
                        });
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.add_sized(
                                [70.0, 26.0],
                                egui::Checkbox::new(&mut column.auto_increment, ""),
                            );
                        });
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.add_sized(
                                [180.0, 26.0],
                                egui::TextEdit::singleline(&mut column.default_value)
                                    .hint_text("如 0 或 CURRENT_TIMESTAMP"),
                            );
                        });
                        if ui
                            .add_enabled_ui(!busy, |ui| {
                                ui.add_sized([50.0, 26.0], egui::Button::new("移除"))
                            })
                            .inner
                            .clicked()
                        {
                            remove_column = Some(index);
                        }
                    });
                    ui.add_space(4.0);
                }
                if let Some(index) = remove_column.take() {
                    form.columns.remove(index);
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("＋ 添加字段"))
                    .clicked()
                {
                    form.columns.push(CreateTableColumn {
                        name: String::new(),
                        data_type: match form.engine {
                            Engine::MySql => "VARCHAR",
                            Engine::Postgres => "VARCHAR",
                            Engine::SqlServer => "NVARCHAR",
                        }
                        .into(),
                        nullable: true,
                        ..Default::default()
                    });
                }
            });
        ui.add_space(16.0);
        ui.separator();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            create = ui
                .add_enabled(
                    !busy && !form.name.trim().is_empty() && !form.columns.is_empty(),
                    egui::Button::new(RichText::new("创建数据表").color(Color32::WHITE))
                        .fill(accent(ui)),
                )
                .on_hover_text("保存当前字段定义并在数据库中创建表")
                .clicked();
            cancel = ui.add_enabled(!busy, egui::Button::new("取消")).clicked();
            if busy {
                ui.label(RichText::new("正在创建数据表…").weak());
            }
        });
        if cancel {
            self.table_create = None;
        } else if create {
            let form = self.table_create.take().expect("table form still set");
            let connection = form.connection;
            self.table_create = Some(form);
            let form = self.table_create.as_ref().expect("table form restored");
            let request = Request::CreateTable {
                database: form.database.clone(),
                schema: form.schema.clone(),
                name: form.name.trim().to_owned(),
                columns: form.columns.clone(),
            };
            self.table_create_pending = Some(connection);
            self.request_on(connection, request, "正在新建数据表…");
        }
    }
}
