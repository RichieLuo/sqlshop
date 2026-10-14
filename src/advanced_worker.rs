//! Independent disposable connections for diagnostic and permission documents.
//! Plan sockets never become query or management sockets. No automatic retry.
use crate::{
    advanced::{
        AdvancedFailure, AdvancedOperation, AdvancedResponse, AdvancedResult, AdvancedSource,
    },
    db::Session,
    model::{Credentials, Engine},
};
use eframe::egui;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
use uuid::Uuid;

pub struct AdvancedRequest {
    pub document: Uuid,
    pub generation: u64,
    pub source: AdvancedSource,
    pub credentials: Credentials,
    pub operation: AdvancedOperation,
}

enum Command {
    Run(Box<AdvancedRequest>),
    Close(Uuid),
}

struct ActiveTask {
    task: tokio::task::JoinHandle<()>,
    mutation: bool,
}

pub struct AdvancedWorker {
    sender: tokio::sync::mpsc::UnboundedSender<Command>,
    receiver: mpsc::Receiver<AdvancedResponse>,
}

impl AdvancedWorker {
    pub fn new(ctx: egui::Context) -> Self {
        let (sender, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let (responses, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    while let Some(command) = requests.blocking_recv() {
                        if let Command::Run(request) = command {
                            let _ = responses.send(AdvancedResponse {
                                document: request.document,
                                generation: request.generation,
                                source: request.source,
                                result: Err(AdvancedFailure {
                                    message: format!("高级功能后台运行时初始化失败：{error}"),
                                    outcome_unknown: false,
                                }),
                            });
                            ctx.request_repaint();
                        }
                    }
                    return;
                }
            };
            let local = tokio::task::LocalSet::new();
            runtime.block_on(local.run_until(async move {
                let mut active:HashMap<Uuid,ActiveTask>=HashMap::new();
                while let Some(command)=requests.recv().await {
                    match command {
                        Command::Close(document)=>{
                            if active.get(&document).is_some_and(|task|!task.mutation || task.task.is_finished()) && let Some(task)=active.remove(&document) {
                                task.task.abort();
                            }
                        }
                        Command::Run(request)=>{
                            let request=*request;
                            if let Some(previous)=active.get(&request.document) && previous.mutation && !previous.task.is_finished() {
                                    let _=responses.send(AdvancedResponse {document:request.document,generation:request.generation,source:request.source,result:Err(AdvancedFailure{message:"该页面仍在处理权限变更，请等待结果后再继续。".into(),outcome_unknown:false})});ctx.request_repaint();continue;
                            }
                            if let Some(previous)=active.remove(&request.document) {previous.task.abort();}
                            let mutation=request.operation.is_mutation();
                            let document=request.document;
                            let responses=responses.clone();let ctx=ctx.clone();
                            let task=tokio::task::spawn_local(async move {
                                let submitted=Arc::new(AtomicBool::new(false));
                                let acknowledged=Arc::new(AtomicBool::new(false));
                                let timeout=if mutation {Duration::from_secs(120)} else {Duration::from_secs(60)};
                                let result=tokio::time::timeout(timeout,execute(&request,&submitted,&acknowledged)).await;
                                let result=match result {
                                    Ok(Ok(output))=>Ok(output),
                                    Ok(Err(error))=>{
                                        let outcome_unknown=error.downcast_ref::<crate::db::AccountChangeFailure>().map_or_else(||mutation && submitted.load(Ordering::SeqCst) && !crate::db::query_error_preserves_session(&error) && error.downcast_ref::<crate::db::EditRejected>().is_none(),|failure|failure.outcome_unknown);
                                        let message=if outcome_unknown {format!("权限变更结果未知，请读取最新授权核实；未自动重试。\n{error:#}")} else {format!("{error:#}")};
                                        Err(AdvancedFailure{message,outcome_unknown})
                                    }
                                    Err(_)=>{
                                        if acknowledged.load(Ordering::SeqCst) {
                                            let sql=match &request.operation {AdvancedOperation::ApplySecurity{preview_sql,..}|AdvancedOperation::ApplyAccount{preview_sql,..}=>preview_sql.clone(),_=>String::new()};
                                            Ok(AdvancedResult::SecurityChanged{catalog:None,warning:Some("刷新权限目录超时；变更已收到服务器成功确认。".into()),sql})
                                        } else {
                                            let outcome_unknown=mutation && submitted.load(Ordering::SeqCst);
                                            Err(AdvancedFailure{message:if outcome_unknown {"操作超时，权限变更结果未知；请读取最新授权核实，未自动重试。".into()} else {"操作超时；诊断连接已丢弃，请确认网络与服务器状态后再操作。".into()},outcome_unknown})
                                        }
                                    }
                                };
                                let _=responses.send(AdvancedResponse {document:request.document,generation:request.generation,source:request.source,result});ctx.request_repaint();
                            });
                            active.insert(document,ActiveTask{task,mutation});
                        }
                    }
                }
                // Closing the service discards every socket. The host prevents
                // application exit during acknowledged in-flight mutations.
                for (_,task) in active {task.task.abort();}
            }));
        });
        Self { sender, receiver }
    }

    pub fn send(&self, request: AdvancedRequest) -> Result<(), String> {
        self.sender
            .send(Command::Run(Box::new(request)))
            .map_err(|_| "高级功能后台服务已关闭。".into())
    }

    pub fn try_recv(&self) -> Result<AdvancedResponse, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }

    pub fn close(&self, document: Uuid) {
        let _ = self.sender.send(Command::Close(document));
    }
}

async fn execute(
    request: &AdvancedRequest,
    submitted: &AtomicBool,
    acknowledged: &AtomicBool,
) -> anyhow::Result<AdvancedResult> {
    if std::env::var_os("SQLSHOP_SCREENSHOT").is_some() {
        return Err(crate::db::EditRejected("离线界面预览不执行数据库请求。".into()).into());
    }
    let source = &request.source;
    anyhow::ensure!(
        source.profile.id == source.target.connection,
        "高级功能连接身份不符，操作未执行"
    );
    let database = source
        .target
        .database
        .as_deref()
        .filter(|database| !database.is_empty())
        .ok_or_else(|| crate::db::EditRejected("请先选择数据库再打开此功能。".into()))?;
    let mut bootstrap = source.profile.clone();
    bootstrap.database = database.into();
    let mut session = Session::connect(&bootstrap, &request.credentials)
        .await
        .map_err(|error| crate::db::connection_error(&source.profile, error))?;
    if session.current_database().await?.as_deref() != Some(database) {
        session.select_database(database).await?;
    }
    anyhow::ensure!(
        session.current_database().await?.as_deref() == Some(database),
        "高级功能未能进入指定数据库，操作未执行"
    );
    if let Some(schema) = &source.target.schema {
        match source.profile.engine {
            Engine::Postgres | Engine::SqlServer => session.select_query_schema(schema).await?,
            Engine::MySql => anyhow::ensure!(
                schema == database,
                "MySQL 数据库与 Schema 目标不符，操作未执行"
            ),
        }
    }
    // Session exists only for this operation. Future cancellation/timeouts drop
    // it, so an interrupted SHOWPLAN can never leak into another request.
    Ok(match &request.operation {
        AdvancedOperation::RefreshDiagnostics(kind) => {
            AdvancedResult::Data(session.diagnostics(*kind).await?)
        }
        AdvancedOperation::Explain(sql) => AdvancedResult::Data(session.explain_query(sql).await?),
        AdvancedOperation::LoadPrincipals => {
            AdvancedResult::Principals(session.security_principals().await?)
        }
        AdvancedOperation::LoadGrants(principal) => AdvancedResult::Grants {
            principal: principal.clone(),
            output: session.security_grants(principal).await?,
        },
        AdvancedOperation::ApplySecurity {
            change,
            preview_sql,
        } => {
            submitted.store(true, Ordering::SeqCst);
            session.apply_security_change(change, preview_sql).await?;
            acknowledged.store(true, Ordering::SeqCst);
            let (catalog, warning) = match session.security_principals().await {
                Ok(catalog) => (Some(catalog), None),
                Err(error) => (None, Some(format!("{error:#}"))),
            };
            AdvancedResult::SecurityChanged {
                catalog,
                warning,
                sql: preview_sql.clone(),
            }
        }
        AdvancedOperation::ApplyAccount {
            change,
            preview_sql,
            preview_source,
        } => {
            if !source.same_binding(preview_source) {
                return Err(crate::db::EditRejected(
                    "账号变更的预览目标已变化，请重新预览。".into(),
                )
                .into());
            }
            submitted.store(true, Ordering::SeqCst);
            session.apply_account_change(change, preview_sql).await?;
            acknowledged.store(true, Ordering::SeqCst);
            let (catalog, warning) = match session.security_principals().await {
                Ok(catalog) => (Some(catalog), None),
                Err(_) => (
                    None,
                    Some(
                        "账号变更已收到服务器确认，但目录刷新失败；请重新连接读取最新状态。".into(),
                    ),
                ),
            };
            AdvancedResult::SecurityChanged {
                catalog,
                warning,
                sql: preview_sql.clone(),
            }
        }
    })
}
