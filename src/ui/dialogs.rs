use super::*;

impl App {
    pub(super) fn finish_inline_cell(&mut self, cancel: bool, movement: CellMovement) {
        let Some(editor) = self.cell_editor.take() else {
            return;
        };
        if cancel {
            return;
        }
        let valid = self
            .sessions
            .get(&editor.connection)
            .is_some_and(|session| {
                session.connected.is_some()
                    && !session.busy
                    && session.active_database.as_ref() == Some(&editor.change.database)
            });
        let Some(index) = self.tabs.iter().position(|tab| tab.id == editor.tab) else {
            return;
        };
        if !valid || self.tabs[index].table_snapshot_stale {
            self.cell_editor = Some(editor);
            self.fail("表快照或连接已变化，输入已保留；取消编辑后刷新。".into());
            return;
        }
        let tab = &mut self.tabs[index];
        let location = tab.table_data.as_ref().and_then(|data| {
            let row = data
                .keys
                .iter()
                .position(|keys| keys == &editor.change.key_values)?;
            let column = data
                .columns
                .iter()
                .position(|column| column.name == editor.change.column)?;
            Some((row, column))
        });
        let mut change = editor.change;
        change.value = if editor.is_null {
            None
        } else {
            Some(editor.text)
        };
        tab.stage_cell(change);
        self.status = format!("已暂存 {} 格；提交后写入数据库", tab.staged_cells.len());
        if let Some((row, column)) = location {
            let Some(set) = tab
                .output
                .as_ref()
                .and_then(|output| output.sets.get(tab.result_index))
            else {
                return;
            };
            let visible = grid_columns_ui::tab_visible(tab, set.columns.len());
            let columns = visible.len();
            let count = set.rows.len().saturating_mul(columns);
            let current = row * columns
                + visible
                    .iter()
                    .position(|&index| index == column)
                    .unwrap_or(0);
            let next = match movement {
                CellMovement::Stay => None,
                CellMovement::Across(step) => {
                    let mut position = current as i64 + i64::from(step);
                    let mut found = None;
                    while position >= 0 && position < count as i64 {
                        let pair = (
                            position as usize / columns,
                            visible[position as usize % columns],
                        );
                        if grid_cell_editor(tab, pair.0, pair.1, true).is_some() {
                            found = Some(pair);
                            break;
                        }
                        position += i64::from(step);
                    }
                    found
                }
                CellMovement::Down(step) => {
                    let next_row = row as i64 + i64::from(step);
                    (next_row >= 0 && next_row < set.rows.len() as i64)
                        .then_some((next_row as usize, column))
                }
            };
            tab.selected_cell = next.or(Some((row, column)));
            tab.cell_anchor = tab.selected_cell;
            tab.selected_rows = None;
            if let Some((row, column)) = next {
                self.cell_editor = grid_cell_editor(tab, row, column, true);
            }
        }
    }

    pub(super) fn cell_editor_window(&mut self, _ctx: &egui::Context) {
        // The grid owns the editor. Keep this lifecycle hook for pending snapshots.
        if self
            .cell_editor
            .as_ref()
            .is_some_and(|editor| !self.tabs.iter().any(|tab| tab.id == editor.tab))
        {
            self.cell_editor = None;
        }
    }

    pub(super) fn connection_window(&mut self, ctx: &egui::Context) {
        let busy = self.test_busy
            || self.redis_test_worker.is_some()
            || self.form.as_ref().is_some_and(|form| match form.kind {
                ConnectionKind::Sql(_) => self.session_busy(form.id()),
                ConnectionKind::Redis => self.redis_document(form.id()).is_some_and(|doc| doc.busy),
            });
        let Some(form) = &mut self.form else { return };
        let redis_save_error = self.redis_config_error.clone();
        let mut open = true;
        let mut save = false;
        let mut connect = false;
        let mut test = false;
        let mut cancel = false;
        let mut selection_error = None;
        dialog_window("数据库连接", ctx)
            .open(&mut open)
            .resizable(true)
            .default_width((ctx.content_rect().width() - 120.0).clamp(620.0, 940.0))
            .min_width(620.0)
            .show(ctx, |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    connection_heading(ui, form);
                    ui.separator();
                    ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
                    egui::ScrollArea::vertical()
                        .id_salt("connection-fields")
                        .scroll_bar_visibility(
                            egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                        )
                        .max_height((ctx.content_rect().height() - 340.0).max(180.0))
                        .show(ui, |ui| {
                            if ui.available_width() >= 820.0 {
                                ui.columns(2, |columns| {
                                    columns[0].group(|ui| database_fields(ui, form));
                                    columns[1].group(|ui| {
                                        selection_error = connection_ssh_fields(ui, form);
                                    });
                                });
                            } else {
                                ui.group(|ui| database_fields(ui, form));
                                ui.add_space(10.0);
                                ui.group(|ui| {
                                    selection_error = connection_ssh_fields(ui, form);
                                });
                            }
                            ui.add_space(10.0);
                            ui.group(|ui| {
                                ui.set_min_width(ui.available_width());
                                ui.strong("数据库 TLS");
                                match form.kind {
                                    ConnectionKind::Sql(_) => {
                                        database_tls_settings(ui, &mut form.profile)
                                    }
                                    ConnectionKind::Redis => {
                                        ui.checkbox(&mut form.redis_profile.tls, "启用 TLS 加密")
                                            .on_hover_text(
                                                "校验证书和主机名；暂不支持 TLS + SSH。",
                                            );
                                        if form.redis_profile.tls && form.redis_profile.ssh.enabled
                                        {
                                            ui.colored_label(
                                                ui.visuals().error_fg_color,
                                                "TLS 与 SSH 不能同时启用，请关闭其中一项。",
                                            );
                                        }
                                    }
                                }
                            });
                        });
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(4.0);
                    let can_save = form.kind != ConnectionKind::Redis || redis_save_error.is_none();
                    if !can_save {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            redis_save_error.as_deref().unwrap_or_default(),
                        );
                    }
                    ui.horizontal(|ui| {
                        test = ui.button("测试连接").clicked();
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            connect = ui
                                .add_enabled(can_save, primary_button(ui, "保存并连接"))
                                .clicked();
                            save = ui
                                .add_enabled(can_save, egui::Button::new("保存"))
                                .clicked();
                            cancel = ui.button("取消").clicked();
                        });
                    });
                });
                if busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("正在处理…");
                    });
                }
                if self.error {
                    ui.colored_label(ui.visuals().error_fg_color, &self.status);
                } else {
                    ui.label(RichText::new(&self.status).small().weak());
                }
            });
        let kind = form.kind;
        let profile = form.profile.clone();
        let redis_profile = form.redis_profile.clone();
        let credentials = form.credentials();
        if !open || cancel {
            self.form = None;
        }
        if let Some(error) = selection_error {
            self.fail(error);
        }
        if test {
            match kind {
                ConnectionKind::Sql(_) => match profile.validate() {
                    Ok(()) => self.request(
                        Request::Test(profile.clone(), credentials.clone()),
                        "正在测试连接…",
                    ),
                    Err(error) => self.fail(error.to_string()),
                },
                ConnectionKind::Redis => {
                    if let Err(error) =
                        self.test_redis_connection(redis_profile.clone(), credentials.clone())
                    {
                        self.fail(format!("{error:#}"));
                    }
                }
            }
        }
        if save || connect {
            let id = match kind {
                ConnectionKind::Sql(_) => profile.id,
                ConnectionKind::Redis => redis_profile.id,
            };
            let result = match kind {
                ConnectionKind::Sql(_) => {
                    self.save_connection(profile.clone(), credentials.clone())
                }
                ConnectionKind::Redis => {
                    self.save_redis_connection(redis_profile, credentials.clone())
                }
            };
            match result {
                Ok(()) => {
                    self.form = None;
                    if connect {
                        match kind {
                            ConnectionKind::Sql(_) => self
                                .request(Request::Connect(profile, credentials), "正在连接数据库…"),
                            ConnectionKind::Redis => {
                                self.open_redis_connection(id, true);
                            }
                        }
                    }
                }
                Err(error) => self.fail(format!("{error:#}")),
            }
        }
    }

    pub(super) fn delete_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.confirm_delete else {
            return;
        };
        let redis = self.is_redis_connection(id);
        let busy = if redis {
            self.redis_document(id)
                .is_some_and(|doc| doc.busy || doc.is_dirty() || doc.has_pending_confirmation())
        } else {
            self.session_busy(id) || self.query_connection_busy(id)
        };
        let mut cancel = false;
        let mut delete = false;
        dialog_window("删除本机连接", ctx).show(ctx, |ui| {
            ui.label("删除本机连接及凭据，不删除服务器数据。");
            if redis {
                ui.label("将关闭此连接的 Redis 标签。");
            } else if self
                .sessions
                .get(&id)
                .is_some_and(|s| s.connected.is_some())
                || self.query_connection_used(id)
            {
                ui.label("将关闭此连接的所有会话，未提交事务可能回滚。");
            }
            dialog_actions(ui, |ui| {
                delete = ui
                    .add_enabled(!busy, danger_button(ui, "删除连接"))
                    .clicked();
                cancel = ui.button("取消").clicked();
            });
        });
        if cancel {
            self.confirm_delete = None;
        }
        if delete && redis {
            if let Err(error) = self.delete_redis_connection(id) {
                self.fail(format!("{error:#}"));
            }
            return;
        }
        if delete {
            let profiles: Vec<_> = self
                .profiles
                .iter()
                .filter(|p| p.id != id)
                .cloned()
                .collect();
            let result = (|| -> anyhow::Result<()> {
                let store = self
                    .store
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("配置目录不可用"))?;
                store.forget_credentials(id)?;
                store.save(&profiles)?;
                Ok(())
            })();
            match result {
                Ok(()) => {
                    self.close_query_sessions(id);
                    self.profiles = profiles;
                    self.credentials.remove(&id);
                    self.selected = self
                        .profiles
                        .first()
                        .map(|p| p.id)
                        .or_else(|| self.redis_profiles.first().map(|p| p.id));
                    self.tree_selection.selected = self.selected.map(TreeNode::Connection);
                    self.confirm_delete = None;
                    // Deletion explicitly closes only this connection's session.
                    self.sessions.remove(&id);
                }
                Err(error) => self.fail(format!("{error:#}")),
            }
        }
    }

    pub(super) fn delete_rows_window(&mut self, ctx: &egui::Context) {
        let Some((tab_id, target, primary_keys, key_values)) = self.delete_rows.clone() else {
            return;
        };
        let mut cancel = false;
        let mut confirm = false;
        let can_delete = !self.session_busy(target.connection)
            && self.cell_editor.is_none()
            && self
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .is_some_and(|tab| !tab.table_snapshot_stale && !tab.has_table_drafts());
        dialog_window("删除选中行", ctx).show(ctx, |ui| {
            ui.label(format!(
                "将从 {}.{} 删除 {} 行。此操作无法撤销。",
                target.schema,
                target.name,
                key_values.len()
            ));
            dialog_actions(ui, |ui| {
                confirm = ui
                    .add_enabled(can_delete, danger_button(ui, "删除行"))
                    .clicked();
                cancel = ui.button("取消").clicked();
            });
        });
        if cancel {
            self.delete_rows = None;
        }
        if confirm {
            let request = Request::DeleteRows {
                tab: tab_id,
                target: QueryTarget {
                    schema: None,
                    connection: target.connection,
                    database: Some(target.database.clone()),
                },
                table: Table {
                    schema: target.schema.clone(),
                    name: target.name.clone(),
                    kind: "BASE TABLE".into(),
                },
                primary_keys,
                key_values,
            };
            self.delete_rows = None;
            self.request_on(target.connection, request, "正在删除选中行…");
        }
    }

    pub(super) fn schema_dialog_window(&mut self, ctx: &egui::Context) {
        let rename_preview = self.schema_dialog.as_ref().and_then(|dialog| {
            let SchemaDialogAction::RenameTable {
                connection,
                database,
                table,
            } = &dialog.action
            else {
                return None;
            };
            Some(
                if let Some(message) = self.table_rename_rejection(*connection, database, table) {
                    Err(message)
                } else {
                    let session = &self.sessions[connection];
                    let engine = session
                        .connected
                        .as_ref()
                        .expect("connected rename target")
                        .engine;
                    let new_name = dialog.name.trim();
                    sqlshop::db::rename_table_sql(engine, table, new_name)
                        .map_err(|error| error.to_string())
                        .and_then(|sql| {
                            if session
                                .tables
                                .iter()
                                .any(|known| known.schema == table.schema && known.name == new_name)
                            {
                                Err("新表名称已存在".into())
                            } else {
                                Ok(sql)
                            }
                        })
                },
            )
        });
        let Some(dialog) = self.schema_dialog.as_mut() else {
            return;
        };
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;
        let title = match dialog.action {
            SchemaDialogAction::RenameTable { .. } => "重命名数据表",
            SchemaDialogAction::CreateDatabase { .. } => "新建数据库",
            SchemaDialogAction::DropDatabase { .. } => "删除数据库",
            SchemaDialogAction::DropTable { .. } => "删除数据表",
        };
        dialog_window(title, ctx).open(&mut open).show(ctx, |ui| {
            match &mut dialog.action {
                SchemaDialogAction::RenameTable {
                    database, table, ..
                } => {
                    ui.label(format!(
                        "当前数据表：{database}.{}.{}",
                        table.schema, table.name
                    ));
                    ui.label("新表名称");
                    ui.add(dialog_input(&mut dialog.name).desired_width(480.0));
                    match &rename_preview {
                        Some(Ok(sql)) => {
                            ui.label("将执行以下 SQL：");
                            ui.monospace(sql);
                        }
                        Some(Err(message)) => {
                            ui.colored_label(ui.visuals().warn_fg_color, message);
                        }
                        None => {}
                    }
                    ui.label("引用旧表名的 SQL、视图或存储过程可能需要更新。");
                }
                SchemaDialogAction::CreateDatabase { .. } => {
                    ui.label("数据库名称");
                    ui.add(dialog_input(&mut dialog.name).desired_width(360.0));
                }
                SchemaDialogAction::DropDatabase { database, .. } => {
                    ui.label(format!(
                        "确定删除数据库 {database}？其中的所有表和数据都会被删除，且无法撤销。"
                    ));
                }
                SchemaDialogAction::DropTable {
                    database, table, ..
                } => {
                    ui.label(format!(
                        "确定删除数据表 {database}.{}.{}？表内所有数据都会被删除，且无法撤销。",
                        table.schema, table.name
                    ));
                }
            }
            dialog_actions(ui, |ui| {
                let label = if matches!(dialog.action, SchemaDialogAction::RenameTable { .. }) {
                    "重命名"
                } else if matches!(
                    dialog.action,
                    SchemaDialogAction::DropDatabase { .. } | SchemaDialogAction::DropTable { .. }
                ) {
                    "删除"
                } else {
                    "创建"
                };
                confirm = ui
                    .add_enabled(
                        rename_preview
                            .as_ref()
                            .is_none_or(|preview| preview.is_ok()),
                        if label == "删除" {
                            danger_button(ui, label)
                        } else {
                            primary_button(ui, label)
                        },
                    )
                    .clicked();
                cancel = ui.button("取消").clicked();
            });
        });
        if cancel || !open {
            self.schema_dialog = None;
            return;
        }
        if !confirm {
            return;
        }
        let dialog = self.schema_dialog.take().expect("dialog still set");
        let (connection, request, status, created_database) = match dialog.action {
            SchemaDialogAction::RenameTable {
                connection,
                database,
                table,
            } => (
                connection,
                Request::RenameTable {
                    database,
                    table,
                    new_name: dialog.name.trim().into(),
                },
                "正在重命名数据表…",
                None,
            ),
            SchemaDialogAction::CreateDatabase { connection } => {
                let database = dialog.name.trim().to_owned();
                (
                    connection,
                    Request::CreateDatabase(database.clone()),
                    "正在新建数据库…",
                    Some(database),
                )
            }
            SchemaDialogAction::DropDatabase {
                connection,
                database,
            } => (
                connection,
                Request::DropDatabase(database),
                "正在删除数据库…",
                None,
            ),
            SchemaDialogAction::DropTable {
                connection,
                database,
                table,
            } => (
                connection,
                Request::DropTable { database, table },
                "正在删除数据表…",
                None,
            ),
        };
        if let Some(database) = created_database {
            self.database_create_pending = Some((connection, database));
        }
        self.request_on(connection, request, status);
    }

    pub(super) fn open_csv_import(&mut self) {
        let tab = &self.tabs[self.active_tab];
        if tab.table_snapshot_stale {
            self.fail("表数据已过期，请刷新后再导入".into());
            return;
        }
        let (Some(target), Some(table), Some(data)) = (
            tab.target.clone(),
            tab.table_target.as_ref(),
            tab.table_data.as_ref(),
        ) else {
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("CSV", &["csv"])
            .pick_file()
        else {
            return;
        };
        let result = (|| -> anyhow::Result<_> {
            let file = std::fs::File::open(&path)?;
            anyhow::ensure!(
                file.metadata()?.len() <= 64 * 1024 * 1024,
                "CSV 文件超过 64 MiB；请分批导入"
            );
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(
                &mut std::io::Read::take(file, 64 * 1024 * 1024 + 1),
                &mut bytes,
            )?;
            anyhow::ensure!(
                bytes.len() <= 64 * 1024 * 1024,
                "CSV 文件超过 64 MiB；请分批导入"
            );
            parse_nullable_csv(&bytes)
        })();
        match result {
            Ok((headers, rows)) => {
                let mapping = headers
                    .iter()
                    .map(|name| {
                        data.columns
                            .iter()
                            .find(|c| c.editable && c.name == *name)
                            .map(|c| c.name.clone())
                    })
                    .collect();
                self.csv_import = Some(CsvImportForm {
                    tab: tab.id,
                    target,
                    table: Table {
                        schema: table.schema.clone(),
                        name: table.name.clone(),
                        kind: "BASE TABLE".into(),
                    },
                    headers,
                    mapping,
                    rows,
                });
            }
            Err(error) => self.fail(format!("读取 CSV 失败：{error:#}")),
        }
    }

    pub(super) fn csv_import_window(&mut self, ctx: &egui::Context) {
        if let Some((connection, tab_id)) = self
            .csv_import
            .as_ref()
            .map(|form| (form.target.connection, form.tab))
        {
            let busy = self.session_busy(connection);
            let stale = self
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .is_none_or(|tab| tab.table_snapshot_stale);
            let available = self
                .tabs
                .iter()
                .find(|t| t.id == tab_id)
                .and_then(|t| t.table_data.as_ref())
                .map(|data| {
                    data.columns
                        .iter()
                        .filter(|c| c.editable)
                        .map(|c| c.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let form = self.csv_import.as_mut().unwrap();
            let mut open = true;
            let mut submit = false;
            dialog_window("CSV 导入 · 映射预览", ctx)
                .open(&mut open)
                .default_width(700.0)
                .show(ctx, |ui| {
                    ui.label(format!(
                        "{} / {}.{} · {} 行",
                        form.target.database.as_deref().unwrap_or(""),
                        form.table.schema,
                        form.table.name,
                        form.rows.len()
                    ));
                    ui.label("列映射").on_hover_text(
                        "未引号的空字段为 NULL，引号内空值为空字符串；未映射列使用默认值。",
                    );
                    for (index, header) in form.headers.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(format!("{header} →"));
                            egui::ComboBox::from_id_salt(("csv-map", index))
                                .selected_text(form.mapping[index].as_deref().unwrap_or("忽略列"))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut form.mapping[index], None, "忽略列");
                                    for name in &available {
                                        ui.selectable_value(
                                            &mut form.mapping[index],
                                            Some(name.clone()),
                                            name,
                                        );
                                    }
                                });
                        });
                    }
                    egui::ScrollArea::vertical()
                        .max_height(140.0)
                        .show(ui, |ui| {
                            for row in form.rows.iter().take(5) {
                                ui.label(format!("{:?}", row));
                            }
                        });
                    let valid = !form.rows.is_empty()
                        && form.mapping.iter().any(Option::is_some)
                        && form.mapping.iter().flatten().enumerate().all(|(i, name)| {
                            form.mapping
                                .iter()
                                .flatten()
                                .take(i)
                                .all(|previous| previous != name)
                        });
                    if stale {
                        ui.label("表数据已过期，请取消并刷新后重新导入。");
                    }
                    dialog_actions(ui, |ui| {
                        submit = ui
                            .add_enabled(
                                valid && !busy && !stale,
                                primary_button(ui, format!("确认事务导入 {} 行", form.rows.len())),
                            )
                            .clicked();
                    });
                });
            if !open && !busy {
                self.csv_import = None;
            }
            if submit {
                let form = self.csv_import.as_ref().unwrap();
                let selected = form
                    .mapping
                    .iter()
                    .enumerate()
                    .filter_map(|(i, name)| name.as_ref().map(|name| (i, name.clone())))
                    .collect::<Vec<_>>();
                let columns = selected.iter().map(|(_, name)| name.clone()).collect();
                let rows = form
                    .rows
                    .iter()
                    .map(|row| selected.iter().map(|(i, _)| row[*i].clone()).collect())
                    .collect();
                let request = Request::InsertRows {
                    tab: form.tab,
                    target: form.target.clone(),
                    table: form.table.clone(),
                    columns,
                    rows,
                };
                self.request(request, "正在事务导入 CSV…");
            }
        }
    }

    pub(super) fn export_window(&mut self, ctx: &egui::Context) {
        let Some(path) = &mut self.export_path else {
            return;
        };
        let mut open = true;
        let mut export = false;
        let mut cancel = false;
        let selected_count = self.export_rows.as_ref().map(|(_, rows)| rows.len());
        let complete_table =
            selected_count.is_none() && self.tabs[self.active_tab].table_target.is_some();
        dialog_window(
            if selected_count.is_some() {
                "导出选中行"
            } else if complete_table {
                "完整导出表（含当前筛选）"
            } else {
                "导出当前结果集"
            },
            ctx,
        )
        .open(&mut open)
        .default_width(500.0)
        .show(ctx, |ui| {
            ui.label("保存路径");
            ui.add(dialog_input(path).desired_width(f32::INFINITY));
            ui.label(
                RichText::new(if let Some(count) = selected_count {
                    format!("导出所选的 {count} 行；NULL 为未引号空字段，文本始终加引号。")
                } else if complete_table {
                    "导出此表全部匹配筛选的行（不限当前页）；需要主键；NULL 为未引号空字段，文本始终加引号。".into()
                } else {
                    "仅导出当前展示的查询结果；NULL 为未引号空字段，文本始终加引号。".into()
                })
                .small()
                .weak(),
            );
            dialog_actions(ui, |ui| {
                export = ui.add(primary_button(ui, "保存 CSV")).clicked();
                cancel = ui.button("取消").clicked();
            });
        });
        let destination = PathBuf::from(path.as_str());
        if !open || cancel {
            self.export_path = None;
            self.export_rows = None;
        }
        if export && complete_table {
            let tab = &self.tabs[self.active_tab];
            if let (Some(target), Some(table)) = (tab.target.clone(), tab.table_target.as_ref()) {
                let request = Request::ExportTable {
                    tab: tab.id,
                    target,
                    table: Table {
                        schema: table.schema.clone(),
                        name: table.name.clone(),
                        kind: "BASE TABLE".into(),
                    },
                    options: tab.browse.clone(),
                    path: destination,
                };
                self.export_path = None;
                self.request(request, "正在完整导出表数据…");
            }
        } else if export {
            let tab = &self.tabs[self.active_tab];
            let result = tab
                .output
                .as_ref()
                .and_then(|o| o.sets.get(tab.result_index))
                .ok_or_else(|| anyhow::anyhow!("没有可导出的结果集"))
                .and_then(|set| {
                    if let Some((tab_id, rows)) = &self.export_rows {
                        anyhow::ensure!(*tab_id == tab.id, "导出目标标签已改变");
                        export_csv_rows(set, rows.iter().copied(), &destination)
                    } else {
                        export_csv(set, &destination)
                    }
                });
            match result {
                Ok(()) => {
                    self.export_path = None;
                    self.export_rows = None;
                    self.status = format!("已导出: {}", destination.display());
                    self.error = false;
                }
                Err(error) => self.fail(format!("导出失败: {error:#}")),
            }
        }
    }
}
