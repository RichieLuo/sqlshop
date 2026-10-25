mod writes;

use super::{Driver, Session, values};
use crate::model::{
    CellKind, CellUpdate, Engine, InsertValue, QueryOutput, ResultSet, Table, TableColumn,
    TableData,
};
use anyhow::{Context, Result};
use sqlx::Connection;
use std::{
    collections::{BTreeMap, HashSet},
    fmt,
    time::Instant,
};

pub(super) struct SqlServerVarcharColumn {
    pub collation: String,
    pub max_bytes: i16,
}

#[derive(Debug)]
pub(crate) struct EditRejected(pub String);
impl fmt::Display for EditRejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// COMMIT was sent or attempted but no reliable completion was received.
/// Any nested driver error remains available for diagnostics.
#[derive(Debug)]
pub(crate) struct CommitOutcomeUnknown;
impl fmt::Display for CommitOutcomeUnknown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("COMMIT 未确认，提交结果未知；草稿已保留，请先核实，勿直接重复提交")
    }
}

impl Session {
    pub(super) async fn sqlserver_varchar_columns(
        &mut self,
        table: &Table,
    ) -> Result<BTreeMap<String, SqlServerVarcharColumn>> {
        let Driver::SqlServer(client) = &mut self.driver else {
            return Ok(BTreeMap::new());
        };
        let mut query = tiberius::Query::new(
            "SELECT c.name,c.collation_name,c.max_length FROM sys.columns c JOIN sys.objects o ON o.object_id=c.object_id JOIN sys.schemas s ON s.schema_id=o.schema_id WHERE s.name=@P1 AND o.name=@P2 AND c.system_type_id IN (167,175)",
        );
        query.bind(table.schema.as_str());
        query.bind(table.name.as_str());
        let rows = query.query(client).await?.into_first_result().await?;
        rows.into_iter()
            .map(|row| {
                Ok((
                    row.get::<&str, _>(0)
                        .context("列字符集元数据缺少名称")?
                        .to_owned(),
                    SqlServerVarcharColumn {
                        collation: row
                            .get::<&str, _>(1)
                            .context("无法读取列字符集，拒绝可能有损的文本转换")?
                            .to_owned(),
                        max_bytes: row.get::<i16, _>(2).context("列字符集元数据缺少字节长度")?,
                    },
                ))
            })
            .collect()
    }

    async fn ensure_sqlserver_text_inputs(
        &mut self,
        table: &Table,
        columns: &[TableColumn],
        changes: &[CellUpdate],
        names: &[String],
        rows: &[Vec<InsertValue>],
    ) -> Result<()> {
        if self.profile.engine != Engine::SqlServer {
            return Ok(());
        }
        let mut inputs = Vec::new();
        for change in changes {
            if let Some(value) = change.value.as_deref()
                && let Some(column) = columns
                    .iter()
                    .find(|c| c.name == change.column && sqlserver_varchar_type(c))
            {
                inputs.push((column, value));
            }
        }
        for row in rows {
            for (name, value) in names.iter().zip(row) {
                if let InsertValue::Text(value) = value
                    && let Some(column) = columns
                        .iter()
                        .find(|c| c.name == *name && sqlserver_varchar_type(c))
                {
                    inputs.push((column, value.as_str()));
                }
            }
        }
        if inputs.is_empty() {
            return Ok(());
        }
        let metadata = self.sqlserver_varchar_columns(table).await?;
        let Driver::SqlServer(client) = &mut self.driver else {
            unreachable!()
        };
        let mut checked = HashSet::new();
        for (column, value) in inputs {
            if !checked.insert((column.name.as_str(), value)) {
                continue;
            }
            let actual = metadata.get(&column.name).ok_or_else(|| {
                EditRejected("目标列或字符集元数据已改变，未执行写入；请刷新表数据".into())
            })?;
            let complete = sqlserver_varchar_expression("@P1", &actual.collation);
            let length_guard = if actual.max_bytes == -1 {
                String::new()
            } else {
                format!(" AND DATALENGTH({complete}) <= {}", actual.max_bytes)
            };
            let sql = format!(
                "SELECT CASE WHEN CONVERT(varbinary(max),CONVERT(nvarchar(max),{complete})) = CONVERT(varbinary(max),CAST(@P1 AS nvarchar(max))){length_guard} THEN 1 ELSE 0 END"
            );
            let mut query = tiberius::Query::new(sql);
            query.bind(value);
            let row = query
                .query(client)
                .await?
                .into_row()
                .await?
                .context("文本无损转换检查未返回结果")?;
            if row.get::<i32, _>(0) != Some(1) {
                return Err(EditRejected(format!(
                    "列 {} 的输入无法按列字符集无损保存，或超过列字节长度；未执行写入",
                    column.name
                ))
                .into());
            }
        }
        Ok(())
    }

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
        if self
            .table_read_interrupt
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
        {
            return Err(EditRejected(
                "表读取已停止，后续请求未发送；保留上次数据，请刷新后继续编辑".into(),
            )
            .into());
        }
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
                       WHERE i.indrelid=c.oid AND i.indisprimary AND k.position <= i.indnkeyatts AND k.attnum=a.attnum)::text,
                COALESCE((WITH RECURSIVE base AS (
                    SELECT t.oid,t.typtype,t.typbasetype UNION ALL
                    SELECT bt.oid,bt.typtype,bt.typbasetype FROM pg_type bt JOIN base b ON bt.oid=b.typbasetype WHERE b.typtype='d'
                ) SELECT CASE oid WHEN 'pg_catalog.varchar'::regtype THEN 'pg_catalog.varchar'
                    WHEN 'pg_catalog.bpchar'::regtype THEN 'pg_catalog.bpchar' END FROM base WHERE typtype<>'d'),
                    pg_catalog.format_type(a.atttypid,a.atttypmod))::text
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
                // A bounded character cast truncates before assignment checks.
                // Domains over character types also need an unbounded input;
                // assigning to the real column still enforces its domain checks.
                Engine::Postgres => field(5)?.to_owned(),
                Engine::SqlServer => declared.clone(),
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
        validate_assignment_value(engine, column, params.last().unwrap().as_deref())?;
        expressions.push(assignment_expression(engine, column, params.len()));
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
        validate_assignment_value(engine, column, change.value.as_deref())?;
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
                assignment_expression(engine, column, index + 1)
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
    let optimistic = changed_columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let position = changes.len() + keys.len() + index + 1;
            original_value_predicate(engine, column, position)
        })
        .collect::<Vec<_>>()
        .join(" AND ");
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
            Engine::SqlServer => format!("CONVERT(varbinary(max), {parameter}, 1)"),
        };
    }
    match engine {
        // A bound string has lower collation precedence than the column. Casting it
        // to CHAR can give both operands implicit precedence with different collations.
        Engine::MySql if column.kind == CellKind::Text => parameter,
        Engine::Postgres => format!(
            "CAST(CAST({parameter} AS text) AS {})",
            postgres_parameter_type(&column.sql_type)
        ),
        Engine::SqlServer if sqlserver_text_type(column) => {
            format!("CAST({parameter} AS nvarchar(max))")
        }
        _ => format!("CAST({parameter} AS {})", column.sql_type),
    }
}

// Comparisons and assignments must both retain the complete input. In particular,
// a filter for a long string must not become a filter for its truncated prefix.
fn postgres_parameter_type(sql_type: &str) -> &str {
    for prefix in ["character varying(", "varchar("] {
        if sql_type.starts_with(prefix) && sql_type.ends_with(')') {
            return "pg_catalog.varchar";
        }
    }
    for prefix in ["character(", "char("] {
        if sql_type.starts_with(prefix) && sql_type.ends_with(')') {
            return "pg_catalog.bpchar";
        }
    }
    sql_type
}

pub(super) fn sqlserver_text_type(column: &TableColumn) -> bool {
    matches!(
        column.sql_type.split('(').next(),
        Some("varchar" | "char" | "nvarchar" | "nchar" | "text" | "ntext")
    )
}

pub(super) fn sqlserver_varchar_type(column: &TableColumn) -> bool {
    matches!(column.sql_type.split('(').next(), Some("varchar" | "char"))
}

pub(super) fn sqlserver_varchar_expression(parameter: &str, collation: &str) -> String {
    format!(
        "CAST({parameter} COLLATE {} AS varchar(max))",
        Engine::SqlServer.quote(collation)
    )
}

fn assignment_expression(engine: Engine, column: &TableColumn, index: usize) -> String {
    // Keep numeric, temporal, enum and other custom-type conversions. Text and
    // binary conversions above deliberately omit a length; the destination
    // column, rather than an explicit narrowing CAST, must validate the value.
    parameter_expression(engine, column, index)
}

fn validate_assignment_value(
    engine: Engine,
    column: &TableColumn,
    value: Option<&str>,
) -> Result<()> {
    let Some(value) = value else { return Ok(()) };
    if engine != Engine::SqlServer {
        return Ok(());
    }
    let Some((base, length)) = column.sql_type.split_once('(') else {
        return Ok(());
    };
    let Some(length) = length
        .strip_suffix(')')
        .and_then(|n| n.parse::<usize>().ok())
    else {
        return Ok(());
    };
    // SQL Server can truncate binary assignments without a warning even with
    // ANSI_WARNINGS enabled. Validate the original hex bytes before sending SQL.
    let too_long = match base {
        "binary" | "varbinary" => value
            .strip_prefix("0x")
            .is_some_and(|hex| hex.len() / 2 > length),
        "nvarchar" | "nchar" => value.encode_utf16().count() > length,
        "varchar" | "char" => value.chars().count() > length,
        _ => false,
    };
    if too_long {
        return Err(EditRejected(format!(
            "列 {} 的输入超过 {} 长度，未执行写入",
            column.name, column.declared_type
        ))
        .into());
    }
    Ok(())
}

pub(super) fn original_value_predicate(
    engine: Engine,
    column: &TableColumn,
    index: usize,
) -> String {
    let parameter = placeholder(engine, index);
    let displayed = display_expression(engine, column);
    match engine {
        // json/xml have no equality operator, and citext/nondeterministic
        // collations can equate changed strings. Compare the exact representation
        // originally read, without reinterpreting it through the column type.
        Engine::Postgres => format!(
            "({displayed} COLLATE \"C\") IS NOT DISTINCT FROM (CAST({parameter} AS text) COLLATE \"C\")"
        ),
        Engine::MySql => format!("BINARY {displayed} <=> BINARY {parameter}"),
        Engine::SqlServer => format!(
            "(CONVERT(varbinary(max),CAST({displayed} AS nvarchar(max))) = CONVERT(varbinary(max),CAST({parameter} AS nvarchar(max))) OR ({displayed} IS NULL AND {parameter} IS NULL))"
        ),
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
mod workflow_tests;

impl std::error::Error for EditRejected {}

impl std::error::Error for CommitOutcomeUnknown {}
