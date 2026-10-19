//! Bound object design plans. Preview and execution share this exact builder.
use crate::{
    model::Engine,
    object_catalog::{CatalogObject, ObjectKind},
};
use anyhow::{Result, bail, ensure};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaSnapshot {
    pub name: String,
    pub owner: String,
    pub identity: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaChange {
    Create { name: String, owner: String },
    Rename { name: String, to: String },
    Owner { name: String, owner: String },
    Drop { name: String },
}
impl SchemaChange {
    pub fn name(&self) -> &str {
        match self {
            Self::Create { name, .. }
            | Self::Rename { name, .. }
            | Self::Owner { name, .. }
            | Self::Drop { name } => name,
        }
    }
    pub fn creates(&self) -> bool {
        matches!(self, Self::Create { .. })
    }
    pub fn sql(&self, engine: Engine) -> Result<String> {
        let q = |s: &str| -> Result<String> {
            ensure!(
                !s.trim().is_empty() && !s.contains('\0'),
                "名称不能为空或包含 NUL"
            );
            Ok(engine.quote(s))
        };
        let name = q(self.name())?;
        Ok(match self {
            Self::Create { owner, .. } if engine == Engine::MySql => {
                ensure!(owner.is_empty(), "MySQL 数据库不支持 Schema 所有者");
                format!("CREATE DATABASE {name};")
            }
            Self::Create { owner, .. } => format!(
                "CREATE SCHEMA {name}{};",
                if owner.trim().is_empty() {
                    String::new()
                } else {
                    format!(" AUTHORIZATION {}", q(owner)?)
                }
            ),
            Self::Rename { to, .. } => {
                ensure!(
                    engine == Engine::Postgres,
                    "仅 PostgreSQL 支持 Schema 改名；MySQL 数据库和 SQL Server Schema 不支持此操作"
                );
                format!("ALTER SCHEMA {name} RENAME TO {};", q(to)?)
            }
            Self::Owner { owner, .. } => match engine {
                Engine::Postgres => format!("ALTER SCHEMA {name} OWNER TO {};", q(owner)?),
                Engine::SqlServer => {
                    format!("ALTER AUTHORIZATION ON SCHEMA::{name} TO {};", q(owner)?)
                }
                _ => bail!("MySQL 数据库不支持 Schema 所有者"),
            },
            Self::Drop { .. } => format!(
                "DROP {} {name}{};",
                if engine == Engine::MySql {
                    "DATABASE"
                } else {
                    "SCHEMA"
                },
                if engine == Engine::Postgres {
                    " RESTRICT"
                } else {
                    ""
                }
            ),
        })
    }
}

pub fn check_schema_snapshot(
    change: &SchemaChange,
    expected: &[SchemaSnapshot],
    current: &[SchemaSnapshot],
) -> Result<()> {
    let lookup =
        |list: &[SchemaSnapshot], name: &str| list.iter().find(|s| s.name == name).cloned();
    if change.creates() {
        ensure!(
            lookup(current, change.name()).is_none(),
            "Schema 已存在，未执行创建"
        );
    } else {
        let before = lookup(expected, change.name())
            .ok_or_else(|| anyhow::anyhow!("请先读取 Schema 属性"))?;
        ensure!(
            !before.identity.is_empty(),
            "Schema 缺少服务器身份，请重新读取属性"
        );
        ensure!(
            lookup(current, change.name()) == Some(before),
            "Schema 所有者或身份已改变，请刷新后再确认"
        );
    }
    if let SchemaChange::Rename { to, .. } = change {
        ensure!(lookup(current, to).is_none(), "目标 Schema 名称已存在");
    }
    Ok(())
}

pub fn check_definition_snapshot(expected: &str, current: &str) -> Result<()> {
    ensure!(
        expected == current,
        "服务器对象定义已改变，请关闭设计器并重新读取；草稿仍保留"
    );
    Ok(())
}

pub fn supported(engine: Engine, kind: ObjectKind, edit: bool) -> bool {
    match kind {
        ObjectKind::Table => false,
        ObjectKind::MaterializedView => engine == Engine::Postgres && !edit,
        ObjectKind::Sequence => engine != Engine::MySql,
        _ => true,
    }
}

pub fn template(engine: Engine, object: &CatalogObject) -> String {
    let name = format!(
        "{}.{}",
        engine.quote(&object.schema),
        engine.quote(&object.name)
    );
    match (engine, object.kind) {
        (_, ObjectKind::View) => format!("CREATE VIEW {name} AS\nSELECT 1 AS example;"),
        (Engine::Postgres, ObjectKind::MaterializedView) => {
            format!("CREATE MATERIALIZED VIEW {name} AS\nSELECT 1 AS example\nWITH NO DATA;")
        }
        (Engine::Postgres, ObjectKind::Function) => {
            format!("CREATE FUNCTION {name}() RETURNS integer\nLANGUAGE sql AS $$ SELECT 1; $$;")
        }
        (Engine::Postgres, ObjectKind::Procedure) => {
            format!("CREATE PROCEDURE {name}()\nLANGUAGE plpgsql AS $$ BEGIN NULL; END; $$;")
        }
        (Engine::Postgres, ObjectKind::Trigger) => format!(
            "CREATE TRIGGER {} BEFORE INSERT ON {}.{}\nFOR EACH ROW EXECUTE FUNCTION {}.trigger_function();",
            engine.quote(&object.name),
            engine.quote(&object.schema),
            engine.quote(object.parent.as_deref().unwrap_or("target_table")),
            engine.quote(&object.schema)
        ),
        (Engine::MySql, ObjectKind::Function) => format!(
            "CREATE FUNCTION {name}() RETURNS INTEGER\nDETERMINISTIC\nBEGIN\n  RETURN 1;\nEND;"
        ),
        (Engine::MySql, ObjectKind::Procedure) => {
            format!("CREATE PROCEDURE {name}()\nBEGIN\n  SELECT 1;\nEND;")
        }
        (Engine::MySql, ObjectKind::Trigger) => format!(
            "CREATE TRIGGER {name} BEFORE INSERT ON {}.{}\nFOR EACH ROW\nBEGIN\n  SET NEW.column_name = NEW.column_name;\nEND;",
            engine.quote(&object.schema),
            engine.quote(object.parent.as_deref().unwrap_or("target_table"))
        ),
        (Engine::SqlServer, ObjectKind::Function) => {
            format!("CREATE FUNCTION {name}() RETURNS int AS\nBEGIN\n  RETURN 1;\nEND;")
        }
        (Engine::SqlServer, ObjectKind::Procedure) => {
            format!("CREATE PROCEDURE {name} AS\nBEGIN\n  SELECT 1;\nEND;")
        }
        (Engine::SqlServer, ObjectKind::Trigger) => format!(
            "CREATE TRIGGER {name} ON {}.{} AFTER INSERT AS\nBEGIN\n  SET NOCOUNT ON;\nEND;",
            engine.quote(&object.schema),
            engine.quote(object.parent.as_deref().unwrap_or("target_table"))
        ),
        (_, ObjectKind::Sequence) => format!("CREATE SEQUENCE {name} START WITH 1 INCREMENT BY 1;"),
        _ => String::new(),
    }
}

#[derive(Debug)]
struct Token {
    value: String,
    start: usize,
    end: usize,
    quoted: bool,
}
/// A deliberately small DDL lexer: quoted literals and dollar bodies are opaque.
/// It verifies module identity; server parsing remains authoritative for bodies.
fn tokens(sql: &str, engine: Engine) -> Result<Vec<Token>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < sql.len() {
        let c = sql[i..].chars().next().unwrap();
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        if sql[i..].starts_with("--") || (engine == Engine::MySql && c == '#') {
            i = sql[i..].find('\n').map_or(sql.len(), |n| i + n);
            continue;
        }
        if sql[i..].starts_with("/*") {
            ensure!(
                !sql[i..].starts_with("/*!") && !sql[i..].starts_with("/*M!"),
                "不接受可执行注释"
            );
            let mut depth = 1;
            i += 2;
            while i < sql.len() && depth > 0 {
                if sql[i..].starts_with("/*") {
                    depth += 1;
                    i += 2;
                } else if sql[i..].starts_with("*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += sql[i..].chars().next().unwrap().len_utf8();
                }
            }
            ensure!(depth == 0, "注释未闭合");
            continue;
        }
        let start = i;
        if c == '$'
            && engine == Engine::Postgres
            && let Some(n) = sql[i + 1..].find('$')
        {
            let delimiter = &sql[i..i + n + 2];
            if delimiter[1..delimiter.len() - 1]
                .chars()
                .all(|ch| ch.is_alphanumeric() || ch == '_')
            {
                let body = i + delimiter.len();
                i = body
                    + sql[body..]
                        .find(delimiter)
                        .ok_or_else(|| anyhow::anyhow!("dollar quote 未闭合"))?
                    + delimiter.len();
                out.push(Token {
                    value: "<body>".into(),
                    start,
                    end: i,
                    quoted: true,
                });
                continue;
            }
        }
        if matches!(c, '\'' | '"' | '`' | '[') {
            let close = if c == '[' { ']' } else { c };
            i += c.len_utf8();
            let mut value = String::new();
            let mut closed = false;
            while i < sql.len() {
                let ch = sql[i..].chars().next().unwrap();
                i += ch.len_utf8();
                if ch == close {
                    if sql[i..].starts_with(close) {
                        value.push(close);
                        i += close.len_utf8();
                    } else {
                        closed = true;
                        break;
                    }
                } else {
                    ensure!(
                        ch != '\\' || engine != Engine::MySql,
                        "设计器不接受含反斜杠的 MySQL 引用；请使用 SQL 转义引号"
                    );
                    value.push(ch);
                }
            }
            ensure!(closed, "引号未闭合");
            out.push(Token {
                value: if c == '\'' {
                    format!("<literal:{value}>")
                } else {
                    value
                },
                start,
                end: i,
                quoted: true,
            });
            continue;
        }
        if c.is_alphanumeric() || matches!(c, '_' | '@' | '$') {
            i += c.len_utf8();
            while i < sql.len() {
                let ch = sql[i..].chars().next().unwrap();
                if ch.is_alphanumeric() || matches!(ch, '_' | '@' | '$') {
                    i += ch.len_utf8();
                } else {
                    break;
                }
            }
        } else {
            i += c.len_utf8();
        }
        out.push(Token {
            value: sql[start..i].into(),
            start,
            end: i,
            quoted: false,
        });
    }
    Ok(out)
}
fn word(t: &Token, s: &str) -> bool {
    !t.quoted && t.value.eq_ignore_ascii_case(s)
}

/// Catalog definitions may omit schema qualification. Canonicalize only the
/// declared identity tokens; never replace matching names in body text.
pub fn editable_definition(engine: Engine, object: &CatalogObject, sql: &str) -> Result<String> {
    let t = tokens(sql, engine)?;
    let keyword = match object.kind {
        ObjectKind::View | ObjectKind::MaterializedView => "VIEW",
        ObjectKind::Function => "FUNCTION",
        ObjectKind::Procedure => "PROCEDURE",
        ObjectKind::Trigger => "TRIGGER",
        ObjectKind::Sequence => "SEQUENCE",
        _ => return Ok(sql.into()),
    };
    let k = t
        .iter()
        .position(|t| word(t, keyword) || (object.kind == ObjectKind::Procedure && word(t, "PROC")))
        .ok_or_else(|| anyhow::anyhow!("服务器定义缺少对象类型"))?;
    let first = t
        .get(k + 1)
        .ok_or_else(|| anyhow::anyhow!("服务器定义缺少名称"))?;
    let end = if t.get(k + 2).is_some_and(|t| t.value == ".") {
        t.get(k + 3)
            .ok_or_else(|| anyhow::anyhow!("名称不完整"))?
            .end
    } else {
        first.end
    };
    let qualified = if object.kind == ObjectKind::Trigger && engine == Engine::Postgres {
        engine.quote(&object.name)
    } else {
        format!(
            "{}.{}",
            engine.quote(&object.schema),
            engine.quote(&object.name)
        )
    };
    let mut result = sql.to_owned();
    result.replace_range(first.start..end, &qualified);
    if word(&t[k], "PROC") {
        result.replace_range(t[k].start..t[k].end, "PROCEDURE");
    }
    if object.kind == ObjectKind::Trigger {
        let t = tokens(&result, engine)?;
        if let (Some(on), Some(parent)) = (
            t.iter().position(|t| word(t, "ON")),
            object.parent.as_deref(),
        ) && let Some(first) = t.get(on + 1)
        {
            let end = if t.get(on + 2).is_some_and(|t| t.value == ".") {
                t.get(on + 3)
                    .ok_or_else(|| anyhow::anyhow!("所属表名称不完整"))?
                    .end
            } else {
                first.end
            };
            result.replace_range(
                first.start..end,
                &format!("{}.{}", engine.quote(&object.schema), engine.quote(parent)),
            );
        }
    }
    let head = tokens(&result, engine)?;
    if head.first().is_some_and(|t| word(t, "ALTER")) && object.kind != ObjectKind::Sequence {
        result.replace_range(head[0].start..head[0].end, "CREATE");
    }
    Ok(result)
}

/// Builds explicit replacement DDL. No global string replacement, CASCADE,
/// hidden destructive fallback, or client DELIMITER/GO commands.
pub fn object_plan(
    engine: Engine,
    object: &CatalogObject,
    ddl: &str,
    edit: bool,
) -> Result<Vec<String>> {
    ensure!(
        supported(engine, object.kind, edit),
        "此对象不支持原位修改；物化视图请新建后显式迁移依赖"
    );
    ensure!(
        !object.name.trim().is_empty() && !object.schema.trim().is_empty(),
        "必须指定 Schema 和对象名"
    );
    let t = tokens(ddl, engine)?;
    ensure!(!t.is_empty(), "定义不能为空");
    ensure!(
        !t.iter().any(|t| word(t, "GO") || word(t, "DELIMITER")),
        "设计器使用单个服务器批次，不接受 GO / DELIMITER"
    );
    ensure!(
        word(&t[0], "CREATE")
            || (edit && object.kind == ObjectKind::Sequence && word(&t[0], "ALTER")),
        "定义必须以 CREATE 开始；修改序列请使用 ALTER SEQUENCE"
    );
    let keyword = match object.kind {
        ObjectKind::View => "VIEW",
        ObjectKind::MaterializedView => "VIEW",
        ObjectKind::Function => "FUNCTION",
        ObjectKind::Procedure => "PROCEDURE",
        ObjectKind::Trigger => "TRIGGER",
        ObjectKind::Sequence => "SEQUENCE",
        _ => unreachable!(),
    };
    let k = t
        .iter()
        .position(|t| word(t, keyword))
        .ok_or_else(|| anyhow::anyhow!("定义对象类型不符"))?;
    ensure!(k < 16, "无法识别对象定义头");
    // Only the known CREATE modifiers may precede the object keyword. DEFINER
    // values are identifiers/strings; semicolons and arbitrary commands are not.
    ensure!(
        !t[1..k]
            .iter()
            .any(|t| matches!(t.value.as_str(), ";" | "(" | ")")),
        "对象定义头无效"
    );
    if object.kind == ObjectKind::MaterializedView {
        ensure!(k > 0 && word(&t[k - 1], "MATERIALIZED"), "必须定义物化视图");
    } else {
        ensure!(
            !t[..k].iter().any(|t| word(t, "MATERIALIZED")),
            "对象类型不符"
        );
    }
    if object.kind == ObjectKind::Trigger && engine == Engine::Postgres {
        ensure!(
            t.get(k + 1).is_some_and(|t| t.value == object.name),
            "触发器名称与绑定对象不符"
        );
        let on = t
            .iter()
            .position(|t| word(t, "ON"))
            .ok_or_else(|| anyhow::anyhow!("触发器必须指定所属表"))?;
        let parent = object
            .parent
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("请填写触发器所属表"))?;
        ensure!(
            t.len() > on + 3
                && t[on + 1].value == object.schema
                && t[on + 2].value == "."
                && t[on + 3].value == parent,
            "触发器所属表与绑定目标不符"
        );
    } else {
        ensure!(
            t.len() > k + 3
                && t[k + 1].value == object.schema
                && t[k + 2].value == "."
                && t[k + 3].value == object.name,
            "定义必须使用所绑定对象的完整 Schema.名称，不允许修改身份"
        );
    }
    if object.kind == ObjectKind::Trigger && engine != Engine::Postgres {
        let on = t
            .iter()
            .position(|t| word(t, "ON"))
            .ok_or_else(|| anyhow::anyhow!("触发器必须指定所属表"))?;
        let parent = object
            .parent
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("请填写触发器所属表"))?;
        ensure!(
            t.len() > on + 3
                && t[on + 1].value == object.schema
                && t[on + 2].value == "."
                && t[on + 3].value == parent,
            "触发器所属表与绑定目标不符"
        );
    }
    let is_module = matches!(
        object.kind,
        ObjectKind::Function | ObjectKind::Procedure | ObjectKind::Trigger
    );
    if engine == Engine::MySql && is_module {
        // The outer compound block must consume the complete batch. Internal
        // statements belong to the routine; no SQL may follow its final END.
        if let Some(begin) = t.iter().position(|t| word(t, "BEGIN")) {
            ensure!(
                !t[..begin].iter().any(|t| t.value == ";"),
                "复合正文前不允许额外语句"
            );
            let mut depth = 0usize;
            let mut end = None;
            for (i, token) in t.iter().enumerate().skip(begin) {
                if word(token, "BEGIN")
                    || (word(token, "CASE")
                        && !i
                            .checked_sub(1)
                            .and_then(|j| t.get(j))
                            .is_some_and(|t| word(t, "END")))
                {
                    depth += 1;
                }
                if word(token, "END")
                    && !t.get(i + 1).is_some_and(|t| {
                        ["IF", "LOOP", "WHILE", "REPEAT"].iter().any(|w| word(t, w))
                    })
                {
                    ensure!(depth > 0, "END 不匹配");
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
            }
            let end = end.ok_or_else(|| anyhow::anyhow!("BEGIN / END 未闭合"))?;
            ensure!(
                t[end + 1..].is_empty() || (t[end + 1..].len() == 1 && t[end + 1].value == ";"),
                "复合正文结束后不允许额外 SQL"
            );
        } else {
            ensure!(
                t.iter().filter(|t| t.value == ";").count() <= 1
                    && t.iter()
                        .position(|t| t.value == ";")
                        .is_none_or(|i| i == t.len() - 1),
                "仅接受单个对象定义"
            );
        }
    } else if !(engine == Engine::SqlServer && is_module) {
        ensure!(
            t.iter().filter(|t| t.value == ";").count() <= 1
                && t.iter()
                    .position(|t| t.value == ";")
                    .is_none_or(|i| i == t.len() - 1),
            "仅接受单个对象定义；过程正文请使用 dollar quote"
        );
    }
    if !edit {
        ensure!(word(&t[0], "CREATE"), "新建对象必须使用 CREATE");
        ensure!(
            !t[1..k]
                .iter()
                .any(|t| word(t, "REPLACE") || word(t, "ALTER")),
            "新建操作不允许覆盖已有对象"
        );
        return Ok(vec![ddl.to_owned()]);
    }
    if object.kind == ObjectKind::Sequence {
        ensure!(
            word(&t[0], "ALTER"),
            "序列修改必须使用 ALTER SEQUENCE，不能重置当前值"
        );
        return Ok(vec![ddl.to_owned()]);
    }
    let mut body = ddl.to_owned();
    match engine {
        Engine::Postgres | Engine::MySql
            if engine == Engine::Postgres || object.kind == ObjectKind::View =>
        {
            if !t[1..k].iter().any(|t| word(t, "REPLACE")) {
                body.insert_str(t[0].end, " OR REPLACE");
            }
            Ok(vec![body])
        }
        Engine::SqlServer => {
            ensure!(
                !t[1..k].iter().any(|t| word(t, "REPLACE")),
                "SQL Server 不支持 OR REPLACE"
            );
            body.replace_range(t[0].start..t[k].start, "ALTER ");
            Ok(vec![body])
        }
        Engine::MySql => Ok(vec![
            format!(
                "DROP {keyword} {}.{};",
                engine.quote(&object.schema),
                engine.quote(&object.name)
            ),
            body,
        ]),
        Engine::Postgres => unreachable!("handled above"),
    }
}

/// PostgreSQL overload identity arguments are frozen during edits. A signature
/// change is a new object and requires the explicit create workflow.
pub fn validate_edit_identity(
    engine: Engine,
    object: &CatalogObject,
    original: &str,
    draft: &str,
) -> Result<()> {
    if engine != Engine::Postgres || !object.kind.is_routine() {
        return Ok(());
    }
    fn args(sql: &str) -> Result<Vec<(String, bool)>> {
        let t = tokens(sql, Engine::Postgres)?;
        let kind = t
            .iter()
            .position(|t| word(t, "FUNCTION") || word(t, "PROCEDURE"))
            .ok_or_else(|| anyhow::anyhow!("例程类型缺失"))?;
        let start = kind + 4;
        ensure!(
            t.get(start).is_some_and(|t| t.value == "("),
            "例程必须使用完整限定名称并指定参数"
        );
        let mut depth = 0;
        let mut result = Vec::new();
        for token in &t[start..] {
            if token.value == "(" {
                depth += 1;
            }
            if token.value == ")" {
                depth -= 1;
            }
            result.push((
                if token.quoted {
                    token.value.clone()
                } else {
                    token.value.to_lowercase()
                },
                token.quoted,
            ));
            if depth == 0 {
                return Ok(result);
            }
        }
        bail!("参数列表未闭合")
    }
    ensure!(
        args(original)? == args(draft)?,
        "修改例程不能改变参数身份；请使用新建例程工作流"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn obj(kind: ObjectKind) -> CatalogObject {
        CatalogObject {
            schema: "public".into(),
            name: "v".into(),
            kind,
            owner: None,
            comment: None,
            estimated_rows: None,
            size_bytes: None,
            created_at: None,
            updated_at: None,
            engine: None,
            signature: None,
            parent: None,
        }
    }
    #[test]
    fn schema_dialects_and_escaping() {
        assert_eq!(
            SchemaChange::Rename {
                name: "a\"b".into(),
                to: "c".into()
            }
            .sql(Engine::Postgres)
            .unwrap(),
            "ALTER SCHEMA \"a\"\"b\" RENAME TO \"c\";"
        );
        assert!(
            SchemaChange::Rename {
                name: "a".into(),
                to: "b".into()
            }
            .sql(Engine::MySql)
            .is_err()
        );
        assert_eq!(
            SchemaChange::Drop { name: "a".into() }
                .sql(Engine::Postgres)
                .unwrap(),
            "DROP SCHEMA \"a\" RESTRICT;"
        );
    }
    #[test]
    fn identity_and_single_statement() {
        let o = obj(ObjectKind::View);
        assert!(
            object_plan(
                Engine::Postgres,
                &o,
                "CREATE VIEW public.other AS SELECT 1",
                true
            )
            .is_err()
        );
        assert!(
            object_plan(
                Engine::Postgres,
                &o,
                "CREATE VIEW public.v AS SELECT 1; DROP TABLE public.t;",
                true
            )
            .is_err()
        );
        assert!(
            object_plan(
                Engine::Postgres,
                &o,
                "CREATE VIEW public.v AS SELECT ';'",
                true
            )
            .unwrap()[0]
                .starts_with("CREATE OR REPLACE VIEW")
        );
    }
    #[test]
    fn dialect_replacement() {
        let o = obj(ObjectKind::Procedure);
        let ddl = template(Engine::MySql, &o);
        let plan = object_plan(Engine::MySql, &o, &ddl, true).unwrap();
        assert_eq!(plan.len(), 2);
        assert!(plan[0].starts_with("DROP PROCEDURE"));
        let ddl = template(Engine::SqlServer, &o);
        assert!(
            object_plan(Engine::SqlServer, &o, &ddl, true).unwrap()[0]
                .starts_with("ALTER PROCEDURE")
        );
    }
    #[test]
    fn postgres_body_is_opaque() {
        let o = obj(ObjectKind::Function);
        assert!(object_plan(Engine::Postgres, &o, &template(Engine::Postgres, &o), true).is_ok());
    }
    #[test]
    fn creation_never_overwrites() {
        assert!(
            object_plan(
                Engine::Postgres,
                &obj(ObjectKind::View),
                "CREATE OR REPLACE VIEW public.v AS SELECT 1",
                false
            )
            .is_err()
        );
    }
    #[test]
    fn rejects_mysql_extra_statements_and_executable_comments() {
        let o = obj(ObjectKind::Procedure);
        assert!(object_plan(Engine::MySql,&o,"CREATE PROCEDURE public.v() BEGIN SELECT 1; END; DROP TABLE public.t; BEGIN SELECT 2; END;",true).is_err());
        assert!(
            object_plan(
                Engine::MySql,
                &o,
                "CREATE PROCEDURE public.v() BEGIN SELECT 1; END; /*! DROP TABLE t */",
                true
            )
            .is_err()
        );
        assert!(object_plan(Engine::MySql,&o,"CREATE PROCEDURE public.v() BEGIN IF 1 THEN SELECT 1; END IF; CASE 1 WHEN 1 THEN SELECT 2; END CASE; END;",true).is_ok());
    }
    #[test]
    fn postgres_overload_identity_is_frozen() {
        let o = obj(ObjectKind::Function);
        let a =
            "CREATE FUNCTION public.v(a integer) RETURNS integer LANGUAGE sql AS $$SELECT a;$$;";
        let b = "CREATE FUNCTION public.v(a text) RETURNS integer LANGUAGE sql AS $$SELECT 1;$$;";
        assert!(validate_edit_identity(Engine::Postgres, &o, a, b).is_err());
        assert!(
            validate_edit_identity(
                Engine::Postgres,
                &o,
                a,
                &a.replace("SELECT a", "SELECT a+1")
            )
            .is_ok()
        );
    }
    #[test]
    fn canonicalization_only_changes_declared_identity() {
        let o = obj(ObjectKind::Procedure);
        let sql = "ALTER PROC v AS BEGIN SELECT 'v'; END;";
        let canonical = editable_definition(Engine::SqlServer, &o, sql).unwrap();
        assert_eq!(
            canonical,
            "CREATE PROCEDURE [public].[v] AS BEGIN SELECT 'v'; END;"
        );
        assert!(
            object_plan(Engine::SqlServer, &o, &canonical, true).unwrap()[0]
                .starts_with("ALTER PROCEDURE")
        );
    }
    #[test]
    fn postgres_trigger_parent_cannot_change() {
        let mut o = obj(ObjectKind::Trigger);
        o.parent = Some("items".into());
        let ddl = template(Engine::Postgres, &o);
        assert!(object_plan(Engine::Postgres, &o, &ddl, true).is_ok());
        assert!(
            object_plan(
                Engine::Postgres,
                &o,
                &ddl.replace("\"items\"", "\"orders\""),
                true
            )
            .is_err()
        );
    }
    #[test]
    fn sequence_edit_is_explicit_and_preserves_current_value_by_default() {
        let o = obj(ObjectKind::Sequence);
        assert!(
            object_plan(
                Engine::Postgres,
                &o,
                "CREATE SEQUENCE public.v START WITH 1",
                true
            )
            .is_err()
        );
        assert_eq!(
            object_plan(
                Engine::Postgres,
                &o,
                "ALTER SEQUENCE public.v INCREMENT BY 2",
                true
            )
            .unwrap(),
            vec!["ALTER SEQUENCE public.v INCREMENT BY 2"]
        );
        assert!(!supported(
            Engine::Postgres,
            ObjectKind::MaterializedView,
            true
        ));
    }
    #[test]
    fn rechecks_reject_recreated_or_changed_objects() {
        let expected = SchemaSnapshot {
            name: "public".into(),
            owner: "alice".into(),
            identity: "100".into(),
        };
        let mut recreated = expected.clone();
        recreated.identity = "101".into();
        let change = SchemaChange::Drop {
            name: "public".into(),
        };
        assert!(
            check_schema_snapshot(&change, std::slice::from_ref(&expected), &[recreated]).is_err()
        );
        let mut changed = expected.clone();
        changed.owner = "bob".into();
        assert!(
            check_schema_snapshot(&change, std::slice::from_ref(&expected), &[changed]).is_err()
        );
        assert!(
            check_schema_snapshot(
                &change,
                std::slice::from_ref(&expected),
                std::slice::from_ref(&expected)
            )
            .is_ok()
        );
        assert!(
            check_definition_snapshot("CREATE VIEW s.v AS SELECT 1", "CREATE VIEW s.v AS SELECT 2")
                .is_err()
        );
        assert!(check_definition_snapshot("same", "same").is_ok());
    }
}
