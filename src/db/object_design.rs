use super::Session;
use crate::{
    model::Engine,
    object_catalog::{CatalogObject, ObjectKind},
    object_design::{
        SchemaChange, SchemaSnapshot, check_definition_snapshot, check_schema_snapshot,
        object_plan, validate_edit_identity,
    },
};
use anyhow::{Result, bail, ensure};

impl Session {
    pub async fn design_schemas(&mut self) -> Result<Vec<SchemaSnapshot>> {
        let sql = match self.profile.engine {
            Engine::Postgres => {
                "SELECT n.nspname::text,pg_get_userbyid(n.nspowner)::text,n.oid::text FROM pg_namespace n WHERE n.nspname NOT LIKE 'pg_%' AND n.nspname<>'information_schema' ORDER BY n.nspname"
            }
            Engine::MySql => {
                "SELECT SCHEMA_NAME, '', SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME NOT IN ('mysql','information_schema','performance_schema','sys') ORDER BY SCHEMA_NAME"
            }
            Engine::SqlServer => {
                "SELECT s.name, COALESCE(USER_NAME(s.principal_id),''), CONVERT(nvarchar(20),s.schema_id) FROM sys.schemas s WHERE s.name NOT IN ('sys','INFORMATION_SCHEMA') AND s.schema_id<16384 ORDER BY s.name"
            }
        };
        let output = self.query_internal(sql).await?;
        ensure!(
            !output.sets.iter().any(|set| set.truncated),
            "Schema 目录被截断，不能安全管理"
        );
        Ok(output
            .sets
            .into_iter()
            .flat_map(|s| s.rows)
            .filter_map(|row| {
                Some(SchemaSnapshot {
                    name: row.first()?.clone()?,
                    owner: row.get(1).and_then(Clone::clone).unwrap_or_default(),
                    identity: row.get(2).and_then(Clone::clone).unwrap_or_default(),
                })
            })
            .collect())
    }
    pub async fn apply_schema_design(
        &mut self,
        change: &SchemaChange,
        expected: &[SchemaSnapshot],
        preview: &str,
    ) -> Result<()> {
        let engine = self.profile.engine;
        let sql = change.sql(engine)?;
        ensure!(sql == preview, "SQL 预览已改变，请重新确认");
        let current = self.design_schemas().await?;
        check_schema_snapshot(change, expected, &current)?;
        if engine == Engine::MySql && matches!(change, SchemaChange::Drop { .. }) {
            // DROP DATABASE has no RESTRICT. Never let it silently remove data,
            // routines, or scheduled events that the schema form did not show.
            let output=self.parameter_query("SELECT (SELECT COUNT(*) FROM information_schema.TABLES WHERE TABLE_SCHEMA=?)+(SELECT COUNT(*) FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA=?)+(SELECT COUNT(*) FROM information_schema.EVENTS WHERE EVENT_SCHEMA=?)",&[change.name(),change.name(),change.name()]).await?;
            let count = output
                .sets
                .first()
                .and_then(|s| s.rows.first())
                .and_then(|r| r.first())
                .and_then(|v| v.as_deref())
                .and_then(|v| v.parse::<u64>().ok())
                .ok_or_else(|| anyhow::anyhow!("无法核对数据库是否为空"))?;
            ensure!(
                count == 0,
                "数据库非空，禁止删除；请先显式处理所有表、视图、例程和事件"
            );
            bail!(
                "MySQL 不支持原子的 DROP DATABASE RESTRICT；为避免并发创建导致误删，请通过明确的数据删除流程操作"
            );
        }
        self.query_internal(&sql).await?;
        Ok(())
    }
    /// Check only the target namespace. Browsing catalogs are intentionally
    /// bounded and can have unrelated unavailable categories; neither should
    /// prevent creating a new, unoccupied target object.
    async fn design_object_exists(&mut self, object: &CatalogObject) -> Result<bool> {
        let (sql, parameters): (&str, Vec<&str>) = match (self.profile.engine, object.kind) {
            (Engine::Postgres, ObjectKind::Trigger) => (
                "SELECT COUNT(*)::text FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND t.tgname=$2 AND c.relname=$3",
                vec![
                    &object.schema,
                    &object.name,
                    object
                        .parent
                        .as_deref()
                        .ok_or_else(|| anyhow::anyhow!("触发器缺少所属表"))?,
                ],
            ),
            (Engine::Postgres, ObjectKind::Function | ObjectKind::Procedure) => (
                "SELECT COUNT(*)::text FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname=$1 AND p.proname=$2",
                vec![&object.schema, &object.name],
            ),
            (Engine::Postgres, _) => (
                "SELECT COUNT(*)::text FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2",
                vec![&object.schema, &object.name],
            ),
            (Engine::MySql, ObjectKind::Trigger) => (
                "SELECT COUNT(*) FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA=? AND TRIGGER_NAME=?",
                vec![&object.schema, &object.name],
            ),
            (Engine::MySql, ObjectKind::Function | ObjectKind::Procedure) => (
                "SELECT COUNT(*) FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA=? AND ROUTINE_NAME=? AND ROUTINE_TYPE=?",
                vec![
                    &object.schema,
                    &object.name,
                    if object.kind == ObjectKind::Function {
                        "FUNCTION"
                    } else {
                        "PROCEDURE"
                    },
                ],
            ),
            (Engine::MySql, _) => (
                "SELECT COUNT(*) FROM information_schema.TABLES WHERE TABLE_SCHEMA=? AND TABLE_NAME=?",
                vec![&object.schema, &object.name],
            ),
            (Engine::SqlServer, _) => (
                "SELECT COUNT(*) FROM sys.objects o JOIN sys.schemas s ON s.schema_id=o.schema_id WHERE s.name=@P1 AND o.name=@P2",
                vec![&object.schema, &object.name],
            ),
        };
        let output = self.parameter_query(sql, &parameters).await?;
        let count = output
            .sets
            .first()
            .and_then(|set| set.rows.first())
            .and_then(|row| row.first())
            .and_then(|value| value.as_deref())
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| anyhow::anyhow!("无法核对目标对象身份"))?;
        Ok(count != 0)
    }

    pub async fn apply_object_design(
        &mut self,
        object: &CatalogObject,
        expected: Option<&str>,
        draft: &str,
        preview: &[String],
    ) -> Result<String> {
        let engine = self.profile.engine;
        if engine == Engine::MySql {
            ensure!(
                self.current_database().await?.as_deref() == Some(object.schema.as_str()),
                "MySQL 对象必须属于当前绑定数据库"
            );
        }
        let plan = object_plan(engine, object, draft, expected.is_some())?;
        ensure!(plan == preview, "SQL 预览已改变，请重新确认");
        if let Some(expected) = expected {
            validate_edit_identity(engine, object, expected, draft)?;
            let current = self.object_definition(object).await?;
            check_definition_snapshot(expected, &current)?;
        } else {
            ensure!(
                !self.design_object_exists(object).await?,
                "目标对象名称已存在，请刷新目录后编辑"
            );
        }
        // PostgreSQL DDL is transactional. MySQL replacement explicitly consists
        // of DROP + CREATE and the UI displays both and warns about implicit commit.
        let transactional = engine == Engine::Postgres;
        if transactional {
            self.query_internal("BEGIN").await?;
        }
        for (index, sql) in plan.iter().enumerate() {
            if let Err(error) = self.query_internal(sql).await {
                if transactional {
                    let _ = self.query_internal("ROLLBACK").await;
                }
                return Err(error.context(if engine==Engine::MySql&&index>0 {"旧对象已删除但重建失败；草稿仍保留，请修正后按新建对象恢复，并重新核对权限和依赖"} else {"对象定义未保存"}));
            }
        }
        if transactional {
            self.query_internal("COMMIT").await?;
        }
        Ok("对象定义已保存，请刷新目录核对服务器定义".into())
    }
}
