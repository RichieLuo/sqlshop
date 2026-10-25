mod fixtures;

mod jobs;

use super::*;
use sqlshop::{
    advanced::{AdvancedDocument, AdvancedKind, AdvancedSource},
    advanced_worker::AdvancedRequest,
    backup_worker::{BackupRequest, JobScheduler},
    jobs::{JobSchedule, JobSpec, JobState},
};

pub(super) struct JobsDocument {
    draft: Option<JobSpec>,
    interval: u32,
    once: bool,
    time: String,
    confirmed: bool,
    confirmed_profile: Option<String>,
    message: Option<String>,
    outputs: HashMap<Uuid, QueryOutput>,
}
impl JobsDocument {
    pub fn new(draft: Option<JobSpec>) -> Self {
        Self {
            draft,
            interval: 60,
            once: false,
            time: (chrono::Local::now() + chrono::Duration::hours(1))
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            confirmed: false,
            confirmed_profile: None,
            message: None,
            outputs: HashMap::new(),
        }
    }
    pub fn dirty(&self) -> bool {
        self.draft.is_some()
    }
}

impl QueryTab {
    pub(super) fn management_document(&self) -> bool {
        self.advanced.is_some()
            || self.oracle.is_some()
            || self.redis.is_some()
            || self.backup.is_some()
            || self.local_database.is_some()
            || self.jobs.is_some()
            || self.compare.is_some()
    }
}

impl App {
    pub(super) fn management_menu(&mut self, ui: &mut egui::Ui) {
        use sqlshop::diagnostics::DiagnosticKind;
        self.object_design_window();
        let ready = self.management_source().is_some();
        ui.menu_button("工具", |ui| {
            ui.add_enabled_ui(ready, |ui| {
                if ui.button("新建视图 / 例程 / 触发器 / 序列…").clicked() {
                    if let Some(source) = self.management_source() {
                        self.open_object_designer(source, None, None, false);
                    }
                    ui.close();
                }
                if ui.button("Schema / 数据库管理…").clicked() {
                    if let Some(source) = self.management_source() {
                        self.open_object_designer(source, None, None, true);
                    }
                    ui.close();
                }
                ui.separator();
                for kind in [
                    AdvancedKind::Plan,
                    AdvancedKind::Diagnostics(DiagnosticKind::Activity),
                    AdvancedKind::Diagnostics(DiagnosticKind::Locks),
                    AdvancedKind::Diagnostics(DiagnosticKind::SlowQueries),
                    AdvancedKind::Security,
                ] {
                    if ui.button(kind.label()).clicked() {
                        self.open_advanced(kind);
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("原生备份与恢复…").clicked() {
                    self.open_backup();
                    ui.close();
                }
                if ui.button("表数据 / 结构比较与同步…").clicked() {
                    self.open_compare();
                    ui.close();
                }
            });
            if ui.button("SQL 任务调度…").clicked() {
                self.open_jobs();
                ui.close();
            }
            ui.separator();
            if ui.button("Oracle 连接与查询…").clicked() {
                if self.management_tab("Oracle 查询".into(), None).is_some() {
                    self.tabs[self.active_tab].oracle =
                        Some(oracle_database_ui::OracleDocument::new(&self.context));
                }
                ui.close();
            }
            if ui.button("打开 SQLite 文件…").clicked() {
                self.open_local_database(false);
                ui.close();
            }
            if ui.button("新建 SQLite 文件…").clicked() {
                self.open_local_database(true);
                ui.close();
            }
        });
    }

    fn management_source(&self) -> Option<AdvancedSource> {
        let target = if self.browser_visible {
            self.browser_scope
                .as_ref()
                .map(|scope| QueryTarget {
                    connection: scope.connection,
                    database: scope.database.clone(),
                    schema: scope.schema.clone(),
                })
                .or_else(|| self.current_target())?
        } else {
            self.execution_target()?
        };
        target.database.as_ref()?;
        let profile = self
            .profiles
            .iter()
            .find(|profile| profile.id == target.connection)?
            .clone();
        Some(AdvancedSource::new(profile, target))
    }

    pub(super) fn management_tab(
        &mut self,
        name: String,
        target: Option<QueryTarget>,
    ) -> Option<Uuid> {
        if !self.apply_ui_tab_action(TabAction::New) {
            return None;
        }
        let tab = &mut self.tabs[self.active_tab];
        tab.sql.clear();
        tab.saved_sql.clear();
        tab.name = name;
        tab.target = target;
        tab.focus_editor = false;
        self.browser_visible = false;
        Some(tab.id)
    }

    fn open_advanced(&mut self, kind: AdvancedKind) {
        let Some(source) = self.management_source() else {
            self.fail("请先选择数据库".into());
            return;
        };
        if let Some(index) = self.tabs.iter().position(|tab| {
            tab.advanced
                .as_ref()
                .is_some_and(|doc| doc.kind == kind && doc.source.same_binding(&source))
        }) {
            self.active_tab = index;
            self.browser_visible = false;
            return;
        }
        let sql = self.tabs[self.active_tab].sql.clone();
        let Some(id) = self.management_tab(
            format!(
                "{} · {}",
                kind.label(),
                source.target.database.as_deref().unwrap_or_default()
            ),
            Some(source.target.clone()),
        ) else {
            return;
        };
        let mut doc = AdvancedDocument::new(source, kind);
        doc.id = id;
        if kind == AdvancedKind::Plan && !sql.trim().is_empty() {
            doc.sql = sql;
        }
        self.tabs[self.active_tab].advanced = Some(doc);
        // A plan is requested explicitly after inspecting the SQL on its page.
        if kind != AdvancedKind::Plan {
            let operation = self.tabs[self.active_tab]
                .advanced
                .as_ref()
                .unwrap()
                .refresh_operation();
            self.dispatch_advanced(operation);
        }
    }

    fn dispatch_advanced(&mut self, operation: sqlshop::advanced::AdvancedOperation) {
        if self.screenshot_path.is_some() {
            return;
        }
        let index = self.active_tab;
        let Some(source) = self.tabs[index]
            .advanced
            .as_ref()
            .map(|doc| doc.source.clone())
        else {
            return;
        };
        if self
            .profiles
            .iter()
            .find(|profile| profile.id == source.profile.id)
            .is_none_or(|profile| {
                serde_json::to_value(profile).ok() != serde_json::to_value(&source.profile).ok()
            })
        {
            self.tabs[index].advanced.as_mut().unwrap().error =
                Some("连接配置已改变，请重新打开此页面再操作".into());
            return;
        }
        let credentials = match self.credentials_for(&source.profile) {
            Ok(Some(credentials)) => credentials,
            result => {
                self.tabs[index].advanced.as_mut().unwrap().error = Some(match result {
                    Ok(None) => "此连接缺少凭据".into(),
                    Err(error) => error.to_string(),
                    _ => unreachable!(),
                });
                return;
            }
        };
        let doc = self.tabs[index].advanced.as_mut().unwrap();
        let Some(generation) = doc.begin(operation.clone()) else {
            return;
        };
        if let Err(error) = self.advanced_worker.send(AdvancedRequest {
            document: doc.id,
            generation,
            source,
            credentials,
            operation,
        }) {
            doc.busy = false;
            doc.pending = None;
            doc.error = Some(error);
        }
    }

    fn open_backup(&mut self) {
        let Some(source) = self.management_source() else {
            return;
        };
        let database = source.target.database.clone().unwrap();
        if let Some(index) = self.tabs.iter().position(|tab| {
            tab.backup.as_ref().is_some_and(|doc| {
                doc.profile.id == source.profile.id
                    && doc.config.database == database
                    && serde_json::to_value(&doc.profile).ok()
                        == serde_json::to_value(&source.profile).ok()
            })
        }) {
            self.active_tab = index;
            self.browser_visible = false;
            return;
        }
        let Some(id) = self.management_tab(format!("备份 · {database}"), Some(source.target))
        else {
            return;
        };
        self.tabs[self.active_tab].backup =
            Some(backup_ui::BackupDocument::new(id, source.profile, database));
    }

    fn open_compare(&mut self) {
        let Some(source) = self.management_source() else {
            return;
        };
        let table =
            if !self.browser_visible {
                self.tabs[self.active_tab]
                    .table_target
                    .as_ref()
                    .map(|table| Table {
                        schema: table.schema.clone(),
                        name: table.name.clone(),
                        kind: self.tabs[self.active_tab]
                            .table_kind
                            .clone()
                            .unwrap_or_else(|| "BASE TABLE".into()),
                    })
            } else {
                match &self.browser_state.selected {
                    Some(object_browser_ui::BrowserItem::Object {
                        connection,
                        database,
                        table,
                    }) if *connection == source.target.connection
                        && source.target.database.as_ref() == Some(database) =>
                    {
                        Some(table.clone())
                    }
                    _ => None,
                }
            }
            .unwrap_or_else(|| Table {
                schema: source.target.schema.clone().unwrap_or_else(|| {
                    match source.profile.engine {
                        Engine::MySql => source.target.database.clone().unwrap_or_default(),
                        Engine::Postgres => "public".into(),
                        Engine::SqlServer => "dbo".into(),
                    }
                }),
                name: String::new(),
                kind: "BASE TABLE".into(),
            });
        let endpoint = sqlshop::compare::CompareEndpoint {
            profile: source.profile,
            target: source.target.clone(),
            table,
        };
        if let Some(id) = self.management_tab("表比较与同步".into(), Some(source.target)) {
            self.tabs[self.active_tab].compare = Some(compare_ui::CompareDocument::new(
                id,
                endpoint.clone(),
                endpoint,
            ));
        }
    }

    fn dispatch_compare(&mut self, action: compare_ui::CompareAction) {
        if self.screenshot_path.is_some() {
            return;
        }
        let index = self.active_tab;
        let document = self.tabs[index].id;
        let generation = match &action {
            compare_ui::CompareAction::Compare { generation, .. }
            | compare_ui::CompareAction::Apply { generation, .. }
            | compare_ui::CompareAction::ApplyStructure { generation, .. } => *generation,
        };
        let request = (|| -> anyhow::Result<sqlshop::compare_worker::CompareRequest> {
            let checked_credentials =
                |endpoint: &sqlshop::compare::CompareEndpoint| -> anyhow::Result<Credentials> {
                    anyhow::ensure!(
                        self.profiles
                            .iter()
                            .find(|profile| profile.id == endpoint.profile.id)
                            .is_some_and(|profile| serde_json::to_value(profile).ok()
                                == serde_json::to_value(&endpoint.profile).ok()),
                        "比较连接配置已改变，请重新选择来源再比较"
                    );
                    self.credentials_for(&endpoint.profile)?
                        .ok_or_else(|| anyhow::anyhow!("比较连接缺少凭据"))
                };
            Ok(match action {
                compare_ui::CompareAction::Compare {
                    generation,
                    source,
                    target,
                } => {
                    let source_credentials = checked_credentials(&source)?;
                    let target_credentials = checked_credentials(&target)?;
                    sqlshop::compare_worker::CompareRequest::Compare {
                        document,
                        generation,
                        source,
                        target,
                        source_credentials,
                        target_credentials,
                    }
                }
                compare_ui::CompareAction::Apply { generation, report } => {
                    let _ = checked_credentials(&report.source.endpoint)?;
                    let target_credentials = checked_credentials(&report.target.endpoint)?;
                    sqlshop::compare_worker::CompareRequest::Apply {
                        document,
                        generation,
                        report,
                        target_credentials,
                    }
                }
                compare_ui::CompareAction::ApplyStructure {
                    generation,
                    report,
                    candidate,
                } => {
                    let source_credentials = checked_credentials(&report.source.endpoint)?;
                    let target_credentials = checked_credentials(&report.target.endpoint)?;
                    sqlshop::compare_worker::CompareRequest::ApplyStructure {
                        document,
                        generation,
                        report,
                        candidate,
                        source_credentials,
                        target_credentials,
                    }
                }
            })
        })();
        if let Err(error) = request.and_then(|request| self.compare_worker.send(request)) {
            self.tabs[index]
                .compare
                .as_mut()
                .unwrap()
                .dispatch_failed(generation, format!("{error:#}"));
        }
    }

    fn dispatch_backup(&mut self, action: backup_ui::BackupAction) {
        if self.screenshot_path.is_some() {
            return;
        }
        let index = self.active_tab;
        let Some(profile) = self.tabs[index]
            .backup
            .as_ref()
            .map(|doc| doc.profile.clone())
        else {
            return;
        };
        let document = self.tabs[index].id;
        let generation = match &action {
            backup_ui::BackupAction::Detect { generation, .. }
            | backup_ui::BackupAction::Execute { generation, .. } => *generation,
        };
        let request = (|| -> anyhow::Result<BackupRequest> {
            anyhow::ensure!(
                self.profiles
                    .iter()
                    .find(|current| current.id == profile.id)
                    .is_some_and(|current| serde_json::to_value(current).ok()
                        == serde_json::to_value(&profile).ok()),
                "连接配置已改变，请重新打开备份页"
            );
            Ok(match action {
                backup_ui::BackupAction::Detect { generation, config } => BackupRequest::Detect {
                    document,
                    generation,
                    profile,
                    config,
                },
                backup_ui::BackupAction::Execute { generation, config } => {
                    let credentials = self
                        .credentials_for(&profile)?
                        .ok_or_else(|| anyhow::anyhow!("此连接缺少凭据"))?;
                    BackupRequest::Execute {
                        document,
                        generation,
                        profile,
                        credentials,
                        config,
                    }
                }
            })
        })();
        if let Err(error) = request.and_then(|request| self.backup_worker.send(request)) {
            self.tabs[index]
                .backup
                .as_mut()
                .unwrap()
                .dispatch_failed(generation, format!("{error:#}"));
        }
    }

    fn open_local_database(&mut self, create: bool) {
        if self.screenshot_path.is_some() {
            return;
        }
        let dialog = rfd::FileDialog::new().add_filter("SQLite", &["sqlite", "sqlite3", "db"]);
        let path = if create {
            dialog.save_file()
        } else {
            dialog.pick_file()
        };
        let Some(path) = path else {
            return;
        };
        if create {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            if let Err(error) = options.open(&path) {
                self.fail(format!("无法创建 SQLite 文件（不覆盖已有文件）：{error}"));
                return;
            }
        }
        let identity = match std::fs::canonicalize(&path) {
            Ok(path) => path,
            Err(error) => {
                self.fail(error.to_string());
                return;
            }
        };
        if let Some(index) = self.tabs.iter().position(|tab| {
            tab.local_database
                .as_ref()
                .is_some_and(|doc| doc.path == identity)
        }) {
            self.active_tab = index;
            self.browser_visible = false;
            return;
        }
        let name = identity
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if self
            .management_tab(format!("SQLite · {name}"), None)
            .is_some()
        {
            self.tabs[self.active_tab].local_database = Some(
                local_database_ui::LocalDocument::new(identity, self.context.clone()),
            );
        }
    }

    pub(super) fn poll_management(&mut self) {
        for doc in self.tabs.iter_mut().filter_map(|tab| tab.redis.as_mut()) {
            doc.poll();
        }
        for doc in self.tabs.iter_mut().filter_map(|tab| tab.oracle.as_mut()) {
            doc.poll();
        }
        while let Some(response) = self.compare_worker.try_recv() {
            if let Some(doc) = self
                .tabs
                .iter_mut()
                .filter_map(|tab| tab.compare.as_mut())
                .find(|doc| doc.id == response.document)
            {
                doc.apply_response(response);
            }
        }
        while let Ok(response) = self.advanced_worker.try_recv() {
            if let Some(doc) = self
                .tabs
                .iter_mut()
                .filter_map(|tab| tab.advanced.as_mut())
                .find(|doc| doc.id == response.document)
            {
                doc.apply_response(response);
            }
        }
        while let Some(response) = self.backup_worker.try_recv() {
            if let Some(doc) = self
                .tabs
                .iter_mut()
                .filter_map(|tab| tab.backup.as_mut())
                .find(|doc| doc.id == response.document)
            {
                doc.apply_response(response);
            }
        }
        for doc in self
            .tabs
            .iter_mut()
            .filter_map(|tab| tab.local_database.as_mut())
        {
            doc.poll();
        }
        if self.screenshot_path.is_some() {
            return;
        }
        if !self.jobs_loaded
            && let Some(store) = &self.store
        {
            self.jobs_loaded = true;
            match JobScheduler::load(&store.directory) {
                Ok(scheduler) => self.job_scheduler = Some(scheduler),
                Err(error) => self.fail(format!("任务调度加载失败，未执行计划：{error:#}")),
            }
        }
        if self.last_job_tick.elapsed().as_secs() >= 1 {
            self.last_job_tick = std::time::Instant::now();
            if let Some(mut scheduler) = self.job_scheduler.take() {
                let profiles = self.profiles.clone();
                match scheduler.tick(chrono::Utc::now().timestamp(), &profiles, |profile| {
                    self.credentials_for(profile)?
                        .ok_or_else(|| anyhow::anyhow!("缺少计划任务凭据"))
                }) {
                    Ok(completed) => {
                        for completion in completed {
                            self.status = completion.summary.clone();
                            self.error = !completion.success;
                            if let Some(output) = completion.output {
                                for doc in self.tabs.iter_mut().filter_map(|tab| tab.jobs.as_mut())
                                {
                                    doc.outputs.insert(completion.id, output.clone());
                                }
                            }
                        }
                    }
                    Err(error) => self.fail(format!("计划任务未派发：{error:#}")),
                }
                self.job_scheduler = Some(scheduler);
            }
        }
        if self.tabs.iter().any(|tab| {
            tab.backup.as_ref().is_some_and(|doc| doc.pending.is_some())
                || tab
                    .compare
                    .as_ref()
                    .is_some_and(|doc| doc.pending.is_some())
        }) {
            self.context
                .request_repaint_after(std::time::Duration::from_millis(200));
        }
        if self.job_scheduler.as_ref().is_some_and(|scheduler| {
            scheduler
                .store
                .records()
                .iter()
                .any(|record| record.spec.enabled || record.state == JobState::Running)
        }) {
            self.context
                .request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    pub(super) fn management_workspace(&mut self, ui: &mut egui::Ui) {
        let index = self.active_tab;
        if let Some(doc) = self.tabs[index].redis.as_mut() {
            doc.show(ui);
            return;
        }
        if let Some(doc) = self.tabs[index].oracle.as_mut() {
            doc.show(ui);
            return;
        }
        if let Some(doc) = self.tabs[index].advanced.as_mut() {
            let action = advanced_ui::show(ui, doc);
            if let Some(advanced_ui::AdvancedAction::Run(operation)) = action {
                self.dispatch_advanced(operation);
            }
        } else if let Some(doc) = self.tabs[index].backup.as_mut() {
            let action = egui::ScrollArea::vertical()
                .id_salt(("backup", doc.id))
                .show(ui, |ui| backup_ui::show(ui, doc))
                .inner;
            if let Some(action) = action {
                self.dispatch_backup(action);
            }
        } else if let Some(doc) = self.tabs[index].local_database.as_mut() {
            local_database_ui::show(ui, doc);
        } else if self.tabs[index].jobs.is_some() {
            self.jobs_workspace(ui);
        } else if let Some(doc) = self.tabs[index].compare.as_mut() {
            let action = compare_ui::show(ui, doc, &self.profiles);
            if let Some(action) = action {
                self.dispatch_compare(action);
            }
        }
    }
}

pub(super) fn output(ui: &mut egui::Ui, output: &QueryOutput) {
    output_view(ui, output);
}
fn output_view(ui: &mut egui::Ui, output: &QueryOutput) {
    for message in &output.messages {
        ui.add(
            egui::Label::new(format!("{}：{}", message.severity, message.text))
                .selectable(true)
                .wrap(),
        );
    }
    for (index, set) in output.sets.iter().enumerate() {
        ui.push_id(index, |ui| {
            ui.label(format!(
                "结果 {} · {} 行 · {} ms{}",
                index + 1,
                set.rows.len(),
                output.elapsed_ms,
                if set.truncated {
                    "（显示截断）"
                } else {
                    ""
                }
            ));
            if let Some(affected) = set.affected {
                ui.label(format!("服务器报告 {affected} 行"));
            }
            if !set.columns.is_empty() {
                egui::ScrollArea::horizontal()
                    .id_salt("management-result")
                    .show(ui, |ui| {
                        let mut table = TableBuilder::new(ui).striped(true).resizable(true);
                        for _ in &set.columns {
                            table = table.column(Column::initial(170.0).range(70.0..=800.0));
                        }
                        table
                            .header(26.0, |mut header| {
                                for name in &set.columns {
                                    header.col(|ui| {
                                        ui.strong(name);
                                    });
                                }
                            })
                            .body(|body| {
                                body.rows(24.0, set.rows.len(), |mut row| {
                                    for value in &set.rows[row.index()] {
                                        row.col(|ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    value.as_deref().unwrap_or("NULL"),
                                                )
                                                .truncate(),
                                            )
                                            .on_hover_text(value.as_deref().unwrap_or("NULL"));
                                        });
                                    }
                                });
                            });
                    });
            }
        });
    }
}
