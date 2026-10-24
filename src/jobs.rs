//! Application-owned SQL schedules. No connection credentials or shell task commands.
use crate::{
    db::Session,
    model::{Credentials, Profile, QueryOutput},
    workspace::QueryTarget,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobSchedule {
    EveryMinutes(u32),
    /// Unix seconds in UTC, converted by the UI from the user's chosen time.
    OnceUtc(i64),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobSpec {
    pub id: Uuid,
    pub name: String,
    pub profile: Uuid,
    pub database: String,
    #[serde(default)]
    pub schema: Option<String>,
    pub sql: String,
    pub schedule: JobSchedule,
    pub enabled: bool,
    #[serde(default)]
    pub connection_fingerprint: Option<String>,
}

impl JobSpec {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(!self.name.trim().is_empty(), "任务名称不能为空");
        anyhow::ensure!(
            !self.database.trim().is_empty() && !self.database.contains('\0'),
            "任务目标数据库无效"
        );
        anyhow::ensure!(
            !self.sql.trim().is_empty() && !self.sql.contains('\0'),
            "任务 SQL 无效"
        );
        if let JobSchedule::EveryMinutes(minutes) = self.schedule {
            anyhow::ensure!(
                (1..=525_600).contains(&minutes),
                "任务间隔应为 1 至 525600 分钟"
            );
        }
        if let JobSchedule::OnceUtc(at) = self.schedule {
            anyhow::ensure!(at > 0, "执行时间无效");
        }
        Ok(())
    }

    pub fn target(&self) -> QueryTarget {
        QueryTarget {
            connection: self.profile,
            database: Some(self.database.clone()),
            schema: self.schema.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobState {
    Pending,
    Running,
    Succeeded,
    Failed,
    Interrupted,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobRecord {
    pub spec: JobSpec,
    pub state: JobState,
    pub next_due_utc: Option<i64>,
    pub last_started_utc: Option<i64>,
    pub last_finished_utc: Option<i64>,
    pub run: Option<Uuid>,
    pub summary: String,
}

#[derive(Clone, Debug)]
pub struct JobClaim {
    pub spec: JobSpec,
    pub run: Uuid,
}

#[derive(Serialize, Deserialize)]
struct SavedJobs {
    version: u32,
    jobs: Vec<JobRecord>,
}

pub struct JobStore {
    directory: PathBuf,
    records: Vec<JobRecord>,
}

impl JobStore {
    pub fn load(directory: impl AsRef<Path>) -> Result<Self> {
        Self::load_at(directory, chrono::Utc::now().timestamp())
    }

    pub fn load_at(directory: impl AsRef<Path>, now_utc: i64) -> Result<Self> {
        let directory = directory.as_ref().to_owned();
        let path = directory.join("jobs.json");
        let mut records = match fs::read(&path) {
            Ok(data) => {
                let saved: SavedJobs =
                    serde_json::from_slice(&data).context("调度任务文件损坏，原文件未修改")?;
                anyhow::ensure!(saved.version == 1, "不支持的任务文件版本");
                saved.jobs
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error).context("读取调度任务失败"),
        };
        let mut ids = std::collections::HashSet::new();
        let mut changed = false;
        for record in &mut records {
            record.spec.validate()?;
            anyhow::ensure!(ids.insert(record.spec.id), "任务文件包含重复任务标识");
            if record.state == JobState::Running {
                record.state = JobState::Interrupted;
                record.summary =
                    "上次执行被中断，服务器结果未知；未自动重跑。检查结果后手动启用任务。".into();
                record.spec.enabled = false;
                record.run = None;
                record.next_due_utc = None;
                changed = true;
            } else if record.spec.enabled && record.next_due_utc.is_some_and(|due| due < now_utc) {
                // Do not catch up missed executions on application startup.
                match record.spec.schedule {
                    JobSchedule::EveryMinutes(minutes) => {
                        record.next_due_utc = Some(now_utc.saturating_add(i64::from(minutes) * 60));
                        record.summary = "应用关闭期间的计划已跳过；从下一个间隔继续。".into();
                    }
                    JobSchedule::OnceUtc(_) => {
                        record.spec.enabled = false;
                        record.next_due_utc = None;
                        record.state = JobState::Interrupted;
                        record.summary =
                            "应用关闭期间错过单次计划；未自动执行，请重新设定时间。".into();
                    }
                }
                changed = true;
            }
        }
        let store = Self { directory, records };
        if changed {
            store.save()?;
        }
        Ok(store)
    }

    pub fn records(&self) -> &[JobRecord] {
        &self.records
    }

    pub fn save(&self) -> Result<()> {
        fs::create_dir_all(&self.directory)?;
        let saved = SavedJobs {
            version: 1,
            jobs: self.records.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&saved)?;
        let temporary = self.directory.join(format!("jobs.{}.tmp", Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, self.directory.join("jobs.json"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result.context("保存调度任务失败；未执行计划")
    }

    pub fn upsert(&mut self, spec: JobSpec, now_utc: i64) -> Result<()> {
        spec.validate()?;
        if let JobSchedule::OnceUtc(at) = spec.schedule {
            anyhow::ensure!(
                !spec.enabled || at > now_utc,
                "启用的单次计划应晚于当前时间"
            );
        }
        anyhow::ensure!(
            !self
                .records
                .iter()
                .any(|r| r.spec.id == spec.id && r.state == JobState::Running),
            "正在执行的任务不能修改"
        );
        let snapshot = self.records.clone();
        let next_due_utc = if spec.enabled {
            Some(match spec.schedule {
                JobSchedule::EveryMinutes(minutes) => {
                    now_utc.saturating_add(i64::from(minutes) * 60)
                }
                JobSchedule::OnceUtc(at) => at,
            })
        } else {
            None
        };
        let record = JobRecord {
            spec,
            state: JobState::Pending,
            next_due_utc,
            last_started_utc: None,
            last_finished_utc: None,
            run: None,
            summary: String::new(),
        };
        if let Some(existing) = self
            .records
            .iter_mut()
            .find(|r| r.spec.id == record.spec.id)
        {
            *existing = record;
        } else {
            self.records.push(record);
        }
        if let Err(error) = self.save() {
            self.records = snapshot;
            return Err(error);
        }
        Ok(())
    }

    pub fn delete(&mut self, id: Uuid) -> Result<()> {
        anyhow::ensure!(
            !self
                .records
                .iter()
                .any(|r| r.spec.id == id && r.state == JobState::Running),
            "正在执行的任务不能删除"
        );
        let snapshot = self.records.clone();
        self.records.retain(|r| r.spec.id != id);
        if let Err(error) = self.save() {
            self.records = snapshot;
            return Err(error);
        }
        Ok(())
    }

    /// Persist a claim BEFORE dispatch. One running job has exactly one token.
    pub fn claim_due(&mut self, now_utc: i64) -> Result<Vec<JobClaim>> {
        let snapshot = self.records.clone();
        let mut claimed = Vec::new();
        for record in &mut self.records {
            if !record.spec.enabled
                || record.state == JobState::Running
                || record.next_due_utc.is_none_or(|due| due > now_utc)
            {
                continue;
            }
            let run = Uuid::new_v4();
            record.run = Some(run);
            record.state = JobState::Running;
            record.last_started_utc = Some(now_utc);
            record.last_finished_utc = None;
            record.summary = "执行中".into();
            // No overlap, no missed-interval catch-up and no retry after errors.
            record.next_due_utc = match record.spec.schedule {
                JobSchedule::EveryMinutes(minutes) => {
                    Some(now_utc.saturating_add(i64::from(minutes) * 60))
                }
                JobSchedule::OnceUtc(_) => None,
            };
            claimed.push(JobClaim {
                spec: record.spec.clone(),
                run,
            });
        }
        if !claimed.is_empty()
            && let Err(error) = self.save()
        {
            self.records = snapshot;
            return Err(error);
        }
        Ok(claimed)
    }

    pub fn finish(
        &mut self,
        id: Uuid,
        run: Uuid,
        now_utc: i64,
        success: bool,
        summary: String,
    ) -> Result<()> {
        let snapshot = self.records.clone();
        let record = self
            .records
            .iter_mut()
            .find(|r| r.spec.id == id)
            .context("任务不存在")?;
        anyhow::ensure!(
            record.state == JobState::Running && record.run == Some(run),
            "忽略已过期的任务返回结果"
        );
        record.state = if success {
            JobState::Succeeded
        } else {
            JobState::Failed
        };
        record.run = None;
        record.last_finished_utc = Some(now_utc);
        record.summary = summary.chars().take(4000).collect();
        // A failed write may have committed before losing its response. Require
        // explicit user re-enabling rather than automatically retrying it.
        if !success || matches!(record.spec.schedule, JobSchedule::OnceUtc(_)) {
            record.spec.enabled = false;
            record.next_due_utc = None;
        } else if let JobSchedule::EveryMinutes(minutes) = record.spec.schedule {
            record.next_due_utc = Some(now_utc.saturating_add(i64::from(minutes) * 60));
        }
        if let Err(error) = self.save() {
            self.records = snapshot;
            return Err(error);
        }
        Ok(())
    }
}

/// A scheduler run owns its own connection; document sessions are untouched.
/// No automatic begin/commit/retry is added to the user's SQL. SQL that leaves
/// a transaction open is reported and rolled back by closing this connection.
pub async fn execute_sql_job(
    profile: &Profile,
    credentials: &Credentials,
    spec: &JobSpec,
) -> Result<QueryOutput> {
    spec.validate()?;
    anyhow::ensure!(profile.id == spec.profile, "任务连接与实际连接不一致");
    anyhow::ensure!(
        spec.connection_fingerprint.as_ref() == Some(&serde_json::to_string(profile)?),
        "任务连接配置已变化或缺少配置快照，未执行；请重新确认计划"
    );
    if profile.engine == crate::model::Engine::MySql {
        anyhow::ensure!(
            spec.schema
                .as_ref()
                .is_none_or(|schema| schema == &spec.database),
            "MySQL 任务的 Schema 必须与目标数据库一致，SQL 未执行"
        );
    }
    let mut session = Session::connect_to_database(profile, credentials, &spec.database).await?;
    if let Some(schema) = &spec.schema
        && profile.engine != crate::model::Engine::MySql
    {
        session.select_query_schema(schema).await?;
    }
    let output = session.query(&spec.sql).await?;
    let state = session.transaction_state().await?;
    anyhow::ensure!(
        state == crate::query_control::QueryTransactionState::Idle,
        "任务 SQL 未结束事务（{state:?}）；连接将关闭，未自动提交，请核对服务器结果"
    );
    Ok(output)
}
