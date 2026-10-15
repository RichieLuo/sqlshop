//! Lazy, read-only table-tree metadata on disposable connections. Credentials
//! remain in requests, never in the returned cache or its identity.
use crate::{
    db::{Session, TableStructure},
    model::{Credentials, Profile, ResultSet, Table},
    workspace::QueryTarget,
};
use std::{sync::mpsc, time::Duration};
use uuid::Uuid;

#[derive(Clone)]
pub struct TableTreeDetails {
    pub structure: Result<TableStructure, String>,
    pub triggers: Result<ResultSet, String>,
}

pub struct TableMetadataRequest {
    pub generation: Uuid,
    pub profile: Profile,
    pub credentials: Credentials,
    pub target: QueryTarget,
    pub table: Table,
}

pub struct TableMetadataResponse {
    pub generation: Uuid,
    pub profile: Profile,
    pub target: QueryTarget,
    pub table: Table,
    pub result: Result<TableTreeDetails, String>,
}

pub struct TableMetadataWorker {
    sender: tokio::sync::mpsc::Sender<TableMetadataRequest>,
    pub receiver: mpsc::Receiver<TableMetadataResponse>,
}

impl TableMetadataWorker {
    pub fn start(wake: impl Fn() + Send + 'static) -> Self {
        let (sender, mut requests) = tokio::sync::mpsc::channel::<TableMetadataRequest>(1);
        let (responses, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            while let Some(request) = requests.blocking_recv() {
                let result = match &runtime {
                    Ok(runtime) => runtime.block_on(async {
                        match tokio::time::timeout(Duration::from_secs(60), read(&request)).await {
                            Ok(result) => result.map_err(|error| format!("读取失败：{error:#}")),
                            Err(_) => Err("结构读取超过60秒，已关闭读取连接；可手动重试".into()),
                        }
                    }),
                    Err(error) => Err(format!("无法启动结构读取线程：{error}")),
                };
                if responses
                    .send(TableMetadataResponse {
                        generation: request.generation,
                        profile: request.profile,
                        target: request.target,
                        table: request.table,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
                wake();
            }
        });
        Self { sender, receiver }
    }

    pub fn request(&self, request: TableMetadataRequest) -> anyhow::Result<bool> {
        match self.sender.try_send(request) {
            Ok(()) => Ok(true),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => Ok(false),
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                anyhow::bail!("结构读取线程已退出，请重启应用")
            }
        }
    }
}

async fn read(request: &TableMetadataRequest) -> anyhow::Result<TableTreeDetails> {
    anyhow::ensure!(
        request.target.connection == request.profile.id,
        "结构读取连接不符"
    );
    let database = request
        .target
        .database
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("请先选择数据库"))?;
    let mut bootstrap = request.profile.clone();
    bootstrap.database = database.into();
    let mut session = Session::connect(&bootstrap, &request.credentials)
        .await
        .map_err(|error| crate::db::connection_error(&request.profile, error))?;
    if session.current_database().await?.as_deref() != Some(database) {
        session.select_database(database).await?;
    }
    let structure = session.table_structure(&request.table).await;
    let reusable = structure
        .as_ref()
        .err()
        .is_none_or(crate::db::query_error_preserves_session);
    let triggers = if reusable {
        session
            .table_triggers(&request.table)
            .await
            .map_err(|error| format!("{error:#}"))
    } else {
        Err("读取结构时连接中断，触发器未读取".into())
    };
    Ok(TableTreeDetails {
        structure: structure.map_err(|error| format!("{error:#}")),
        triggers,
    })
}
