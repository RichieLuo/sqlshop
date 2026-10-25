use super::*;

impl RedisDocument {
    pub(super) fn new_key_ui(&mut self, ui: &mut egui::Ui) {
        let Some(draft) = &mut self.new_key else {
            return;
        };
        let mut create = false;
        let mut cancel = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.add_enabled_ui(!self.busy, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong("新建键");
                    ui.add(egui::TextEdit::singleline(&mut draft.key).hint_text("Key（UTF-8）"));
                    egui::ComboBox::from_id_salt("redis-new-kind")
                        .selected_text(draft.kind.label())
                        .show_ui(ui, |ui| {
                            for kind in [
                                RedisKind::String,
                                RedisKind::Hash,
                                RedisKind::List,
                                RedisKind::Set,
                                RedisKind::ZSet,
                            ] {
                                let label = kind.label();
                                ui.selectable_value(&mut draft.kind, kind.clone(), label);
                            }
                        });
                    ui.checkbox(&mut draft.hex, "值为 Hex");
                });
                if draft.kind == RedisKind::Hash {
                    ui.horizontal(|ui| {
                        ui.label("Field");
                        ui.text_edit_singleline(&mut draft.field);
                    });
                }
                if draft.kind == RedisKind::ZSet {
                    ui.horizontal(|ui| {
                        ui.label("Score");
                        ui.add(egui::DragValue::new(&mut draft.score).speed(0.1));
                    });
                }
                ui.add(
                    egui::TextEdit::multiline(&mut draft.value)
                        .hint_text("初始值 / 首个成员")
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    create = ui
                        .add_enabled(!draft.key.is_empty(), egui::Button::new("创建（拒绝覆盖）"))
                        .clicked();
                    cancel = ui.button("取消新建").clicked();
                });
            });
        });
        if create {
            let value = if draft.hex {
                parse_hex(&draft.value)
            } else {
                Ok(draft.value.as_bytes().to_vec())
            };
            match value {
                Ok(value) => {
                    let request = RedisRequest::Create {
                        key: draft.key.as_bytes().to_vec(),
                        kind: draft.kind.clone(),
                        value,
                        field: draft.field.as_bytes().to_vec(),
                        score: draft.score,
                    };
                    self.request(request);
                    self.creating = self.busy;
                    // Keep the new-key draft until the server confirms success.
                }
                Err(error) => self.error = Some(format!("初始值无效：{error:#}")),
            }
        }
        if cancel {
            self.new_key = None;
        }
    }

    pub(super) fn confirmation_ui(&mut self, ui: &mut egui::Ui) {
        let Some(confirmation) = &self.confirmation else {
            return;
        };
        let mut confirm = false;
        let mut cancel = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.strong(format!(
                "确认写入 · {}:{} / DB {}",
                self.profile.host, self.profile.port, self.database
            ));
            match confirmation {
                Confirmation::Namespace { prefix, count, database, .. } => {
                    ui.colored_label(ui.visuals().warn_fg_color, format!("删除 db{database} 中命名空间下预览到的 {count} 个键（不可撤销）"));
                    ui.monospace(display_bytes(prefix));
                    ui.label("删除预览内的全部键；可能部分完成，不可撤销。")
                        .on_hover_text("按字面前缀匹配，包括未加载键；仅删除预览键名，同名键被并发替换仍会删除。分批 UNLINK，不自动重试。");
                }
                Confirmation::Mutation { key, label, .. } => {
                    ui.label(label);
                    ui.monospace(display_bytes(key));
                }
                Confirmation::Command(args) => {
                    ui.label("执行写命令，不可撤销。");
                    ui.monospace(
                        args.iter()
                            .map(|arg| display_bytes(arg))
                            .collect::<Vec<_>>()
                            .join(" "),
                    );
                }
            }
            ui.horizontal(|ui| {
                confirm = ui
                    .add_enabled(!self.busy && !self.is_dirty(), egui::Button::new("确认执行"))
                    .clicked();
                cancel = ui
                    .add_enabled(!self.busy, egui::Button::new("取消"))
                    .clicked();
            });
        });
        if cancel {
            self.confirmation = None;
        }
        if confirm && let Some(confirmation) = self.confirmation.take() {
            match confirmation {
                Confirmation::Mutation { key, change, .. } => self.mutate(key, change),
                Confirmation::Command(args) => self.execute_command(args, true),
                Confirmation::Namespace {
                    token, database, ..
                } => {
                    if database != self.database || !self.connected {
                        self.error = Some(
                            "数据库或连接已改变，命名空间删除预览已失效，请重新扫描确认".into(),
                        );
                    } else {
                        self.stale = true;
                        self.request(RedisRequest::DeleteNamespace { token });
                    }
                }
            }
        }
    }

    pub(super) fn console_ui(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong(format!("Redis 控制台 · DB {}", self.database));
                if ui.button("清空输出").clicked() {
                    self.console.clear();
                }
            });
            egui::ScrollArea::vertical()
                .id_salt("redis-console-output")
                .max_height(115.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for output in &self.console {
                        ui.monospace(output);
                    }
                });
            let mut run = false;
            ui.add_enabled_ui(
                !self.busy && !self.is_dirty() && self.confirmation.is_none(),
                |ui| {
                    ui.horizontal(|ui| {
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut self.command)
                                .font(egui::TextStyle::Monospace)
                                .desired_width((ui.available_width() - 70.0).max(100.0))
                                .hint_text("例如 GET \"user:1\""),
                        );
                        run = ui.button("执行").clicked()
                            || (response.lost_focus()
                                && ui.input(|input| input.key_pressed(egui::Key::Enter)));
                        if response.has_focus() {
                            if ui.input(|input| input.key_pressed(egui::Key::ArrowUp))
                                && !self.history.is_empty()
                            {
                                let index =
                                    self.history_index.map_or(self.history.len() - 1, |index| {
                                        index.saturating_sub(1)
                                    });
                                self.history_index = Some(index);
                                self.command = self.history[index].clone();
                            }
                            if ui.input(|input| input.key_pressed(egui::Key::ArrowDown))
                                && let Some(index) = self.history_index
                            {
                                self.history_index =
                                    (index + 1 < self.history.len()).then_some(index + 1);
                                self.command = self
                                    .history_index
                                    .map_or_else(String::new, |index| self.history[index].clone());
                            }
                        }
                    });
                },
            );
            if run {
                match parse_command(&self.command) {
                    Ok(args) if args.is_empty() => {}
                    Ok(args) => {
                        let command = String::from_utf8_lossy(&args[0]).to_ascii_uppercase();
                        if matches!(command.as_str(), "AUTH" | "HELLO" | "ACL" | "CONFIG") {
                            self.error =
                                Some("凭据和管理命令不进入控制台或历史，请使用连接设置".into());
                        } else if command_needs_confirmation(&args) {
                            self.confirmation = Some(Confirmation::Command(args));
                        } else {
                            self.execute_command(args, false);
                        }
                    }
                    Err(error) => self.error = Some(format!("命令解析失败：{error:#}")),
                }
            }
        });
    }

    pub(super) fn execute_command(&mut self, args: Vec<Vec<u8>>, confirmed: bool) {
        self.history.push_back(self.command.clone());
        while self.history.len() > 100 {
            self.history.pop_front();
        }
        self.history_index = None;
        self.push_console(format!("> DB {} · {}", self.database, self.command));
        self.request(RedisRequest::Command { args, confirmed });
    }
}
