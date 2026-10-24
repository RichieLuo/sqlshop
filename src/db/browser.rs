use super::{
    Driver, EditRejected, Session,
    table_edit::{display_expression, parameter_expression, placeholder},
};
use crate::model::{CellKind, Engine, ResultSet, Table, TableColumn, TableData};
use anyhow::{Context, Result, ensure};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterOperator {
    Eq,
    NotEq,
    Contains,
    Gt,
    Lt,
    IsNull,
    IsNotNull,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnFilter {
    pub column: String,
    pub operator: FilterOperator,
    /// Required for comparisons; ignored for IsNull and IsNotNull.
    pub value: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnSort {
    pub column: String,
    pub descending: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableBrowseOptions {
    pub offset: u64,
    pub page_size: u32,
    pub filters: Vec<ColumnFilter>,
    pub sorts: Vec<ColumnSort>,
}
impl Default for TableBrowseOptions {
    fn default() -> Self {
        Self {
            offset: 0,
            page_size: 200,
            filters: vec![],
            sorts: vec![],
        }
    }
}
#[derive(Clone, Debug)]
pub struct TablePage {
    pub data: TableData,
    pub has_more: bool,
    pub sql: String,
    pub parameters: Vec<String>,
}

impl TablePage {
    pub fn execution_sql(&self) -> String {
        let mut text = self.sql.clone();
        if !self.parameters.is_empty() {
            text.push_str("\n-- 绑定参数（按占位符顺序，值以 JSON 字符串表示）");
            for (index, value) in self.parameters.iter().enumerate() {
                text.push_str(&format!(
                    "\n-- {}: {}",
                    index + 1,
                    serde_json::to_string(value).expect("string serialization cannot fail")
                ));
            }
        }
        text
    }
}
#[derive(Clone, Debug)]
pub struct TableStructure {
    pub constraints: ResultSet,
    pub columns: ResultSet,
    pub indexes: ResultSet,
    pub foreign_keys: ResultSet,
    /// MySQL SHOW CREATE output, or explicitly limited reconstructed DDL.
    pub ddl: String,
}

fn browse_predicates(
    engine: Engine,
    columns: &[TableColumn],
    options: &TableBrowseOptions,
) -> Result<(Vec<String>, Vec<String>)> {
    ensure!(
        (1..=1000).contains(&options.page_size),
        "每页行数必须在 1 到 1000 之间"
    );
    ensure!(
        options.offset < i64::MAX as u64 - u64::from(options.page_size),
        "分页偏移量过大"
    );
    ensure!(
        options.filters.len() <= 100 && options.sorts.len() <= columns.len(),
        "筛选或排序条件过多"
    );
    let find = |name: &str| {
        columns
            .iter()
            .find(|c| c.name == name)
            .context("筛选或排序列已不存在，请刷新表结构")
    };
    let mut parameters = Vec::new();
    let mut predicates = Vec::new();
    for filter in &options.filters {
        let column = find(&filter.column)?;
        let name = engine.quote(&column.name);
        let operator = match filter.operator {
            FilterOperator::IsNull => {
                predicates.push(format!("{name} IS NULL"));
                continue;
            }
            FilterOperator::IsNotNull => {
                predicates.push(format!("{name} IS NOT NULL"));
                continue;
            }
            FilterOperator::Eq => "=",
            FilterOperator::NotEq => "<>",
            FilterOperator::Gt => ">",
            FilterOperator::Lt => "<",
            FilterOperator::Contains => "LIKE",
        };
        ensure!(column.kind != CellKind::Unsupported, "此类型暂不支持值筛选");
        let value = filter
            .value
            .as_ref()
            .context("筛选值不能为空；查找 NULL 请使用 IsNull")?;
        if filter.operator == FilterOperator::Contains {
            ensure!(column.kind == CellKind::Text, "包含筛选只支持文本列");
            // Escape wildcard characters using a non-backslash escape, including
            // SQL Server's additional bracket wildcard syntax.
            let value = value
                .replace('!', "!!")
                .replace('%', "!%")
                .replace('_', "!_");
            let value = if engine == Engine::SqlServer {
                value.replace('[', "![")
            } else {
                value
            };
            parameters.push(format!("%{value}%"));
            predicates.push(format!(
                "{} LIKE {} ESCAPE '!'",
                if engine == Engine::MySql {
                    name.clone()
                } else {
                    display_expression(engine, column)
                },
                placeholder(engine, parameters.len())
            ));
        } else {
            if column.kind == CellKind::Binary {
                ensure!(
                    value.strip_prefix("0x").is_some_and(
                        |v| v.len() % 2 == 0 && v.bytes().all(|b| b.is_ascii_hexdigit())
                    ),
                    "二进制筛选值必须是 0x 开头的完整十六进制文本"
                );
            }
            parameters.push(value.clone());
            predicates.push(format!(
                "{name} {operator} {}",
                parameter_expression(engine, column, parameters.len())
            ));
        }
    }
    Ok((predicates, parameters))
}

fn page_sql(
    engine: Engine,
    table: &Table,
    columns: &[TableColumn],
    options: &TableBrowseOptions,
) -> Result<(String, Vec<String>)> {
    let (predicates, parameters) = browse_predicates(engine, columns, options)?;
    let find = |name: &str| {
        columns
            .iter()
            .find(|column| column.name == name)
            .context("筛选或排序列已不存在，请刷新表结构")
    };
    let mut order = Vec::new();
    let mut seen = Vec::new();
    for sort in &options.sorts {
        let column = find(&sort.column)?;
        ensure!(column.kind != CellKind::Unsupported, "此类型暂不支持排序");
        ensure!(!seen.contains(&column.name.as_str()), "排序列不能重复");
        seen.push(column.name.as_str());
        order.push(format!(
            "{} {}",
            engine.quote(&column.name),
            if sort.descending { "DESC" } else { "ASC" }
        ));
    }
    for key in columns.iter().filter(|c| c.primary_key) {
        if !seen.contains(&key.name.as_str()) {
            order.push(format!("{} ASC", engine.quote(&key.name)));
        }
    }
    let stable = columns.iter().any(|c| c.primary_key);
    if order.is_empty() && engine == Engine::SqlServer {
        order.push("(SELECT NULL)".into());
    }
    let projection = columns
        .iter()
        .map(|column| browse_projection(engine, column))
        .collect::<Vec<_>>()
        .join(", ");
    let mut sql = format!(
        "SELECT {projection} FROM {}.{}",
        engine.quote(&table.schema),
        engine.quote(&table.name)
    );
    if !predicates.is_empty() {
        sql.push_str(&format!(" WHERE {}", predicates.join(" AND ")));
    }
    if !order.is_empty() {
        sql.push_str(&format!(" ORDER BY {}", order.join(", ")));
    }
    let limit = options.page_size + 1;
    match engine {
        Engine::SqlServer => sql.push_str(&format!(
            " OFFSET {} ROWS FETCH NEXT {limit} ROWS ONLY",
            options.offset
        )),
        _ => sql.push_str(&format!(" LIMIT {limit} OFFSET {}", options.offset)),
    }
    if !stable {
        sql.push_str(" /* No primary key: pagination order is not guaranteed stable. */");
    }
    Ok((sql, parameters))
}

fn mysql_native_text(declared_type: &str) -> bool {
    let base = declared_type
        .split(['(', ' '])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        base.as_str(),
        "char" | "varchar" | "tinytext" | "text" | "mediumtext" | "longtext" | "enum" | "set"
    )
}

fn browse_projection(engine: Engine, column: &TableColumn) -> String {
    let name = engine.quote(&column.name);
    if engine == Engine::MySql {
        match column.kind {
            CellKind::Integer | CellKind::Unsigned | CellKind::Boolean => return name,
            CellKind::Text if mysql_native_text(&column.declared_type) => return name,
            // Keep the database's canonical spelling for optimistic edit comparisons:
            // decimal precision, temporal fractions and JSON whitespace must not change.
            CellKind::Text
            | CellKind::Decimal
            | CellKind::Float
            | CellKind::Date
            | CellKind::DateTime
            | CellKind::Time => {
                return format!("CAST({name} AS CHAR CHARACTER SET utf8mb4) AS {name}");
            }
            _ => {}
        }
    }
    format!("{} AS {name}", display_expression(engine, column))
}

fn set_table_metadata(set: &mut ResultSet, columns: &[TableColumn]) {
    set.columns = columns.iter().map(|c| c.name.clone()).collect();
    set.column_types = columns.iter().map(|c| c.declared_type.clone()).collect();
}

impl Session {
    /// COUNT is requested only by explicit last-page navigation. The count and
    /// read do not lock the table; concurrent writes may change the last page.
    pub async fn table_last_page(
        &mut self,
        table: &Table,
        options: &TableBrowseOptions,
    ) -> Result<(TablePage, u64)> {
        let columns = self.table_columns(table).await?;
        let mut last_options = options.clone();
        last_options.offset = 0;
        // Validate sorts as well as predicates before a potentially costly count.
        let _ = page_sql(self.profile.engine, table, &columns, &last_options)
            .map_err(|error| EditRejected(error.to_string()))?;
        let (predicates, parameters) =
            browse_predicates(self.profile.engine, &columns, &last_options)
                .map_err(|error| EditRejected(error.to_string()))?;
        let count = if self.profile.engine == Engine::SqlServer {
            "COUNT_BIG(*)"
        } else {
            "COUNT(*)"
        };
        let mut sql = format!(
            "SELECT {count} AS row_count FROM {}.{}",
            self.profile.engine.quote(&table.schema),
            self.profile.engine.quote(&table.name)
        );
        if !predicates.is_empty() {
            sql.push_str(&format!(" WHERE {}", predicates.join(" AND ")));
        }
        let parameters = parameters.iter().map(String::as_str).collect::<Vec<_>>();
        let output = self.parameter_query(&sql, &parameters).await?;
        let count = output
            .sets
            .first()
            .and_then(|set| set.rows.first())
            .and_then(|row| row.first())
            .and_then(|value| value.as_deref())
            .context("未返回筛选后的记录总数")?
            .parse::<u64>()
            .context("记录总数格式无效")?;
        let size = u64::from(last_options.page_size);
        let offset = (count.saturating_sub(1) / size)
            .checked_mul(size)
            .context("末页偏移量过大")?;
        last_options.offset = offset;
        let page = self.table_page(table, &last_options).await?;
        Ok((page, offset))
    }

    pub async fn table_page(
        &mut self,
        table: &Table,
        options: &TableBrowseOptions,
    ) -> Result<TablePage> {
        let columns = self.table_columns(table).await?;
        let writable = self.table_writable(table).await?;
        let (sql, parameters) = page_sql(self.profile.engine, table, &columns, options)
            .map_err(|error| EditRejected(error.to_string()))?;
        let refs = parameters.iter().map(String::as_str).collect::<Vec<_>>();
        let mut output = self
            .parameter_query_columns(&sql, &refs, Some(&columns))
            .await?;
        let set = &mut output.sets[0];
        let has_more = set.rows.len() > options.page_size as usize;
        set.rows.truncate(options.page_size as usize);
        set_table_metadata(set, &columns);
        let indexes = columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.primary_key)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        let keys = set
            .rows
            .iter()
            .map(|row| indexes.iter().map(|i| row[*i].clone()).collect())
            .collect();
        let primary_keys = indexes.iter().map(|i| columns[*i].name.clone()).collect();
        Ok(TablePage {
            data: TableData {
                output,
                columns,
                primary_keys,
                keys,
                writable,
            },
            has_more,
            sql,
            parameters,
        })
    }

    pub async fn export_table_csv(
        &mut self,
        database: &str,
        table: &Table,
        options: &TableBrowseOptions,
        path: &std::path::Path,
    ) -> Result<u64> {
        if self.current_database().await?.as_deref() != Some(database) {
            return Err(EditRejected("当前数据库已改变，导出未执行".into()).into());
        }
        let columns = self.table_columns(table).await?;
        ensure!(
            columns.iter().all(|c| c.kind != CellKind::Unsupported),
            "此表包含无法无损导出的列"
        );
        let mut options = options.clone();
        options.offset = 0;
        options.page_size = 1000;
        ensure!(
            columns.iter().any(|c| c.primary_key),
            "完整导出要求主键以保证分页顺序稳定"
        );
        let _ = page_sql(self.profile.engine, table, &columns, &options)?;
        ensure!(path.file_name().is_some(), "请选择 CSV 文件路径");
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => ensure!(metadata.file_type().is_file(), "导出目标不是普通文件"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        ensure!(
            columns
                .iter()
                .filter(|c| c.primary_key)
                .all(|c| c.kind != CellKind::Unsupported),
            "主键类型不支持稳定导出"
        );
        let temporary =
            path.with_extension(format!("sqlshop-{}.tmp", uuid::Uuid::new_v4().simple()));
        let result = async {
            let mut writer = std::io::BufWriter::new(std::fs::File::create_new(&temporary)?);
            crate::model::write_csv_row(
                &mut writer,
                &columns
                    .iter()
                    .map(|c| Some(c.name.clone()))
                    .collect::<Vec<_>>(),
            )?;
            let mut total = 0u64;
            loop {
                let page = self.table_page(table, &options).await?;
                for row in &page.data.output.sets[0].rows {
                    crate::model::write_csv_row(&mut writer, row)?;
                    total += 1;
                }
                if !page.has_more {
                    break;
                }
                options.offset = options
                    .offset
                    .checked_add(page.data.output.sets[0].rows.len() as u64)
                    .context("导出分页偏移量溢出")?;
            }
            std::io::Write::flush(&mut writer)?;
            writer.get_ref().sync_all()?;
            drop(writer);
            std::fs::rename(&temporary, path)?;
            Ok(total)
        }
        .await;
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }

    pub(super) async fn ensure_edit_transaction_safe(&mut self) -> Result<()> {
        let rejected = || {
            EditRejected(
                "当前连接可能处于用户事务或关闭了自动提交；请完成事务并重新连接后再编辑表格".into(),
            )
        };
        match &mut self.driver {
            Driver::MySql(_) if self.raw_query_used => return Err(rejected().into()),
            Driver::MySql(connection) => {
                let autocommit: i64 =
                    sqlx::query_scalar("SELECT CAST(@@session.autocommit AS SIGNED)")
                        .fetch_one(connection)
                        .await?;
                if autocommit != 1 {
                    return Err(rejected().into());
                }
            }
            Driver::Postgres(connection) => {
                // SAVEPOINT does not begin/end transactions. A unique name avoids
                // shadowing the user's savepoints. 25P01 is the server's exact
                // no-transaction response, not a guess based on query text.
                let name = format!("sqlshop_probe_{}", uuid::Uuid::new_v4().simple());
                match sqlx::raw_sql(&format!("SAVEPOINT {name}"))
                    .execute(&mut *connection)
                    .await
                {
                    Ok(_) => {
                        sqlx::raw_sql(&format!("RELEASE SAVEPOINT {name}"))
                            .execute(connection)
                            .await?;
                        return Err(rejected().into());
                    }
                    Err(sqlx::Error::Database(error))
                        if error.code().as_deref() == Some("25P01") => {}
                    Err(sqlx::Error::Database(error))
                        if error.code().as_deref() == Some("25P02") =>
                    {
                        return Err(rejected().into());
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Driver::SqlServer(client) => {
                let row = client.simple_query("SELECT @@TRANCOUNT AS transaction_count, (@@OPTIONS & 2) AS implicit_transactions, (@@OPTIONS & 8) AS ansi_warnings").await?.into_row().await?.context("无法读取事务状态")?;
                if row.get::<i32, _>(0) != Some(0) || row.get::<i32, _>(1) != Some(0) {
                    return Err(rejected().into());
                }
                if row.get::<i32, _>(2) != Some(8) {
                    return Err(EditRejected(
                        "当前连接关闭了 ANSI_WARNINGS，可能静默截断文本；请重新连接后再编辑表格"
                            .into(),
                    )
                    .into());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
                name: "name".into(),
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
            schema: "odd]`\"".into(),
            name: "table]`\"".into(),
            kind: "BASE TABLE".into(),
        }
    }
    #[test]
    fn headers_use_declared_types_even_without_rows() {
        let mut columns = columns();
        columns[0].declared_type = "bigint unsigned".into();
        columns[0].sql_type = "UNSIGNED".into();
        columns[1].declared_type = "varchar(100)".into();
        columns[1].sql_type = "CHAR CHARACTER SET utf8mb4".into();
        let mut set = ResultSet {
            column_types: vec!["cast result".into()],
            ..Default::default()
        };
        set_table_metadata(&mut set, &columns);
        assert_eq!(set.columns, ["id", "name"]);
        assert_eq!(set.column_types, ["bigint unsigned", "varchar(100)"]);
        assert_eq!(set.columns.len(), set.column_types.len());
        assert!(set.rows.is_empty());
    }

    #[test]
    fn execution_sql_preserves_template_and_encodes_bound_values() {
        let mut page = TablePage {
            data: TableData {
                output: Default::default(),
                columns: vec![],
                primary_keys: vec![],
                writable: false,
                keys: vec![],
            },
            has_more: false,
            sql: "SELECT * FROM items WHERE name = $1 LIMIT 201 OFFSET 200".into(),
            parameters: vec![],
        };
        assert_eq!(page.execution_sql(), page.sql);
        page.parameters = vec!["O'Brien\n-- injected comment".into(), "罗子言".into()];
        let text = page.execution_sql();
        assert!(text.starts_with(&page.sql));
        assert!(text.contains("-- 1: \"O'Brien\\n-- injected comment\""));
        assert!(text.contains("-- 2: \"罗子言\""));
        assert_eq!(text.lines().count(), 4);
    }

    #[test]
    fn parameterized_paging_and_stable_ties() {
        for engine in Engine::ALL {
            let options = TableBrowseOptions {
                offset: 20,
                page_size: 10,
                filters: vec![ColumnFilter {
                    column: "name".into(),
                    operator: FilterOperator::Contains,
                    value: Some("x%' OR 1=1 --_![".into()),
                }],
                sorts: vec![ColumnSort {
                    column: "name".into(),
                    descending: true,
                }],
            };
            let (sql, values) = page_sql(engine, &table(), &columns(), &options).unwrap();
            assert!(!sql.contains("OR 1=1"));
            assert!(values[0].contains("!%"));
            assert!(sql.contains(&format!(
                "{} DESC, {} ASC",
                engine.quote("name"),
                engine.quote("id")
            )));
            assert!(sql.contains("11"));
            assert!(sql.contains("20"));
        }
    }
    #[test]
    fn mysql_browse_uses_native_columns_and_only_necessary_conversions() {
        let (sql, _) = page_sql(
            Engine::MySql,
            &table(),
            &columns(),
            &TableBrowseOptions::default(),
        )
        .unwrap();
        assert!(sql.starts_with("SELECT `id`, `name` FROM "));
        assert!(!sql.contains("CAST"));
        assert!(!sql.contains("COLLATE"));
        for (kind, declared_type) in [
            (CellKind::Decimal, "decimal(65,30)"),
            (CellKind::Float, "float"),
            (CellKind::DateTime, "datetime(6)"),
            (CellKind::Time, "time(6)"),
            (CellKind::Text, "json"),
        ] {
            let mut column = columns().remove(1);
            column.kind = kind;
            column.declared_type = declared_type.into();
            assert_eq!(
                browse_projection(Engine::MySql, &column),
                "CAST(`name` AS CHAR CHARACTER SET utf8mb4) AS `name`"
            );
        }
        let mut column = columns().remove(1);
        column.kind = CellKind::Binary;
        assert!(browse_projection(Engine::MySql, &column).contains("HEX(`name`)"));
        column.kind = CellKind::Unsupported;
        assert!(browse_projection(Engine::MySql, &column).contains("CASE WHEN"));
    }

    #[test]
    fn mysql_text_filters_preserve_column_collation() {
        let mut cols = columns();
        cols[1].sql_type = "CHAR CHARACTER SET utf8mb4".into();
        for (operator, predicate) in [
            (FilterOperator::Eq, "`name` = ?"),
            (FilterOperator::NotEq, "`name` <> ?"),
            (FilterOperator::Gt, "`name` > ?"),
            (FilterOperator::Lt, "`name` < ?"),
            (FilterOperator::Contains, "`name` LIKE ? ESCAPE '!'"),
        ] {
            let options = TableBrowseOptions {
                filters: vec![ColumnFilter {
                    column: "name".into(),
                    operator,
                    value: Some("AbpIdentity".into()),
                }],
                ..Default::default()
            };
            let (sql, parameters) = page_sql(Engine::MySql, &table(), &cols, &options).unwrap();
            let condition = sql
                .split_once(" WHERE ")
                .unwrap()
                .1
                .split(" ORDER BY ")
                .next()
                .unwrap();
            assert_eq!(condition, predicate);
            assert!(!condition.contains("CAST"));
            assert!(!condition.contains("COLLATE"));
            assert_eq!(
                parameters,
                vec![if operator == FilterOperator::Contains {
                    "%AbpIdentity%"
                } else {
                    "AbpIdentity"
                }]
            );
        }
    }

    #[test]
    fn rejects_unknown_identifiers_and_invalid_limits() {
        let mut options = TableBrowseOptions {
            page_size: 0,
            ..Default::default()
        };
        assert!(page_sql(Engine::Postgres, &table(), &columns(), &options).is_err());
        options.page_size = 1001;
        assert!(page_sql(Engine::Postgres, &table(), &columns(), &options).is_err());
        options.page_size = 10;
        options.offset = u64::MAX;
        assert!(page_sql(Engine::Postgres, &table(), &columns(), &options).is_err());
        options.offset = 0;
        options.sorts.push(ColumnSort {
            column: "name; DROP TABLE x".into(),
            descending: false,
        });
        assert!(page_sql(Engine::Postgres, &table(), &columns(), &options).is_err());
    }
    #[test]
    fn null_filters_and_no_primary_key_warning() {
        let mut cols = columns();
        cols[0].primary_key = false;
        let options = TableBrowseOptions {
            filters: vec![ColumnFilter {
                column: "name".into(),
                operator: FilterOperator::IsNull,
                value: None,
            }],
            ..Default::default()
        };
        for engine in Engine::ALL {
            let (sql, params) = page_sql(engine, &table(), &cols, &options).unwrap();
            assert!(params.is_empty());
            assert!(sql.contains("IS NULL"));
            assert!(sql.contains("not guaranteed stable"));
        }
    }
}
