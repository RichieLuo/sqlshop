use crate::{
    backup::{self, BackupResult, DraftBackupConfig},
    jobs::{self, JobClaim, JobStore},
    model::{Credentials, Profile, QueryOutput},
};
use anyhow::{Context, Result};
use std::{collections::VecDeque, path::Path, sync::mpsc};
use uuid::Uuid;

pub enum BackupRequest {
    Detect {
        document: Uuid,
        generation: Uuid,
        profile: Profile,
        config: DraftBackupConfig,
    },
    Execute {
        document: Uuid,
        generation: Uuid,
        profile: Profile,
        credentials: Credentials,
        config: DraftBackupConfig,
    },
}

#[derive(Clone, Debug)]
pub enum BackupResponseKind {
    ToolDetected(Result<String, String>),
    Finished(Result<BackupResult, String>),
}
#[derive(Clone, Debug)]
pub struct BackupResponse {
    pub document: Uuid,
    pub generation: Uuid,
    pub profile: Uuid,
    pub database: String,
    pub source_fingerprint: String,
    pub kind: BackupResponseKind,
}

pub struct BackupWorker {
    sender: tokio::sync::mpsc::UnboundedSender<BackupRequest>,
    receiver: mpsc::Receiver<BackupResponse>,
}
impl BackupWorker {
    pub fn start() -> Self {
        let (sender, mut requests) = tokio::sync::mpsc::unbounded_channel::<BackupRequest>();
        let (responses, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            let local = tokio::task::LocalSet::new();
            local.block_on(&runtime, async move {
                while let Some(request) = requests.recv().await {
                    let (document, generation, profile_id, database, source_fingerprint, kind) =
                        match request {
                            BackupRequest::Detect {
                                document,
                                generation,
                                profile,
                                config,
                            } => {
                                let result = backup::detect_tool(&profile, &config)
                                    .await
                                    .map_err(|e| format!("{e:#}"));
                                (
                                    document,
                                    generation,
                                    profile.id,
                                    config.database.clone(),
                                    serde_json::to_string(&(&profile, &config)).unwrap_or_default(),
                                    BackupResponseKind::ToolDetected(result),
                                )
                            }
                            BackupRequest::Execute {
                                document,
                                generation,
                                profile,
                                credentials,
                                config,
                            } => {
                                let result = backup::execute(&profile, &credentials, &config)
                                    .await
                                    .map_err(|e| redact(&format!("{e:#}"), &credentials));
                                (
                                    document,
                                    generation,
                                    profile.id,
                                    config.database.clone(),
                                    serde_json::to_string(&(&profile, &config)).unwrap_or_default(),
                                    BackupResponseKind::Finished(result),
                                )
                            }
                        };
                    if responses
                        .send(BackupResponse {
                            document,
                            generation,
                            profile: profile_id,
                            database,
                            source_fingerprint,
                            kind,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            });
        });
        Self { sender, receiver }
    }
    pub fn send(&self, request: BackupRequest) -> Result<()> {
        self.sender
            .send(request)
            .map_err(|_| anyhow::anyhow!("备份工作线程已停止，任务未发送"))
    }
    pub fn try_recv(&self) -> Option<BackupResponse> {
        self.receiver.try_recv().ok()
    }
}

fn redact(message: &str, credentials: &Credentials) -> String {
    let mut result = message.to_owned();
    for secret in [&credentials.database_password, &credentials.ssh_secret] {
        if !secret.is_empty() {
            result = result.replace(secret, "[已隐去凭据]");
        }
    }
    result.chars().take(8000).collect()
}

struct JobRequest {
    claim: JobClaim,
    profile: Profile,
    credentials: Credentials,
}
#[derive(Clone, Debug)]
pub struct JobCompletion {
    pub id: Uuid,
    pub run: Uuid,
    pub profile: Uuid,
    pub database: String,
    pub success: bool,
    pub summary: String,
    pub output: Option<QueryOutput>,
}
struct JobWorker {
    sender: tokio::sync::mpsc::UnboundedSender<JobRequest>,
    receiver: mpsc::Receiver<JobCompletion>,
}
impl JobWorker {
    fn start() -> Self {
        let (sender, mut requests) = tokio::sync::mpsc::unbounded_channel::<JobRequest>();
        let (responses, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            let local = tokio::task::LocalSet::new();
            local.block_on(&runtime, async move {
                while let Some(request) = requests.recv().await {
                    let responses = responses.clone();
                    tokio::task::spawn_local(async move {
                        let JobRequest { claim, profile, credentials } = request;
                        let result = jobs::execute_sql_job(&profile, &credentials, &claim.spec).await;
                        let (success, summary, output) = match result {
                            Ok(output) => {
                                let rows: usize = output.sets.iter().map(|s| s.rows.len()).sum();
                                let affected: u64 = output.sets.iter().filter_map(|s| s.affected).sum();
                                (true, format!("完成：{} 个结果集，显示 {rows} 行，服务器报告行数 {affected}，{} ms", output.sets.len(), output.elapsed_ms), Some(output))
                            },
                            Err(error) => {
                                let output = crate::db::query_partial_output(&error);
                                (false, redact(&format!("{error:#}\n任务已暂停，请核对服务器结果后手动启用；未自动重试。"), &credentials), output)
                            },
                        };
                        let _ = responses.send(JobCompletion { id: claim.spec.id, run: claim.run, profile: profile.id,
                            database: claim.spec.database, success, summary, output });
                    });
                }
            });
        });
        Self { sender, receiver }
    }
}

/// Poll only while the application is open. Enabling a persisted JobSpec is
/// the user's authorization for its schedule. Credentials are resolved at run
/// time and never stored in jobs.json. Restarted/failed writes are not retried.
pub struct JobScheduler {
    pub store: JobStore,
    worker: JobWorker,
    pending_results: VecDeque<JobCompletion>,
}
impl JobScheduler {
    pub fn load(directory: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            store: JobStore::load(directory)?,
            worker: JobWorker::start(),
            pending_results: VecDeque::new(),
        })
    }
    pub fn tick(
        &mut self,
        now_utc: i64,
        profiles: &[Profile],
        mut credentials: impl FnMut(&Profile) -> Result<Credentials>,
    ) -> Result<Vec<JobCompletion>> {
        while let Ok(result) = self.worker.receiver.try_recv() {
            self.pending_results.push_back(result);
        }
        let mut completed = Vec::new();
        while let Some(result) = self.pending_results.front() {
            // If persistence fails, retain the completion and prohibit new
            // claims until the next tick successfully saves it.
            self.store.finish(
                result.id,
                result.run,
                now_utc,
                result.success,
                result.summary.clone(),
            )?;
            completed.push(self.pending_results.pop_front().expect("front exists"));
        }
        for claim in self.store.claim_due(now_utc)? {
            let dispatched = (|| -> Result<()> {
                let profile = profiles
                    .iter()
                    .find(|p| p.id == claim.spec.profile)
                    .context("任务连接不存在，未执行")?;
                anyhow::ensure!(
                    claim.spec.connection_fingerprint.as_ref()
                        == Some(&serde_json::to_string(profile)?),
                    "任务连接配置已变化或缺少配置快照，未执行；请重新确认计划"
                );
                let credentials = credentials(profile).context("任务凭据不可用，未执行")?;
                self.worker
                    .sender
                    .send(JobRequest {
                        claim: claim.clone(),
                        profile: profile.clone(),
                        credentials,
                    })
                    .map_err(|_| anyhow::anyhow!("任务工作线程已停止，未执行"))?;
                Ok(())
            })();
            if let Err(error) = dispatched {
                self.pending_results.push_back(JobCompletion {
                    id: claim.spec.id,
                    run: claim.run,
                    profile: claim.spec.profile,
                    database: claim.spec.database,
                    success: false,
                    summary: format!("{error:#}"),
                    output: None,
                });
            }
        }
        Ok(completed)
    }
}
