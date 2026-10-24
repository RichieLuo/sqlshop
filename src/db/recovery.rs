use super::{
    Session,
    table_edit::{
        SqlServerVarcharColumn, display_expression, original_value_predicate, parameter_expression,
        sqlserver_text_type, sqlserver_varchar_expression,
    },
};
use crate::{
    model::{CellKind, CellUpdate, Engine, InsertValue, Table, TableColumn},
    recovery::{
        DraftVerification, InsertVerification, RowVerification, ServerRowEvidence,
        VerificationState,
    },
};
use anyhow::{Context, Result, ensure};
use std::collections::BTreeMap;

impl Session {
    /// Use a newly connected management session. Do not run this on the failed
    /// connection: it can still contain the transaction whose outcome is unknown.
    /// No retry, write, transaction control, or draft rebasing is performed here.
    pub async fn verify_table_draft(
        &mut self,
        database: &str,
        table: &Table,
        changes: &[CellUpdate],
        insert_columns: &[String],
        inserts: &[Vec<InsertValue>],
    ) -> Result<DraftVerification> {
        self.verify_table_draft_with_columns(database, table, changes, insert_columns, inserts, &[])
            .await
    }

    /// Supply the immutable column snapshot belonging to the draft. Schema
    /// changes cannot reinterpret the old values as a new type or row identity.
    pub async fn verify_table_draft_with_columns(
        &mut self,
        database: &str,
        table: &Table,
        changes: &[CellUpdate],
        insert_columns: &[String],
        inserts: &[Vec<InsertValue>],
        expected_columns: &[TableColumn],
    ) -> Result<DraftVerification> {
        ensure!(
            !self.raw_query_used,
            "请使用新连接核实提交结果，不能沿用执行过 SQL 的会话"
        );
        ensure!(
            changes.len() <= 10_000 && inserts.len() <= 1_000,
            "核实草稿超过单次上限"
        );
        ensure!(
            self.current_database().await?.as_deref() == Some(database),
            "核实连接的数据库与草稿不一致"
        );
        ensure!(
            changes
                .iter()
                .all(|change| change.database == database && change.table == *table),
            "草稿目标不一致，未执行核实"
        );
        ensure!(
            inserts.iter().all(|row| row.len() == insert_columns.len()),
            "新增草稿的列映射不完整"
        );
        let columns = self.table_columns(table).await?;
        let varchar_columns = self.sqlserver_varchar_columns(table).await?;
        let schema_matches = !expected_columns.is_empty()
            && columns.len() == expected_columns.len()
            && columns
                .iter()
                .zip(expected_columns)
                .all(|(actual, expected)| {
                    actual.name == expected.name
                        && actual.declared_type == expected.declared_type
                        && actual.sql_type == expected.sql_type
                        && actual.kind == expected.kind
                        && actual.primary_key == expected.primary_key
                        && actual.editable == expected.editable
                });
        let mut groups: Vec<Vec<&CellUpdate>> = Vec::new();
        for change in changes {
            if let Some(group) = groups.iter_mut().find(|group| {
                group[0].key_values == change.key_values
                    && group[0].primary_keys == change.primary_keys
            }) {
                group.push(change);
            } else {
                groups.push(vec![change]);
            }
        }
        ensure!(groups.len() <= 1_000, "核实记录超过单次上限");
        let mut rows = Vec::with_capacity(groups.len());
        for group in groups {
            let first = group[0];
            let mut row = RowVerification {
                primary_keys: first.primary_keys.clone(),
                original_key: first.key_values.clone(),
                intended_key: first.key_values.clone(),
                changes: group.iter().map(|change| (*change).clone()).collect(),
                state: VerificationState::Unknown,
                reason: String::new(),
                evidence: Vec::new(),
            };
            for (index, key) in row.primary_keys.iter().enumerate() {
                if let Some(change) = group.iter().find(|change| change.column == *key)
                    && let Some(value) = row.intended_key.get_mut(index)
                {
                    *value = change.value.clone();
                }
            }
            let keys = columns
                .iter()
                .filter(|column| column.primary_key)
                .collect::<Vec<_>>();
            if !schema_matches
                || keys.is_empty()
                || keys
                    .iter()
                    .any(|column| column.kind == CellKind::Unsupported)
                || keys
                    .iter()
                    .map(|column| &column.name)
                    .ne(row.primary_keys.iter())
                || keys.len() != row.original_key.len()
                || row.original_key.iter().any(Option::is_none)
                || group.iter().enumerate().any(|(index, change)| {
                    change.key_values != first.key_values
                        || change.primary_keys != first.primary_keys
                        || !columns.iter().any(|column| {
                            column.name == change.column && column.kind != CellKind::Unsupported
                        })
                        || group[..index]
                            .iter()
                            .any(|previous| previous.column == change.column)
                })
            {
                row.reason = "缺少原表结构快照，或主键、列类型、可写属性与草稿快照不一致，不能重新解释旧草稿；草稿已保留".into();
                rows.push(row);
                continue;
            }
            row.evidence.push(
                self.lookup_draft_row(
                    table,
                    &columns,
                    &keys,
                    &row.original_key,
                    &group,
                    &varchar_columns,
                )
                .await?,
            );
            if row.intended_key != row.original_key && row.intended_key.iter().all(Option::is_some)
            {
                row.evidence.push(
                    self.lookup_draft_row(
                        table,
                        &columns,
                        &keys,
                        &row.intended_key,
                        &group,
                        &varchar_columns,
                    )
                    .await?,
                );
            }
            classify(&mut row);
            rows.push(row);
        }
        Ok(DraftVerification {
            database: database.into(),
            table: table.clone(),
            checked_at: chrono::Utc::now().to_rfc3339(),
            rows,
            insert_columns: insert_columns.to_vec(),
            inserts: inserts.iter().enumerate().map(|(draft_index, values)| InsertVerification {
                draft_index,
                values: values.clone(),
                state: VerificationState::Unknown,
                reason: "未收到已提交记录的可靠标识或回执，不能根据相似行或查无记录推断新增是否提交；请人工核对，暂勿重复提交".into(),
            }).collect(),
            note: "这是新连接按主键读取的当前值证据，不是 COMMIT 回执；其他会话、触发器及两次读取之间的变化仍可能影响判断。核实不修改、清除或自动重试本地草稿。".into(),
        })
    }

    async fn lookup_draft_row(
        &mut self,
        table: &Table,
        columns: &[TableColumn],
        keys: &[&TableColumn],
        key_values: &[Option<String>],
        changes: &[&CellUpdate],
        varchar_columns: &BTreeMap<String, SqlServerVarcharColumn>,
    ) -> Result<ServerRowEvidence> {
        let engine = self.profile.engine;
        let mut parameters = Vec::new();
        let predicates = keys
            .iter()
            .zip(key_values)
            .map(|(column, value)| {
                parameters.push(value.as_deref().expect("validated primary key").to_owned());
                format!(
                    "{} = {}",
                    engine.quote(&column.name),
                    parameter_expression(engine, column, parameters.len())
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        // SELECT placeholders precede WHERE placeholders for MySQL, so construct
        // comparison parameters first and key parameters last.
        let key_parameters = std::mem::take(&mut parameters);
        let mut fields = columns
            .iter()
            .map(|column| display_expression(engine, column))
            .collect::<Vec<_>>();
        for intended in [false, true] {
            let checks = changes
                .iter()
                .map(|change| {
                    let column = columns
                        .iter()
                        .find(|column| column.name == change.column)
                        .expect("validated column");
                    let value = if intended {
                        &change.value
                    } else {
                        &change.original
                    };
                    comparison(
                        engine,
                        column,
                        value.as_deref(),
                        intended,
                        &mut parameters,
                        varchar_columns.get(&column.name),
                    )
                })
                .collect::<Result<Vec<_>>>()?
                .join(" AND ");
            fields.push(format!("CASE WHEN {checks} THEN '1' ELSE '0' END"));
        }
        // Rebuild key placeholders after comparison parameters for numbered engines.
        let predicates = if engine == Engine::MySql {
            predicates
        } else {
            keys.iter()
                .enumerate()
                .map(|(index, column)| {
                    format!(
                        "{} = {}",
                        engine.quote(&column.name),
                        parameter_expression(engine, column, parameters.len() + index + 1)
                    )
                })
                .collect::<Vec<_>>()
                .join(" AND ")
        };
        parameters.extend(key_parameters);
        let relation = format!(
            "{}.{}",
            engine.quote(&table.schema),
            engine.quote(&table.name)
        );
        let sql = match engine {
            Engine::SqlServer => format!(
                "SELECT TOP (2) {} FROM {relation} WHERE {predicates}",
                fields.join(", ")
            ),
            _ => format!(
                "SELECT {} FROM {relation} WHERE {predicates} LIMIT 2",
                fields.join(", ")
            ),
        };
        let refs = parameters.iter().map(String::as_str).collect::<Vec<_>>();
        let output = self.parameter_query(&sql, &refs).await?;
        let set = output.sets.into_iter().next().context("核实未返回结果集")?;
        ensure!(
            set.rows.iter().all(|row| row.len() == columns.len() + 2),
            "核实返回了不完整的记录"
        );
        let matches_original =
            set.rows.len() == 1 && set.rows[0][columns.len()].as_deref() == Some("1");
        let matches_intended =
            set.rows.len() == 1 && set.rows[0][columns.len() + 1].as_deref() == Some("1");
        Ok(ServerRowEvidence {
            lookup_key: key_values.to_vec(),
            columns: columns.iter().map(|column| column.name.clone()).collect(),
            rows: set
                .rows
                .into_iter()
                .map(|mut row| {
                    row.truncate(columns.len());
                    row
                })
                .collect(),
            matches_original,
            matches_intended,
        })
    }
}

fn comparison(
    engine: Engine,
    column: &TableColumn,
    value: Option<&str>,
    intended: bool,
    parameters: &mut Vec<String>,
    varchar_column: Option<&SqlServerVarcharColumn>,
) -> Result<String> {
    let name = engine.quote(&column.name);
    let Some(value) = value else {
        return Ok(format!("{name} IS NULL"));
    };
    parameters.push(value.to_owned());
    let position = parameters.len();
    if !intended {
        return Ok(original_value_predicate(engine, column, position));
    }
    let converted = parameter_expression(engine, column, position);
    Ok(match engine {
        Engine::Postgres => format!(
            "({} COLLATE \"C\") IS NOT DISTINCT FROM (CAST({converted} AS text) COLLATE \"C\")",
            display_expression(engine, column)
        ),
        Engine::SqlServer if column.kind == CellKind::Binary => {
            let bound = column.sql_type.split_once('(').and_then(|(_, width)| {
                width
                    .strip_suffix(')')
                    .and_then(|width| width.parse::<usize>().ok())
            });
            let guard = bound
                .map(|width| format!("DATALENGTH({converted}) <= {width} AND "))
                .unwrap_or_default();
            format!(
                "({guard}CONVERT(varbinary(max), {name}) = CONVERT(varbinary(max), CONVERT({}, {converted})))",
                column.sql_type
            )
        }
        Engine::SqlServer if sqlserver_text_type(column) => {
            // Canonicalize fixed-width padding only after proving a narrowing
            // cast would retain every byte. An overlength draft must not be
            // reported as matching a truncated value already on the server.
            let (base, width) = column
                .sql_type
                .split_once('(')
                .unwrap_or((&column.sql_type, "max)"));
            let bound = width
                .strip_suffix(')')
                .and_then(|width| width.parse::<usize>().ok());
            let complete = if base.starts_with('n') {
                format!("CAST({converted} AS nvarchar(max))")
            } else {
                let actual =
                    varchar_column.context("无法读取目标列字符集，不能核实可能有损的文本转换")?;
                sqlserver_varchar_expression(&converted, &actual.collation)
            };
            let length_guard = bound
                .map(|width| {
                    format!(
                        "DATALENGTH({complete}) <= {} AND ",
                        if base.starts_with('n') {
                            width * 2
                        } else {
                            width
                        }
                    )
                })
                .unwrap_or_default();
            let roundtrip_guard = if base.starts_with('n') {
                String::new()
            } else {
                format!(
                    "CONVERT(varbinary(max),CONVERT(nvarchar(max),{complete})) = CONVERT(varbinary(max),{converted}) AND "
                )
            };
            format!(
                "({length_guard}{roundtrip_guard}CONVERT(varbinary(max), {name}) = CONVERT(varbinary(max), CAST({complete} AS {})))",
                column.sql_type
            )
        }
        Engine::SqlServer => {
            format!("CONVERT(varbinary(max), {name}) = CONVERT(varbinary(max), {converted})")
        }
        Engine::MySql if column.kind == CellKind::Text => {
            format!("BINARY {} = BINARY ?", display_expression(engine, column))
        }
        Engine::MySql if column.kind == CellKind::Binary => {
            format!("BINARY {name} = BINARY {converted}")
        }
        Engine::MySql => format!(
            "BINARY {} = BINARY CAST({converted} AS CHAR CHARACTER SET utf8mb4)",
            display_expression(engine, column)
        ),
    })
}

fn classify(row: &mut RowVerification) {
    let found = row
        .evidence
        .iter()
        .filter(|evidence| !evidence.rows.is_empty())
        .collect::<Vec<_>>();
    let (state, reason) = if found.is_empty() {
        (
            VerificationState::Missing,
            "原主键与目标主键均未找到；可能已删除、再次改键或未提交，不能确定历史结果",
        )
    } else if found.len() > 1 || found.iter().any(|evidence| evidence.rows.len() != 1) {
        (
            VerificationState::Conflict,
            "原键与目标键同时存在或定位到多行，无法证明记录身份",
        )
    } else {
        let evidence = found[0];
        match (evidence.matches_original, evidence.matches_intended) {
            (true, true) => (
                VerificationState::Unknown,
                "原值与暂存值经服务器类型转换后无法区分",
            ),
            (true, false) if evidence.lookup_key == row.original_key => (
                VerificationState::Original,
                "原主键记录的待改字段仍符合原值；此结果不构成未提交的历史证明",
            ),
            (false, true) if evidence.lookup_key == row.intended_key => (
                VerificationState::Intended,
                "目标主键记录的待改字段符合暂存值；此结果不构成事务提交回执",
            ),
            _ => (
                VerificationState::Conflict,
                "当前记录的身份或字段值与原值、暂存目标不一致",
            ),
        }
    };
    row.state = state;
    row.reason = reason.into();
}
