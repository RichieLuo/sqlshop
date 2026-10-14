use super::{
    Session,
    table_edit::{display_expression, parameter_expression},
};
use crate::{
    model::{CellKind, CellUpdate, Engine, InsertValue, Table, TableColumn},
    recovery::{
        DraftVerification, InsertVerification, RowVerification, ServerRowEvidence,
        VerificationState,
    },
};
use anyhow::{Context, Result, ensure};

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
        let engine = self.profile.engine;
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
                self.lookup_draft_row(engine, table, &columns, &keys, &row.original_key, &group)
                    .await?,
            );
            if row.intended_key != row.original_key && row.intended_key.iter().all(Option::is_some)
            {
                row.evidence.push(
                    self.lookup_draft_row(
                        engine,
                        table,
                        &columns,
                        &keys,
                        &row.intended_key,
                        &group,
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
        engine: Engine,
        table: &Table,
        columns: &[TableColumn],
        keys: &[&TableColumn],
        key_values: &[Option<String>],
        changes: &[&CellUpdate],
    ) -> Result<ServerRowEvidence> {
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
                    comparison(engine, column, value.as_deref(), intended, &mut parameters)
                })
                .collect::<Vec<_>>()
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
) -> String {
    let name = engine.quote(&column.name);
    let Some(value) = value else {
        return format!("{name} IS NULL");
    };
    parameters.push(value.to_owned());
    let position = parameters.len();
    let converted = parameter_expression(engine, column, position);
    match engine {
        Engine::Postgres => format!("{name} IS NOT DISTINCT FROM {converted}"),
        Engine::SqlServer => {
            format!("CONVERT(varbinary(max), {name}) = CONVERT(varbinary(max), {converted})")
        }
        Engine::MySql if !intended || column.kind == CellKind::Text => {
            format!("BINARY {} = BINARY ?", display_expression(engine, column))
        }
        Engine::MySql if column.kind == CellKind::Binary => {
            format!("BINARY {name} = BINARY {converted}")
        }
        Engine::MySql => format!(
            "BINARY {} = BINARY CAST({converted} AS CHAR CHARACTER SET utf8mb4)",
            display_expression(engine, column)
        ),
    }
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
