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

    fn management_tab(&mut self, name: String, target: Option<QueryTarget>) -> Option<Uuid> {
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

    fn open_jobs(&mut self) {
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
            sql: self.tabs[self.active_tab].sql.clone(),
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

    pub(super) fn poll_management(&mut self) {
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

    pub(super) fn management_fixture(&mut self) {
        if self.screenshot_path.is_none() {
            return;
        }
        let mode = std::env::var("SQLSHOP_SCREENSHOT_VIEW").unwrap_or_default();
        if !matches!(
            mode.as_str(),
            "plan"
                | "diagnostics"
                | "security"
                | "backup"
                | "compare"
                | "jobs"
                | "stop"
                | "transaction"
                | "stopped"
                | "oracle"
                | "sqlite"
        ) {
            return;
        }
        let profile = Profile {
            name: "离线界面示例（未连接数据库）".into(),
            engine: Engine::Postgres,
            port: 5432,
            ..Default::default()
        };
        let id = profile.id;
        let target = QueryTarget {
            connection: id,
            database: Some("app_dev".into()),
            schema: Some("public".into()),
        };
        self.profiles = vec![profile.clone()];
        self.selected = Some(id);
        let mut manager = ConnectionSession::new(&self.context);
        manager.connected = Some(profile.clone());
        manager.active_database = target.database.clone();
        manager.databases = vec!["app_dev".into()];
        self.sessions.insert(id, manager);
        self.browser_visible = false;
        self.inspector_visible = false;
        self.tabs[0].target = Some(target.clone());
        self.tabs[0].sql = "SELECT status, COUNT(*) FROM public.orders GROUP BY status;".into();
        self.tabs[0].saved_sql = self.tabs[0].sql.clone();
        self.status = "离线布局示例 · 没有建立连接或执行数据库操作".into();
        let source = AdvancedSource::new(profile.clone(), target.clone());
        match mode.as_str() {
            "oracle" => {
                self.tabs[0].oracle = Some(oracle_database_ui::OracleDocument::new(&self.context));
                self.tabs[0].name = "Oracle 查询".into();
                self.tabs[0].sql.clear();
                self.tabs[0].saved_sql.clear();
            }
            "sqlite" => {
                let mut doc = local_database_ui::LocalDocument::new(
                    PathBuf::from("离线示例.sqlite（未打开文件）"),
                    self.context.clone(),
                );
                doc.output = Some(QueryOutput {
                    sets: vec![ResultSet {
                        columns: vec!["id".into(), "name".into()],
                        rows: vec![vec![Some("1".into()), Some("离线示例记录".into())]],
                        ..Default::default()
                    }],
                    ..Default::default()
                });
                self.tabs[0].local_database = Some(doc);
                self.tabs[0].name = "SQLite 查询".into();
                self.tabs[0].sql.clear();
                self.tabs[0].saved_sql.clear();
            }
            "plan" | "diagnostics" | "security" => {
                let kind = match mode.as_str() {
                    "plan" => AdvancedKind::Plan,
                    "security" => AdvancedKind::Security,
                    _ => AdvancedKind::Diagnostics(sqlshop::diagnostics::DiagnosticKind::Activity),
                };
                let mut doc = AdvancedDocument::new(source, kind);
                doc.id = self.tabs[0].id;
                doc.sql = self.tabs[0].sql.clone();
                doc.notice = Some("以下内容为离线布局示例，不是服务器读取结果".into());
                if mode == "plan" {
                    doc.last_plan_sql = Some(doc.sql.clone());
                    doc.output = Some(QueryOutput { sets: vec![ResultSet { columns: vec!["QUERY PLAN".into()], rows: vec![vec![Some(r#"[{"Plan":{"Node Type":"Aggregate","Strategy":"Hashed","Startup Cost":230.0,"Total Cost":231.0,"Plan Rows":3,"Plans":[{"Node Type":"Seq Scan","Relation Name":"orders","Alias":"orders","Plan Rows":8000,"Total Cost":190.0}]}}]"#.into())]], ..Default::default() }], elapsed_ms: 4, ..Default::default() });
                } else if mode == "security" {
                    use sqlshop::security::{AccessPrincipal, PrincipalKind, SecurityCatalog};
                    let role = AccessPrincipal {
                        name: "report_reader".into(),
                        host: None,
                        kind: PrincipalKind::Role,
                        enabled: Some(false),
                        default_schema: None,
                        identity: None,
                    };
                    let user = AccessPrincipal {
                        name: "app_reader".into(),
                        host: None,
                        kind: PrincipalKind::User,
                        enabled: Some(true),
                        default_schema: None,
                        identity: None,
                    };
                    doc.selected_principal = Some(role.clone());
                    doc.role = Some(role.clone());
                    doc.member = Some(user.clone());
                    doc.catalog = Some(SecurityCatalog {
                        principals: vec![role, user],
                        ..Default::default()
                    });
                    doc.output = Some(QueryOutput {
                        sets: vec![ResultSet {
                            columns: vec!["对象".into(), "权限".into(), "来源".into()],
                            rows: vec![vec![
                                Some("public.orders".into()),
                                Some("SELECT".into()),
                                Some("直接授权（示例）".into()),
                            ]],
                            ..Default::default()
                        }],
                        ..Default::default()
                    });
                } else {
                    doc.output = Some(QueryOutput { sets: vec![ResultSet { columns: vec!["pid".into(), "database".into(), "user".into(), "state".into(), "wait_event".into(), "query".into()], rows: vec![vec![Some("1032".into()), Some("app_dev".into()), Some("app_reader".into()), Some("active".into()), None, Some("SELECT status, COUNT(*) FROM public.orders GROUP BY status".into())], vec![Some("1038".into()), Some("app_dev".into()), Some("app_writer".into()), Some("idle in transaction".into()), Some("ClientRead".into()), Some("UPDATE public.orders SET status = 'paid' WHERE id = 42".into())]], ..Default::default() }], ..Default::default() });
                }
                self.tabs[0].name = kind.label().into();
                self.tabs[0].sql.clear();
                self.tabs[0].saved_sql.clear();
                self.tabs[0].advanced = Some(doc);
            }
            "backup" => {
                let mut doc = backup_ui::BackupDocument::new(self.tabs[0].id, profile, "app_dev");
                doc.config.location = sqlshop::backup::BackupLocation::LocalFile(PathBuf::from(
                    "/tmp/example-app_dev.dump",
                ));
                self.tabs[0].name = "备份 · app_dev".into();
                self.tabs[0].sql.clear();
                self.tabs[0].saved_sql.clear();
                self.tabs[0].backup = Some(doc);
            }
            "compare" => {
                let endpoint = sqlshop::compare::CompareEndpoint {
                    profile,
                    target,
                    table: Table {
                        schema: "public".into(),
                        name: "orders".into(),
                        kind: "BASE TABLE".into(),
                    },
                };
                let mut destination = endpoint.clone();
                destination.target.database = Some("app_stage".into());
                self.tabs[0].compare = Some(compare_ui::CompareDocument::new(
                    self.tabs[0].id,
                    endpoint,
                    destination,
                ));
                self.tabs[0].name = "表比较与同步".into();
                self.tabs[0].sql.clear();
                self.tabs[0].saved_sql.clear();
            }
            "jobs" => {
                let spec = JobSpec {
                    id: Uuid::new_v4(),
                    name: "订单状态统计（示例）".into(),
                    profile: id,
                    database: "app_dev".into(),
                    schema: Some("public".into()),
                    sql: self.tabs[0].sql.clone(),
                    schedule: JobSchedule::EveryMinutes(60),
                    enabled: false,
                    connection_fingerprint: None,
                };
                self.tabs[0].jobs = Some(JobsDocument::new(Some(spec)));
                self.tabs[0].name = "任务调度".into();
                self.tabs[0].sql.clear();
                self.tabs[0].saved_sql.clear();
            }
            "stopped" => {
                self.tabs[0].query_started = true;
                self.tabs[0].query_error = Some("查询已停止，服务器已确认取消。先前语句可能已执行或提交。\n离线消息布局示例，未执行真实取消。".into());
                self.tabs[0].query_elapsed_ms = Some(1200);
                super::query_result_ui::show_error_view(&self.context, self.tabs[0].id);
            }
            _ => {
                let tab = self.tabs[0].id;
                self.query_sessions.insert(
                    tab,
                    query_session_ui::QuerySessionState {
                        worker: sqlshop::worker::QueryWorker::start(tab, || {}),
                        profile,
                        target,
                        busy: mode == "stop",
                        connected: true,
                        user_sql: true,
                        schemas_target: None,
                        schemas: vec!["public".into()],
                        schemas_loading: false,
                        schemas_error: None,
                        execution: (mode == "stop").then(Uuid::new_v4),
                        cancellable: mode == "stop",
                        cancel_requested: false,
                        transaction: sqlshop::query_control::QueryTransactionState::Active,
                        transaction_verified: true,
                    },
                );
            }
        }
    }

    fn jobs_workspace(&mut self, ui: &mut egui::Ui) {
        let records = self
            .job_scheduler
            .as_ref()
            .map(|scheduler| scheduler.store.records().to_vec())
            .unwrap_or_default();
        let doc = self.tabs[self.active_tab].jobs.as_mut().unwrap();
        let mut save = None;
        let mut delete = None;
        ui.heading("SQL 任务调度");
        ui.label("应用打开时按计划运行。失败或中断会暂停，错过的计划不补跑。SQL 以明文保存在本机，凭据只在运行时读取。");
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
