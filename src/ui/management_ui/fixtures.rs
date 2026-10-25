use super::*;

impl App {
    pub(in crate::ui) fn management_fixture(&mut self) {
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
                | "redis"
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
            "redis" => {
                let document = redis_ui::RedisDocument::fixture(&self.context);
                let profile = document.bound_profile().clone();
                self.selected = Some(profile.id);
                self.redis_profiles = vec![profile.clone()];
                self.expand_connection = Some(profile.id);
                self.tabs[0].redis = Some(document);
                self.tabs[0].target = None;
                self.tabs[0].name = profile.name;
                self.tabs[0].sql.clear();
                self.tabs[0].saved_sql.clear();
            }
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
}
