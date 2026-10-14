use super::{Driver, Session, validate_identifier};
use crate::model::Engine;
use anyhow::{Context, Result};

impl Session {
    /// A native restore is permitted only into an empty named target. This
    /// preflight does not lock out other applications; the UI must close its
    /// own write sessions and the user must arrange exclusive access.
    pub async fn ensure_restore_target_empty(&mut self, database: &str) -> Result<()> {
        validate_identifier(database, "恢复目标数据库")?;
        let system = match self.profile.engine {
            Engine::MySql => ["mysql", "information_schema", "performance_schema", "sys"]
                .contains(&database.to_ascii_lowercase().as_str()),
            Engine::Postgres => {
                ["template0", "template1"].contains(&database.to_ascii_lowercase().as_str())
            }
            Engine::SqlServer => ["master", "model", "msdb", "tempdb"]
                .contains(&database.to_ascii_lowercase().as_str()),
        };
        anyhow::ensure!(!system, "当前恢复流程不支持替换系统数据库");
        if matches!(self.driver, Driver::SqlServer(_)) {
            self.select_database("master").await?;
        } else {
            // Local CLI restores require an existing accessible target and do
            // not create/drop databases themselves.
            self.select_database(database).await?;
        }
        let count: i64 = match &mut self.driver {
            Driver::MySql(connection) => sqlx::query_scalar(
                "SELECT (SELECT COUNT(*) FROM information_schema.TABLES WHERE TABLE_SCHEMA = ?) + (SELECT COUNT(*) FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA = ?) + (SELECT COUNT(*) FROM information_schema.EVENTS WHERE EVENT_SCHEMA = ?)",
            ).bind(database).bind(database).bind(database).fetch_one(connection).await?,
            Driver::Postgres(connection) => sqlx::query_scalar(
                "SELECT (SELECT COUNT(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname !~ '^pg_' AND n.nspname <> 'information_schema') + (SELECT COUNT(*) FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname !~ '^pg_' AND n.nspname <> 'information_schema')",
            ).fetch_one(connection).await?,
            Driver::SqlServer(client) => {
                let exists = client.query("SELECT DB_ID(@P1)", &[&database]).await?.into_row().await?.context("无法检查恢复目标")?;
                if exists.try_get::<i32, _>(0)?.is_none() {
                    let visibility = client.simple_query("SELECT CAST(HAS_PERMS_BY_NAME(NULL,NULL,'VIEW ANY DATABASE') AS int)").await?.into_row().await?.context("无法核实数据库目录可见性")?;
                    anyhow::ensure!(visibility.try_get::<i32, _>(0)? == Some(1), "数据库目录可见性不足，无法证明恢复目标不存在，未执行恢复");
                    return Ok(());
                }
                let visibility = client.query("SELECT CAST(HAS_PERMS_BY_NAME(@P1,'DATABASE','VIEW DEFINITION') AS int)", &[&database]).await?.into_row().await?.context("无法核实目标元数据可见性")?;
                anyhow::ensure!(visibility.try_get::<i32, _>(0)? == Some(1), "缺少目标 VIEW DEFINITION，不能从受限目录证明数据库为空，未执行恢复");
                let sql = format!("SELECT COUNT_BIG(*) FROM {}.sys.objects WHERE is_ms_shipped = 0", Engine::SqlServer.quote(database));
                let row = client.simple_query(sql).await?.into_row().await?.context("无法检查目标是否为空")?;
                row.try_get::<i64, _>(0)?.context("目标对象数量为空")?
            }
        };
        anyhow::ensure!(
            count == 0,
            "恢复目标含有 {count} 个对象；当前恢复流程只接受空数据库，不会自动覆盖或删除已有对象"
        );
        Ok(())
    }
}
