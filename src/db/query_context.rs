use super::*;

impl Session {
    /// Discovery runs on a separate short-lived connection owned by the query
    /// worker; it must never alter an existing user transaction or search path.
    pub async fn query_schemas(&mut self) -> Result<Vec<String>> {
        let Driver::Postgres(connection) = &mut self.driver else {
            bail!("此引擎不支持独立的查询 Schema 选择");
        };
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT nspname::text FROM pg_namespace WHERE has_schema_privilege(oid, 'USAGE') AND nspname NOT LIKE 'pg_temp_%' AND nspname NOT LIKE 'pg_toast%' ORDER BY nspname",
        )
        .fetch_all(connection)
        .await?)
    }

    pub async fn select_query_schema(&mut self, schema: &str) -> Result<()> {
        validate_identifier(schema, "Schema 名称")?;
        if matches!(&self.driver, Driver::SqlServer(_)) {
            anyhow::ensure!(
                self.current_query_schema().await?.as_deref() == Some(schema),
                "SQL Server 当前用户默认 Schema 已变化；请重新选择查询数据库，访问其他 Schema 请使用限定对象名。SQL 未执行"
            );
            return Ok(());
        }
        let Driver::Postgres(connection) = &mut self.driver else {
            bail!("此引擎不能在查询会话中改变默认 Schema；请使用带 Schema 的对象名称");
        };
        let accessible = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = $1 AND has_schema_privilege(oid, 'USAGE'))",
        )
        .bind(schema)
        .fetch_one(&mut *connection)
        .await?;
        anyhow::ensure!(
            accessible,
            "指定 Schema 不存在或当前用户没有 USAGE 权限，SQL 未执行"
        );
        // Identifiers are quoted per dialect, including embedded double quotes.
        // SET is used outside a user transaction, before their first statement.
        let sql = format!("SET search_path TO {}", Engine::Postgres.quote(schema));
        sqlx::raw_sql(&sql).execute(&mut *connection).await?;
        let actual = sqlx::query_scalar::<_, Option<String>>("SELECT current_schema()::text")
            .fetch_one(connection)
            .await?;
        anyhow::ensure!(
            actual.as_deref() == Some(schema),
            "查询会话未能进入指定 Schema，SQL 未执行"
        );
        Ok(())
    }

    pub async fn current_query_schema(&mut self) -> Result<Option<String>> {
        match &mut self.driver {
            Driver::Postgres(connection) => Ok(sqlx::query_scalar::<_, Option<String>>(
                "SELECT current_schema()::text",
            )
            .fetch_one(connection)
            .await?),
            Driver::SqlServer(client) => {
                // SQL Server's default schema belongs to the database user.
                // Observe it; selecting a target must never ALTER USER.
                let row = client
                    .simple_query("SELECT SCHEMA_NAME()")
                    .await?
                    .into_row()
                    .await?;
                Ok(row.and_then(|row| row.get::<&str, _>(0).map(str::to_owned)))
            }
            Driver::MySql(_) => Ok(None),
        }
    }
}
