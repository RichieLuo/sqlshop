/// A query tab owns this worker independently of its server's catalog/table
/// worker. Dropping an idle worker releases its session (and rolls back open
/// transactions). An in-flight operation runs to completion or timeout first;
/// neither dropping the worker nor queued Disconnect requests cancel that SQL.
/// Do not move it between tabs or recreate it after an ordinary SQL error.
use super::*;

pub struct QueryWorker {
    sender: tokio::sync::mpsc::UnboundedSender<QueryRequest>,
    cancellations: tokio::sync::mpsc::UnboundedSender<Uuid>,
    pub receiver: mpsc::Receiver<QueryResponse>,
}

// Intentionally has no Debug implementation: requests contain credentials.
// Keep the connection payload owned, matching the management Request API.
#[allow(clippy::large_enum_variant)]
pub enum QueryRequest {
    Transaction {
        target: QueryTarget,
        command: crate::query_control::TransactionCommand,
    },
    InspectTransaction {
        target: QueryTarget,
    },
    InspectContext {
        target: QueryTarget,
    },
    LoadSchemas {
        profile: Profile,
        credentials: Credentials,
        target: QueryTarget,
    },
    Run {
        profile: Profile,
        credentials: Credentials,
        target: QueryTarget,
        sql: String,
    },
    Disconnect,
}

pub enum QueryResponse {
    ContextChecked {
        tab: Uuid,
        target: QueryTarget,
    },
    ExecutionStarted {
        tab: Uuid,
        execution: Uuid,
        cancellable: bool,
        reason: Option<String>,
    },
    CancelStatus {
        tab: Uuid,
        execution: Uuid,
        message: String,
        failed: bool,
    },
    TransactionState {
        tab: Uuid,
        state: crate::query_control::QueryTransactionState,
        verified: bool,
    },
    ControlFinished {
        tab: Uuid,
        message: String,
        connected: bool,
    },
    SchemasLoaded {
        tab: Uuid,
        target: QueryTarget,
        schemas: Vec<String>,
    },
    SchemaLoadError {
        tab: Uuid,
        target: QueryTarget,
        message: String,
    },
    Finished {
        tab: Uuid,
        output: QueryOutput,
        database: Option<String>,
        schema: Option<String>,
    },
    Error {
        tab: Uuid,
        message: String,
        database: Option<String>,
        schema: Option<String>,
        connected: bool,
        diagnostic: Option<crate::model::QueryDiagnostic>,
        elapsed_ms: Option<u128>,
        partial_output: Option<QueryOutput>,
    },
    Disconnected {
        tab: Uuid,
    },
}

struct QuerySession {
    session: Session,
    target: QueryTarget,
    engine: Engine,
    profile: Profile,
}

fn query_profile_matches(left: &Profile, right: &Profile) -> bool {
    left.engine == right.engine
        && left.sqlserver_auth == right.sqlserver_auth
        && left.sqlserver_instance == right.sqlserver_instance
        && left.host == right.host
        && left.port == right.port
        && left.username == right.username
        && left.database == right.database
        && left.tls == right.tls
        && left.trust_certificate == right.trust_certificate
        && left.ssh.enabled == right.ssh.enabled
        && (!left.ssh.enabled
            || (left.ssh.host == right.ssh.host
                && left.ssh.port == right.ssh.port
                && left.ssh.username == right.ssh.username
                && left.ssh.auth == right.ssh.auth
                && left.ssh.private_key == right.ssh.private_key
                && left.ssh.known_hosts == right.ssh.known_hosts
                && left.ssh.fingerprint == right.ssh.fingerprint))
}

impl QuerySession {
    async fn connect(
        profile: &Profile,
        credentials: &Credentials,
        target: &QueryTarget,
    ) -> anyhow::Result<Self> {
        // A restricted PostgreSQL account may only have CONNECT on the target.
        // Include it in the established bootstrap fallback without changing the
        // saved profile or the management worker's database.
        let mut bootstrap = profile.clone();
        if let Some(database) = &target.database {
            bootstrap.database = database.clone();
        }
        let mut session = Session::connect(&bootstrap, credentials)
            .await
            .map_err(|error| crate::db::connection_error(profile, error))?;
        let mut database = session.current_database().await?;
        if let Some(selected) = &target.database {
            if database.as_deref() != Some(selected.as_str()) {
                session.select_database(selected).await?;
                database = session.current_database().await?;
            }
            anyhow::ensure!(
                database.as_deref() == Some(selected.as_str()),
                "查询会话未能进入指定数据库，SQL 未执行"
            );
        }
        if let Some(schema) = &target.schema {
            if matches!(profile.engine, Engine::Postgres | Engine::SqlServer) {
                session.select_query_schema(schema).await?;
            } else {
                anyhow::ensure!(
                    profile.engine == Engine::MySql && target.database.as_ref() == Some(schema),
                    "此引擎不能改变会话默认 Schema；请使用限定对象名，SQL 未执行"
                );
            }
        }
        let schema = session.current_query_schema().await?;
        let _ = session.prepare_query_context().await;
        // Failure to establish a safe cancellation identity disables Stop only.
        let _ = session.prepare_query_control().await;
        session.confirm_observed_query_database(database.clone());
        Ok(Self {
            session,
            target: QueryTarget {
                connection: target.connection,
                database,
                schema,
            },
            engine: profile.engine,
            profile: profile.clone(),
        })
    }

    async fn refresh_database(&mut self) -> anyhow::Result<()> {
        if self.engine != Engine::Postgres {
            self.target.database = self.session.observed_query_database().await?;
            if self.engine == Engine::MySql {
                self.target.schema = self.target.database.clone();
            }
            // SQL Server's default schema is captured on connection or explicit
            // inspection. A hidden SCHEMA_NAME() would overwrite @@ROWCOUNT.
            return Ok(());
        }
        match self.session.current_query_schema().await {
            Ok(schema) => self.target.schema = schema,
            Err(error)
                if self.engine == Engine::Postgres
                    && crate::db::query_error_preserves_session(&error) => {}
            Err(error) => return Err(error),
        }
        Ok(())
    }
}

impl QueryWorker {
    pub fn start(tab: Uuid, wake: impl Fn() + Send + 'static) -> Self {
        let (sender, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let (cancellations, mut cancellation_requests) = tokio::sync::mpsc::unbounded_channel();
        let (responses, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let wake = std::rc::Rc::new(wake);
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = responses.send(QueryResponse::Error {
                        tab,
                        message: error.to_string(),
                        database: None,
                        schema: None,
                        connected: false,
                        diagnostic: None,
                        elapsed_ms: None,
                        partial_output: None,
                    });
                    wake();
                    return;
                }
            };
            // The driver futures stay on this actor's current-thread runtime.
            // Local tasks let independent schema discovery progress during SQL
            // I/O without adding Send bounds to the existing driver APIs.
            let local = tokio::task::LocalSet::new();
            runtime.block_on(local.run_until(async move {
                let mut live: Option<QuerySession> = None;
                while let Some(request) = requests.recv().await {
                    let response = match request {
                        QueryRequest::InspectContext { target } => {
                            if let Some(current) = live.as_mut().filter(|current| current.target == target) {
                                let result = tokio::time::timeout(Duration::from_secs(30), async {
                                    let database = current.session.current_database().await?;
                                    let schema = current.session.current_query_schema().await?;
                                    current.session.confirm_observed_query_database(database.clone());
                                    current.target.database = database; current.target.schema = schema;
                                    Ok::<_, anyhow::Error>(())
                                }).await;
                                let connected = matches!(&result, Ok(Ok(()))) || matches!(&result, Ok(Err(error)) if crate::db::query_error_preserves_session(error));
                                let message = match result {
                                    Ok(Ok(())) => { let _ = responses.send(QueryResponse::ContextChecked { tab, target: current.target.clone() }); "执行上下文已重新读取；服务器语句诊断值可能已改变".into() },
                                    Ok(Err(error)) => format!("检查上下文失败：{error:#}"),
                                    Err(_) => "检查上下文超时，查询连接已关闭".into(),
                                };
                                if !connected { live = None; }
                                QueryResponse::ControlFinished { tab, message, connected }
                            } else { QueryResponse::ControlFinished { tab, message: "原查询会话已变化，未检查上下文".into(), connected: live.is_some() } }
                        }
                        QueryRequest::Transaction { target, command } => {
                            if let Some(current) = live.as_mut().filter(|current| current.target == target) {
                                let result = tokio::time::timeout(Duration::from_secs(60), current.session.transaction_command(command)).await;
                                let mut connected = matches!(&result, Ok(Ok(_))) || matches!(&result, Ok(Err(e)) if crate::db::query_error_preserves_session(e) || e.downcast_ref::<crate::db::EditRejected>().is_some());
                                let state = if connected {
                                    match tokio::time::timeout(Duration::from_secs(15), current.session.transaction_state()).await {
                                        Ok(Ok(state)) => state,
                                        _ => { connected = false; crate::query_control::QueryTransactionState::Unknown }
                                    }
                                } else { crate::query_control::QueryTransactionState::Unknown };
                                let _ = responses.send(QueryResponse::TransactionState { tab, state, verified: false });
                                let message = match result { Ok(Ok(_)) => format!("事务操作完成：{command:?}{}", if connected { "" } else { "；后续状态读取失败，查询连接已关闭" }), Ok(Err(e)) => format!("{e:#}；事务结果请核实，不自动重试"), Err(_) => "事务操作超时，提交结果未知，请重新连接核实".into() };
                                if !connected { live = None; }
                                QueryResponse::ControlFinished { tab, message, connected }
                            } else {
                                QueryResponse::ControlFinished { tab, message: "原查询会话已变化，事务操作未执行".into(), connected: live.is_some() }
                            }
                        }
                        QueryRequest::InspectTransaction { target } => {
                            if let Some(current) = live.as_mut().filter(|current| current.target == target) {
                                let result = tokio::time::timeout(Duration::from_secs(30), current.session.inspect_transaction_state()).await;
                                let connected = matches!(&result, Ok(Ok(_))) || matches!(&result, Ok(Err(e)) if crate::db::query_error_preserves_session(e));
                                let (state, verified, message) = match result { Ok(Ok(state)) => (state, true, "事务状态已核实".into()), Ok(Err(e)) => (crate::query_control::QueryTransactionState::Unknown, false, format!("核实事务状态失败：{e:#}")), Err(_) => (crate::query_control::QueryTransactionState::Unknown, false, "核实事务状态超时，查询连接已关闭".into()) };
                                let _ = responses.send(QueryResponse::TransactionState { tab, state, verified });
                                if !connected { live = None; }
                                QueryResponse::ControlFinished { tab, message, connected }
                            } else { QueryResponse::ControlFinished { tab, message: "没有可核实的原查询会话".into(), connected: live.is_some() } }
                        }
                        QueryRequest::LoadSchemas { profile, credentials, target } => {
                            let schema_responses = responses.clone();
                            let schema_wake = wake.clone();
                            // This task owns a separate read-only connection. It
                            // must not delay Run/Disconnect on the user's actor.
                            tokio::task::spawn_local(async move {
                                let loaded = tokio::time::timeout(Duration::from_secs(30), async {
                                    anyhow::ensure!(profile.id == target.connection, "Schema 列表目标与连接配置不一致");
                                    let mut discovery_target = target.clone();
                                    // A saved schema may have been removed; listing the
                                    // remaining schemas must still be possible.
                                    discovery_target.schema = None;
                                    let mut discovery = QuerySession::connect(&profile, &credentials, &discovery_target).await?;
                                    discovery.session.query_schemas().await
                                }).await;
                                let response = match loaded {
                                    Ok(Ok(schemas)) => QueryResponse::SchemasLoaded { tab, target, schemas },
                                    error => QueryResponse::SchemaLoadError {
                                        tab, target,
                                        message: match error {
                                            Ok(Err(error)) => format!("读取 Schema 列表失败：{error:#}"),
                                            Err(_) => "读取 Schema 列表超过 30 秒".into(),
                                            _ => unreachable!(),
                                        },
                                    },
                                };
                                if schema_responses.send(response).is_ok() {
                                    schema_wake();
                                }
                            });
                            continue;
                        }
                        QueryRequest::Disconnect => {
                            live = None;
                            QueryResponse::Disconnected { tab }
                        }
                        QueryRequest::Run { profile, credentials, target, sql } => {
                            let invalid = if profile.id != target.connection {
                                Some("查询目标与连接配置不一致，SQL 未执行")
                            } else if live.as_ref().is_some_and(|current| current.target != target) {
                                Some("查询标签仍持有原数据库会话；请先显式断开或重新绑定目标，SQL 未执行")
                            } else if live.as_ref().is_some_and(|current| !query_profile_matches(&current.profile, &profile)) {
                                Some("连接配置已改变；请先显式结束当前查询会话再使用新配置，SQL 未执行")
                            } else if sql.trim().is_empty() {
                                Some("SQL 不能为空")
                            } else {
                                None
                            };
                            if let Some(message) = invalid {
                                QueryResponse::Error {
                                    tab,
                                    message: message.into(),
                                    database: live.as_ref().and_then(|current| current.target.database.clone()),
                                    schema: live.as_ref().and_then(|current| current.target.schema.clone()),
                                    connected: live.is_some(),
                                    diagnostic: None,
                                    elapsed_ms: None,
                                    partial_output: None,
                                }
                            } else {
                                let connected = if live.is_some() {
                                    Ok(())
                                } else {
                                    match tokio::time::timeout(Duration::from_secs(30),
                                        QuerySession::connect(&profile, &credentials, &target)
                                    ).await {
                                        Ok(Ok(current)) => { live = Some(current); Ok(()) }
                                        Ok(Err(error)) => Err(format!("建立查询会话失败，SQL 未执行：{error:#}")),
                                        Err(_) => Err("建立查询会话超过 30 秒，SQL 未执行".into()),
                                    }
                                };
                                match connected {
                                    Err(message) => QueryResponse::Error { tab, message, database: None, schema: None, connected: false, diagnostic: None, elapsed_ms: None, partial_output: None },
                                    Ok(()) => {
                                        let current = live.as_mut().expect("connected query session");
                                        let started = std::time::Instant::now();
                                        let mut failed_query_diagnostic = None;
                                        let mut failed_query_output = None;
                                        let result = tokio::time::timeout(Duration::from_secs(3600), async {
                                            let result = cancellable_query(&mut current.session, &sql, tab, &mut cancellation_requests, &responses, &wake).await.map(|output| {
                                                // Keep ownership outside the timed future so a failed
                                                // probe or timeout cannot discard completed results.
                                                failed_query_output = Some(output);
                                            });
                                            if let Err(error) = &result {
                                                failed_query_diagnostic = Some(crate::db::query_diagnostic(error, &sql));
                                                failed_query_output = crate::db::query_partial_output(error);
                                            }
                                            let state = tokio::time::timeout(Duration::from_secs(15), current.session.transaction_state()).await.map_err(|_| anyhow::anyhow!("事务尾响应超时，查询连接不再复用"))??;
                                            let _ = responses.send(QueryResponse::TransactionState { tab, state, verified: false });
                                            match result {
                                                Ok(()) => {
                                                    tokio::time::timeout(Duration::from_secs(15), current.refresh_database()).await.map_err(|_| anyhow::anyhow!("查询上下文观察超时，连接不再复用"))??;
                                                    Ok::<_, anyhow::Error>(Ok(failed_query_output.take().expect("completed query output")))
                                                }
                                                Err(error) if crate::db::query_error_preserves_session(&error) && error.downcast_ref::<CancelTransportFailure>().is_none() => {
                                                    let diagnostic = crate::db::query_diagnostic(&error, &sql);
                                                    let partial_output = crate::db::query_partial_output(&error);
                                                    tokio::time::timeout(Duration::from_secs(15), current.refresh_database()).await.map_err(|_| anyhow::anyhow!("查询上下文观察超时，连接不再复用"))??;
                                                    let message = if crate::db::is_query_cancelled(&error) { "查询已停止，服务器已确认取消。先前语句可能已经执行或提交；需要时显式回滚并核实数据。".to_owned() } else { format!("{error:#}\n查询会话已保留；未结束的事务可显式执行 ROLLBACK。批处理中先前语句可能已提交，勿直接重试整批写入。") };
                                                    Ok(Err((message, diagnostic, partial_output)))
                                                }
                                                Err(error) => Err(error),
                                            }
                                        }).await;
                                        match result {
                                            Ok(Ok(Ok(output))) => QueryResponse::Finished {
                                                tab, output, database: current.target.database.clone(),
                                                schema: current.target.schema.clone(),
                                            },
                                            Ok(Ok(Err((message, diagnostic, partial_output)))) => QueryResponse::Error {
                                                tab, message, database: current.target.database.clone(), connected: true,
                                                schema: current.target.schema.clone(),
                                                diagnostic: Some(diagnostic), elapsed_ms: Some(started.elapsed().as_millis()),
                                                partial_output,
                                            },
                                            failure => {
                                                let (message, diagnostic) = match failure {
                                                    Ok(Err(error)) => (format!("{error:#}\n查询连接已关闭；先前语句可能已执行或提交，请重新连接并核对实际数据，勿直接重试整批写入。"), failed_query_diagnostic),
                                                    Err(_) => ("查询超过 3600 秒，连接已关闭；写入是否提交请重新连接后核对，勿直接重试。".into(), None),
                                                    _ => unreachable!(),
                                                };
                                                live = None;
                                                QueryResponse::Error { tab, message, database: None, schema: None, connected: false, diagnostic, elapsed_ms: Some(started.elapsed().as_millis()), partial_output: failed_query_output }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    };
                    if responses.send(response).is_err() { break; }
                    wake();
                }
            }));
        });
        Self {
            sender,
            cancellations,
            receiver,
        }
    }

    pub fn cancel(&self, execution: Uuid) -> anyhow::Result<()> {
        self.cancellations
            .send(execution)
            .map_err(|_| anyhow::anyhow!("原查询工作线程已退出"))
    }

    pub fn send(&self, request: QueryRequest) -> anyhow::Result<()> {
        self.sender
            .send(request)
            .map_err(|_| anyhow::anyhow!("查询工作线程已退出，请重新打开查询标签"))
    }
}

async fn cancellable_query(
    session: &mut Session,
    sql: &str,
    tab: Uuid,
    cancellations: &mut tokio::sync::mpsc::UnboundedReceiver<Uuid>,
    responses: &mpsc::Sender<QueryResponse>,
    wake: &std::rc::Rc<impl Fn()>,
) -> anyhow::Result<QueryOutput> {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    session.prepare_user_query().await?;
    let execution = Uuid::new_v4();
    let (handle, reason) = match session.cancel_handle() {
        Ok(handle) => (Some(handle), None),
        Err(error) => (None, Some(format!("停止不可用：{error:#}"))),
    };
    let _ = responses.send(QueryResponse::ExecutionStarted {
        tab,
        execution,
        cancellable: handle.is_some(),
        reason,
    });
    wake();
    let active = Arc::new(AtomicBool::new(true));
    struct ExecutionGuard {
        active: Arc<AtomicBool>,
        handle: Option<crate::db::QueryCancelHandle>,
        tasks: Vec<tokio::task::AbortHandle>,
    }
    impl Drop for ExecutionGuard {
        fn drop(&mut self) {
            self.active.store(false, Ordering::SeqCst);
            if let Some(handle) = &self.handle {
                handle.disarm();
            }
            for task in &self.tasks {
                task.abort();
            }
        }
    }
    let mut guard = ExecutionGuard {
        active: active.clone(),
        handle: handle.clone(),
        tasks: Vec::new(),
    };
    let mut control: Option<tokio::task::JoinHandle<anyhow::Result<bool>>> = None;
    let mut control_failure = None;
    let mut requested = false;
    let result;
    {
        let query = session.query(sql);
        tokio::pin!(query);
        loop {
            tokio::select! {
                value = &mut query => { result = value; break; }
                Some(generation) = cancellations.recv() => {
                    if generation == execution && !requested && let Some(handle) = handle.clone() {
                        requested = true;
                        let active = active.clone();
                        control = Some(tokio::task::spawn_local(async move {
                            tokio::time::timeout(Duration::from_secs(30), handle.cancel_if_active(active)).await
                                .map_err(|_| anyhow::anyhow!("取消控制通道超时，不能确认取消已送达"))?
                        }));
                        guard.tasks.push(control.as_ref().unwrap().abort_handle());
                    }
                }
                completed = async { control.as_mut().expect("enabled cancellation").await }, if control.is_some() => {
                    control = None;
                    let outcome = completed.map_err(anyhow::Error::from).and_then(|result| result);
                    let (message, failed) = match outcome {
                        Ok(true) => ("已发送停止请求，正在等待服务器确认…".into(), false),
                        Ok(false) => ("查询已经结束，停止请求未发送".into(), false),
                        Err(error) => { let message = format!("停止请求失败：{error:#}；正在等待原查询结束"); control_failure = Some(message.clone()); (message, true) }
                    };
                    let _ = responses.send(QueryResponse::CancelStatus { tab, execution, message, failed });
                    wake();
                }
            }
        }
    }
    active.store(false, Ordering::SeqCst);
    if let Some(handle) = &handle {
        handle.disarm();
    }
    // A sent cancellation must finish its transport before we reuse this socket.
    if let Some(control) = control
        && let Err(error) = control
            .await
            .map_err(anyhow::Error::from)
            .and_then(|result| result)
    {
        control_failure = Some(format!("取消控制通道未完成：{error:#}"));
    }
    if let Some(message) = control_failure {
        let output = match &result {
            Ok(output) => Some(output.clone()),
            Err(error) => crate::db::query_partial_output(error),
        };
        let failure = CancelTransportFailure(format!(
            "{message}；原查询可能已完成，连接不再复用，请核实写入结果"
        ));
        let error = match result {
            Ok(_) => anyhow::Error::new(failure),
            Err(error) => error.context(failure),
        };
        return Err(match output {
            Some(output) => crate::db::retain_query_output(error, output),
            None => error,
        });
    }
    drop(guard);
    result
}
