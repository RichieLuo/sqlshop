use crate::{
    db::{Session, StructureChange, TableStructure, column_edit_info, structure_change_sql},
    model::{CellKind, CellUpdate, Credentials, Engine, InsertValue, Profile, Table, TableData},
    workspace::QueryTarget,
};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;

pub const COMPARE_ROW_LIMIT: u32 = 1000;

#[derive(Clone, Debug)]
pub struct CompareEndpoint {
    pub profile: Profile,
    pub target: QueryTarget,
    pub table: Table,
}
impl CompareEndpoint {
    pub fn validate(&self) -> Result<()> {
        self.profile.validate()?;
        anyhow::ensure!(
            self.profile.id == self.target.connection,
            "比较端点连接不一致"
        );
        anyhow::ensure!(
            self.target
                .database
                .as_ref()
                .is_some_and(|s| !s.is_empty() && !s.contains('\0')),
            "比较需要绑定数据库"
        );
        anyhow::ensure!(
            !self.table.name.is_empty() && !self.table.schema.is_empty(),
            "比较需要绑定 Schema 与表名"
        );
        if self.profile.engine == Engine::MySql {
            anyhow::ensure!(
                self.target.database.as_deref() == Some(self.table.schema.as_str()),
                "MySQL Schema 应与目标数据库一致"
            );
        }
        Ok(())
    }
    pub fn fingerprint(&self) -> String {
        serde_json::to_string(
            &serde_json::json!({"profile": self.profile, "target": self.target,
            "schema": self.table.schema, "table": self.table.name, "kind": self.table.kind}),
        )
        .unwrap_or_default()
    }
    pub fn label(&self) -> String {
        format!(
            "{} / {} / {}.{}",
            self.profile.name,
            self.target.database.as_deref().unwrap_or("未选择数据库"),
            self.table.schema,
            self.table.name
        )
    }
}

#[derive(Clone, Debug)]
pub struct TableSnapshot {
    pub endpoint: CompareEndpoint,
    pub data: TableData,
    pub structure: TableStructure,
    pub complete: bool,
}
#[derive(Clone, Debug)]
pub struct StructureDifference {
    pub section: String,
    pub name: String,
    pub source: Option<String>,
    pub target: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowDifferenceKind {
    Changed,
    SourceOnly,
    TargetOnly,
}
#[derive(Clone, Debug)]
pub struct DataDifference {
    pub key: String,
    pub kind: RowDifferenceKind,
    pub column: Option<String>,
    pub source: Option<String>,
    pub target: Option<String>,
}
#[derive(Clone, Debug, Default)]
struct SyncPlan {
    updates: Vec<CellUpdate>,
    insert_names: Vec<String>,
    inserts: Vec<Vec<InsertValue>>,
}
#[derive(Clone, Debug)]
pub struct CompareReport {
    pub source: TableSnapshot,
    pub target: TableSnapshot,
    pub structure_differences: Vec<StructureDifference>,
    pub data_differences: Vec<DataDifference>,
    pub blockers: Vec<String>,
    plan: SyncPlan,
    structure_candidates: Vec<StructureSyncCandidate>,
}
#[derive(Clone, Debug)]
pub struct StructureSyncCandidate {
    pub label: String,
    pub reason: Option<String>,
    change: Option<StructureChange>,
    sql: Option<String>,
}
impl StructureSyncCandidate {
    pub fn supported(&self) -> bool {
        self.change.is_some() && self.sql.is_some()
    }
    pub fn preview_sql(&self) -> Option<&str> {
        self.sql.as_deref()
    }
}

impl CompareReport {
    pub fn structure_candidates(&self) -> &[StructureSyncCandidate] {
        &self.structure_candidates
    }

    pub fn can_apply(&self) -> bool {
        self.blockers.is_empty() && (!self.plan.updates.is_empty() || !self.plan.inserts.is_empty())
    }
    pub fn update_count(&self) -> usize {
        self.plan.updates.len()
    }
    pub fn insert_count(&self) -> usize {
        self.plan.inserts.len()
    }
    pub fn sync_preview(&self) -> String {
        let updates = self.plan.updates.iter().map(|change| serde_json::json!({
            "primary_key_columns": change.primary_keys, "primary_key_values": change.key_values,
            "column": change.column, "original": change.original, "new_value": change.value,
        })).collect::<Vec<_>>();
        let inserts = self
            .plan
            .inserts
            .iter()
            .map(|row| {
                row.iter()
                    .map(|v| match v {
                        InsertValue::Null => serde_json::Value::Null,
                        InsertValue::Text(s) => serde_json::Value::String(s.clone()),
                        InsertValue::Default => serde_json::json!({"default":true}),
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        serde_json::to_string_pretty(&serde_json::json!({
            "source": self.source.endpoint.label(), "destination": self.target.endpoint.label(),
            "mode": "one transaction; update changed writable cells and insert source-only rows; preserve target-only rows",
            "updates": updates, "insert_columns": self.plan.insert_names, "insert_rows": inserts,
        })).unwrap_or_default()
    }
}

pub async fn open_endpoint(
    endpoint: &CompareEndpoint,
    credentials: &Credentials,
) -> Result<Session> {
    endpoint.validate()?;
    let mut session = Session::connect(&endpoint.profile, credentials).await?;
    session
        .select_database(
            endpoint
                .target
                .database
                .as_deref()
                .context("数据库未选择")?,
        )
        .await?;
    Ok(session)
}

pub async fn compare_tables(
    source: &CompareEndpoint,
    source_credentials: &Credentials,
    target: &CompareEndpoint,
    target_credentials: &Credentials,
) -> Result<CompareReport> {
    let mut source_session = open_endpoint(source, source_credentials).await?;
    let mut target_session = open_endpoint(target, target_credentials).await?;
    let source_snapshot = source_session.compare_snapshot(source).await?;
    let target_snapshot = target_session.compare_snapshot(target).await?;
    let mut report = build_report(source_snapshot, target_snapshot)?;
    populate_structure_candidates(&mut report, &mut source_session).await;
    Ok(report)
}

fn set_signature(set: &crate::model::ResultSet) -> String {
    // Column names and actual rows, preserving NULL vs empty. Row order for
    // catalog metadata is significant (ordinal columns/key positions).
    serde_json::to_string(&(&set.columns, &set.rows, set.truncated)).unwrap_or_default()
}
fn structure_signature(structure: &TableStructure) -> String {
    format!(
        "{}\n{}\n{}\n{}",
        set_signature(&structure.columns),
        set_signature(&structure.indexes),
        set_signature(&structure.foreign_keys),
        structure.ddl
    )
}
fn rows(snapshot: &TableSnapshot) -> &[Vec<Option<String>>] {
    snapshot
        .data
        .output
        .sets
        .first()
        .map(|s| s.rows.as_slice())
        .unwrap_or_default()
}

fn decimal_key(value: &str) -> Result<String> {
    let value = value.trim();
    let negative = value.starts_with('-');
    let value = value
        .strip_prefix('-')
        .or_else(|| value.strip_prefix('+'))
        .unwrap_or(value);
    let parts = value.split('.').collect::<Vec<_>>();
    anyhow::ensure!(
        (1..=2).contains(&parts.len())
            && parts.iter().all(|s| s.bytes().all(|b| b.is_ascii_digit()))
            && parts.iter().any(|s| !s.is_empty()),
        "无法无损解析十进制主键"
    );
    let integer = parts[0].trim_start_matches('0');
    let fraction = parts
        .get(1)
        .copied()
        .unwrap_or_default()
        .trim_end_matches('0');
    let integer = if integer.is_empty() { "0" } else { integer };
    let sign = if negative && (integer != "0" || !fraction.is_empty()) {
        "-"
    } else {
        ""
    };
    Ok(if fraction.is_empty() {
        format!("{sign}{integer}")
    } else {
        format!("{sign}{integer}.{fraction}")
    })
}
fn typed_value(kind: CellKind, value: &Option<String>) -> Result<String> {
    let Some(value) = value else {
        return Ok("null".into());
    };
    Ok(match kind {
        CellKind::Integer => format!(
            "integer:{}",
            value.parse::<i128>().context("整数值无法无损解析")?
        ),
        CellKind::Unsigned => format!(
            "unsigned:{}",
            value.parse::<u128>().context("无符号整数值无法无损解析")?
        ),
        CellKind::Decimal => format!("decimal:{}", decimal_key(value)?),
        CellKind::Binary => {
            let hex = value
                .strip_prefix("0x")
                .or_else(|| value.strip_prefix("0X"))
                .context("二进制值缺少0x前缀")?;
            anyhow::ensure!(
                hex.len() % 2 == 0 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
                "二进制值无效"
            );
            format!("binary:{}", hex.to_ascii_lowercase())
        }
        CellKind::Boolean => match value.to_ascii_lowercase().as_str() {
            "true" | "t" | "1" => "boolean:true".into(),
            "false" | "f" | "0" => "boolean:false".into(),
            _ => bail!("布尔值无法解析"),
        },
        CellKind::Date => format!(
            "date:{}",
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").context("日期值无法解析")?
        ),
        CellKind::DateTime => {
            let normalized = value.replace(' ', "T");
            if let Ok(date) = chrono::DateTime::parse_from_rfc3339(&normalized) {
                format!(
                    "datetime_offset:{}",
                    date.with_timezone(&chrono::Utc).to_rfc3339()
                )
            } else if let Ok(date) =
                chrono::DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f%#z")
            {
                format!(
                    "datetime_offset:{}",
                    date.with_timezone(&chrono::Utc).to_rfc3339()
                )
            } else {
                format!(
                    "datetime:{}",
                    chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
                        .or_else(|_| chrono::NaiveDateTime::parse_from_str(
                            value,
                            "%Y-%m-%dT%H:%M:%S%.f"
                        ))
                        .context("日期时间值无法无损解析")?
                )
            }
        }
        CellKind::Text => format!("text:{}", serde_json::to_string(value)?),
        // Floating-point/time variants are displayed and compared exactly as
        // received. They are not accepted as synchronization primary keys.
        CellKind::Float | CellKind::Time => format!("{kind:?}:{}", serde_json::to_string(value)?),
        CellKind::Unsupported => bail!("包含不支持的值类型"),
    })
}
fn keyed_rows(snapshot: &TableSnapshot) -> Result<BTreeMap<String, Vec<Option<String>>>> {
    anyhow::ensure!(
        !snapshot.data.primary_keys.is_empty(),
        "缺少可用主键，不能判定行身份"
    );
    let key_indexes = snapshot
        .data
        .primary_keys
        .iter()
        .map(|name| {
            snapshot
                .data
                .columns
                .iter()
                .position(|c| &c.name == name)
                .context("主键列元数据不完整")
        })
        .collect::<Result<Vec<_>>>()?;
    let mut keyed = BTreeMap::new();
    for row in rows(snapshot) {
        anyhow::ensure!(
            row.len() == snapshot.data.columns.len(),
            "数据行与列元数据不一致"
        );
        let mut key = Vec::new();
        for index in &key_indexes {
            let column = &snapshot.data.columns[*index];
            anyhow::ensure!(
                !matches!(
                    column.kind,
                    CellKind::Float | CellKind::Time | CellKind::Unsupported
                ),
                "主键类型不支持无损比较"
            );
            anyhow::ensure!(row[*index].is_some(), "主键含NULL，无法稳定识别行");
            key.push(typed_value(column.kind, &row[*index])?);
        }
        let key = serde_json::to_string(&key)?;
        anyhow::ensure!(
            keyed.insert(key, row.clone()).is_none(),
            "主键重复或主键规范化产生重复，不能同步"
        );
    }
    Ok(keyed)
}
fn structure_diffs(source: &TableStructure, target: &TableStructure) -> Vec<StructureDifference> {
    let mut differences = Vec::new();
    for (section, left, right) in [
        ("字段", &source.columns, &target.columns),
        ("索引", &source.indexes, &target.indexes),
        ("外键", &source.foreign_keys, &target.foreign_keys),
    ] {
        let to_map = |set: &crate::model::ResultSet| {
            let name = set.columns.iter().position(|c| c == "name").unwrap_or(0);
            let ordinal = set.columns.iter().position(|c| c == "ordinal");
            set.rows
                .iter()
                .map(|row| {
                    let key = format!(
                        "{}{}",
                        row.get(name)
                            .and_then(Option::as_deref)
                            .unwrap_or("<无名称>"),
                        ordinal
                            .map(|i| format!(
                                " / {}",
                                row.get(i).and_then(Option::as_deref).unwrap_or("?")
                            ))
                            .unwrap_or_default()
                    );
                    let fields = set.columns.iter().zip(row).collect::<BTreeMap<_, _>>();
                    (key, serde_json::to_string(&fields).unwrap_or_default())
                })
                .collect::<BTreeMap<_, _>>()
        };
        let left = to_map(left);
        let right = to_map(right);
        for name in left
            .keys()
            .chain(right.keys())
            .collect::<std::collections::BTreeSet<_>>()
        {
            if left.get(name) != right.get(name) {
                differences.push(StructureDifference {
                    section: section.into(),
                    name: name.clone(),
                    source: left.get(name).cloned(),
                    target: right.get(name).cloned(),
                });
            }
        }
    }
    differences
}

pub fn build_report(source: TableSnapshot, target: TableSnapshot) -> Result<CompareReport> {
    let mut report = CompareReport {
        structure_differences: structure_diffs(&source.structure, &target.structure),
        source,
        target,
        data_differences: Vec::new(),
        blockers: Vec::new(),
        plan: SyncPlan::default(),
        structure_candidates: Vec::new(),
    };
    if !report.source.complete || !report.target.complete {
        report.blockers.push(format!(
            "至少一侧超过 {COMPARE_ROW_LIMIT} 行或结果不完整；只显示所读取内容，禁止同步"
        ));
    }
    if !report.target.data.writable {
        report.blockers.push("目标不是可事务写入的普通表".into());
    }
    if report.source.endpoint.profile.engine != report.target.endpoint.profile.engine {
        report
            .blockers
            .push("跨引擎比较只读；未实施自动类型迁移".into());
    }
    let left_columns = &report.source.data.columns;
    let right_columns = &report.target.data.columns;
    let compatible = left_columns.len() == right_columns.len()
        && left_columns.iter().zip(right_columns).all(|(l, r)| {
            l.name == r.name
                && l.kind == r.kind
                && l.declared_type.eq_ignore_ascii_case(&r.declared_type)
        });
    if !compatible {
        report
            .blockers
            .push("字段名称、顺序或类型不兼容；数据同步不可用".into());
    }
    if report.source.data.primary_keys != report.target.data.primary_keys {
        report.blockers.push("两侧主键列不一致".into());
    }
    if !compatible || report.source.data.primary_keys != report.target.data.primary_keys {
        return Ok(report);
    }
    // Different text encodings/collations can lose information during a cast.
    for field in ["character_set", "collation"] {
        let values = |snapshot: &TableSnapshot| {
            snapshot
                .structure
                .columns
                .columns
                .iter()
                .position(|c| c == field)
                .map(|index| {
                    snapshot
                        .structure
                        .columns
                        .rows
                        .iter()
                        .map(|r| r.get(index).cloned().unwrap_or(None))
                        .collect::<Vec<_>>()
                })
        };
        if values(&report.source) != values(&report.target) {
            report
                .blockers
                .push(format!("两侧字段 {field} 不一致，禁止自动同步"));
        }
    }
    let left = match keyed_rows(&report.source) {
        Ok(rows) => rows,
        Err(e) => {
            report.blockers.push(format!("源：{e:#}"));
            return Ok(report);
        }
    };
    let right = match keyed_rows(&report.target) {
        Ok(rows) => rows,
        Err(e) => {
            report.blockers.push(format!("目标：{e:#}"));
            return Ok(report);
        }
    };
    let key_indexes = report
        .target
        .data
        .primary_keys
        .iter()
        .map(|name| {
            right_columns
                .iter()
                .position(|c| &c.name == name)
                .expect("keyed_rows verified")
        })
        .collect::<Vec<_>>();
    report.plan.insert_names = right_columns
        .iter()
        .filter(|c| c.editable)
        .map(|c| c.name.clone())
        .collect();
    for (key, source_row) in &left {
        if let Some(target_row) = right.get(key) {
            for (index, column) in right_columns.iter().enumerate() {
                let same = typed_value(column.kind, &source_row[index])
                    .ok()
                    .zip(typed_value(column.kind, &target_row[index]).ok())
                    .map(|(l, r)| l == r);
                if same == Some(true) || (same.is_none() && source_row[index] == target_row[index])
                {
                    continue;
                }
                report.data_differences.push(DataDifference {
                    key: key.clone(),
                    kind: RowDifferenceKind::Changed,
                    column: Some(column.name.clone()),
                    source: source_row[index].clone(),
                    target: target_row[index].clone(),
                });
                if column.primary_key {
                    continue;
                }
                if !column.editable {
                    report
                        .blockers
                        .push(format!("列 {} 为生成列或不可写；差异只读", column.name));
                    continue;
                }
                if column.kind == CellKind::Unsupported || same.is_none() {
                    report
                        .blockers
                        .push(format!("列 {} 不能无损解释或写入", column.name));
                    continue;
                }
                report.plan.updates.push(CellUpdate {
                    database: report
                        .target
                        .endpoint
                        .target
                        .database
                        .clone()
                        .expect("endpoint verified"),
                    table: report.target.endpoint.table.clone(),
                    primary_keys: report.target.data.primary_keys.clone(),
                    key_values: key_indexes.iter().map(|i| target_row[*i].clone()).collect(),
                    column: column.name.clone(),
                    original: target_row[index].clone(),
                    value: source_row[index].clone(),
                });
            }
        } else {
            report.data_differences.push(DataDifference {
                key: key.clone(),
                kind: RowDifferenceKind::SourceOnly,
                column: None,
                source: Some(serde_json::to_string(source_row)?),
                target: None,
            });
            if key_indexes.iter().any(|i| !right_columns[*i].editable) {
                report.blockers.push("源独有行的主键为生成值，不能保留完整主键新增；未启用 IDENTITY_INSERT 或重映射主键".into());
            }
            for (index, column) in right_columns.iter().enumerate().filter(|(_, c)| c.editable) {
                if typed_value(column.kind, &source_row[index]).is_err() {
                    report
                        .blockers
                        .push(format!("新增列 {} 无法无损解释", column.name));
                }
            }
            report.plan.inserts.push(
                right_columns
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.editable)
                    .map(|(i, _)| {
                        source_row[i]
                            .clone()
                            .map(InsertValue::Text)
                            .unwrap_or(InsertValue::Null)
                    })
                    .collect(),
            );
        }
    }
    for (key, row) in &right {
        if !left.contains_key(key) {
            report.data_differences.push(DataDifference {
                key: key.clone(),
                kind: RowDifferenceKind::TargetOnly,
                column: None,
                source: None,
                target: Some(serde_json::to_string(row)?),
            });
        }
    }
    if report.sync_preview().len() > 1024 * 1024 {
        report
            .blockers
            .push("同步操作预览超过 1 MiB，禁止自动应用；请缩小表范围".into());
    }
    report.blockers.sort();
    report.blockers.dedup();
    Ok(report)
}

pub async fn apply_sync(
    report: &CompareReport,
    target_credentials: &Credentials,
) -> Result<(usize, usize)> {
    anyhow::ensure!(report.can_apply(), "此比较结果不允许同步");
    let mut session = open_endpoint(&report.target.endpoint, target_credentials).await?;
    let current = session.compare_snapshot(&report.target.endpoint).await?;
    anyhow::ensure!(
        current.complete
            && structure_signature(&current.structure)
                == structure_signature(&report.target.structure),
        "目标结构已改变，未执行同步；请重新比较"
    );
    anyhow::ensure!(
        keyed_rows(&current)? == keyed_rows(&report.target)?,
        "目标数据已改变，未执行同步；请重新比较"
    );
    session
        .save_table_draft(
            report
                .target
                .endpoint
                .target
                .database
                .as_deref()
                .context("目标数据库未选择")?,
            &report.target.endpoint.table,
            &report.plan.updates,
            &report.plan.insert_names,
            &report.plan.inserts,
        )
        .await
        .context("同步提交失败，请核对目标服务器结果；未自动重试")
}

#[derive(Clone, Debug)]
pub struct ErRelation {
    pub name: String,
    pub local_columns: Vec<String>,
    pub referenced_schema: String,
    pub referenced_table: String,
    pub referenced_columns: Vec<String>,
}

pub fn er_relations(structure: &TableStructure) -> Result<Vec<ErRelation>> {
    let set = &structure.foreign_keys;
    anyhow::ensure!(!set.truncated, "外键目录被截断，不能生成完整关系图");
    if set.rows.is_empty() {
        return Ok(Vec::new());
    }
    let index = |name| {
        set.columns
            .iter()
            .position(|c| c == name)
            .with_context(|| format!("外键目录缺少 {name}"))
    };
    let (name, ordinal, local, schema, table, remote) = (
        index("name")?,
        index("ordinal")?,
        index("column")?,
        index("referenced_schema")?,
        index("referenced_table")?,
        index("referenced_column")?,
    );
    type ErCatalogColumn = (u32, String, String, String, String);
    let mut groups: BTreeMap<String, Vec<ErCatalogColumn>> = BTreeMap::new();
    for row in &set.rows {
        let field = |i| {
            row.get(i)
                .and_then(Option::as_deref)
                .context("外键目录字段为空")
        };
        groups.entry(field(name)?.into()).or_default().push((
            field(ordinal)?.parse()?,
            field(local)?.into(),
            field(schema)?.into(),
            field(table)?.into(),
            field(remote)?.into(),
        ));
    }
    groups
        .into_iter()
        .map(|(name, mut rows)| {
            rows.sort_by_key(|r| r.0);
            let first = rows.first().context("外键为空")?;
            anyhow::ensure!(
                rows.iter().all(|r| r.2 == first.2 && r.3 == first.3),
                "复合外键引用目录不一致"
            );
            Ok(ErRelation {
                name,
                local_columns: rows.iter().map(|r| r.1.clone()).collect(),
                referenced_schema: first.2.clone(),
                referenced_table: first.3.clone(),
                referenced_columns: rows.iter().map(|r| r.4.clone()).collect(),
            })
        })
        .collect()
}

fn catalog_field<'a>(
    set: &crate::model::ResultSet,
    row: &'a [Option<String>],
    name: &str,
) -> Option<&'a str> {
    set.columns
        .iter()
        .position(|c| c == name)
        .and_then(|i| row.get(i))
        .and_then(Option::as_deref)
}
fn no_catalog_attribute(set: &crate::model::ResultSet, row: &[Option<String>], name: &str) -> bool {
    catalog_field(set, row, name).is_none_or(str::is_empty)
}
fn simple_column_candidate(
    report: &CompareReport,
    row: &[Option<String>],
) -> Result<StructureChange> {
    let set = &report.source.structure.columns;
    let engine = report.source.endpoint.profile.engine;
    let name = catalog_field(set, row, "name").context("源字段缺少名称")?;
    let nullable = catalog_field(set, row, "nullable").context("源字段缺少可空属性")?;
    anyhow::ensure!(
        matches!(nullable, "YES" | "true" | "1"),
        "仅复制可空字段；不自动更改或回填目标已有行"
    );
    for attribute in [
        "default",
        "default_name",
        "default_or_generation",
        "extra",
        "identity",
        "generated",
        "generation_expression",
        "computed",
        "identity_seed",
        "identity_increment",
        "comment",
        "collation",
        "character_set",
    ] {
        anyhow::ensure!(
            no_catalog_attribute(set, row, attribute),
            "字段带有 {attribute} 属性，简化新增接口无法完整保留"
        );
    }
    for attribute in ["rowguid", "sparse", "hidden"] {
        anyhow::ensure!(
            catalog_field(set, row, attribute).is_none_or(|v| v == "0" || v.is_empty()),
            "字段带有 {attribute} 扩展属性"
        );
    }
    anyhow::ensure!(
        catalog_field(set, row, "generated_always")
            .is_none_or(|v| v.is_empty() || v == "NOT_APPLICABLE"),
        "字段有GENERATED ALWAYS属性"
    );
    let mut type_row = row.to_vec();
    // A source index is a separate operation. It does not prevent adding a
    // nullable field; target constraints/indexes are never implicitly copied.
    if engine == Engine::MySql
        && let Some(index) = set.columns.iter().position(|c| c == "key")
    {
        type_row[index] = Some(String::new());
    }
    let info = column_edit_info(engine, &type_row)?;
    Ok(StructureChange::AddColumn {
        name: name.into(),
        data_type: info.data_type,
        size: info.size,
    })
}
async fn simple_index_column(
    report: &CompareReport,
    source_session: &mut Session,
    name: &str,
    rows: &[&Vec<Option<String>>],
) -> Result<String> {
    let set = &report.source.structure.indexes;
    match report.source.endpoint.profile.engine {
        Engine::MySql => {
            anyhow::ensure!(rows.len()==1,"只复制普通单列索引；复合索引保持只读");
            let row=rows[0];
            anyhow::ensure!(catalog_field(set,row,"non_unique")==Some("1"),"主键和唯一索引不自动复制");
            anyhow::ensure!(catalog_field(set,row,"type")==Some("BTREE"),"仅支持普通BTREE索引");
            anyhow::ensure!(catalog_field(set,row,"direction").is_none_or(|v|v.is_empty()||v=="A"),"降序索引不自动复制");
            anyhow::ensure!(no_catalog_attribute(set,row,"prefix_length")&&no_catalog_attribute(set,row,"expression")&&no_catalog_attribute(set,row,"comment"),"带前缀、表达式或注释的索引不自动复制");
            anyhow::ensure!(catalog_field(set,row,"visible").is_none_or(|v|v.is_empty()||v=="YES"),"隐藏索引不自动复制");
            Ok(catalog_field(set,row,"column").context("索引列为空")?.into())
        },
        Engine::Postgres => {
            source_session.comparison_plain_index_column(&report.source.endpoint.table,name).await?
                .context("不是有效的普通单列默认BTREE索引，或包含唯一性、表达式、过滤、排序、INCLUDE、存储或扩展操作符属性")
        },
        Engine::SqlServer => {
            anyhow::ensure!(rows.len()==1,"只复制普通单列索引；复合或INCLUDE索引保持只读");
            let row=rows[0];
            anyhow::ensure!(catalog_field(set,row,"type")==Some("NONCLUSTERED"),"聚簇或特殊类型索引不自动复制");
            for attribute in ["unique","primary","unique_constraint","descending","included","disabled","ignore_dup_key"] {
                anyhow::ensure!(catalog_field(set,row,attribute)==Some("0"),"索引带有 {attribute} 属性，不自动复制");
            }
            anyhow::ensure!(catalog_field(set,row,"key_ordinal")==Some("1")&&no_catalog_attribute(set,row,"filter"),"索引包含过滤或非普通键属性");
            anyhow::ensure!(catalog_field(set,row,"fill_factor")==Some("0")&&catalog_field(set,row,"data_space")==Some("PRIMARY")&&catalog_field(set,row,"allow_row_locks")==Some("1")&&catalog_field(set,row,"allow_page_locks")==Some("1"),"自定义物理索引选项不自动复制");
            Ok(catalog_field(set,row,"column").context("索引列为空")?.into())
        },
    }
}
async fn populate_structure_candidates(report: &mut CompareReport, source_session: &mut Session) {
    if report.source.endpoint.profile.engine != report.target.endpoint.profile.engine
        || !report.target.data.writable
    {
        report.structure_candidates.push(StructureSyncCandidate {
            label: "结构同步".into(),
            reason: Some("仅对同引擎、可事务写入普通表生成可视化结构操作".into()),
            change: None,
            sql: None,
        });
        return;
    }
    let mut candidates = Vec::new();
    let target_columns = &report.target.structure.columns;
    for row in &report.source.structure.columns.rows {
        let Some(name) = catalog_field(&report.source.structure.columns, row, "name") else {
            continue;
        };
        if target_columns
            .rows
            .iter()
            .any(|r| catalog_field(target_columns, r, "name") == Some(name))
        {
            continue;
        }
        let result = simple_column_candidate(report, row).and_then(|change| {
            let sql = structure_change_sql(
                report.target.endpoint.profile.engine,
                &report.target.endpoint.table,
                &change,
            )?;
            Ok((change, sql))
        });
        let (change, sql, reason) = match result {
            Ok((change, sql)) => (Some(change), Some(sql), None),
            Err(error) => (None, None, Some(format!("{error:#}"))),
        };
        candidates.push(StructureSyncCandidate {
            label: format!("新增字段 {name}"),
            reason,
            change,
            sql,
        });
    }
    let indexes = &report.source.structure.indexes;
    let mut groups: BTreeMap<String, Vec<&Vec<Option<String>>>> = BTreeMap::new();
    for row in &indexes.rows {
        if let Some(name) = catalog_field(indexes, row, "name") {
            groups.entry(name.into()).or_default().push(row);
        }
    }
    for (name, rows) in groups {
        if report.target.structure.indexes.rows.iter().any(|r| {
            catalog_field(&report.target.structure.indexes, r, "name") == Some(name.as_str())
        }) {
            continue;
        }
        let result = async {
            let column = simple_index_column(report, source_session, &name, &rows).await?;
            anyhow::ensure!(
                target_columns
                    .rows
                    .iter()
                    .any(|r| catalog_field(target_columns, r, "name") == Some(column.as_str())),
                "目标尚无字段 {column}；先新增字段并重新比较"
            );
            let change = StructureChange::CreateIndex {
                name: name.clone(),
                columns: vec![column],
            };
            let sql = structure_change_sql(
                report.target.endpoint.profile.engine,
                &report.target.endpoint.table,
                &change,
            )?;
            Ok::<_, anyhow::Error>((change, sql))
        }
        .await;
        let (change, sql, reason) = match result {
            Ok((change, sql)) => (Some(change), Some(sql), None),
            Err(error) => (None, None, Some(format!("{error:#}"))),
        };
        candidates.push(StructureSyncCandidate {
            label: format!("新增索引 {name}"),
            reason,
            change,
            sql,
        });
    }
    if report
        .structure_differences
        .iter()
        .any(|d| d.section == "外键")
    {
        candidates.push(StructureSyncCandidate {
            label: "外键差异".into(),
            reason: Some("外键、约束及依赖需手动处理，当前结构同步不自动添加或删除".into()),
            change: None,
            sql: None,
        });
    }
    if report
        .structure_differences
        .iter()
        .any(|d| d.source.is_none() || d.target.is_some())
    {
        candidates.push(StructureSyncCandidate {
            label: "已有定义差异与目标独有对象".into(),
            reason: Some(
                "不自动删除字段/索引，也不自动修改已有类型、约束或定义；只提供新增候选".into(),
            ),
            change: None,
            sql: None,
        });
    }
    report.structure_candidates = candidates;
}

pub async fn apply_structure_sync(
    report: &CompareReport,
    candidate: usize,
    source_credentials: &Credentials,
    target_credentials: &Credentials,
) -> Result<String> {
    let operation = report
        .structure_candidates
        .get(candidate)
        .context("结构操作已不存在，请重新比较")?;
    let change = operation
        .change
        .as_ref()
        .context("此结构差异不支持自动应用")?;
    let sql = operation.sql.as_deref().context("结构SQL预览不存在")?;
    // A candidate is scoped to one immutable typed change, never arbitrary SQL.
    let mut source_session = open_endpoint(&report.source.endpoint, source_credentials).await?;
    let mut target_session = open_endpoint(&report.target.endpoint, target_credentials).await?;
    let current_source = source_session
        .table_structure(&report.source.endpoint.table)
        .await?;
    let current_target = target_session
        .table_structure(&report.target.endpoint.table)
        .await?;
    anyhow::ensure!(
        structure_signature(&current_source) == structure_signature(&report.source.structure),
        "来源结构已变化，未执行；请重新比较"
    );
    anyhow::ensure!(
        structure_signature(&current_target) == structure_signature(&report.target.structure),
        "目标结构已变化，未执行；请重新比较"
    );
    target_session
        .change_structure(
            report
                .target
                .endpoint
                .target
                .database
                .as_deref()
                .context("目标数据库未选择")?,
            &report.target.endpoint.table,
            change,
            sql,
        )
        .await
        .context("结构操作失败；请核对目标定义，未自动重试")?;
    Ok(format!("已执行 {}；请重新比较刷新定义。", operation.label))
}
