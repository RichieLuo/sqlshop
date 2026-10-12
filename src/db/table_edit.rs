use super::{Driver, Session, values};
use crate::model::{
    CellKind, CellUpdate, Engine, QueryOutput, ResultSet, Table, TableColumn, TableData,
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

impl Session {
    async fn parameter_query(&mut self, sql: &str, parameters: &[&str]) -> Result<QueryOutput> {
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
                    .map(values::mysql)
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
        })
    }

    async fn table_columns(&mut self, table: &Table) -> Result<Vec<TableColumn>> {
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
                CASE WHEN c.EXTRA LIKE '%GENERATED%' OR c.EXTRA LIKE '%auto_increment%' OR t.TABLE_TYPE <> 'BASE TABLE' OR t.ENGINE <> 'InnoDB' THEN 'true' ELSE 'false' END,
                CASE WHEN c.COLUMN_KEY='PRI' THEN 'true' ELSE 'false' END
                FROM information_schema.COLUMNS c JOIN information_schema.TABLES t ON t.TABLE_SCHEMA=c.TABLE_SCHEMA AND t.TABLE_NAME=c.TABLE_NAME
                WHERE c.TABLE_SCHEMA=? AND c.TABLE_NAME=? ORDER BY c.ORDINAL_POSITION"#
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
                _ => declared,
            };
            columns.push(TableColumn {
                name,
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
        let columns = self.table_columns(table).await?;
        let engine = self.profile.engine;
        let expressions = columns
            .iter()
            .map(|column| {
                format!(
                    "{} AS {}",
                    display_expression(engine, column),
                    engine.quote(&column.name)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let table_name = format!(
            "{}.{}",
            engine.quote(&table.schema),
            engine.quote(&table.name)
        );
        let primary_keys: Vec<_> = columns
            .iter()
            .filter(|c| c.primary_key)
            .map(|c| c.name.clone())
            .collect();
        let order = if primary_keys.is_empty() {
            String::new()
        } else {
            format!(
                " ORDER BY {}",
                primary_keys
                    .iter()
                    .map(|name| engine.quote(name))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let sql = match engine {
            Engine::SqlServer => format!("SELECT TOP (200) {expressions} FROM {table_name}{order}"),
            _ => format!("SELECT {expressions} FROM {table_name}{order} LIMIT 200"),
        };
        let mut output = self.query(&sql).await?;
        if output.sets.is_empty() {
            output.sets.push(ResultSet::default());
        }
        output.sets[0].columns = columns.iter().map(|column| column.name.clone()).collect();
        output.sets[0].affected = None;
        let indexes: Vec<_> = columns
            .iter()
            .enumerate()
            .filter(|(_, column)| column.primary_key)
            .map(|(index, _)| index)
            .collect();
        let keys = output.sets[0]
            .rows
            .iter()
            .map(|row| indexes.iter().map(|index| row[*index].clone()).collect())
            .collect();
        Ok(TableData {
            output,
            columns,
            primary_keys,
            keys,
        })
    }

    pub async fn update_cell(&mut self, change: &CellUpdate) -> Result<()> {
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
                    Ok(1) => tx.commit().await?,
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
                    Ok(1) => tx.commit().await?,
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
                            .await?
                            .into_results()
                            .await?;
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
}

fn rejected_update(error: Result<u64, impl fmt::Display>) -> EditRejected {
    EditRejected(match error {
        Ok(0) => "该行已被其他操作修改或删除，请刷新后再编辑；本次修改未保存".into(),
        Ok(_) => "匹配到多行或触发器返回了额外写入，修改已回滚".into(),
        Err(error) => format!("保存失败，修改已回滚：{error}"),
    })
}

fn valid_hex(value: &str) -> bool {
    value
        .strip_prefix("0x")
        .is_some_and(|hex| hex.len() % 2 == 0 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn placeholder(engine: Engine, index: usize) -> String {
    match engine {
        Engine::MySql => "?".into(),
        Engine::Postgres => format!("${index}"),
        Engine::SqlServer => format!("@P{index}"),
    }
}

fn parameter_expression(engine: Engine, column: &TableColumn, index: usize) -> String {
    let parameter = placeholder(engine, index);
    if column.kind == CellKind::Binary {
        return match engine {
            Engine::Postgres => format!("decode(substr(CAST({parameter} AS text),3),'hex')"),
            Engine::MySql => format!("UNHEX(SUBSTRING({parameter},3))"),
            Engine::SqlServer => format!("CONVERT({}, {parameter}, 1)", column.sql_type),
        };
    }
    match engine {
        Engine::Postgres => format!("CAST(CAST({parameter} AS text) AS {})", column.sql_type),
        _ => format!("CAST({parameter} AS {})", column.sql_type),
    }
}

fn display_expression(engine: Engine, column: &TableColumn) -> String {
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
