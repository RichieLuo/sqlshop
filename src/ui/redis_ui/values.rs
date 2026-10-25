use super::*;

impl RedisDocument {
    pub(super) fn mutate(&mut self, key: Vec<u8>, change: RedisMutation) {
        self.request(RedisRequest::Mutate { key, change });
        if self.busy {
            self.stale = true;
        }
    }

    pub(super) fn string_ui(
        &mut self,
        ui: &mut egui::Ui,
        key: &RedisKey,
        bytes: &[u8],
        writable: bool,
    ) {
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!self.value_dirty() && !self.busy, |ui| {
                for format in [ValueFormat::Text, ValueFormat::Json, ValueFormat::Hex] {
                    if ui
                        .add_enabled(
                            format == ValueFormat::Hex || std::str::from_utf8(bytes).is_ok(),
                            egui::Button::selectable(self.value_format == format, format.label()),
                        )
                        .clicked()
                    {
                        self.value_format = format;
                        self.value_text = format.show(bytes);
                        self.value_original = self.value_text.clone();
                    }
                }
            });
            if ui
                .add_enabled(
                    writable && !key.truncated && self.value_format != ValueFormat::Hex,
                    egui::Button::new("格式化 JSON"),
                )
                .clicked()
            {
                match serde_json::from_str::<serde_json::Value>(&self.value_text)
                    .and_then(|value| serde_json::to_string_pretty(&value))
                {
                    Ok(text) => self.value_text = text,
                    Err(error) => self.error = Some(format!("JSON 无效：{error}")),
                }
            }
            if ui.button("复制显示内容").clicked() {
                ui.ctx().copy_text(self.value_text.clone());
            }
        });
        ui.add_enabled(
            writable && !key.truncated,
            egui::TextEdit::multiline(&mut self.value_text)
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY)
                .desired_rows(13),
        );
        if ui
            .add_enabled(
                writable && !key.truncated && self.value_dirty(),
                egui::Button::new("保存值（保留 TTL）"),
            )
            .clicked()
        {
            match self.value_format.bytes(&self.value_text) {
                Ok(value) => self.mutate(
                    key.key.clone(),
                    RedisMutation::SetString {
                        value,
                        expected: bytes.to_vec(),
                    },
                ),
                Err(error) => self.error = Some(format!("内容无效：{error:#}")),
            }
        }
    }

    pub(super) fn collection_ui(&mut self, ui: &mut egui::Ui, key: &RedisKey, writable: bool) {
        let rows: Vec<(Vec<u8>, Vec<u8>, f64)> = match &key.value {
            RedisValue::Hash(rows) => rows
                .iter()
                .map(|(field, value)| (field.clone(), value.clone(), 0.0))
                .collect(),
            RedisValue::List(rows) => rows
                .iter()
                .enumerate()
                .map(|(index, value)| (index.to_string().into_bytes(), value.clone(), 0.0))
                .collect(),
            RedisValue::Set(rows) => rows
                .iter()
                .map(|value| (Vec::new(), value.clone(), 0.0))
                .collect(),
            RedisValue::ZSet(rows) => rows
                .iter()
                .map(|(value, score)| (Vec::new(), value.clone(), *score))
                .collect(),
            _ => Vec::new(),
        };
        let mut chosen = None;
        TableBuilder::new(ui)
            .id_salt("redis-members")
            .striped(true)
            .resizable(true)
            .column(Column::auto().at_least(80.0))
            .column(Column::remainder().at_least(100.0))
            .max_scroll_height(220.0)
            .header(24.0, |mut header| {
                header.col(|ui| {
                    ui.strong(if key.kind == RedisKind::ZSet {
                        "Score"
                    } else if key.kind == RedisKind::Hash {
                        "Field"
                    } else {
                        "索引"
                    });
                });
                header.col(|ui| {
                    ui.strong("Value / Member · 单击选择编辑");
                });
            })
            .body(|mut body| {
                for (index, (field, value, score)) in rows.iter().enumerate() {
                    body.row(24.0, |mut row| {
                        row.set_selected(
                            self.member
                                .as_ref()
                                .is_some_and(|draft| draft.index == index),
                        );
                        row.col(|ui| {
                            ui.label(if key.kind == RedisKind::ZSet {
                                score.to_string()
                            } else if key.kind == RedisKind::Hash {
                                preview(field)
                            } else {
                                index.to_string()
                            });
                        });
                        row.col(|ui| {
                            if ui
                                .add_enabled(
                                    !self.busy && !self.value_dirty(),
                                    egui::Button::selectable(
                                        self.member
                                            .as_ref()
                                            .is_some_and(|draft| draft.index == index),
                                        preview(value),
                                    ),
                                )
                                .on_hover_text(
                                    display_bytes(value).chars().take(4096).collect::<String>(),
                                )
                                .clicked()
                            {
                                chosen = Some(index);
                            }
                        });
                    });
                }
            });
        if let Some(index) = chosen {
            let (field, value, score) = &rows[index];
            self.member = Some(MemberDraft::new(
                index,
                field.clone(),
                value.clone(),
                *score,
            ));
        }
        let mut mutation = None;
        let mut removal = None;
        let no_add_draft = self.add_value.is_empty() && self.add_field.is_empty();
        if let Some(member) = &mut self.member {
            ui.separator();
            ui.strong(format!("编辑选中项 #{}", member.index));
            ui.add_enabled_ui(writable && no_add_draft, |ui| {
                if key.kind == RedisKind::ZSet {
                    ui.horizontal(|ui| {
                        ui.label("Score");
                        ui.add(egui::DragValue::new(&mut member.score).speed(0.1));
                    });
                }
                let was_hex = member.hex;
                ui.add_enabled_ui(!member.dirty(), |ui| {
                    ui.checkbox(&mut member.hex, "Hex 编辑");
                });
                if was_hex != member.hex {
                    member.text = if member.hex {
                        format_hex(&member.value)
                    } else {
                        String::from_utf8_lossy(&member.value).into_owned()
                    };
                    member.original = member.text.clone();
                    // Invalid UTF-8 is never silently converted back into editable text.
                    if !member.hex && std::str::from_utf8(&member.value).is_err() {
                        member.hex = true;
                        member.text = format_hex(&member.value);
                        member.original = member.text.clone();
                    }
                }
                ui.add_enabled(
                    matches!(key.kind, RedisKind::Hash | RedisKind::List),
                    egui::TextEdit::multiline(&mut member.text)
                        .font(egui::TextStyle::Monospace)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    let editable = matches!(
                        key.kind,
                        RedisKind::Hash | RedisKind::List | RedisKind::ZSet
                    );
                    if ui
                        .add_enabled(editable && member.dirty(), egui::Button::new("保存此项"))
                        .clicked()
                    {
                        match member.bytes() {
                            Ok(value) => {
                                mutation = Some(match key.kind {
                                    RedisKind::Hash => RedisMutation::HashSet {
                                        field: member.field.clone(),
                                        value,
                                    },
                                    RedisKind::List => RedisMutation::ListSet {
                                        index: member.index as i64,
                                        expected: member.value.clone(),
                                        value,
                                    },
                                    _ => RedisMutation::ZSetAdd {
                                        score: member.score,
                                        value: member.value.clone(),
                                    },
                                })
                            }
                            Err(error) => self.error = Some(format!("Hex 无效：{error:#}")),
                        }
                    }
                    if ui
                        .add_enabled(
                            !member.dirty() && !matches!(key.kind, RedisKind::List),
                            egui::Button::new("移除此项…"),
                        )
                        .clicked()
                    {
                        removal = Some(match key.kind {
                            RedisKind::Hash => RedisMutation::HashDelete {
                                field: member.field.clone(),
                            },
                            RedisKind::Set => RedisMutation::SetRemove {
                                value: member.value.clone(),
                            },
                            _ => RedisMutation::ZSetRemove {
                                value: member.value.clone(),
                            },
                        });
                    }
                });
            });
        }
        if let Some(change) = mutation {
            self.mutate(key.key.clone(), change);
        }
        if let Some(change) = removal {
            self.confirmation = Some(Confirmation::Mutation {
                key: key.key.clone(),
                change,
                label: "移除选中的集合项（移除最后一项会删除键）".into(),
            });
        }
        ui.separator();
        egui::CollapsingHeader::new("添加集合项").show(ui, |ui| {
            ui.add_enabled_ui(writable && !self.value_dirty(), |ui| {
                if key.kind == RedisKind::Hash {
                    ui.horizontal(|ui| {
                        ui.label("Field（文本）");
                        ui.text_edit_singleline(&mut self.add_field);
                    });
                }
                if key.kind == RedisKind::ZSet {
                    ui.horizontal(|ui| {
                        ui.label("Score");
                        ui.add(egui::DragValue::new(&mut self.add_score).speed(0.1));
                    });
                }
                ui.checkbox(&mut self.add_hex, "Value 使用 Hex");
                ui.add(
                    egui::TextEdit::multiline(&mut self.add_value)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                if ui
                    .button(if key.kind == RedisKind::List {
                        "追加到列表末尾"
                    } else {
                        "添加 / 更新"
                    })
                    .clicked()
                {
                    let value = if self.add_hex {
                        parse_hex(&self.add_value)
                    } else {
                        Ok(self.add_value.as_bytes().to_vec())
                    };
                    match value {
                        Ok(value) => {
                            let change = match key.kind {
                                RedisKind::Hash => RedisMutation::HashSet {
                                    field: self.add_field.as_bytes().to_vec(),
                                    value,
                                },
                                RedisKind::List => RedisMutation::ListPush { value },
                                RedisKind::Set => RedisMutation::SetAdd { value },
                                _ => RedisMutation::ZSetAdd {
                                    score: self.add_score,
                                    value,
                                },
                            };
                            self.mutate(key.key.clone(), change);
                        }
                        Err(error) => self.error = Some(format!("Hex 无效：{error:#}")),
                    }
                }
            });
        });
    }
}
