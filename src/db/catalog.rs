use super::*;

impl Session {
    pub async fn databases(&mut self) -> Result<Vec<String>> {
        let sql = match self.profile.engine {
            // MySQL's catalog identifiers use a binary collation, which SQLx
            // classifies as VARBINARY. Normalize known text columns so names
            // aren't decoded as hex while real binary query values still are.
            Engine::MySql => {
                "SELECT CAST(SCHEMA_NAME AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS database_name FROM information_schema.SCHEMATA ORDER BY SCHEMA_NAME"
            }
            Engine::Postgres => {
                "SELECT datname::text FROM pg_database WHERE datallowconn AND NOT datistemplate AND has_database_privilege(datname, 'CONNECT') ORDER BY datname"
            }
            Engine::SqlServer => {
                "SELECT name FROM sys.databases WHERE state = 0 AND HAS_DBACCESS(name) = 1 ORDER BY name"
            }
        };
        let output = self.query_internal(sql).await?;
        let mut databases = Vec::new();
        for set in output.sets {
            anyhow::ensure!(
                !set.truncated,
                "数据库列表超过 {ROW_LIMIT} 项，请使用 SQL 浏览完整列表"
            );
            for row in set.rows {
                if let Some(Some(database)) = row.first() {
                    databases.push(database.clone());
                }
            }
        }
        databases.sort();
        databases.dedup();
        Ok(databases)
    }

    pub async fn select_database(&mut self, database: &str) -> Result<()> {
        anyhow::ensure!(
            !database.is_empty() && !database.contains('\0'),
            "数据库名称无效"
        );
        match &mut self.driver {
            Driver::MySql(connection) => {
                let sql = Engine::MySql
                    .select_database_sql(database)
                    .context("切换语句缺失")?;
                // USE isn't supported by MySQL's prepared statement protocol.
                sqlx::raw_sql(&sql).execute(connection).await?;
            }
            Driver::SqlServer(client) => {
                let sql = Engine::SqlServer
                    .select_database_sql(database)
                    .context("切换语句缺失")?;
                client.simple_query(sql).await?.into_results().await?;
            }
            Driver::Postgres(_) => {
                // Replace only after the new connection is ready; failed selection
                // must not destroy the previous session or its SSH tunnel.
                let mut replacement =
                    Self::open(&self.profile, &self.credentials, Some(database)).await?;
                replacement.ping().await?;
                *self = replacement;
            }
        }
        Ok(())
    }

    pub async fn create_database(&mut self, database: &str) -> Result<()> {
        self.ensure_edit_transaction_safe().await?;
        validate_identifier(database, "数据库名称")?;
        self.query_internal(&format!(
            "CREATE DATABASE {}",
            self.profile.engine.quote(database)
        ))
        .await?;
        Ok(())
    }

    pub async fn drop_database(&mut self, database: &str) -> Result<()> {
        self.ensure_edit_transaction_safe().await?;
        validate_identifier(database, "数据库名称")?;
        anyhow::ensure!(
            !is_system_database(self.profile.engine, database),
            "系统数据库不能通过此操作删除"
        );
        anyhow::ensure!(
            self.current_database().await?.as_deref() != Some(database),
            "不能删除当前正在使用的数据库；请先切换到其他数据库"
        );
        self.query_internal(&format!(
            "DROP DATABASE {}",
            self.profile.engine.quote(database)
        ))
        .await?;
        Ok(())
    }

    pub async fn create_table(
        &mut self,
        database: &str,
        schema: &str,
        table: &str,
        columns: &[CreateTableColumn],
    ) -> Result<()> {
        self.ensure_edit_transaction_safe().await?;
        validate_identifier(schema, "Schema 名称")?;
        validate_identifier(table, "表名称")?;
        anyhow::ensure!(!columns.is_empty(), "请至少添加一个字段");
        anyhow::ensure!(
            self.current_database().await?.as_deref() == Some(database),
            "当前数据库与新建表的目标数据库不符，请先选择目标数据库"
        );
        let engine = self.profile.engine;
        let mut names = Vec::with_capacity(columns.len());
        let mut primary_keys = Vec::new();
        let mut auto_increment_count = 0;
        let mut definitions = Vec::with_capacity(columns.len() + 1);
        for column in columns {
            validate_identifier(&column.name, "字段名称")?;
            anyhow::ensure!(
                !names
                    .iter()
                    .any(|name: &String| name.eq_ignore_ascii_case(&column.name)),
                "字段名称重复：{}",
                column.name
            );
            names.push(column.name.clone());
            let data_type = allowed_column_type(engine, &column.data_type)?;
            let size = column.size.trim();
            if !size.is_empty() {
                anyhow::ensure!(
                    size.len() <= 16
                        && size
                            .split(',')
                            .all(|part| { part.parse::<u32>().is_ok_and(|value| value > 0) }),
                    "字段长度或精度格式无效：{}",
                    column.name
                );
            }
            let sql_type = if size.is_empty() {
                data_type.to_owned()
            } else {
                format!("{data_type}({size})")
            };
            if column.auto_increment {
                auto_increment_count += 1;
                anyhow::ensure!(
                    ["TINYINT", "SMALLINT", "INT", "INTEGER", "BIGINT"]
                        .iter()
                        .any(|integer| data_type.eq_ignore_ascii_case(integer)),
                    "自增字段必须使用整数类型：{}",
                    column.name
                );
                anyhow::ensure!(
                    column.default_value.trim().is_empty(),
                    "自增字段不能同时设置默认值"
                );
            }
            if column.primary_key {
                primary_keys.push(engine.quote(&column.name));
            }
            let mut definition = format!("{} {sql_type}", engine.quote(&column.name));
            match (engine, column.auto_increment) {
                (Engine::MySql, true) => definition.push_str(" AUTO_INCREMENT"),
                (Engine::Postgres, true) => {
                    definition.push_str(" GENERATED BY DEFAULT AS IDENTITY")
                }
                (Engine::SqlServer, true) => definition.push_str(" IDENTITY(1,1)"),
                _ => {}
            }
            if !column.nullable || column.primary_key || column.auto_increment {
                definition.push_str(" NOT NULL");
            } else {
                definition.push_str(" NULL");
            }
            let default = column.default_value.trim();
            if !default.is_empty() {
                anyhow::ensure!(
                    !default.contains(';') && !default.contains('\0'),
                    "默认值不能包含分号或空字符：{}",
                    column.name
                );
                definition.push_str(" DEFAULT ");
                definition.push_str(default);
            }
            definitions.push(definition);
        }
        anyhow::ensure!(auto_increment_count <= 1, "一张表最多只能有一个自增字段");
        if !primary_keys.is_empty() {
            definitions.push(format!("PRIMARY KEY ({})", primary_keys.join(", ")));
        }
        self.query_internal(&format!(
            "CREATE TABLE {}.{} ({})",
            engine.quote(schema),
            engine.quote(table),
            definitions.join(", ")
        ))
        .await?;
        Ok(())
    }

    pub async fn drop_table(&mut self, database: &str, table: &Table) -> Result<()> {
        self.ensure_edit_transaction_safe().await?;
        validate_identifier(&table.schema, "Schema 名称")?;
        validate_identifier(&table.name, "表名称")?;
        anyhow::ensure!(
            self.current_database().await?.as_deref() == Some(database),
            "当前数据库与删除表的目标数据库不符，请先选择目标数据库"
        );
        let exists_as_table = self.tables().await?.iter().any(|existing| {
            existing.schema == table.schema
                && existing.name == table.name
                && existing.kind.eq_ignore_ascii_case("BASE TABLE")
        });
        anyhow::ensure!(exists_as_table, "目标不存在或不是普通数据表，未执行删除");
        let engine = self.profile.engine;
        self.query_internal(&format!(
            "DROP TABLE {}.{}",
            engine.quote(&table.schema),
            engine.quote(&table.name)
        ))
        .await?;
        Ok(())
    }

    pub async fn tables(&mut self) -> Result<Vec<Table>> {
        // Queries are fixed strings. Table preview identifiers are escaped per engine.
        let sql = match &self.driver {
            Driver::MySql(_) => {
                "SELECT CAST(TABLE_SCHEMA AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS TABLE_SCHEMA, CAST(TABLE_NAME AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS TABLE_NAME, CAST(TABLE_TYPE AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS TABLE_TYPE FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_SCHEMA, TABLE_NAME"
            }
            Driver::Postgres(_) => {
                "SELECT table_schema, table_name, table_type FROM information_schema.tables WHERE table_schema NOT IN ('pg_catalog', 'information_schema') ORDER BY table_schema, table_name"
            }
            Driver::SqlServer(_) => {
                "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES ORDER BY TABLE_SCHEMA, TABLE_NAME"
            }
        };
        let output = self.query_internal(sql).await?;
        let mut tables = Vec::new();
        for set in output.sets {
            if set.truncated {
                bail!("表目录超过 {ROW_LIMIT} 项，请使用 SQL 查询 information_schema 浏览完整目录");
            }
            for row in set.rows {
                if row.len() >= 3 {
                    tables.push(Table {
                        schema: row[0].clone().context("表目录缺少 schema")?,
                        name: row[1].clone().context("表目录缺少表名")?,
                        kind: row[2].clone().unwrap_or_default(),
                    });
                }
            }
        }
        Ok(tables)
    }
}
