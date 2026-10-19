//! Typed table-designer operations. Expressions never become extra statements.
use super::{Session, TableStructure, validate_identifier};
use crate::model::{Engine, Table};
use anyhow::{Result, ensure};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesignSnapshot {
    pub columns: Vec<Vec<Option<String>>>,
    pub indexes: Vec<Vec<Option<String>>>,
    pub foreign_keys: Vec<Vec<Option<String>>>,
    pub constraints: Vec<Vec<Option<String>>>,
    pub ddl: String,
}
impl From<&TableStructure> for DesignSnapshot {
    fn from(s: &TableStructure) -> Self {
        Self {
            columns: s.columns.rows.clone(),
            indexes: s.indexes.rows.clone(),
            foreign_keys: s.foreign_keys.rows.clone(),
            constraints: s.constraints.rows.clone(),
            ddl: s.ddl.clone(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstraintKind {
    PrimaryKey,
    Unique,
    Check,
    ForeignKey,
}
impl ConstraintKind {
    pub fn catalog_name(self) -> &'static str {
        match self {
            Self::PrimaryKey => "PRIMARY KEY",
            Self::Unique => "UNIQUE",
            Self::Check => "CHECK",
            Self::ForeignKey => "FOREIGN KEY",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReferenceAction {
    #[default]
    NoAction,
    Restrict,
    Cascade,
    SetNull,
    SetDefault,
}
impl ReferenceAction {
    pub fn sql(self) -> &'static str {
        match self {
            Self::NoAction => "NO ACTION",
            Self::Restrict => "RESTRICT",
            Self::Cascade => "CASCADE",
            Self::SetNull => "SET NULL",
            Self::SetDefault => "SET DEFAULT",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstraintDesign {
    pub name: String,
    pub kind: ConstraintKind,
    pub columns: Vec<String>,
    pub expression: String,
    pub referenced_schema: String,
    pub referenced_table: String,
    pub referenced_columns: Vec<String>,
    pub on_delete: ReferenceAction,
    pub on_update: ReferenceAction,
    pub deferrable: bool,
    pub initially_deferred: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DesignerOperation {
    RenameColumn {
        old: String,
        new: String,
    },
    SetDefault {
        column: String,
        expression: Option<String>,
    },
    /// Engine conversion rules apply; no implicit USING/rebuild is invented.
    AlterType {
        column: String,
        data_type: String,
        using: Option<String>,
    },
    AddGenerated {
        column: String,
        data_type: String,
        expression: String,
        stored: bool,
    },
    AddIdentity {
        column: String,
        always: bool,
    },
    PutConstraint {
        previous: Option<(String, ConstraintKind)>,
        definition: ConstraintDesign,
    },
    DropConstraint {
        name: String,
        kind: ConstraintKind,
    },
}

/// Validate a single SQL expression/type fragment. Quotes are preserved, all
/// comments and statement separators are refused, and parentheses must balance.
/// This is a boundary lexer, not a claim that an expression is side-effect free.
pub fn validate_design_expression(text: &str, engine: Engine) -> Result<()> {
    ensure!(
        !text.trim().is_empty() && text.len() <= 65536,
        "表达式为空或超过 64 KiB"
    );
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut depth = 0usize;
    while i < chars.len() {
        let c = chars[i];
        ensure!(
            c != '\0' && (c == '\n' || c == '\r' || c == '\t' || !c.is_control()),
            "表达式包含控制字符"
        );
        ensure!(
            c != ';'
                && c != '$'
                && !(c == '#' && engine == Engine::MySql)
                && !(c == '-' && chars.get(i + 1) == Some(&'-'))
                && !(c == '/' && chars.get(i + 1) == Some(&'*')),
            "表达式不能含语句分隔符、注释或 dollar quote"
        );
        if c == '\''
            || c == '"'
            || (c == '`' && engine == Engine::MySql)
            || (c == '[' && engine == Engine::SqlServer)
        {
            let end = if c == '[' { ']' } else { c };
            i += 1;
            let mut closed = false;
            while i < chars.len() {
                ensure!(chars[i] != '\0', "表达式包含 NUL");
                ensure!(chars[i] != '\\', "表达式请使用双写引号，不使用反斜杠转义");
                if chars[i] == end {
                    if chars.get(i + 1) == Some(&end) {
                        i += 2;
                        continue;
                    }
                    closed = true;
                    i += 1;
                    break;
                }
                i += 1;
            }
            ensure!(closed, "表达式引号未闭合");
            continue;
        }
        if c == '(' {
            depth += 1;
        } else if c == ')' {
            ensure!(depth > 0, "表达式括号不匹配");
            depth -= 1;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            i += 1;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            ensure!(
                ![
                    "SELECT", "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "GRANT",
                    "REVOKE", "EXEC", "EXECUTE", "UNION", "INTO", "FROM", "GO"
                ]
                .contains(&word.to_ascii_uppercase().as_str()),
                "表达式不能包含 SQL 语句或子查询"
            );
            continue;
        }
        i += 1;
    }
    ensure!(depth == 0, "表达式括号未闭合");
    Ok(())
}
fn quote_literal(s: &str, engine: Engine) -> String {
    format!(
        "{}'{}'",
        if engine == Engine::SqlServer { "N" } else { "" },
        s.replace('\'', "''")
    )
}
fn columns_sql(columns: &[String], engine: Engine) -> Result<String> {
    ensure!(
        !columns.is_empty() && columns.len() <= 32,
        "请选择 1 至 32 个字段"
    );
    for (i, c) in columns.iter().enumerate() {
        validate_identifier(c, "字段")?;
        ensure!(!columns[..i].contains(c), "字段不能重复");
    }
    Ok(columns
        .iter()
        .map(|c| engine.quote(c))
        .collect::<Vec<_>>()
        .join(", "))
}
fn designer_type(text: &str, engine: Engine) -> Result<String> {
    validate_design_expression(text, engine)?;
    // Types are declarations, not expressions: allow only a whitelisted base
    // and numeric precision/scale or MAX. No SQL suffixes can cross this bound.
    let mut upper = text.trim().to_ascii_uppercase();
    if engine == Engine::Postgres {
        let base = upper.split('(').next().unwrap_or("").trim();
        let canonical = match base {
            "INT" | "INT4" => "INTEGER",
            "INT2" => "SMALLINT",
            "INT8" => "BIGINT",
            "DECIMAL" => "NUMERIC",
            "BOOL" => "BOOLEAN",
            _ => base,
        };
        if canonical != base {
            upper = format!(
                "{canonical}{}",
                upper.find('(').map(|i| &upper[i..]).unwrap_or("")
            );
        }
    }
    let base = upper.split('(').next().unwrap_or("").trim();
    ensure!(
        super::column_type_options(engine).contains(&base),
        "请选择当前引擎支持的字段类型"
    );
    if let Some((_, suffix)) = upper.split_once('(') {
        let args = suffix
            .strip_suffix(')')
            .ok_or_else(|| anyhow::anyhow!("类型参数不完整"))?;
        if args != "MAX"
            || engine != Engine::SqlServer
            || !["VARCHAR", "NVARCHAR", "VARBINARY"].contains(&base)
        {
            let nums: Vec<_> = args.split(',').collect();
            ensure!(!nums.is_empty() && nums.len() <= 2, "类型参数过多");
            for n in nums {
                ensure!(n.trim().parse::<u32>().is_ok(), "类型参数必须是非负整数");
            }
        }
    } else {
        ensure!(upper == base, "字段类型无效");
    }
    Ok(upper)
}
fn drop_clause(engine: Engine, name: &str, kind: ConstraintKind) -> Result<String> {
    validate_identifier(name, "约束名称")?;
    Ok(if engine == Engine::MySql {
        match kind {
            ConstraintKind::PrimaryKey => "DROP PRIMARY KEY".into(),
            ConstraintKind::Unique => format!("DROP INDEX {}", engine.quote(name)),
            ConstraintKind::ForeignKey => format!("DROP FOREIGN KEY {}", engine.quote(name)),
            ConstraintKind::Check => format!("DROP CHECK {}", engine.quote(name)),
        }
    } else {
        format!("DROP CONSTRAINT {}", engine.quote(name))
    })
}
fn constraint_clause(engine: Engine, d: &ConstraintDesign) -> Result<String> {
    validate_identifier(&d.name, "约束名称")?;
    let clause = match d.kind {
        ConstraintKind::PrimaryKey => format!("PRIMARY KEY ({})", columns_sql(&d.columns, engine)?),
        ConstraintKind::Unique => format!("UNIQUE ({})", columns_sql(&d.columns, engine)?),
        ConstraintKind::Check => {
            validate_design_expression(&d.expression, engine)?;
            format!("CHECK ({})", d.expression)
        }
        ConstraintKind::ForeignKey => {
            validate_identifier(&d.referenced_schema, "引用 Schema")?;
            validate_identifier(&d.referenced_table, "引用表")?;
            ensure!(
                d.columns.len() == d.referenced_columns.len(),
                "外键两侧字段数量不一致"
            );
            ensure!(
                engine != Engine::MySql
                    || ![d.on_delete, d.on_update].contains(&ReferenceAction::SetDefault),
                "MySQL 不支持 SET DEFAULT 外键动作"
            );
            ensure!(
                engine != Engine::SqlServer
                    || ![d.on_delete, d.on_update].contains(&ReferenceAction::Restrict),
                "SQL Server 请使用 NO ACTION 替代 RESTRICT"
            );
            format!(
                "FOREIGN KEY ({}) REFERENCES {}.{} ({}) ON DELETE {} ON UPDATE {}",
                columns_sql(&d.columns, engine)?,
                engine.quote(&d.referenced_schema),
                engine.quote(&d.referenced_table),
                columns_sql(&d.referenced_columns, engine)?,
                d.on_delete.sql(),
                d.on_update.sql()
            )
        }
    };
    ensure!(
        !d.initially_deferred || d.deferrable,
        "初始延迟需要启用可延迟"
    );
    ensure!(
        !d.deferrable || (engine == Engine::Postgres && d.kind != ConstraintKind::Check),
        "此引擎/约束类型不支持可延迟选项"
    );
    let tail = if d.deferrable {
        if d.initially_deferred {
            " DEFERRABLE INITIALLY DEFERRED"
        } else {
            " DEFERRABLE INITIALLY IMMEDIATE"
        }
    } else {
        ""
    };
    Ok(format!(
        "ADD CONSTRAINT {} {clause}{tail}",
        engine.quote(&d.name)
    ))
}
fn mysql_modified_definition(row: &[Option<String>], ty: &str) -> Result<String> {
    ensure!(row.len() >= 12, "字段元数据不完整");
    let value = |i| row.get(i).and_then(Option::as_deref).unwrap_or("");
    ensure!(value(8).is_empty(), "生成列的类型不能通过普通字段转换修改");
    let extra = value(5).to_ascii_lowercase();
    let (before, on_update) = extra
        .split_once("on update ")
        .map_or((extra.as_str(), None), |(a, b)| (a, Some(b.trim())));
    ensure!(
        before
            .replace("auto_increment", "")
            .replace("default_generated", "")
            .trim()
            .is_empty(),
        "存在尚不能完整保留的字段扩展属性"
    );
    let mut definition = ty.to_owned();
    let base = ty.split('(').next().unwrap_or(ty).trim();
    if [
        "CHAR",
        "VARCHAR",
        "TINYTEXT",
        "TEXT",
        "MEDIUMTEXT",
        "LONGTEXT",
        "ENUM",
        "SET",
    ]
    .contains(&base)
    {
        for (keyword, index) in [("CHARACTER SET", 6), ("COLLATE", 7)] {
            if !value(index).is_empty() {
                validate_identifier(value(index), keyword)?;
                definition.push_str(&format!(" {keyword} {}", Engine::MySql.quote(value(index))));
            }
        }
    }
    definition.push_str(if value(3) == "YES" {
        " NULL"
    } else {
        " NOT NULL"
    });
    if let Some(default) = row.get(4).and_then(Option::as_deref) {
        if extra.contains("default_generated") {
            validate_design_expression(default, Engine::MySql)?;
            definition.push_str(&format!(" DEFAULT ({default})"));
        } else {
            ensure!(
                !default.contains('\\'),
                "默认值含反斜杠，不能在未知 sql_mode 下安全重写"
            );
            definition.push_str(&format!(
                " DEFAULT {}",
                quote_literal(default, Engine::MySql)
            ));
        }
    }
    if let Some(expression) = on_update {
        validate_design_expression(expression, Engine::MySql)?;
        definition.push_str(&format!(" ON UPDATE {expression}"));
    }
    if extra.contains("auto_increment") {
        definition.push_str(" AUTO_INCREMENT");
    }
    if !value(9).is_empty() {
        ensure!(
            !value(9).contains('\\'),
            "字段注释含反斜杠，不能在未知 sql_mode 下安全重写"
        );
        definition.push_str(&format!(
            " COMMENT {}",
            quote_literal(value(9), Engine::MySql)
        ));
    }
    Ok(definition)
}
fn atomic_sqlserver(statements: &[String]) -> String {
    format!(
        "BEGIN TRY\nBEGIN TRANSACTION;\n{};\nCOMMIT TRANSACTION;\nEND TRY\nBEGIN CATCH\nIF @@TRANCOUNT > 0 ROLLBACK TRANSACTION;\nTHROW;\nEND CATCH",
        statements.join(";\n")
    )
}
pub fn designer_sql(
    engine: Engine,
    table: &Table,
    operation: &DesignerOperation,
    snapshot: &DesignSnapshot,
) -> Result<String> {
    let q = format!(
        "{}.{}",
        engine.quote(&table.schema),
        engine.quote(&table.name)
    );
    let column_row = |name: &str| -> Result<&Vec<Option<String>>> {
        snapshot
            .columns
            .iter()
            .find(|r| r.get(1).and_then(Option::as_deref) == Some(name))
            .ok_or_else(|| anyhow::anyhow!("字段不存在，请刷新结构"))
    };
    let ddl = match operation {
        DesignerOperation::RenameColumn { old, new } => {
            validate_identifier(old, "原字段名称")?;
            validate_identifier(new, "新字段名称")?;
            column_row(old)?;
            ensure!(
                !snapshot
                    .columns
                    .iter()
                    .any(|r| r.get(1).and_then(Option::as_deref) == Some(new)),
                "新字段名称已存在"
            );
            if engine == Engine::SqlServer {
                format!(
                    "EXEC sys.sp_rename {}, {}, N'COLUMN'",
                    quote_literal(&format!("{q}.{}", engine.quote(old)), engine),
                    quote_literal(new, engine)
                )
            } else {
                format!(
                    "ALTER TABLE {q} RENAME COLUMN {} TO {}",
                    engine.quote(old),
                    engine.quote(new)
                )
            }
        }
        DesignerOperation::SetDefault { column, expression } => {
            validate_identifier(column, "字段")?;
            let row = column_row(column)?;
            let field = |i| row.get(i).and_then(Option::as_deref).unwrap_or("");
            ensure!(
                match engine {
                    Engine::Postgres => field(5).is_empty() && field(6).is_empty(),
                    Engine::MySql => field(8).is_empty() && !field(5).contains("auto_increment"),
                    Engine::SqlServer => field(6).is_empty() && field(8).is_empty(),
                },
                "标识/生成列不能设置普通默认值"
            );
            if let Some(s) = expression {
                validate_design_expression(s, engine)?;
            }
            if engine == Engine::SqlServer {
                let mut statements = Vec::new();
                if !field(4).is_empty() {
                    statements.push(format!(
                        "ALTER TABLE {q} DROP CONSTRAINT {}",
                        engine.quote(field(4))
                    ));
                }
                if let Some(s) = expression {
                    statements.push(format!(
                        "ALTER TABLE {q} ADD DEFAULT ({s}) FOR {}",
                        engine.quote(column)
                    ));
                }
                ensure!(!statements.is_empty(), "此字段没有默认约束");
                if statements.len() > 1 {
                    atomic_sqlserver(&statements)
                } else {
                    statements.remove(0)
                }
            } else {
                format!(
                    "ALTER TABLE {q} ALTER COLUMN {} {}",
                    engine.quote(column),
                    match expression {
                        Some(s) => format!("SET DEFAULT ({s})"),
                        None => "DROP DEFAULT".into(),
                    }
                )
            }
        }
        DesignerOperation::AlterType {
            column,
            data_type,
            using,
        } => {
            let row = column_row(column)?;
            validate_identifier(column, "字段")?;
            let ty = designer_type(data_type, engine)?;
            // MODIFY would drop MySQL attributes unless they are all retained.
            if engine == Engine::MySql {
                format!(
                    "ALTER TABLE {q} MODIFY COLUMN {} {}",
                    engine.quote(column),
                    mysql_modified_definition(row, &ty)?
                )
            } else if engine == Engine::Postgres {
                ensure!(
                    row.get(5)
                        .and_then(Option::as_deref)
                        .unwrap_or("")
                        .is_empty()
                        && row
                            .get(6)
                            .and_then(Option::as_deref)
                            .unwrap_or("")
                            .is_empty(),
                    "标识/生成列不支持此类型转换"
                );
                if let Some(s) = using {
                    validate_design_expression(s, engine)?;
                }
                format!(
                    "ALTER TABLE {q} ALTER COLUMN {} TYPE {ty}{}",
                    engine.quote(column),
                    using
                        .as_ref()
                        .map(|s| format!(" USING ({s})"))
                        .unwrap_or_default()
                )
            } else {
                ensure!(
                    row.len() >= 16
                        && [6, 8, 9]
                            .iter()
                            .all(|i| row[*i].as_deref().unwrap_or("").is_empty())
                        && [11, 12, 15].iter().all(|i| row[*i].as_deref() != Some("1")),
                    "计算/标识/特殊字段不能直接转换类型"
                );
                let null = if row.get(3).and_then(Option::as_deref) == Some("1") {
                    "NULL"
                } else {
                    "NOT NULL"
                };
                let collation = row.get(10).and_then(Option::as_deref).filter(|_| {
                    ["CHAR", "VARCHAR", "NCHAR", "NVARCHAR", "TEXT", "NTEXT"]
                        .contains(&ty.split('(').next().unwrap_or(&ty))
                });
                let collate = if let Some(c) = collation {
                    ensure!(
                        c.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                        "排序规则名称无效"
                    );
                    format!(" COLLATE {c}")
                } else {
                    String::new()
                };
                format!(
                    "ALTER TABLE {q} ALTER COLUMN {} {ty}{collate} {null}",
                    engine.quote(column)
                )
            }
        }
        DesignerOperation::AddGenerated {
            column,
            data_type,
            expression,
            stored,
        } => {
            validate_identifier(column, "字段")?;
            ensure!(column_row(column).is_err(), "字段已存在");
            validate_design_expression(expression, engine)?;
            if engine == Engine::SqlServer {
                format!(
                    "ALTER TABLE {q} ADD {} AS ({expression}){}",
                    engine.quote(column),
                    if *stored { " PERSISTED" } else { "" }
                )
            } else {
                let ty = designer_type(data_type, engine)?;
                ensure!(
                    engine != Engine::Postgres || *stored,
                    "PostgreSQL 生成列此处使用兼容的 STORED 形式"
                );
                format!(
                    "ALTER TABLE {q} ADD COLUMN {} {ty} GENERATED ALWAYS AS ({expression}) {}",
                    engine.quote(column),
                    if *stored { "STORED" } else { "VIRTUAL" }
                )
            }
        }
        DesignerOperation::AddIdentity { column, always } => {
            validate_identifier(column, "字段")?;
            let row = column_row(column)?;
            ensure!(
                engine == Engine::Postgres,
                "给已有列增加标识属性仅支持 PostgreSQL"
            );
            ensure!(
                (4..7).all(|i| row
                    .get(i)
                    .and_then(Option::as_deref)
                    .unwrap_or("")
                    .is_empty()),
                "字段已有默认值/标识/生成属性"
            );
            format!(
                "ALTER TABLE {q} ALTER COLUMN {} ADD GENERATED {} AS IDENTITY",
                engine.quote(column),
                if *always { "ALWAYS" } else { "BY DEFAULT" }
            )
        }
        DesignerOperation::PutConstraint {
            previous,
            definition,
        } => {
            for c in &definition.columns {
                column_row(c)?;
            }
            let add = format!("ALTER TABLE {q} {}", constraint_clause(engine, definition)?);
            if let Some((name, kind)) = previous {
                ensure!(
                    snapshot
                        .constraints
                        .iter()
                        .any(|r| r.first().and_then(Option::as_deref) == Some(name)
                            && r.get(1).and_then(Option::as_deref) == Some(kind.catalog_name())),
                    "原约束不存在或类型不符"
                );
                if engine == Engine::SqlServer {
                    atomic_sqlserver(&[
                        format!("ALTER TABLE {q} {}", drop_clause(engine, name, *kind)?),
                        add,
                    ])
                } else {
                    format!(
                        "ALTER TABLE {q} {}, {}",
                        drop_clause(engine, name, *kind)?,
                        constraint_clause(engine, definition)?
                    )
                }
            } else {
                ensure!(
                    !snapshot
                        .constraints
                        .iter()
                        .any(|r| r.first().and_then(Option::as_deref) == Some(&definition.name)),
                    "约束名称已存在"
                );
                add
            }
        }
        DesignerOperation::DropConstraint { name, kind } => {
            ensure!(
                snapshot
                    .constraints
                    .iter()
                    .any(|r| r.first().and_then(Option::as_deref) == Some(name)
                        && r.get(1).and_then(Option::as_deref) == Some(kind.catalog_name())),
                "约束不存在或类型不符"
            );
            format!("ALTER TABLE {q} {}", drop_clause(engine, name, *kind)?)
        }
    };
    Ok(ddl)
}
impl Session {
    pub(super) async fn validate_designer_operation(
        &mut self,
        table: &Table,
        operation: &DesignerOperation,
        snapshot: &DesignSnapshot,
        current: &TableStructure,
    ) -> Result<()> {
        ensure!(
            &DesignSnapshot::from(current) == snapshot,
            "表定义已改变，未执行；请刷新并重新预览"
        );
        if let DesignerOperation::RenameColumn { old, .. } = operation {
            self.ensure_column_alter_dependencies_safe(table, old)
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expressions_cannot_escape_the_ddl_boundary() {
        for e in [Engine::MySql, Engine::Postgres, Engine::SqlServer] {
            for s in [
                "1); DROP TABLE x;--",
                "1) ADD COLUMN x INT",
                "1 /* comment */",
                "(1",
                "'unfinished",
                "SELECT 1",
            ] {
                assert!(validate_design_expression(s, e).is_err(), "{s}");
            }
            for s in [
                "price * (1 + tax)",
                "'a;''b'",
                "CURRENT_TIMESTAMP",
                "COALESCE(x, 0)",
            ] {
                assert!(validate_design_expression(s, e).is_ok(), "{s}");
            }
        }
    }
    fn snapshot() -> DesignSnapshot {
        DesignSnapshot {
            columns: vec![vec![Some("1".into()), Some("old'col".into())]],
            indexes: vec![],
            foreign_keys: vec![],
            constraints: vec![],
            ddl: String::new(),
        }
    }
    #[test]
    fn rename_is_quoted_and_duplicate_names_are_rejected() {
        let t = Table {
            schema: "s]x".into(),
            name: "t".into(),
            kind: "BASE TABLE".into(),
        };
        let op = DesignerOperation::RenameColumn {
            old: "old'col".into(),
            new: "new]col'".into(),
        };
        let sql = designer_sql(Engine::SqlServer, &t, &op, &snapshot()).unwrap();
        assert_eq!(
            sql,
            "EXEC sys.sp_rename N'[s]]x].[t].[old''col]', N'new]col''', N'COLUMN'"
        );
        assert!(
            designer_sql(
                Engine::Postgres,
                &t,
                &DesignerOperation::RenameColumn {
                    old: "old'col".into(),
                    new: "old'col".into()
                },
                &snapshot()
            )
            .is_err()
        );
    }
    #[test]
    fn type_fragment_rejects_suffixes() {
        for ty in [
            "INT NULL",
            "VARCHAR(2) COLLATE x",
            "INT); DROP TABLE x;--",
            "DECIMAL(2,1,0)",
        ] {
            assert!(designer_type(ty, Engine::Postgres).is_err());
        }
        assert_eq!(
            designer_type("numeric(12,2)", Engine::Postgres).unwrap(),
            "NUMERIC(12,2)"
        );
    }
}
