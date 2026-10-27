use super::*;

impl App {
    pub(in crate::ui) fn workspace_windows(&mut self, ctx: &egui::Context) {
        if let Some(action) = self.confirm_tab_action {
            let mut discard = false;
            let mut cancel = false;
            dialog_window("关闭文档标签？", ctx).show(ctx, |ui| {
                    let names = self.tabs.iter().enumerate().filter(|(index, _)| match action {
                        TabAction::Close(i) => *index == i,
                        TabAction::CloseOthers(i) => *index != i,
                        TabAction::CloseRight(i) => *index > i,
                        _ => false,
                    }).map(|(_, tab)| tab.name.as_str()).collect::<Vec<_>>().join("、");
                    ui.label(format!("关闭标签“{names}”将丢弃未保存的 SQL / Redis 值草稿，并结束对应会话；未提交事务可能回滚。"));
                    dialog_actions(ui, |ui| {
                        discard = ui.add(danger_button(ui, "确认关闭")).clicked();
                        cancel = ui.button("取消，返回保存").clicked();
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
                        "新目标：{} / {}{}",
                        target.connection,
                        target.database.as_deref().unwrap_or("服务器"),
                        target.schema.as_ref().map(|schema| format!(" / {schema}")).unwrap_or_default()
                    ));
                    ui.label("更改目标会结束此标签的独立会话，未提交事务可能回滚；旧结果清除，SQL 文本保留。");
                    dialog_actions(ui, |ui| {
                        confirm = ui.add(primary_button(ui, "确认改绑")).clicked();
                        cancel = ui.button("取消").clicked();
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
                let name = self
                    .profiles
                    .iter()
                    .find(|p| p.id == connection)
                    .map(|p| p.name.as_str())
                    .unwrap_or("原连接");
                ui.label(format!(
                    "切库、重连或断开连接“{name}”可能回滚未提交事务；其他连接不受影响。"
                ));
                dialog_actions(ui, |ui| {
                    confirm = ui.add(primary_button(ui, "确认继续")).clicked();
                    cancel = ui.button("取消").clicked();
                });
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
            let object_dirty = self.object_design_dirty();
            let redis_drafts = self
                .tabs
                .iter()
                .any(|tab| tab.redis.as_ref().is_some_and(|doc| doc.is_dirty()));
            let mut exit = false;
            let mut discard_exit = false;
            let mut cancel = false;
            dialog_window("退出前确认", ctx).show(ctx, |ui| {
                ui.label("退出将丢弃表数据暂存，未提交事务可能回滚；SQL 草稿可选择保存。");
                if redis_drafts {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "Redis 值草稿未保存，直接退出将丢弃。",
                    );
                }
                dialog_actions(ui, |ui| {
                    exit = ui
                        .add_enabled(
                            !busy && !object_dirty && !redis_drafts,
                            primary_button(ui, "保存草稿并退出"),
                        )
                        .clicked();
                    discard_exit = ui
                        .add_enabled(!busy, danger_button(ui, "不保存草稿，直接退出"))
                        .clicked();
                    cancel = ui.button("取消").clicked();
                });
                if busy {
                    ui.label("正在执行数据库操作，请返回并停止查询或等待任务结束后退出。");
                }
                if object_dirty {
                    ui.label("对象设计器有未保存内容，请先保存或复制草稿；直接退出会丢弃。");
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
            dialog_window("查询历史", ctx)
                .open(&mut open)
                .default_width(720.0)
                .show(ctx, |ui| {
                    ui.weak("恢复到新标签后可继续编辑，不会自动执行 SQL。");
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let search_width = (ui.available_width() - 110.0).max(120.0);
                        ui.add_sized(
                            [search_width, 32.0],
                            dialog_input(&mut self.history_search).hint_text("搜索 SQL 或来源…"),
                        );
                        clear = ui
                            .add_enabled(!self.history.is_empty(), egui::Button::new("清空历史"))
                            .clicked();
                    });
                    let needle = self.history_search.to_lowercase();
                    let entries = self
                        .history
                        .iter()
                        .enumerate()
                        .rev()
                        .filter(|(_, entry)| {
                            entry.sql.to_lowercase().contains(&needle)
                                || entry.source.to_lowercase().contains(&needle)
                        })
                        .collect::<Vec<_>>();
                    ui.horizontal(|ui| {
                        ui.weak(if needle.is_empty() {
                            format!("共 {} 条记录", entries.len())
                        } else {
                            format!("找到 {} 条 / 共 {} 条", entries.len(), self.history.len())
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.weak("最近执行在前");
                        });
                    });
                    egui::ScrollArea::vertical()
                        .id_salt("query_history_list")
                        .max_height((ctx.content_rect().height() - 240.0).clamp(120.0, 480.0))
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            if entries.is_empty() {
                                ui.add_space(24.0);
                                ui.vertical_centered(|ui| {
                                    ui.label(if self.history.is_empty() {
                                        "暂无查询历史"
                                    } else {
                                        "没有找到匹配的记录"
                                    });
                                    ui.weak(if self.history.is_empty() {
                                        "执行 SQL 后，可在这里查看和恢复。"
                                    } else {
                                        "试试其他 SQL 关键字或来源名称。"
                                    });
                                });
                                ui.add_space(24.0);
                            }
                            for (index, entry) in entries {
                                ui.push_id((index, entry.timestamp), |ui| {
                                    if history_entry_card(ui, entry) {
                                        restore = Some(entry.clone());
                                    }
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

/// Keep metadata and actions aligned even when a query or source is very long.
fn history_entry_card(ui: &mut egui::Ui, entry: &HistoryEntry) -> bool {
    let mut restore = false;
    egui::Frame::group(ui.style())
        .fill(surface(ui))
        .corner_radius(8)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                restore = ui
                    .add(primary_button(ui, "恢复为新标签"))
                    .on_hover_text("将完整 SQL 恢复到新标签，不会执行")
                    .clicked();
                ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.horizontal(|ui| {
                        let color = if entry.success {
                            accent(ui)
                        } else {
                            ui.visuals().error_fg_color
                        };
                        egui::Frame::new()
                            .fill(color.gamma_multiply(0.12))
                            .corner_radius(4)
                            .inner_margin(egui::Margin::symmetric(6, 2))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(if entry.success { "成功" } else { "失败" })
                                        .size(12.0)
                                        .color(color),
                                );
                            });
                        ui.add(egui::Label::new(RichText::new(&entry.source).strong()).truncate())
                            .on_hover_text(&entry.source);
                    });
                    let timestamp = chrono::DateTime::from_timestamp(entry.timestamp, 0)
                        .map(|time| {
                            time.with_timezone(&chrono::Local)
                                .format("%Y-%m-%d %H:%M:%S")
                                .to_string()
                        })
                        .unwrap_or_else(|| "时间未知".into());
                    ui.label(RichText::new(timestamp).small().weak());
                });
            });
            let expanded_id = ui.make_persistent_id("sql_expanded");
            let mut expanded = ui
                .ctx()
                .data_mut(|data| data.get_temp::<bool>(expanded_id).unwrap_or(false));
            egui::Frame::new()
                .fill(ui.visuals().faint_bg_color)
                .corner_radius(5)
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 2.0;
                    if expanded {
                        egui::ScrollArea::both()
                            .id_salt("full_sql")
                            .max_height(220.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&entry.sql).monospace().size(12.0),
                                    )
                                    .extend(),
                                );
                            });
                    } else {
                        for line in entry.sql.lines().take(3) {
                            // Bound layout work for generated SQL with very long lines.
                            let preview = line.chars().take(240).collect::<String>();
                            ui.add(
                                egui::Label::new(RichText::new(preview).monospace().size(12.0))
                                    .truncate(),
                            );
                        }
                    }
                });
            if ui
                .small_button(if expanded { "收起 SQL" } else { "展开 SQL" })
                .clicked()
            {
                expanded = !expanded;
                ui.ctx()
                    .data_mut(|data| data.insert_temp(expanded_id, expanded));
            }
        });
    restore
}
