use super::*;

impl RedisDocument {
    pub(in crate::ui) fn show(&mut self, ui: &mut egui::Ui) {
        self.poll();
        ui.push_id(self.id, |ui| {
            ui.spacing_mut().item_spacing.y = 5.0;
            ui.horizontal_wrapped(|ui| {
                ui.heading(format!("Redis · {}", self.profile.name));
                if self.busy {
                    ui.spinner();
                }
                if self.connected {
                    ui.label(
                        RichText::new(format!(
                            "{}:{} / DB {}",
                            self.profile.host, self.profile.port, self.database
                        ))
                        .strong(),
                    );
                    if ui
                        .add_enabled(!self.busy && !self.is_dirty(), egui::Button::new("断开"))
                        .clicked()
                    {
                        self.disconnect();
                    }
                } else if ui
                    .add_enabled(
                        !self.busy && !self.is_dirty(),
                        egui::Button::new("连接 Redis"),
                    )
                    .clicked()
                {
                    self.connect();
                }
            });
            if let Some(error) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if let Some(notice) = &self.database_notice {
                ui.colored_label(ui.visuals().warn_fg_color, notice);
            }
            if self.is_dirty() {
                ui.label(RichText::new("有未保存修改").color(ui.visuals().warn_fg_color));
            }
            ui.separator();
            self.confirmation_ui(ui);
            if self.confirmation.is_some() {
                return;
            }
            if !self.connected && self.is_dirty() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "连接已断开，写入结果待核实；草稿已保留。",
                );
                if ui.button("丢弃断线草稿，允许重新连接").clicked() {
                    self.value_text = self.value_original.clone();
                    self.member = None;
                    self.add_value.clear();
                    self.add_field.clear();
                    self.new_key = None;
                }
                if ui.button("复制断线草稿").clicked() {
                    let mut text = self.value_text.clone();
                    if let Some(member) = &self.member {
                        text.push_str(&format!(
                            "\n成员草稿：{}\nScore：{}",
                            member.text, member.score
                        ));
                    }
                    if let Some(draft) = &self.new_key {
                        text.push_str(&format!(
                            "\n新键：{}\nField：{}\nValue：{}",
                            draft.key, draft.field, draft.value
                        ));
                    }
                    text.push_str(&format!(
                        "\n待添加 Field：{}\nValue：{}",
                        self.add_field, self.add_value
                    ));
                    ui.ctx().copy_text(text);
                }
                if self.selected.is_some() {
                    self.value_ui(ui, (ui.available_height() - 40.0).max(100.0));
                }
            }
            if self.connected {
                ui.horizontal(|ui| {
                    ui.label("数据库");
                    ui.add_enabled(
                        !self.busy && !self.is_dirty(),
                        egui::DragValue::new(&mut self.database_draft)
                            .range(0..=u32::MAX)
                            .prefix("DB "),
                    );
                    if ui
                        .add_enabled(
                            !self.busy
                                && !self.is_dirty()
                                && self.confirmation.is_none()
                                && self.database_draft != self.database,
                            egui::Button::new("切换"),
                        )
                        .clicked()
                    {
                        self.request(RedisRequest::SelectDb(self.database_draft));
                    }
                    if ui
                        .add_enabled(
                            !self.busy && !self.is_dirty(),
                            egui::Button::new("刷新数据库列表"),
                        )
                        .clicked()
                    {
                        self.refresh_databases();
                    }
                    ui.toggle_value(&mut self.console_open, "命令控制台");
                });
                if self.new_key.is_some() {
                    self.new_key_ui(ui);
                }
                if self.console_open {
                    self.console_ui(ui);
                }
                let height = (ui.available_height() - 28.0).max(180.0);
                self.key_value_workspace(ui, height);
            } else {
                ui.add_space(20.0);
                ui.weak("未连接");
            }
            ui.separator();
            ui.small(&self.status);
        });
    }

    pub(super) fn key_value_workspace(
        &mut self,
        ui: &mut egui::Ui,
        height: f32,
    ) -> (egui::Rect, egui::Rect) {
        let available = ui.available_rect_before_wrap();
        let area = egui::Rect::from_min_size(available.min, egui::vec2(available.width(), height));
        let panel_id = egui::Id::new((self.id, "redis-key-panel"));
        let mut splitter = egui::Rect::NOTHING;
        let mut scroll_rect = egui::Rect::NOTHING;
        ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            let width = (area.width() * 0.30).clamp(180.0, 340.0);
            let maximum = (area.width() - 240.0).max(180.0);
            let panel = egui::SidePanel::left(panel_id)
                .resizable(true)
                .default_width(width.min(maximum))
                .width_range(180.0..=maximum)
                .frame(egui::Frame::new().inner_margin(egui::Margin {
                    left: 0,
                    right: 8,
                    top: 0,
                    bottom: 0,
                }))
                .show_inside(ui, |ui| self.key_browser(ui, height));
            scroll_rect = panel.inner;
            splitter = egui::Rect::from_center_size(
                egui::pos2(panel.response.rect.right(), area.center().y),
                egui::vec2(10.0, height),
            );
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(
                    ui.available_rect_before_wrap()
                        .shrink2(egui::vec2(6.0, 0.0)),
                ),
                |ui| {
                    ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                    self.value_ui(ui, height);
                },
            );
        });
        ui.advance_cursor_after_rect(area);
        (splitter, scroll_rect)
    }

    pub(super) fn preview_namespace(&mut self, prefix: Vec<u8>) -> bool {
        if !self.connected
            || self.busy
            || self.is_dirty()
            || self.confirmation.is_some()
            || prefix.is_empty()
        {
            return false;
        }
        self.request(RedisRequest::PreviewNamespace { prefix })
    }

    pub(super) fn key_browser(&mut self, ui: &mut egui::Ui, height: f32) -> egui::Rect {
        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
        ui.horizontal(|ui| {
            ui.strong("键空间");
            ui.selectable_value(&mut self.tree_view, true, "树");
            ui.selectable_value(&mut self.tree_view, false, "列表");
        });
        let enter = ui.add(
            egui::TextEdit::singleline(&mut self.pattern)
                .desired_width(f32::INFINITY)
                .hint_text("SCAN MATCH，例如 user:*"),
        );
        let search = enter.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let mut refresh = false;
        ui.horizontal(|ui| {
            refresh = ui
                .add_enabled(!self.busy, egui::Button::new("搜索 / 刷新"))
                .clicked();
            if ui
                .add_enabled(
                    !self.busy && !self.is_dirty(),
                    egui::Button::new("＋ 新建键"),
                )
                .clicked()
            {
                self.new_key = Some(NewKey::default());
            }
        });
        if (search || refresh) && !self.busy {
            self.scan(true);
        }
        ui.small(format!("{} 个已加载", self.keys.len()));
        let mut chosen = None;
        let mut namespace = None;
        let scroll = egui::ScrollArea::both()
            .id_salt("redis-key-tree")
            .auto_shrink([false, false])
            .max_height((height - 150.0).max(80.0))
            .show(ui, |ui| {
                ui.add_enabled_ui(
                    !self.busy && !self.is_dirty() && self.confirmation.is_none(),
                    |ui| {
                        let selected = self.selected.as_ref().map(|key| key.key.as_slice());
                        if self.tree_view {
                            draw_tree(
                                ui,
                                &self.tree,
                                selected,
                                &mut Vec::new(),
                                &mut chosen,
                                &mut namespace,
                                0,
                            );
                        } else {
                            for key in &self.keys {
                                if key_leaf(
                                    ui,
                                    key,
                                    &display_bytes(key),
                                    selected == Some(key.as_slice()),
                                ) {
                                    chosen = Some(key.clone());
                                }
                            }
                        }
                    },
                );
            });
        if let Some(prefix) = namespace {
            self.preview_namespace(prefix);
        }
        if let Some(key) = chosen {
            self.stale = true;
            self.selected = None;
            self.request(RedisRequest::ReadKey { key });
        }
        if self.keys.len() >= KEY_LIMIT {
            ui.weak("已达 20,000 键预览上限，请缩小 MATCH 范围");
        }
        if ui
            .add_enabled(
                !self.busy && self.scan_started && self.cursor != 0 && self.keys.len() < KEY_LIMIT,
                egui::Button::new("继续 SCAN 加载"),
            )
            .clicked()
        {
            self.scan(false);
        }
        scroll.inner_rect
    }

    pub(super) fn value_ui(&mut self, ui: &mut egui::Ui, height: f32) {
        let Some(key) = self.selected.clone() else {
            ui.add_space(24.0);
            ui.heading("选择一个 Key");
            return;
        };
        egui::ScrollArea::vertical()
            .id_salt("redis-value-scroll")
            .max_height(height)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(key.kind.label());
                    ui.add(
                        egui::Label::new(RichText::new(display_bytes(&key.key)).monospace()).wrap(),
                    );
                    if ui.button("复制键名").clicked() {
                        ui.ctx().copy_text(display_bytes(&key.key));
                    }
                    if ui
                        .add_enabled(
                            self.connected && !self.busy && !self.is_dirty(),
                            egui::Button::new("重新读取"),
                        )
                        .clicked()
                    {
                        self.stale = true;
                        self.request(RedisRequest::ReadKey {
                            key: key.key.clone(),
                        });
                    }
                    if ui
                        .add_enabled(
                            (self.value_dirty()
                                || !self.add_value.is_empty()
                                || !self.add_field.is_empty())
                                && !self.busy,
                            egui::Button::new("撤销本地修改"),
                        )
                        .clicked()
                    {
                        self.value_text = self.value_original.clone();
                        self.member = None;
                        self.add_value.clear();
                        self.add_field.clear();
                    }
                });
                ui.weak(format!(
                    "大小 / 元素数：{} · TTL：{}",
                    key.size,
                    ttl_label(key.ttl_ms)
                ));
                if key.truncated {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "预览已截断，截断的 String 不可保存。",
                    );
                }
                if self.stale {
                    ui.colored_label(ui.visuals().warn_fg_color, "数据已过期，请刷新。");
                }
                let writable = self.connected
                    && !self.busy
                    && !self.stale
                    && self.confirmation.is_none()
                    && self.new_key.is_none();
                ui.add_enabled_ui(writable, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("过期秒数");
                        ui.add(egui::DragValue::new(&mut self.ttl).range(1..=i32::MAX as i64));
                        if ui
                            .add_enabled(!self.is_dirty(), egui::Button::new("设置 TTL…"))
                            .clicked()
                        {
                            self.confirmation = Some(Confirmation::Mutation {
                                key: key.key.clone(),
                                change: RedisMutation::Expire { seconds: self.ttl },
                                label: format!("将过期时间设为 {} 秒", self.ttl),
                            });
                        }
                        if ui
                            .add_enabled(!self.is_dirty(), egui::Button::new("永不过期"))
                            .clicked()
                        {
                            self.mutate(key.key.clone(), RedisMutation::Persist);
                        }
                        if ui
                            .add_enabled(!self.is_dirty(), egui::Button::new("删除键…"))
                            .clicked()
                        {
                            self.confirmation = Some(Confirmation::Mutation {
                                key: key.key.clone(),
                                change: RedisMutation::Delete,
                                label: "删除此键及其全部值（不可撤销）".into(),
                            });
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.rename)
                                .hint_text("新键名（UTF-8）")
                                .desired_width(240.0),
                        );
                        if ui
                            .add_enabled(
                                !self.is_dirty()
                                    && self.rename.as_bytes() != key.key
                                    && !self.rename.is_empty(),
                                egui::Button::new("重命名…"),
                            )
                            .clicked()
                        {
                            self.confirmation = Some(Confirmation::Mutation {
                                key: key.key.clone(),
                                change: RedisMutation::Rename {
                                    new_key: self.rename.as_bytes().to_vec(),
                                },
                                label: format!("重命名为 {}（不覆盖已有键）", self.rename),
                            });
                        }
                    });
                });
                ui.separator();
                match &key.value {
                    RedisValue::String(bytes) => self.string_ui(ui, &key, bytes, writable),
                    RedisValue::Unsupported(message) => {
                        ui.weak(message);
                    }
                    _ => self.collection_ui(ui, &key, writable),
                }
            });
    }
}
