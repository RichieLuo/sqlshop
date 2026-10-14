use super::{Driver, Session, values};
use crate::model::{
    CellKind, CellUpdate, Engine, InsertValue, QueryOutput, ResultSet, Table, TableColumn,
    TableData,
};
use anyhow::{Context, Result};
use sqlx::Connection;
use std::{fmt, time::Instant};

#[derive(Debug)]
pub(crate) struct EditRejected(pub String);
impl fmt::Display for EditRejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for EditRejected {}

/// COMMIT was sent or attempted but no reliable completion was received.
/// Any nested driver error remains available for diagnostics.
#[derive(Debug)]
pub(crate) struct CommitOutcomeUnknown;
impl fmt::Display for CommitOutcomeUnknown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("COMMIT 未确认，提交结果未知；草稿已保留，请先核实，勿直接重复提交")
    }
}
impl std::error::Error for CommitOutcomeUnknown {}

impl Session {
    pub(super) async fn table_writable(&mut self, table: &Table) -> Result<bool> {
        let sql = match self.profile.engine {
            Engine::MySql => {
                "SELECT CASE WHEN TABLE_TYPE='BASE TABLE' AND ENGINE='InnoDB' THEN 'true' ELSE 'false' END FROM information_schema.TABLES WHERE TABLE_SCHEMA=? AND TABLE_NAME=?"
            }
            Engine::Postgres => {
                "SELECT (c.relkind IN ('r','p'))::text FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2"
            }
            Engine::SqlServer => {
                "SELECT CASE WHEN o.type='U' THEN 'true' ELSE 'false' END FROM sys.objects o JOIN sys.schemas s ON s.schema_id=o.schema_id WHERE s.name=@P1 AND o.name=@P2"
            }
        };
        let output = self
            .parameter_query(sql, &[&table.schema, &table.name])
            .await?;
        Ok(output
            .sets
            .first()
            .and_then(|set| set.rows.first())
            .and_then(|row| row.first())
            .and_then(Option::as_deref)
            == Some("true"))
    }

    async fn ensure_table_writable(&mut self, table: &Table) -> Result<()> {
        if !self.table_writable(table).await? {
            return Err(EditRejected(
                "目标不是支持事务写入的普通表；MySQL 表必须使用 InnoDB，未执行写入".into(),
            )
            .into());
        }
        Ok(())
    }

    pub(super) async fn parameter_query(
        &mut self,
        sql: &str,
        parameters: &[&str],
    ) -> Result<QueryOutput> {
        self.parameter_query_columns(sql, parameters, None).await
    }

    pub(super) async fn parameter_query_columns(
        &mut self,
        sql: &str,
        parameters: &[&str],
        columns: Option<&[TableColumn]>,
    ) -> Result<QueryOutput> {
        let start = Instant::now();
        let rows = match &mut self.driver {
            Driver::MySql(connection) => {
                let mut query = sqlx::query(sql);
                for parameter in parameters {
                    query = query.bind(*parameter);
                }
                query
                    .fetch_all(connection)
                    .await?
                    .iter()
                    .map(|row| match columns {
                        Some(columns) => values::mysql_table(row, columns),
                        None => values::mysql(row),
                    })
                    .collect()
            }
            Driver::Postgres(connection) => {
                let mut query = sqlx::query(sql);
                for parameter in parameters {
                    query = query.bind(*parameter);
                }
                query
                    .fetch_all(connection)
                    .await?
                    .iter()
                    .map(values::postgres)
                    .collect()
            }
            Driver::SqlServer(client) => {
                let mut query = tiberius::Query::new(sql);
                for parameter in parameters {
                    query.bind(*parameter);
                }
                query
                    .query(client)
                    .await?
                    .into_first_result()
                    .await?
                    .into_iter()
                    .map(values::sqlserver)
                    .collect()
            }
        };
        Ok(QueryOutput {
            sets: vec![ResultSet {
                rows,
                ..Default::default()
            }],
            elapsed_ms: start.elapsed().as_millis(),
            messages: Vec::new(),
        })
    }

    pub async fn table_columns(&mut self, table: &Table) -> Result<Vec<TableColumn>> {
        let sql = match self.profile.engine {
            Engine::Postgres => {
                r#"SELECT a.attname::text, pg_catalog.format_type(a.atttypid,a.atttypmod)::text, t.typname::text,
                (a.attgenerated <> '' OR a.attidentity <> '' OR c.relkind NOT IN ('r','p'))::text,
                EXISTS(SELECT 1 FROM pg_index i, unnest(i.indkey) WITH ORDINALITY k(attnum, position)
                       WHERE i.indrelid=c.oid AND i.indisprimary AND k.position <= i.indnkeyatts AND k.attnum=a.attnum)::text
                FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid JOIN pg_namespace n ON n.oid=c.relnamespace
                JOIN pg_type t ON t.oid=a.atttypid
                WHERE n.nspname=$1 AND c.relname=$2 AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum"#
            }
            Engine::MySql => {
                r#"SELECT
                CAST(c.COLUMN_NAME AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
                CAST(c.COLUMN_TYPE AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
                CAST(c.DATA_TYPE AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
                CASE WHEN c.EXTRA LIKE '%VIRTUAL GENERATED%' OR c.EXTRA LIKE '%STORED GENERATED%' OR c.EXTRA LIKE '%auto_increment%' OR t.TABLE_TYPE <> 'BASE TABLE' OR t.ENGINE <> 'InnoDB' THEN 'true' ELSE 'false' END,
                CASE WHEN c.COLUMN_KEY='PRI' THEN 'true' ELSE 'false' END
                FROM information_schema.COLUMNS c JOIN information_schema.TABLES t
                ON CONVERT(t.TABLE_SCHEMA USING utf8mb4) COLLATE utf8mb4_unicode_ci = CONVERT(c.TABLE_SCHEMA USING utf8mb4) COLLATE utf8mb4_unicode_ci
                AND CONVERT(t.TABLE_NAME USING utf8mb4) COLLATE utf8mb4_unicode_ci = CONVERT(c.TABLE_NAME USING utf8mb4) COLLATE utf8mb4_unicode_ci
                WHERE CONVERT(c.TABLE_SCHEMA USING utf8mb4) COLLATE utf8mb4_unicode_ci = CONVERT(? USING utf8mb4) COLLATE utf8mb4_unicode_ci
                AND CONVERT(c.TABLE_NAME USING utf8mb4) COLLATE utf8mb4_unicode_ci = CONVERT(? USING utf8mb4) COLLATE utf8mb4_unicode_ci
                ORDER BY c.ORDINAL_POSITION"#
            }
            Engine::SqlServer => {
                r#"SELECT c.name,
                t.name + CASE WHEN t.name IN ('varchar','char','varbinary','binary','nvarchar','nchar')
                    THEN '(' + CASE WHEN c.max_length=-1 THEN 'max' ELSE CONVERT(varchar(10),CASE WHEN t.name IN ('nvarchar','nchar') THEN c.max_length/2 ELSE c.max_length END) END + ')'
                    WHEN t.name IN ('decimal','numeric') THEN '('+CONVERT(varchar(10),c.precision)+','+CONVERT(varchar(10),c.scale)+')'
                    WHEN t.name IN ('datetime2','datetimeoffset','time') THEN '('+CONVERT(varchar(10),c.scale)+')' ELSE '' END,
                t.name,
                CASE WHEN c.is_computed=1 OR c.is_identity=1 OR c.generated_always_type<>0 OR o.type<>'U' OR t.name IN ('timestamp','rowversion','text','ntext','image','xml','sql_variant') THEN 'true' ELSE 'false' END,
                CASE WHEN EXISTS(SELECT 1 FROM sys.indexes i JOIN sys.index_columns k ON k.object_id=i.object_id AND k.index_id=i.index_id
                    WHERE i.object_id=c.object_id AND i.is_primary_key=1 AND k.column_id=c.column_id AND k.key_ordinal>0) THEN 'true' ELSE 'false' END
                FROM sys.columns c JOIN sys.objects o ON o.object_id=c.object_id JOIN sys.schemas s ON s.schema_id=o.schema_id
                JOIN sys.types t ON t.user_type_id=c.system_type_id
                WHERE s.name=@P1 AND o.name=@P2 ORDER BY c.column_id"#
            }
        };
        let output = self
            .parameter_query(sql, &[&table.schema, &table.name])
            .await?;
        let mut columns = Vec::new();
        for row in &output.sets[0].rows {
            let field = |index| {
                row.get(index)
                    .and_then(Option::as_deref)
                    .context("表结构元数据不完整")
            };
            let name = field(0)?.to_owned();
            let declared = field(1)?.to_owned();
            let raw = field(2)?;
            let kind = match raw {
                "bytea" | "binary" | "varbinary" | "tinyblob" | "blob" | "mediumblob"
                | "longblob" | "image" | "rowversion" => CellKind::Binary,
                "timestamp" if self.profile.engine == Engine::SqlServer => CellKind::Binary,
                "int2" | "int4" | "int8" | "tinyint" | "smallint" | "mediumint" | "int"
                | "integer" | "bigint" | "year" => {
                    if declared.contains("unsigned") {
                        CellKind::Unsigned
                    } else {
                        CellKind::Integer
                    }
                }
                "numeric" | "decimal" | "money" | "smallmoney" => CellKind::Decimal,
                "real" | "float" | "float4" | "float8" | "double" => CellKind::Float,
                "bool" | "boolean" => CellKind::Boolean,
                "bit" if self.profile.engine == Engine::SqlServer => CellKind::Boolean,
                "bit" => CellKind::Unsupported,
                "date" => CellKind::Date,
                "datetime" | "datetime2" | "datetimeoffset" | "smalldatetime" | "timestamp"
                | "timestamptz" => CellKind::DateTime,
                "time" | "timetz" => CellKind::Time,
                "geometry" | "geography" | "hierarchyid" | "sql_variant" | "point"
                | "linestring" | "polygon" | "multipoint" | "multilinestring" | "multipolygon"
                | "geometrycollection" => CellKind::Unsupported,
                _ => CellKind::Text,
            };
            let sql_type = match self.profile.engine {
                Engine::MySql => match kind {
                    CellKind::Integer => "SIGNED".into(),
                    CellKind::Unsigned => "UNSIGNED".into(),
                    CellKind::Decimal => declared.clone(),
                    CellKind::Float => "DOUBLE".into(),
                    CellKind::Date => "DATE".into(),
                    CellKind::DateTime => "DATETIME(6)".into(),
                    CellKind::Time => "TIME(6)".into(),
                    _ => "CHAR CHARACTER SET utf8mb4".into(),
                },
                _ => declared.clone(),
            };
            columns.push(TableColumn {
                name,
                declared_type: declared,
                sql_type,
                kind,
                editable: field(3)? == "false" && kind != CellKind::Unsupported,
                primary_key: field(4)? == "true",
            });
        }
        anyhow::ensure!(!columns.is_empty(), "表已不存在，或当前账号无法读取其结构");
        Ok(columns)
    }

    pub async fn table_data(&mut self, table: &Table) -> Result<TableData> {
        Ok(self
            .table_page(table, &super::TableBrowseOptions::default())
            .await?
            .data)
    }

    pub async fn update_cell(&mut self, change: &CellUpdate) -> Result<()> {
        self.ensure_edit_transaction_safe().await?;
        if self.current_database().await?.as_deref() != Some(change.database.as_str()) {
            return Err(
                EditRejected("当前数据库已改变，请选择此表所属数据库后再保存".into()).into(),
            );
        }
        let columns = self.table_columns(&change.table).await?;
        let keys: Vec<_> = columns.iter().filter(|column| column.primary_key).collect();
        let column = columns
            .iter()
            .find(|column| column.name == change.column)
            .context("该列已不存在，请刷新表数据")?;
        if !column.editable
            || keys.is_empty()
            || keys.iter().any(|key| key.kind == CellKind::Unsupported)
            || keys
                .iter()
                .map(|key| &key.name)
                .ne(change.primary_keys.iter())
            || keys.len() != change.key_values.len()
            || change.key_values.iter().any(Option::is_none)
        {
            return Err(EditRejected(
                "表结构已改变或缺少可用主键，该单元格不可编辑；请刷新表数据".into(),
            )
            .into());
        }
        if change.original == change.value {
            return Ok(());
        }
        let mut parameters = vec![change.value.clone()];
        parameters.extend(change.key_values.iter().cloned());
        parameters.push(change.original.clone());
        for (column, value) in std::iter::once((column, &parameters[0])).chain(
            keys.iter()
                .zip(change.key_values.iter())
                .map(|(column, value)| (*column, value)),
        ) {
            if column.kind == CellKind::Binary
                && value.as_deref().is_some_and(|value| !valid_hex(value))
            {
                return Err(EditRejected("二进制值请使用 0x 开头的完整十六进制文本".into()).into());
            }
        }
        let engine = self.profile.engine;
        let predicates = keys
            .iter()
            .enumerate()
            .map(|(index, column)| {
                format!(
                    "{} = {}",
                    engine.quote(&column.name),
                    parameter_expression(engine, column, index + 2)
                )
            })
            .collect::<Vec<_>>();
        let old_value = parameter_expression(engine, column, parameters.len());
        let name = engine.quote(&column.name);
        let optimistic = match engine {
            Engine::Postgres => format!("{name} IS NOT DISTINCT FROM {old_value}"),
            Engine::MySql => format!(
                "BINARY {} <=> BINARY {}",
                display_expression(engine, column),
                placeholder(engine, parameters.len())
            ),
            Engine::SqlServer => format!(
                "(CONVERT(varbinary(max),{name}) = CONVERT(varbinary(max),{old_value}) OR ({name} IS NULL AND {old_value} IS NULL))"
            ),
        };
        // SQL Server references the same positional parameter twice; MySQL's ? is used once.
        let sql = format!(
            "UPDATE {}.{} SET {name} = {} WHERE {} AND ({optimistic})",
            engine.quote(&change.table.schema),
            engine.quote(&change.table.name),
            parameter_expression(engine, column, 1),
            predicates.join(" AND ")
        );
        match &mut self.driver {
            Driver::MySql(connection) => {
                let mut tx = connection.begin().await?;
                let mut query = sqlx::query(&sql);
                for parameter in &parameters {
                    query = query.bind(parameter.as_deref());
                }
                match query
                    .execute(&mut *tx)
                    .await
                    .map(|result| result.rows_affected())
                {
                    Ok(1) => tx.commit().await.context(CommitOutcomeUnknown)?,
                    result => {
                        tx.rollback().await.context("回滚修改失败")?;
                        return Err(rejected_update(result).into());
                    }
                }
            }
            Driver::Postgres(connection) => {
                let mut tx = connection.begin().await?;
                let mut query = sqlx::query(&sql);
                for parameter in &parameters {
                    query = query.bind(parameter.as_deref());
                }
                match query
                    .execute(&mut *tx)
                    .await
                    .map(|result| result.rows_affected())
                {
                    Ok(1) => tx.commit().await.context(CommitOutcomeUnknown)?,
                    result => {
                        tx.rollback().await.context("回滚修改失败")?;
                        return Err(rejected_update(result).into());
                    }
                }
            }
            Driver::SqlServer(client) => {
                client
                    .simple_query("BEGIN TRANSACTION")
                    .await?
                    .into_results()
                    .await?;
                let mut query = tiberius::Query::new(&sql);
                for parameter in &parameters {
                    query.bind(parameter.as_deref());
                }
                match query.execute(client).await.map(|result| result.total()) {
                    Ok(1) => {
                        client
                            .simple_query("COMMIT TRANSACTION")
                            .await
                            .context(CommitOutcomeUnknown)?
                            .into_results()
                            .await
                            .context(CommitOutcomeUnknown)?;
                    }
                    result => {
                        client
                            .simple_query("IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION")
                            .await?
                            .into_results()
                            .await?;
                        return Err(rejected_update(result).into());
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn delete_rows(
        &mut self,
        database: &str,
        table: &Table,
        primary_keys: &[String],
        key_values: &[Vec<Option<String>>],
    ) -> Result<usize> {
        self.ensure_edit_transaction_safe().await?;
        if self.current_database().await?.as_deref() != Some(database) {
            return Err(
                EditRejected("当前数据库已改变，请选择此表所属数据库后再删除".into()).into(),
            );
        }
        anyhow::ensure!(!key_values.is_empty(), "没有选中要删除的行");
        self.ensure_table_writable(table).await?;
        let columns = self.table_columns(table).await?;
        let keys: Vec<_> = columns.iter().filter(|column| column.primary_key).collect();
        if keys.is_empty()
            || keys.iter().any(|key| key.kind == CellKind::Unsupported)
            || keys
                .iter()
                .map(|key| key.name.as_str())
                .ne(primary_keys.iter().map(String::as_str))
            || key_values
                .iter()
                .any(|values| values.len() != keys.len() || values.iter().any(Option::is_none))
        {
            return Err(EditRejected(
                "表结构已改变或缺少可用主键，无法安全删除；请刷新表数据".into(),
            )
            .into());
        }
        let engine = self.profile.engine;
        let predicates = keys
            .iter()
            .enumerate()
            .map(|(index, column)| {
                format!(
                    "{} = {}",
                    engine.quote(&column.name),
                    parameter_expression(engine, column, index + 1)
                )
            })
            .collect::<Vec<_>>();
        let sql = format!(
            "DELETE FROM {}.{} WHERE {}",
            engine.quote(&table.schema),
            engine.quote(&table.name),
            predicates.join(" AND ")
        );
        let mut deleted = 0;
        match &mut self.driver {
            Driver::MySql(connection) => {
                let mut tx = connection.begin().await?;
                for values in key_values {
                    let mut query = sqlx::query(&sql);
                    for value in values {
                        query = query.bind(value.as_deref());
                    }
                    match query
                        .execute(&mut *tx)
                        .await
                        .map(|result| result.rows_affected())
                    {
                        Ok(1) => deleted += 1,
                        result => {
                            tx.rollback().await.context("回滚删除失败")?;
                            return Err(rejected_delete(result).into());
                        }
                    }
                }
                tx.commit().await.context(CommitOutcomeUnknown)?;
            }
            Driver::Postgres(connection) => {
                let mut tx = connection.begin().await?;
                for values in key_values {
                    let mut query = sqlx::query(&sql);
                    for value in values {
                        query = query.bind(value.as_deref());
                    }
                    match query
                        .execute(&mut *tx)
                        .await
                        .map(|result| result.rows_affected())
                    {
                        Ok(1) => deleted += 1,
                        result => {
                            tx.rollback().await.context("回滚删除失败")?;
                            return Err(rejected_delete(result).into());
                        }
                    }
                }
                tx.commit().await.context(CommitOutcomeUnknown)?;
            }
            Driver::SqlServer(client) => {
                client
                    .simple_query("BEGIN TRANSACTION")
                    .await?
                    .into_results()
                    .await?;
                for values in key_values {
                    let mut query = tiberius::Query::new(&sql);
                    for value in values {
                        query.bind(value.as_deref());
                    }
                    match query.execute(client).await.map(|result| result.total()) {
                        Ok(1) => deleted += 1,
                        result => {
                            client
                                .simple_query("IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION")
                                .await?
                                .into_results()
                                .await?;
                            return Err(rejected_delete(result).into());
                        }
                    }
                }
                client
                    .simple_query("COMMIT TRANSACTION")
                    .await
                    .context(CommitOutcomeUnknown)?
                    .into_results()
                    .await
                    .context(CommitOutcomeUnknown)?;
            }
        }
        Ok(deleted)
    }

    pub async fn insert_rows(
        &mut self,
        database: &str,
        table: &Table,
        names: &[String],
        rows: &[Vec<InsertValue>],
    ) -> Result<usize> {
        self.ensure_edit_transaction_safe().await?;
        if self.current_database().await?.as_deref() != Some(database) {
            return Err(EditRejected("当前数据库已改变，导入或新增未执行".into()).into());
        }
        self.ensure_table_writable(table).await?;
        let columns = self.table_columns(table).await?;
        let engine = self.profile.engine;
        anyhow::ensure!(!rows.is_empty(), "没有需要新增的数据");
        anyhow::ensure!(names.len() <= columns.len(), "列映射数量超过表列数");
        let prepared = rows
            .iter()
            .map(|row| insert_statement(engine, table, &columns, names, row))
            .collect::<Result<Vec<_>>>()?;
        match &mut self.driver {
            Driver::MySql(connection) => {
                let mut tx = connection.begin().await?;
                for (sql, parameters) in &prepared {
                    let mut query = sqlx::query(sql);
                    for value in parameters {
                        query = query.bind(value.as_deref());
                    }
                    if let Err(error) = query.execute(&mut *tx).await {
                        tx.rollback().await.context("回滚导入失败")?;
                        return Err(EditRejected(format!("新增失败，全部已回滚：{error}")).into());
                    }
                }
                tx.commit().await.context(CommitOutcomeUnknown)?;
            }
            Driver::Postgres(connection) => {
                let mut tx = connection.begin().await?;
                for (sql, parameters) in &prepared {
                    let mut query = sqlx::query(sql);
                    for value in parameters {
                        query = query.bind(value.as_deref());
                    }
                    if let Err(error) = query.execute(&mut *tx).await {
                        tx.rollback().await.context("回滚导入失败")?;
                        return Err(EditRejected(format!("新增失败，全部已回滚：{error}")).into());
                    }
                }
                tx.commit().await.context(CommitOutcomeUnknown)?;
            }
            Driver::SqlServer(client) => {
                client
                    .simple_query("BEGIN TRANSACTION")
                    .await?
                    .into_results()
                    .await?;
                for (sql, parameters) in &prepared {
                    let mut query = tiberius::Query::new(sql);
                    for value in parameters {
                        query.bind(value.as_deref());
                    }
                    if let Err(error) = query.execute(client).await {
                        client
                            .simple_query("IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION")
                            .await?
                            .into_results()
                            .await?;
                        return Err(EditRejected(format!("新增失败，全部已回滚：{error}")).into());
                    }
                }
                client
                    .simple_query("COMMIT TRANSACTION")
                    .await
                    .context(CommitOutcomeUnknown)?
                    .into_results()
                    .await
                    .context(CommitOutcomeUnknown)?;
            }
        }
        Ok(prepared.len())
    }

    pub async fn update_cells(&mut self, changes: &[CellUpdate]) -> Result<usize> {
        let first = changes
            .first()
            .ok_or_else(|| EditRejected("没有待提交的修改".into()))?;
        let (updated_cells, _) = self
            .save_table_draft(&first.database, &first.table, changes, &[], &[])
            .await?;
        Ok(updated_cells)
    }

    /// Apply the complete local table draft in one transaction. Prepare every
    /// statement before beginning, and retain the original row optimistic checks.
    pub async fn save_table_draft(
        &mut self,
        database: &str,
        table: &Table,
        changes: &[CellUpdate],
        names: &[String],
        rows: &[Vec<InsertValue>],
    ) -> Result<(usize, usize)> {
        if changes.is_empty() && rows.is_empty() {
            return Err(EditRejected("没有待提交的新增或修改".into()).into());
        }
        if changes
            .iter()
            .any(|change| change.database != database || change.table != *table)
        {
            return Err(EditRejected("暂存修改与新增行的目标不一致，未执行写入".into()).into());
        }
        self.ensure_edit_transaction_safe().await?;
        if self.current_database().await?.as_deref() != Some(database) {
            return Err(EditRejected("当前数据库已改变，暂存提交未执行".into()).into());
        }
        self.ensure_table_writable(table).await?;
        let columns = self.table_columns(table).await?;
        let engine = self.profile.engine;
        if names.len() > columns.len() {
            return Err(EditRejected("新增行的列映射数量超过表列数".into()).into());
        }
        let mut prepared = Vec::new();
        if let Some(first) = changes.first() {
            let mut groups: Vec<Vec<&CellUpdate>> = Vec::new();
            for change in changes {
                if let Some(group) = groups
                    .iter_mut()
                    .find(|group| group[0].key_values == change.key_values)
                {
                    group.push(change);
                } else {
                    groups.push(vec![change]);
                }
            }
            for group in groups {
                let (sql, parameters) = update_row_statement(engine, &columns, first, &group)?;
                prepared.push((sql, parameters, true));
            }
        }
        for row in rows {
            let (sql, parameters) = insert_statement(engine, table, &columns, names, row)
                .map_err(|error| EditRejected(format!("新增草稿无效，未执行写入：{error:#}")))?;
            prepared.push((sql, parameters, false));
        }
        match &mut self.driver {
            Driver::MySql(connection) => {
                let mut tx = connection.begin().await?;
                for (sql, parameters, update) in &prepared {
                    let mut query = sqlx::query(sql);
                    for value in parameters {
                        query = query.bind(value.as_deref());
                    }
                    let result = query
                        .execute(&mut *tx)
                        .await
                        .map(|result| result.rows_affected())
                        .map_err(Into::into);
                    if let Some(error) = table_draft_statement_error(result, *update) {
                        tx.rollback().await.context("回滚表格暂存提交失败")?;
                        return Err(error);
                    }
                }
                // A failed commit has an unknown outcome; propagate its transport
                // error so the worker closes the connection and preserves drafts.
                tx.commit().await.context(CommitOutcomeUnknown)?;
            }
            Driver::Postgres(connection) => {
                let mut tx = connection.begin().await?;
                for (sql, parameters, update) in &prepared {
                    let mut query = sqlx::query(sql);
                    for value in parameters {
                        query = query.bind(value.as_deref());
                    }
                    let result = query
                        .execute(&mut *tx)
                        .await
                        .map(|result| result.rows_affected())
                        .map_err(Into::into);
                    if let Some(error) = table_draft_statement_error(result, *update) {
                        tx.rollback().await.context("回滚表格暂存提交失败")?;
                        return Err(error);
                    }
                }
                tx.commit().await.context(CommitOutcomeUnknown)?;
            }
            Driver::SqlServer(client) => {
                client
                    .simple_query("BEGIN TRANSACTION")
                    .await?
                    .into_results()
                    .await?;
                for (sql, parameters, update) in &prepared {
                    let mut query = tiberius::Query::new(sql);
                    for value in parameters {
                        query.bind(value.as_deref());
                    }
                    let result = query
                        .execute(client)
                        .await
                        .map(|result| result.total())
                        .map_err(Into::into);
                    if let Some(error) = table_draft_statement_error(result, *update) {
                        client
                            .simple_query("IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION")
                            .await?
                            .into_results()
                            .await?;
                        return Err(error);
                    }
                }
                client
                    .simple_query("COMMIT TRANSACTION")
                    .await
                    .context(CommitOutcomeUnknown)?
                    .into_results()
                    .await
                    .context(CommitOutcomeUnknown)?;
            }
        }
        Ok((changes.len(), rows.len()))
    }

    pub async fn insert_default_row(&mut self, database: &str, table: &Table) -> Result<()> {
        self.ensure_edit_transaction_safe().await?;
        if self.current_database().await?.as_deref() != Some(database) {
            return Err(
                EditRejected("当前数据库已改变，请选择此表所属数据库后再新增行".into()).into(),
            );
        }
        self.ensure_table_writable(table).await?;
        let engine = self.profile.engine;
        let name = format!(
            "{}.{}",
            engine.quote(&table.schema),
            engine.quote(&table.name)
        );
        let sql = match engine {
            Engine::MySql => format!("INSERT INTO {name} () VALUES ()"),
            Engine::Postgres | Engine::SqlServer => {
                format!("INSERT INTO {name} DEFAULT VALUES")
            }
        };
        match &mut self.driver {
            Driver::MySql(connection) => {
                sqlx::query(&sql)
                    .execute(connection)
                    .await
                    .map_err(|error| EditRejected(format!("新增默认行失败：{error}")))?;
            }
            Driver::Postgres(connection) => {
                sqlx::query(&sql)
                    .execute(connection)
                    .await
                    .map_err(|error| EditRejected(format!("新增默认行失败：{error}")))?;
            }
            Driver::SqlServer(client) => {
                let stream = client
                    .simple_query(&sql)
                    .await
                    .map_err(|error| EditRejected(format!("新增默认行失败：{error}")))?;
                stream
                    .into_results()
                    .await
                    .map_err(|error| EditRejected(format!("新增默认行失败：{error}")))?;
            }
        }
        Ok(())
    }
}

// Called before rollback, but returned only once rollback has succeeded.
// Ordinary SQL errors keep a usable connection; transport errors never do.
fn table_draft_statement_error(result: Result<u64>, update: bool) -> Option<anyhow::Error> {
    match result {
        Ok(count) if update && count != 1 => Some(rejected_update(Ok::<u64, &str>(count)).into()),
        Ok(_) => None,
        Err(error) if super::query_error_preserves_session(&error) => {
            Some(EditRejected(format!("暂存提交失败，新增和修改全部已回滚：{error:#}")).into())
        }
        Err(error) => Some(error),
    }
}

fn insert_statement(
    engine: Engine,
    table: &Table,
    columns: &[TableColumn],
    names: &[String],
    row: &[InsertValue],
) -> Result<(String, Vec<Option<String>>)> {
    anyhow::ensure!(names.len() == row.len(), "列映射与数据列数不符");
    let mut selected = Vec::new();
    let mut expressions = Vec::new();
    let mut params = Vec::new();
    for (index, (name, value)) in names.iter().zip(row).enumerate() {
        let column = columns
            .iter()
            .find(|c| c.name == *name && c.editable)
            .ok_or_else(|| EditRejected(format!("列 {name} 已不存在或不可写，请刷新表结构")))?;
        anyhow::ensure!(!names[..index].contains(name), "列映射不能重复");
        if matches!(value, InsertValue::Default) {
            continue;
        }
        selected.push(engine.quote(name));
        let parameter = match value {
            InsertValue::Null => None,
            InsertValue::Text(text) => {
                if column.kind == CellKind::Binary && !valid_hex(text) {
                    return Err(
                        EditRejected("二进制值请使用 0x 开头的完整十六进制文本".into()).into(),
                    );
                }
                Some(text.clone())
            }
            InsertValue::Default => unreachable!(),
        };
        params.push(parameter);
        expressions.push(parameter_expression(engine, column, params.len()));
    }
    let table_name = format!(
        "{}.{}",
        engine.quote(&table.schema),
        engine.quote(&table.name)
    );
    let sql = if selected.is_empty() {
        match engine {
            Engine::MySql => format!("INSERT INTO {table_name} () VALUES ()"),
            _ => format!("INSERT INTO {table_name} DEFAULT VALUES"),
        }
    } else {
        format!(
            "INSERT INTO {table_name} ({}) VALUES ({})",
            selected.join(", "),
            expressions.join(", ")
        )
    };
    Ok((sql, params))
}

fn update_row_statement(
    engine: Engine,
    columns: &[TableColumn],
    first: &CellUpdate,
    changes: &[&CellUpdate],
) -> Result<(String, Vec<Option<String>>)> {
    let row = changes
        .first()
        .ok_or_else(|| EditRejected("没有待提交的修改".into()))?;
    let keys: Vec<_> = columns.iter().filter(|c| c.primary_key).collect();
    if keys.is_empty() || keys.iter().any(|c| c.kind == CellKind::Unsupported) {
        return Err(EditRejected("缺少可用主键，请刷新后重试".into()).into());
    }
    let mut changed_columns = Vec::new();
    for (index, change) in changes.iter().enumerate() {
        if change.database != first.database
            || change.table != first.table
            || change.key_values != row.key_values
        {
            return Err(EditRejected("批量修改目标不一致".into()).into());
        }
        let column = columns
            .iter()
            .find(|c| c.name == change.column && c.editable)
            .ok_or_else(|| EditRejected("列已不存在或不可写，请刷新表数据".into()))?;
        if keys.iter().map(|c| &c.name).ne(change.primary_keys.iter())
            || change.key_values.len() != keys.len()
            || change.key_values.iter().any(Option::is_none)
            || change.original == change.value
        {
            return Err(
                EditRejected("主键或表结构已改变，或存在空修改；请刷新后重试".into()).into(),
            );
        }
        if changes[..index]
            .iter()
            .any(|previous| previous.column == change.column)
        {
            return Err(EditRejected("同一单元格重复暂存，请刷新表数据".into()).into());
        }
        for (c, value) in std::iter::once((column, &change.value))
            .chain(keys.iter().zip(&change.key_values).map(|(c, v)| (*c, v)))
        {
            if c.kind == CellKind::Binary && value.as_deref().is_some_and(|v| !valid_hex(v)) {
                return Err(EditRejected("二进制值请使用 0x 开头的完整十六进制文本".into()).into());
            }
        }
        changed_columns.push(column);
    }
    // One statement per original row: all SET expressions and optimistic checks
    // refer to that same row, even when a primary-key column is being changed.
    let mut params = changes
        .iter()
        .map(|change| change.value.clone())
        .collect::<Vec<_>>();
    params.extend(row.key_values.iter().cloned());
    params.extend(changes.iter().map(|change| change.original.clone()));
    let assignments = changed_columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            format!(
                "{} = {}",
                engine.quote(&column.name),
                parameter_expression(engine, column, index + 1)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let predicates = keys
        .iter()
        .enumerate()
        .map(|(index, key)| {
            format!(
                "{} = {}",
                engine.quote(&key.name),
                parameter_expression(engine, key, changes.len() + index + 1)
            )
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    let optimistic = changed_columns.iter().enumerate().map(|(index, column)| {
        let name = engine.quote(&column.name);
        let position = changes.len() + keys.len() + index + 1;
        let old = parameter_expression(engine, column, position);
        match engine {
            Engine::Postgres => format!("{name} IS NOT DISTINCT FROM {old}"),
            Engine::MySql => format!("BINARY {} <=> BINARY {}", display_expression(engine, column), placeholder(engine, position)),
            Engine::SqlServer => format!("(CONVERT(varbinary(max),{name}) = CONVERT(varbinary(max),{old}) OR ({name} IS NULL AND {old} IS NULL))"),
        }
    }).collect::<Vec<_>>().join(" AND ");
    let sql = format!(
        "UPDATE {}.{} SET {assignments} WHERE {predicates} AND ({optimistic})",
        engine.quote(&first.table.schema),
        engine.quote(&first.table.name)
    );
    Ok((sql, params))
}

fn rejected_update(error: Result<u64, impl fmt::Display>) -> EditRejected {
    EditRejected(match error {
        Ok(0) => "该行已被其他操作修改或删除，请刷新后再编辑；本次修改未保存".into(),
        Ok(_) => "匹配到多行或触发器返回了额外写入，修改已回滚".into(),
        Err(error) => format!("保存失败，修改已回滚：{error}"),
    })
}

fn rejected_delete(error: Result<u64, impl fmt::Display>) -> EditRejected {
    EditRejected(match error {
        Ok(0) => "该行已被其他操作修改或删除，请刷新后再删除；本次操作已回滚".into(),
        Ok(_) => "匹配到多行或触发器返回了额外写入，删除已回滚".into(),
        Err(error) => format!("删除失败，已回滚：{error}"),
    })
}

fn valid_hex(value: &str) -> bool {
    value
        .strip_prefix("0x")
        .is_some_and(|hex| hex.len() % 2 == 0 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub(super) fn placeholder(engine: Engine, index: usize) -> String {
    match engine {
        Engine::MySql => "?".into(),
        Engine::Postgres => format!("${index}"),
        Engine::SqlServer => format!("@P{index}"),
    }
}

pub(super) fn parameter_expression(engine: Engine, column: &TableColumn, index: usize) -> String {
    let parameter = placeholder(engine, index);
    if column.kind == CellKind::Binary {
        return match engine {
            Engine::Postgres => format!("decode(substr(CAST({parameter} AS text),3),'hex')"),
            Engine::MySql => format!("UNHEX(SUBSTRING({parameter},3))"),
            Engine::SqlServer => format!("CONVERT({}, {parameter}, 1)", column.sql_type),
        };
    }
    match engine {
        // A bound string has lower collation precedence than the column. Casting it
        // to CHAR can give both operands implicit precedence with different collations.
        Engine::MySql if column.kind == CellKind::Text => parameter,
        Engine::Postgres => format!("CAST(CAST({parameter} AS text) AS {})", column.sql_type),
        _ => format!("CAST({parameter} AS {})", column.sql_type),
    }
}

pub(super) fn display_expression(engine: Engine, column: &TableColumn) -> String {
    let name = engine.quote(&column.name);
    if column.kind == CellKind::Unsupported {
        return format!("CASE WHEN {name} IS NULL THEN NULL ELSE '[暂不支持的类型]' END");
    }
    match (engine, column.kind) {
        (Engine::Postgres, CellKind::Binary) => format!("'0x' || encode({name},'hex')"),
        (Engine::Postgres, _) => format!("CAST({name} AS text)"),
        (Engine::MySql, CellKind::Binary) => format!("CONCAT('0x',LOWER(HEX({name})))"),
        (Engine::MySql, _) => {
            format!("CAST({name} AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci")
        }
        (Engine::SqlServer, CellKind::Binary) => format!("CONVERT(varchar(max),{name},1)"),
        (Engine::SqlServer, CellKind::Float) => format!("CONVERT(nvarchar(max),{name},3)"),
        (Engine::SqlServer, CellKind::Decimal)
            if matches!(column.sql_type.as_str(), "money" | "smallmoney") =>
        {
            format!("CONVERT(nvarchar(max),{name},2)")
        }
        (Engine::SqlServer, _) => format!("CONVERT(nvarchar(max),{name},121)"),
    }
}

#[cfg(test)]
mod workflow_tests {
    use super::*;

    fn columns() -> Vec<TableColumn> {
        vec![
            TableColumn {
                name: "id".into(),
                declared_type: "integer".into(),
                sql_type: "integer".into(),
                kind: CellKind::Integer,
                editable: true,
                primary_key: true,
            },
            TableColumn {
                name: "note".into(),
                declared_type: "text".into(),
                sql_type: "text".into(),
                kind: CellKind::Text,
                editable: true,
                primary_key: false,
            },
        ]
    }
    fn table() -> Table {
        Table {
            schema: "a`\"]".into(),
            name: "b`\"]".into(),
            kind: "BASE TABLE".into(),
        }
    }
    #[test]
    fn insert_quotes_identifiers_and_binds_null_text_and_default_for_three_engines() {
        for engine in Engine::ALL {
            let names = vec!["id".into(), "note".into()];
            let (sql, params) = insert_statement(
                engine,
                &table(),
                &columns(),
                &names,
                &[
                    InsertValue::Default,
                    InsertValue::Text("NULL' ; DROP TABLE x".into()),
                ],
            )
            .unwrap();
            assert!(sql.contains(&engine.quote("note")));
            assert!(sql.contains(&engine.quote(&table().name)));
            assert!(!sql.contains("DROP TABLE"));
            assert_eq!(params, vec![Some("NULL' ; DROP TABLE x".into())]);
            let (sql, params) = insert_statement(
                engine,
                &table(),
                &columns(),
                &names,
                &[InsertValue::Default, InsertValue::Null],
            )
            .unwrap();
            assert_eq!(params, vec![None]);
            assert_eq!(sql.matches(&placeholder(engine, 1)).count(), 1);
            assert!(
                insert_statement(
                    engine,
                    &table(),
                    &columns(),
                    &["note; DROP".into()],
                    &[InsertValue::Null]
                )
                .is_err()
            );
            assert!(
                insert_statement(
                    engine,
                    &table(),
                    &columns(),
                    &names,
                    &[InsertValue::Null, InsertValue::Null]
                )
                .is_ok()
            );
            assert!(
                insert_statement(
                    engine,
                    &table(),
                    &columns(),
                    &["note".into(), "note".into()],
                    &[InsertValue::Null, InsertValue::Null]
                )
                .is_err()
            );
        }
    }
    #[test]
    fn batch_conflict_sql_preserves_original_null_and_parameter_order() {
        for engine in Engine::ALL {
            let change = CellUpdate {
                database: "db".into(),
                table: table(),
                primary_keys: vec!["id".into()],
                key_values: vec![Some("7".into())],
                column: "note".into(),
                original: None,
                value: Some("NULL".into()),
            };
            let (sql, params) =
                update_row_statement(engine, &columns(), &change, &[&change]).unwrap();
            assert_eq!(params, vec![Some("NULL".into()), Some("7".into()), None]);
            assert!(!sql.contains("'NULL'"));
            assert!(sql.contains(&engine.quote("note")));
            assert!(sql.contains(&placeholder(engine, 3)));
            let mut invalid = change.clone();
            invalid.primary_keys = vec!["injected".into()];
            assert!(update_row_statement(engine, &columns(), &change, &[&invalid]).is_err());
            assert!(rejected_update(Ok::<u64, &str>(0)).0.contains("修改未保存"));
        }
    }
}
