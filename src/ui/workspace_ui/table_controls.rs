use super::*;

impl App {
    pub(in crate::ui) fn table_draft_action_state(&self) -> (bool, bool) {
        let actions = self.table_action_state(self.active_tab);
        let inline_idle = self
            .cell_editor
            .as_ref()
            .is_some_and(|editor| editor.tab == self.tabs[self.active_tab].id)
            && !self.tab_busy(self.active_tab);
        (
            actions.can_submit() || (inline_idle && actions.ready && !actions.stale),
            actions.can_revert() || inline_idle,
        )
    }

    pub(in crate::ui) fn table_pagination(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.spacing_mut().button_padding.y = 4.0;
        ui.spacing_mut().interact_size.y = 24.0;
        let actions = self.table_action_state(self.active_tab);
        let can_insert_row = self.can_add_table_row(self.active_tab);
        let read_worker = self.tabs[self.active_tab]
            .target
            .as_ref()
            .and_then(|t| self.sessions.get(&t.connection))
            .map(|s| &s.worker);
        let read_state = read_worker
            .and_then(|worker| worker.table_read_execution(self.tabs[self.active_tab].id));
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
        let mut refresh = false;
        let read_tab_id = tab.id;
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
            refresh = ui
                .add_enabled(actions.can_read(), egui::Button::new("刷新"))
                .on_hover_text("重新读取当前页；存在暂存时请先提交或撤销暂存")
                .clicked();
            if let (Some(worker), Some(read)) = (read_worker, &read_state) {
                if ui
                    .add_enabled(
                        read.cancellable && !read.requested,
                        egui::Button::new("■ 停止读取"),
                    )
                    .on_hover_text(
                        "停止本次分页/统计；取消确认后保留上次数据，若已完成则显示本次结果",
                    )
                    .clicked()
                    && let Err(error) = worker.cancel_table_read(read_tab_id, read.execution)
                {
                    ui.colored_label(ui.visuals().error_fg_color, error.to_string());
                }
                ui.label(RichText::new(&read.message).small().weak());
            }
        };
        ui.separator();
        egui::ScrollArea::horizontal()
            .id_salt(("table-footer", tab.id))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    edit_buttons(ui);
                    ui.separator();
                    (reload, last_page) = table_page_controls(ui, tab, actions.can_read());
                    table_footer_status(ui, &status, &tab.source, actions.ready);
                });
            });
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
        if last_page {
            self.load_last_table_page(self.active_tab);
        } else if reload {
            self.load_table(self.active_tab);
        }
    }

    pub(in crate::ui) fn table_controls(&mut self, ui: &mut egui::Ui) {
        if self
            .cell_editor
            .as_ref()
            .is_some_and(|editor| editor.tab == self.tabs[self.active_tab].id)
        {
            return;
        }
        let actions = self.table_action_state(self.active_tab);
        let enabled = actions.idle && actions.ready;
        let mut reload = false;
        let mut describe = false;
        let mut export = false;
        let mut import = false;
        let tab = &mut self.tabs[self.active_tab];
        grid_columns_ui::load_table_layout(tab, &self.table_grid_layouts);
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
        ui.scope(|ui| {
            egui::ScrollArea::horizontal()
                .id_salt(("table-toolbar", tab.id))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);
                        ui.add_enabled_ui(enabled, |ui| {
                            describe = ui
                                .add(egui::Button::new("结构 / DDL").frame(false))
                                .clicked();
                            ui.separator();
                            let active =
                                !tab.browse.filters.is_empty() || !tab.browse.sorts.is_empty();
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
                                .on_hover_text(
                                    "展开筛选与排序；检查条件后点击应用，才会重新读取数据库",
                                )
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
                                    ui.menu_button("复制", |ui| {
                                        result_copy_controls(ui, tab, true)
                                    });
                                    ui.menu_button("导出", |ui| {
                                        if ui.button("导出 CSV…").clicked() {
                                            export = true;
                                            ui.close();
                                        }
                                    });
                                },
                            );
                        });
                        grid_columns_ui::tab_controls(ui, tab, true);
                    });
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
                    if !tab.insert_drafts.is_empty() {
                        ui.label(format!("新增 {} 行待提交", tab.insert_drafts.len()));
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
                .on_hover_text(
                    "● 暂存值 · ＋ 新增行；复制和导出使用上次读取的数据库快照，提交后才写入数据库",
                );
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

    pub(in crate::ui) fn discard_staged_cells(&mut self, index: usize) -> bool {
        if self.tab_busy(index) {
            return false;
        }
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

    pub(in crate::ui) fn submit_staged_cells(&mut self, index: usize) -> bool {
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
}
