use super::{Session, validate_identifier};
use crate::model::{Engine, ResultSet, Table};

impl Session {
    /// Table-scoped catalog reads avoid relying on the truncated database-wide
    /// object list. Composite identity is always bound as parameters.
    pub async fn table_triggers(&mut self, table: &Table) -> anyhow::Result<ResultSet> {
        validate_identifier(&table.schema, "Schema")?;
        validate_identifier(&table.name, "表名")?;
        let sql = match self.profile.engine {
            Engine::MySql => {
                r#"SELECT CAST(TRIGGER_SCHEMA AS CHAR CHARACTER SET utf8mb4),
                CAST(TRIGGER_NAME AS CHAR CHARACTER SET utf8mb4),
                CAST(ACTION_TIMING AS CHAR), CAST(EVENT_MANIPULATION AS CHAR),
                CAST(ACTION_STATEMENT AS CHAR CHARACTER SET utf8mb4)
                FROM information_schema.TRIGGERS
                WHERE EVENT_OBJECT_SCHEMA=? AND EVENT_OBJECT_TABLE=?
                ORDER BY TRIGGER_NAME LIMIT 1001"#
            }
            Engine::Postgres => {
                r#"SELECT n.nspname::text,t.tgname::text,
                CASE t.tgenabled WHEN 'D' THEN 'disabled' WHEN 'R' THEN 'replica' WHEN 'A' THEN 'always' ELSE 'enabled' END,
                pg_get_triggerdef(t.oid,true)::text,
                pg_get_triggerdef(t.oid,true)::text
                FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid
                JOIN pg_namespace n ON n.oid=c.relnamespace
                WHERE n.nspname=$1 AND c.relname=$2 AND NOT t.tgisinternal
                ORDER BY t.tgname LIMIT 1001"#
            }
            Engine::SqlServer => {
                r#"SELECT TOP (1001) CAST(s.name AS nvarchar(128)),
                CAST(tr.name AS nvarchar(128)),
                CAST(CASE WHEN tr.is_disabled=1 THEN N'disabled' ELSE N'enabled' END AS nvarchar(16)),
                CAST(CASE WHEN tr.is_instead_of_trigger=1 THEN N'INSTEAD OF' ELSE N'AFTER' END AS nvarchar(16)),
                CAST(m.definition AS nvarchar(max))
                FROM sys.triggers tr JOIN sys.objects p ON p.object_id=tr.parent_id
                JOIN sys.schemas ps ON ps.schema_id=p.schema_id
                JOIN sys.objects o ON o.object_id=tr.object_id
                JOIN sys.schemas s ON s.schema_id=o.schema_id
                LEFT JOIN sys.sql_modules m ON m.object_id=tr.object_id
                WHERE ps.name=@P1 AND p.name=@P2 AND tr.parent_class=1
                ORDER BY tr.name"#
            }
        };
        let mut output = self
            .parameter_query(sql, &[&table.schema, &table.name])
            .await?;
        let mut set = output
            .sets
            .pop()
            .ok_or_else(|| anyhow::anyhow!("触发器目录没有返回结果"))?;
        set.columns = ["schema", "name", "state", "event", "definition"]
            .map(str::to_owned)
            .into();
        if set.rows.len() > super::ROW_LIMIT {
            set.rows.truncate(super::ROW_LIMIT);
            set.truncated = true;
        }
        Ok(set)
    }
}
