//! Table recovery reads use a fresh connection, independent of the failed actor.
use crate::{
    db::{Session, TableBrowseOptions, TablePage},
    model::{CellUpdate, Credentials, InsertValue, Profile, Table, TableColumn},
    recovery::DraftVerification,
    workspace::QueryTarget,
};
use std::{sync::mpsc, time::Duration};
use uuid::Uuid;

// No Debug/serialization implementation: the request temporarily owns credentials.
pub struct RecoveryCheck {
    pub tab: Uuid,
    pub epoch: u64,
    pub profile: Profile,
    pub credentials: Credentials,
    pub target: QueryTarget,
    pub table: Table,
    pub browse: TableBrowseOptions,
    pub changes: Vec<CellUpdate>,
    pub insert_columns: Vec<String>,
    pub inserts: Vec<Vec<InsertValue>>,
    pub expected_columns: Vec<TableColumn>,
}

#[allow(clippy::large_enum_variant)]
pub enum RecoveryResponse {
    Checked {
        tab: Uuid,
        epoch: u64,
        target: QueryTarget,
        report: DraftVerification,
        page: TablePage,
        profile_snapshot: Profile,
    },
    Failed {
        tab: Uuid,
        epoch: u64,
        target: QueryTarget,
        message: String,
        profile_snapshot: Profile,
    },
}

pub struct RecoveryWorker {
    sender: tokio::sync::mpsc::Sender<RecoveryCheck>,
    pub receiver: mpsc::Receiver<RecoveryResponse>,
}

impl RecoveryWorker {
    pub fn start(wake: impl Fn() + Send + 'static) -> Self {
        // Bound outstanding credential-bearing requests. The UI also owns its
        // busy/epoch guard; obsolete results never replace the current document.
        let (sender, mut requests) = tokio::sync::mpsc::channel::<RecoveryCheck>(1);
        let (responses, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => runtime.block_on(async move {
                    while let Some(request) = requests.recv().await {
                        let checked = tokio::time::timeout(Duration::from_secs(120), read(&request)).await;
                        let response = match checked {
                            Ok(Ok((report, page))) => RecoveryResponse::Checked {
                                tab: request.tab,
                                epoch: request.epoch,
                                target: request.target,
                                report,
                                page,
                                profile_snapshot: request.profile,
                            },
                            error => RecoveryResponse::Failed {
                                tab: request.tab,
                                epoch: request.epoch,
                                target: request.target,
                                message: match error {
                                    Ok(Err(error)) => format!("核实失败：{error:#}"),
                                    Err(_) => "核实超过 120 秒，已关闭核实连接；本地草稿仍保留，没有执行写入".into(),
                                    _ => unreachable!(),
                                },
                                profile_snapshot: request.profile,
                            },
                        };
                        if responses.send(response).is_err() { break; }
                        wake();
                    }
                }),
                Err(error) => {
                    while let Some(request) = requests.blocking_recv() {
                        if responses.send(RecoveryResponse::Failed {
                            tab: request.tab,
                            epoch: request.epoch,
                            target: request.target,
                            message: format!("无法启动核实线程：{error}"),
                            profile_snapshot: request.profile,
                        }).is_err() { break; }
                        wake();
                    }
                }
            }
        });
        Self { sender, receiver }
    }

    pub fn check(&self, request: RecoveryCheck) -> anyhow::Result<()> {
        self.sender.try_send(request).map_err(|error| match error {
            tokio::sync::mpsc::error::TrySendError::Full(_) => {
                anyhow::anyhow!("核实线程已有待处理任务，请稍后重试")
            }
            tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                anyhow::anyhow!("核实线程已退出，请重启应用")
            }
        })
    }
}

async fn read(request: &RecoveryCheck) -> anyhow::Result<(DraftVerification, TablePage)> {
    anyhow::ensure!(
        request.profile.id == request.target.connection,
        "核实目标与连接配置不一致"
    );
    let database = request
        .target
        .database
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("草稿未绑定数据库，无法核实"))?;
    let mut bootstrap = request.profile.clone();
    bootstrap.database = database.to_owned();
    let mut session = Session::connect(&bootstrap, &request.credentials)
        .await
        .map_err(|error| crate::db::connection_error(&request.profile, error))?;
    if session.current_database().await?.as_deref() != Some(database) {
        session.select_database(database).await?;
    }
    // Table identities are fully qualified. No user default schema or transaction
    // is changed on either the original management actor or the user's SQL tab.
    let report = session
        .verify_table_draft_with_columns(
            database,
            &request.table,
            &request.changes,
            &request.insert_columns,
            &request.inserts,
            &request.expected_columns,
        )
        .await?;
    let page = session.table_page(&request.table, &request.browse).await?;
    Ok((report, page))
}
