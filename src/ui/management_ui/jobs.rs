use super::*;

impl App {
    pub(in crate::ui) fn open_jobs(&mut self) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.jobs.is_some()) {
            self.active_tab = index;
            self.browser_visible = false;
            return;
        }
        let draft = self.management_source().map(|source| JobSpec {
            id: Uuid::new_v4(),
            name: format!(
                "{} SQL 任务",
                source.target.database.as_deref().unwrap_or_default()
            ),
            profile: source.profile.id,
            database: source.target.database.unwrap(),
            schema: source.target.schema,
            sql: self
                .tabs
                .get(self.active_tab)
                .map(|tab| tab.sql.clone())
                .unwrap_or_default(),
            schedule: JobSchedule::EveryMinutes(60),
            enabled: false,
            connection_fingerprint: Some(
                serde_json::to_string(&source.profile).unwrap_or_default(),
            ),
        });
        if self.management_tab("任务调度".into(), None).is_some() {
            self.tabs[self.active_tab].jobs = Some(JobsDocument::new(draft));
        }
    }

    pub(super) fn jobs_workspace(&mut self, ui: &mut egui::Ui) {
        let records = self
            .job_scheduler
            .as_ref()
            .map(|scheduler| scheduler.store.records().to_vec())
            .unwrap_or_default();
        let doc = self.tabs[self.active_tab].jobs.as_mut().unwrap();
        let mut save = None;
        let mut delete = None;
        ui.heading("SQL 任务调度").on_hover_text(
            "仅在应用运行时调度；失败或中断后暂停，错过不补跑。SQL 明文保存在本机。",
        );
        if self.job_scheduler.is_none() {
            ui.colored_label(ui.visuals().warn_fg_color, "任务存储不可用，计划未启动");
        }
        egui::ScrollArea::vertical()
            .id_salt("jobs-body")
            .show(ui, |ui| {
                for record in &records {
                    ui.group(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(&record.spec.name);
                            ui.label(format!(
                                "{:?} · {} · {}",
                                record.state,
                                record.spec.database,
                                if record.spec.enabled {
                                    "启用"
                                } else {
                                    "暂停"
                                }
                            ));
                            if ui
                                .add_enabled(
                                    record.state != JobState::Running,
                                    egui::Button::new("编辑 / 启用"),
                                )
                                .clicked()
                            {
                                doc.draft = Some(record.spec.clone());
                                doc.confirmed = false;
                                match record.spec.schedule {
                                    JobSchedule::EveryMinutes(minutes) => {
                                        doc.interval = minutes;
                                        doc.once = false;
                                    }
                                    JobSchedule::OnceUtc(at) => {
                                        doc.once = true;
                                        doc.time = chrono::DateTime::from_timestamp(at, 0)
                                            .map(|time| {
                                                time.with_timezone(&chrono::Local)
                                                    .format("%Y-%m-%d %H:%M:%S")
                                                    .to_string()
                                            })
                                            .unwrap_or_default();
                                    }
                                }
                            }
                            if record.spec.enabled
                                && ui
                                    .add_enabled(
                                        record.state != JobState::Running,
                                        egui::Button::new("暂停"),
                                    )
                                    .clicked()
                            {
                                let mut spec = record.spec.clone();
                                spec.enabled = false;
                                save = Some(spec);
                            }
                            if ui
                                .add_enabled(
                                    record.state != JobState::Running,
                                    egui::Button::new("删除计划"),
                                )
                                .clicked()
                            {
                                delete = Some(record.spec.id);
                            }
                        });
                        ui.small(&record.summary);
                        if let Some(at) = record.next_due_utc {
                            ui.small(format!(
                                "下次：{}",
                                chrono::DateTime::from_timestamp(at, 0)
                                    .map(|time| time.with_timezone(&chrono::Local).to_string())
                                    .unwrap_or_default()
                            ));
                        }
                        if let Some(output) = doc.outputs.get(&record.spec.id) {
                            ui.collapsing("最近执行结果", |ui| output_view(ui, output));
                        }
                    });
                }
                if doc.draft.is_none()
                    && ui.button("新增 SQL 计划").clicked()
                    && let Some(profile) = self.profiles.first()
                {
                    doc.draft = Some(JobSpec {
                        id: Uuid::new_v4(),
                        name: "SQL 任务".into(),
                        profile: profile.id,
                        database: String::new(),
                        schema: None,
                        sql: "SELECT 1;".into(),
                        schedule: JobSchedule::EveryMinutes(60),
                        enabled: false,
                        connection_fingerprint: None,
                    });
                }
                if let Some(spec) = &mut doc.draft {
                    ui.separator();
                    ui.strong("计划设置");
                    let before = serde_json::to_string(spec).unwrap_or_default();
                    ui.horizontal(|ui| {
                        ui.label("名称");
                        ui.text_edit_singleline(&mut spec.name);
                    });
                    egui::ComboBox::from_id_salt("job-profile")
                        .selected_text(
                            self.profiles
                                .iter()
                                .find(|profile| profile.id == spec.profile)
                                .map(|profile| profile.name.as_str())
                                .unwrap_or("选择连接"),
                        )
                        .show_ui(ui, |ui| {
                            for profile in &self.profiles {
                                ui.selectable_value(&mut spec.profile, profile.id, &profile.name);
                            }
                        });
                    ui.horizontal_wrapped(|ui| {
                        ui.label("数据库");
                        ui.text_edit_singleline(&mut spec.database);
                        ui.label("Schema（可选）");
                        let mut schema = spec.schema.clone().unwrap_or_default();
                        if ui.text_edit_singleline(&mut schema).changed() {
                            spec.schema = (!schema.trim().is_empty()).then_some(schema);
                        }
                    });
                    ui.add(
                        egui::TextEdit::multiline(&mut spec.sql)
                            .code_editor()
                            .desired_rows(7)
                            .desired_width(f32::INFINITY),
                    );
                    ui.horizontal_wrapped(|ui| {
                        ui.selectable_value(&mut doc.once, false, "按间隔");
                        ui.selectable_value(&mut doc.once, true, "单次");
                        if doc.once {
                            ui.label("本地时间");
                            ui.text_edit_singleline(&mut doc.time);
                        } else {
                            ui.add(egui::DragValue::new(&mut doc.interval).range(1..=525600));
                            ui.label("分钟");
                        }
                    });
                    spec.schedule = if doc.once {
                        use chrono::TimeZone;
                        match chrono::NaiveDateTime::parse_from_str(&doc.time, "%Y-%m-%d %H:%M:%S")
                            .ok()
                            .and_then(|time| chrono::Local.from_local_datetime(&time).single())
                        {
                            Some(time) => JobSchedule::OnceUtc(time.timestamp()),
                            None => JobSchedule::OnceUtc(0),
                        }
                    } else {
                        JobSchedule::EveryMinutes(doc.interval)
                    };
                    ui.checkbox(&mut spec.enabled, "保存后启用计划");
                    if before != serde_json::to_string(spec).unwrap_or_default() {
                        doc.confirmed = false;
                        doc.confirmed_profile = None;
                    }
                    let current_profile = self
                        .profiles
                        .iter()
                        .find(|profile| profile.id == spec.profile);
                    let profile_snapshot =
                        current_profile.and_then(|profile| serde_json::to_string(profile).ok());
                    if let Some(profile) = current_profile {
                        ui.small(format!(
                            "目标连接：{} · {}:{} · {}",
                            profile.engine.label(),
                            profile.host,
                            profile.port,
                            profile.username
                        ));
                    }
                    if doc.confirmed && doc.confirmed_profile != profile_snapshot {
                        doc.confirmed = false;
                        doc.confirmed_profile = None;
                        doc.message = Some("连接配置已改变，请核对目标后重新确认计划".into());
                    }
                    if spec.enabled
                        && ui
                            .checkbox(
                                &mut doc.confirmed,
                                "确认按此连接、目标和计划执行当前 SQL（包含写入）",
                            )
                            .changed()
                    {
                        doc.confirmed_profile = if doc.confirmed {
                            profile_snapshot.clone()
                        } else {
                            None
                        };
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                self.job_scheduler.is_some()
                                    && profile_snapshot.is_some()
                                    && (!spec.enabled
                                        || (doc.confirmed
                                            && doc.confirmed_profile == profile_snapshot)),
                                egui::Button::new("保存计划"),
                            )
                            .clicked()
                        {
                            let mut spec = spec.clone();
                            spec.connection_fingerprint = if spec.enabled {
                                doc.confirmed_profile.clone()
                            } else {
                                profile_snapshot.clone()
                            };
                            save = Some(spec);
                        }
                    });
                }
                if doc.draft.is_some() && ui.button("取消设置").clicked() {
                    doc.draft = None;
                    doc.confirmed = false;
                }
                if let Some(message) = &doc.message {
                    ui.label(message);
                }
            });
        if let Some(scheduler) = &mut self.job_scheduler {
            let result = if let Some(spec) = save {
                Some(scheduler.store.upsert(spec, chrono::Utc::now().timestamp()))
            } else {
                delete.map(|id| scheduler.store.delete(id))
            };
            if let Some(result) = result {
                let doc = self.tabs[self.active_tab].jobs.as_mut().unwrap();
                match result {
                    Ok(()) => {
                        doc.message = Some("计划已保存".into());
                        doc.draft = None;
                        doc.confirmed = false;
                    }
                    Err(error) => doc.message = Some(format!("计划未保存：{error:#}")),
                }
            }
        }
    }
}
