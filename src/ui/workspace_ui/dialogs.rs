use super::*;

impl App {
    pub(in crate::ui) fn workspace_windows(&mut self, ctx: &egui::Context) {
        if let Some(pending) = &self.confirm_sql {
            let mut confirm = false;
            let mut cancel = false;
            dialog_window("确认执行 SQL？", ctx)
                .default_width(720.0)
                .max_width((ctx.available_rect().width() - 32.0).max(200.0))
                .show(ctx, |ui| {
                    ui.label(format!(
                        "连接：{}  ·  数据库：{}{}",
                        pending.connection_name,
                        pending
                            .target
                            .database
                            .as_deref()
                            .unwrap_or("服务器（未选库）"),
                        pending
                            .target
                            .schema
                            .as_ref()
                            .map(|schema| format!("  ·  Schema：{schema}"))
                            .unwrap_or_default()
                    ));
                    ui.label("此 SQL 可能修改数据或结构；批量语句中先前语句可能已提交。");
                    let max_height = (ctx.available_rect().height() * 0.5).clamp(80.0, 400.0);
                    egui::ScrollArea::both()
                        .max_height(max_height)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.label(RichText::new(&pending.sql).monospace());
                        });
                    ui.horizontal(|ui| {
                        cancel = ui.button("取消").clicked();
                        confirm = ui.button("确认执行原始 SQL").clicked();
                    });
                });
            if cancel {
                self.confirm_sql = None;
            } else if confirm && let Some(pending) = self.confirm_sql.take() {
                self.send_sql(pending);
            }
        }
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
                    ui.horizontal(|ui| {
                        cancel = ui.button("取消，返回保存").clicked();
                        discard = ui.button("确认关闭").clicked();
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
                    ui.horizontal(|ui| {
                        cancel = ui.button("取消").clicked();
                        confirm = ui.button("确认改绑").clicked();
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
                ui.horizontal(|ui| {
                    cancel = ui.button("取消").clicked();
                    confirm = ui.button("确认继续").clicked();
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
                ui.horizontal(|ui| {
                    cancel = ui.button("取消").clicked();
                    exit = ui
                        .add_enabled(
                            !busy && !object_dirty && !redis_drafts,
                            egui::Button::new("保存草稿并退出"),
                        )
                        .clicked();
                    discard_exit = ui
                        .add_enabled(!busy, egui::Button::new("不保存草稿，直接退出"))
                        .clicked();
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
            dialog_window("查询历史（仅恢复到编辑器，不执行）", ctx)
                .open(&mut open)
                .default_width(720.0)
                .show(ctx, |ui| {
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
