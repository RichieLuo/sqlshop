use super::{ROW_LIMIT, Session, validate_identifier};
use crate::{
    model::Engine,
    object_catalog::{CatalogObject, ObjectCatalog, ObjectKind},
};
use anyhow::Result;

/// A local definition request rejection, after any response has been consumed.
/// Driver errors retain their original type and are never wrapped in this marker.
#[derive(Debug)]
pub(crate) struct ObjectDefinitionRejected(String);

impl std::fmt::Display for ObjectDefinitionRejected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ObjectDefinitionRejected {}

fn definition_rejected(message: impl Into<String>) -> anyhow::Error {
    ObjectDefinitionRejected(message.into()).into()
}

trait DefinitionOption<T> {
    fn definition_required(self, message: &str) -> Result<T>;
}

impl<T> DefinitionOption<T> for Option<T> {
    fn definition_required(self, message: &str) -> Result<T> {
        self.ok_or_else(|| definition_rejected(message))
    }
}

// Every category is queried separately. Missing routine/trigger permissions must
// not prevent browsing the visible tables, nor silently masquerade as no objects.
// SQL itself bounds each query to ROW_LIMIT + 1; query_internal clips the final
// extra row and signals truncation, without fetching an unbounded catalog.
impl Session {
    pub async fn object_catalog(&mut self) -> Result<ObjectCatalog> {
        let queries: Vec<(&str, &str)> = match self.profile.engine {
            Engine::MySql => vec![
                ("表与视图", MYSQL_RELATIONS),
                ("函数与过程", MYSQL_ROUTINES),
                ("触发器", MYSQL_TRIGGERS),
            ],
            Engine::Postgres => vec![
                ("表、视图与物化视图", PG_RELATIONS),
                ("函数与过程", PG_ROUTINES),
                ("触发器", PG_TRIGGERS),
                ("序列", PG_SEQUENCES),
            ],
            Engine::SqlServer => vec![
                ("表与视图", MSSQL_RELATIONS),
                ("函数、过程与触发器", MSSQL_ROUTINES),
                ("序列", MSSQL_SEQUENCES),
            ],
        };
        let mut catalog = ObjectCatalog::default();
        for (index, (label, sql)) in queries.into_iter().enumerate() {
            match self.query_internal(sql).await {
                Ok(output) => {
                    for set in output.sets {
                        if set.truncated {
                            catalog.truncated = true;
                            catalog.warnings.push(format!(
                                "{label}目录超过 {ROW_LIMIT} 项，仅显示前 {ROW_LIMIT} 项。"
                            ));
                        }
                        for row in set.rows {
                            if let Some(object) = decode_object(&row) {
                                catalog.objects.push(object);
                            }
                        }
                    }
                }
                Err(error) if index == 0 => {
                    return Err(error.context(format!("读取{label}目录失败")));
                }
                Err(error) if super::query_error_preserves_session(&error) => {
                    let unavailable: &[ObjectKind] = match (self.profile.engine, index) {
                        (Engine::MySql | Engine::Postgres, 1) => {
                            &[ObjectKind::Function, ObjectKind::Procedure]
                        }
                        (Engine::MySql | Engine::Postgres, 2) => &[ObjectKind::Trigger],
                        (Engine::Postgres, 3) | (Engine::SqlServer, 2) => &[ObjectKind::Sequence],
                        (Engine::SqlServer, 1) => &[
                            ObjectKind::Function,
                            ObjectKind::Procedure,
                            ObjectKind::Trigger,
                        ],
                        _ => &[],
                    };
                    catalog.unavailable_kinds.extend_from_slice(unavailable);
                    catalog
                        .warnings
                        .push(format!("{label}目录暂不可读取：{error:#}"));
                }
                Err(error) => {
                    return Err(error.context(format!("读取{label}目录时连接中断")));
                }
            }
        }
        catalog.objects.sort_by(|a, b| {
            a.schema
                .cmp(&b.schema)
                .then(a.name.cmp(&b.name))
                .then(a.signature.cmp(&b.signature))
                .then(a.parent.cmp(&b.parent))
        });
        Ok(catalog)
    }

    /// Retrieves a server definition for the exact object. PostgreSQL view and
    /// sequence DDL is explicitly reconstructed from catalogs; it is not a dump.
    pub async fn object_definition(&mut self, object: &CatalogObject) -> Result<String> {
        validate_identifier(&object.schema, "Schema 名称")
            .map_err(|error| definition_rejected(error.to_string()))?;
        validate_identifier(&object.name, "对象名称")
            .map_err(|error| definition_rejected(error.to_string()))?;
        if object.kind == ObjectKind::Table {
            return Ok(self
                .table_structure(&object.as_table().definition_required("对象不是表")?)
                .await?
                .ddl);
        }
        let qualified = format!(
            "{}.{}",
            self.profile.engine.quote(&object.schema),
            self.profile.engine.quote(&object.name)
        );
        match self.profile.engine {
            Engine::MySql => {
                let (keyword, column) = match object.kind {
                    ObjectKind::View => ("VIEW", "Create View"),
                    ObjectKind::Function => ("FUNCTION", "Create Function"),
                    ObjectKind::Procedure => ("PROCEDURE", "Create Procedure"),
                    ObjectKind::Trigger => ("TRIGGER", "SQL Original Statement"),
                    _ => return Err(definition_rejected("MySQL 不支持此类对象定义")),
                };
                let output = self
                    .query_internal(&format!("SHOW CREATE {keyword} {qualified}"))
                    .await?;
                for set in output.sets {
                    if let Some(index) = set
                        .columns
                        .iter()
                        .position(|name| name.eq_ignore_ascii_case(column))
                    {
                        return set
                            .rows
                            .first()
                            .and_then(|row| row.get(index))
                            .and_then(Clone::clone)
                            .definition_required("服务器未公开对象定义，可能缺少权限");
                    }
                }
                Err(definition_rejected("SHOW CREATE 未返回对象定义"))
            }
            Engine::Postgres => {
                let (sql, parameters): (&str, Vec<&str>) = match object.kind {
                    ObjectKind::Function | ObjectKind::Procedure => (
                        "SELECT pg_get_functiondef(p.oid)::text FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname=$1 AND p.proname=$2 AND pg_get_function_identity_arguments(p.oid)=$3 AND (($4='p' AND p.prokind='p') OR ($4='f' AND p.prokind IN ('f','w')))",
                        vec![
                            &object.schema,
                            &object.name,
                            object
                                .signature
                                .as_deref()
                                .definition_required("函数目录缺少签名，无法安全定位重载")?,
                            if object.kind == ObjectKind::Procedure {
                                "p"
                            } else {
                                "f"
                            },
                        ],
                    ),
                    ObjectKind::Trigger => (
                        "SELECT pg_get_triggerdef(t.oid,false)::text FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND t.tgname=$2 AND c.relname=$3 AND NOT t.tgisinternal",
                        vec![
                            &object.schema,
                            &object.name,
                            object
                                .parent
                                .as_deref()
                                .definition_required("触发器目录缺少所属表")?,
                        ],
                    ),
                    ObjectKind::View | ObjectKind::MaterializedView => (
                        "SELECT pg_get_viewdef(c.oid,false)::text FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2 AND c.relkind::text=$3",
                        vec![
                            &object.schema,
                            &object.name,
                            if object.kind == ObjectKind::View {
                                "v"
                            } else {
                                "m"
                            },
                        ],
                    ),
                    ObjectKind::Sequence => (
                        "SELECT format_type(s.seqtypid,NULL)::text, s.seqstart::text,s.seqincrement::text,s.seqmin::text,s.seqmax::text,s.seqcache::text,s.seqcycle::text FROM pg_sequence s JOIN pg_class c ON c.oid=s.seqrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2 AND c.relkind='S'",
                        vec![&object.schema, &object.name],
                    ),
                    _ => return Err(definition_rejected("此类对象暂不提供定义")),
                };
                let output = self.parameter_query(sql, &parameters).await?;
                let rows = &output
                    .sets
                    .first()
                    .definition_required("服务器未返回对象定义")?
                    .rows;
                if rows.len() != 1 {
                    return Err(definition_rejected(
                        "对象不存在、身份发生变化或当前账号无法读取定义",
                    ));
                }
                let row = &rows[0];
                let definition =
                    value(row, 0).definition_required("服务器未公开对象定义，可能缺少权限")?;
                match object.kind {
                    ObjectKind::View | ObjectKind::MaterializedView => Ok(format!(
                        "-- 依据服务器视图查询重建；所有者、授权、选项、注释与依赖需另行导出。\nCREATE {}VIEW {qualified} AS\n{}{};",
                        if object.kind == ObjectKind::MaterializedView {
                            "MATERIALIZED "
                        } else {
                            ""
                        },
                        definition.trim_end().trim_end_matches(';'),
                        if object.kind == ObjectKind::MaterializedView {
                            "\nWITH NO DATA"
                        } else {
                            ""
                        }
                    )),
                    ObjectKind::Sequence => {
                        if row.len() < 7 || (0..7).any(|index| value(row, index).is_none()) {
                            return Err(definition_rejected("服务器序列定义不完整"));
                        }
                        Ok(format!(
                            "-- 依据服务器序列参数重建；不包含当前值、OWNED BY、授权与注释。\nCREATE SEQUENCE {qualified} AS {definition} START WITH {} INCREMENT BY {} MINVALUE {} MAXVALUE {} CACHE {} {}CYCLE;",
                            value(row, 1).unwrap(),
                            value(row, 2).unwrap(),
                            value(row, 3).unwrap(),
                            value(row, 4).unwrap(),
                            value(row, 5).unwrap(),
                            if value(row, 6) == Some("true") {
                                ""
                            } else {
                                "NO "
                            }
                        ))
                    }
                    _ => Ok(definition.to_owned()),
                }
            }
            Engine::SqlServer => {
                if object.kind == ObjectKind::Sequence {
                    let output = self.parameter_query("SELECT CASE WHEN t.is_user_defined=1 THEN QUOTENAME(SCHEMA_NAME(t.schema_id))+'.'+QUOTENAME(t.name) ELSE TYPE_NAME(q.system_type_id)+CASE WHEN q.system_type_id IN (106,108) THEN '('+CONVERT(nvarchar(10),q.precision)+',0)' ELSE '' END END, CONVERT(nvarchar(100),q.start_value),CONVERT(nvarchar(100),q.increment),CONVERT(nvarchar(100),q.minimum_value),CONVERT(nvarchar(100),q.maximum_value),CASE WHEN q.is_cycling=1 THEN 'CYCLE' ELSE 'NO CYCLE' END,CASE WHEN q.is_cached=1 THEN CASE WHEN q.cache_size IS NULL THEN 'CACHE' ELSE 'CACHE '+CONVERT(nvarchar(100),q.cache_size) END ELSE 'NO CACHE' END FROM sys.sequences q JOIN sys.schemas s ON s.schema_id=q.schema_id JOIN sys.types t ON t.user_type_id=q.user_type_id WHERE s.name=@P1 AND q.name=@P2", &[&object.schema,&object.name]).await?;
                    let row = output
                        .sets
                        .first()
                        .and_then(|set| set.rows.first())
                        .definition_required("序列不存在或当前账号无法读取定义")?;
                    if row.len() < 7 || (0..7).any(|index| value(row, index).is_none()) {
                        return Err(definition_rejected("服务器序列定义不完整"));
                    }
                    return Ok(format!(
                        "-- 依据服务器序列参数重建；不包含当前值、授权与注释。\nCREATE SEQUENCE {qualified} AS {} START WITH {} INCREMENT BY {} MINVALUE {} MAXVALUE {} {} {};",
                        value(row, 0).unwrap(),
                        value(row, 1).unwrap(),
                        value(row, 2).unwrap(),
                        value(row, 3).unwrap(),
                        value(row, 4).unwrap(),
                        value(row, 5).unwrap(),
                        value(row, 6).unwrap()
                    ));
                }
                let kind = match object.kind {
                    ObjectKind::View => "VIEW",
                    ObjectKind::Function => "FUNCTION",
                    ObjectKind::Procedure => "PROCEDURE",
                    ObjectKind::Trigger => "TRIGGER",
                    _ => return Err(definition_rejected("SQL Server 不支持此类对象定义")),
                };
                let output = self.parameter_query("SELECT m.definition FROM sys.objects o JOIN sys.schemas s ON s.schema_id=o.schema_id LEFT JOIN sys.sql_modules m ON m.object_id=o.object_id WHERE s.name=@P1 AND o.name=@P2 AND ((@P3='VIEW' AND o.type='V') OR (@P3='FUNCTION' AND o.type IN ('FN','IF','TF','FS','FT')) OR (@P3='PROCEDURE' AND o.type IN ('P','PC')) OR (@P3='TRIGGER' AND o.type IN ('TR','TA')))", &[&object.schema,&object.name,kind]).await?;
                output.sets.first().and_then(|set|set.rows.first()).and_then(|row|value(row,0)).map(str::to_owned).definition_required("对象不存在、定义已加密、CLR 对象没有 T-SQL 定义，或当前账号缺少 VIEW DEFINITION 权限")
            }
        }
    }
}

fn value(row: &[Option<String>], index: usize) -> Option<&str> {
    row.get(index).and_then(Option::as_deref)
}

fn decode_object(row: &[Option<String>]) -> Option<CatalogObject> {
    let kind = match value(row, 2)? {
        "TABLE" => ObjectKind::Table,
        "VIEW" => ObjectKind::View,
        "MATERIALIZED VIEW" => ObjectKind::MaterializedView,
        "FUNCTION" => ObjectKind::Function,
        "PROCEDURE" => ObjectKind::Procedure,
        "TRIGGER" => ObjectKind::Trigger,
        "SEQUENCE" => ObjectKind::Sequence,
        _ => return None,
    };
    let string = |index| {
        value(row, index)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    Some(CatalogObject {
        schema: value(row, 0)?.into(),
        name: value(row, 1)?.into(),
        kind,
        owner: string(3),
        comment: string(4),
        estimated_rows: value(row, 5).and_then(|s| s.parse().ok()),
        size_bytes: value(row, 6).and_then(|s| s.parse().ok()),
        created_at: string(7),
        updated_at: string(8),
        engine: string(9),
        // Empty identity arguments still identify a zero-argument routine.
        signature: value(row, 10).map(str::to_owned),
        parent: string(11),
    })
}

// Result columns across engines: schema, name, kind, owner, comment,
// estimated_rows, size_bytes, created_at, updated_at, engine, signature, parent.
const MYSQL_RELATIONS: &str = r#"SELECT
    CAST(TABLE_SCHEMA AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
    CAST(TABLE_NAME AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
    CASE WHEN TABLE_TYPE IN ('VIEW','SYSTEM VIEW') THEN 'VIEW' ELSE 'TABLE' END,
    NULL,CAST(TABLE_COMMENT AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
    CAST(TABLE_ROWS AS CHAR),CAST(DATA_LENGTH+INDEX_LENGTH AS CHAR),CAST(CREATE_TIME AS CHAR),CAST(UPDATE_TIME AS CHAR),CAST(ENGINE AS CHAR),NULL,NULL
    FROM information_schema.TABLES WHERE TABLE_SCHEMA=DATABASE() ORDER BY TABLE_NAME LIMIT 1001"#;

const MYSQL_ROUTINES: &str = r#"SELECT
    CAST(ROUTINE_SCHEMA AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
    CAST(ROUTINE_NAME AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
    CAST(ROUTINE_TYPE AS CHAR),CAST(DEFINER AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
    CAST(ROUTINE_COMMENT AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
    NULL,NULL,CAST(CREATED AS CHAR),CAST(LAST_ALTERED AS CHAR),NULL,NULL,NULL
    FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA=DATABASE() ORDER BY ROUTINE_TYPE,ROUTINE_NAME LIMIT 1001"#;

const MYSQL_TRIGGERS: &str = r#"SELECT
    CAST(TRIGGER_SCHEMA AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
    CAST(TRIGGER_NAME AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
    'TRIGGER',CAST(DEFINER AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,NULL,NULL,NULL,CAST(CREATED AS CHAR),NULL,NULL,NULL,
    CAST(EVENT_OBJECT_TABLE AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci
    FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA=DATABASE() ORDER BY EVENT_OBJECT_TABLE,TRIGGER_NAME LIMIT 1001"#;

const PG_RELATIONS: &str = r#"SELECT n.nspname::text,c.relname::text,
    CASE c.relkind WHEN 'v' THEN 'VIEW' WHEN 'm' THEN 'MATERIALIZED VIEW' ELSE 'TABLE' END,
    pg_get_userbyid(c.relowner)::text,obj_description(c.oid,'pg_class')::text,
    CASE WHEN c.relkind IN ('r','m') AND c.reltuples>=0 THEN floor(c.reltuples)::bigint::text END,
    CASE WHEN c.relkind IN ('r','m') THEN (c.relpages::bigint*current_setting('block_size')::bigint)::text END,
    NULL,NULL,NULL,NULL,NULL
    FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
    WHERE c.relkind IN ('r','p','v','m','f') AND n.nspname<>'information_schema' AND n.nspname!~'^pg_'
    AND has_schema_privilege(n.oid,'USAGE') AND has_table_privilege(c.oid,'SELECT')
    ORDER BY n.nspname,c.relname LIMIT 1001"#;

const PG_ROUTINES: &str = r#"SELECT n.nspname::text,p.proname::text,
    CASE WHEN p.prokind='p' THEN 'PROCEDURE' ELSE 'FUNCTION' END,
    pg_get_userbyid(p.proowner)::text,obj_description(p.oid,'pg_proc')::text,
    NULL,NULL,NULL,NULL,NULL,pg_get_function_identity_arguments(p.oid)::text,NULL
    FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace
    WHERE p.prokind IN ('f','w','p') AND n.nspname<>'information_schema' AND n.nspname!~'^pg_'
    AND has_schema_privilege(n.oid,'USAGE') AND has_function_privilege(p.oid,'EXECUTE')
    ORDER BY n.nspname,p.prokind,p.proname,pg_get_function_identity_arguments(p.oid) LIMIT 1001"#;

const PG_TRIGGERS: &str = r#"SELECT n.nspname::text,t.tgname::text,'TRIGGER',pg_get_userbyid(c.relowner)::text,
    obj_description(t.oid,'pg_trigger')::text,NULL,NULL,NULL,NULL,NULL,NULL,c.relname::text
    FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace
    WHERE NOT t.tgisinternal AND n.nspname<>'information_schema' AND n.nspname!~'^pg_'
    AND has_schema_privilege(n.oid,'USAGE') AND has_table_privilege(c.oid,'SELECT')
    ORDER BY n.nspname,c.relname,t.tgname LIMIT 1001"#;

// PostgreSQL can evaluate WHERE predicates before the relkind filter. The
// privilege function raises on non-sequences (including TOAST relations), so
// CASE must guard its argument rather than relying on predicate order.
const PG_SEQUENCES: &str = r#"SELECT n.nspname::text,c.relname::text,'SEQUENCE',pg_get_userbyid(c.relowner)::text,
    obj_description(c.oid,'pg_class')::text,NULL,NULL,NULL,NULL,NULL,NULL,NULL
    FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
    WHERE c.relkind='S' AND n.nspname<>'information_schema' AND n.nspname!~'^pg_'
    AND has_schema_privilege(n.oid,'USAGE')
    AND CASE WHEN c.relkind='S' THEN has_sequence_privilege(c.oid,'USAGE,SELECT') ELSE false END
    ORDER BY n.nspname,c.relname LIMIT 1001"#;

const MSSQL_RELATIONS: &str = r#"SELECT TOP (1001) s.name,o.name,CASE WHEN o.type='V' THEN 'VIEW' ELSE 'TABLE' END,
    USER_NAME(COALESCE(o.principal_id,s.principal_id)),CONVERT(nvarchar(max),ep.value),
    CASE WHEN o.type='U' AND t.is_memory_optimized=0 THEN CONVERT(nvarchar(100),p.row_estimate) END,
    CASE WHEN o.type='U' AND t.is_memory_optimized=0 AND NOT EXISTS(SELECT 1 FROM sys.indexes i WHERE i.object_id=o.object_id AND i.type IN (5,6)) THEN CONVERT(nvarchar(100),allocation.size_bytes) END,
    CONVERT(nvarchar(33),o.create_date,126),CONVERT(nvarchar(33),o.modify_date,126),NULL,NULL,NULL
    FROM sys.objects o JOIN sys.schemas s ON s.schema_id=o.schema_id
    LEFT JOIN sys.tables t ON t.object_id=o.object_id
    LEFT JOIN sys.extended_properties ep ON ep.class=1 AND ep.major_id=o.object_id AND ep.minor_id=0 AND ep.name='MS_Description'
    OUTER APPLY(SELECT SUM(CONVERT(bigint,rows)) AS row_estimate FROM sys.partitions WHERE object_id=o.object_id AND index_id IN (0,1)) p
    OUTER APPLY(SELECT SUM(CONVERT(bigint,a.total_pages)*8192) AS size_bytes FROM sys.partitions sp
        JOIN sys.allocation_units a ON (a.type IN (1,3) AND a.container_id=sp.hobt_id) OR (a.type=2 AND a.container_id=sp.partition_id)
        WHERE sp.object_id=o.object_id) allocation
    WHERE o.type IN ('U','V') AND o.is_ms_shipped=0 ORDER BY s.name,o.name"#;

const MSSQL_ROUTINES: &str = r#"SELECT TOP (1001) s.name,o.name,
    CASE WHEN o.type IN ('P','PC') THEN 'PROCEDURE' WHEN o.type IN ('TR','TA') THEN 'TRIGGER' ELSE 'FUNCTION' END,
    USER_NAME(COALESCE(o.principal_id,s.principal_id)),CONVERT(nvarchar(max),ep.value),NULL,NULL,
    CONVERT(nvarchar(33),o.create_date,126),CONVERT(nvarchar(33),o.modify_date,126),NULL,NULL,OBJECT_NAME(o.parent_object_id)
    FROM sys.objects o JOIN sys.schemas s ON s.schema_id=o.schema_id
    LEFT JOIN sys.extended_properties ep ON ep.class=1 AND ep.major_id=o.object_id AND ep.minor_id=0 AND ep.name='MS_Description'
    WHERE o.type IN ('P','PC','FN','IF','TF','FS','FT','TR','TA') AND o.is_ms_shipped=0 ORDER BY s.name,o.type,o.name"#;

const MSSQL_SEQUENCES: &str = r#"SELECT TOP (1001) s.name,q.name,'SEQUENCE',USER_NAME(COALESCE(q.principal_id,s.principal_id)),
    CONVERT(nvarchar(max),ep.value),NULL,NULL,CONVERT(nvarchar(33),q.create_date,126),CONVERT(nvarchar(33),q.modify_date,126),NULL,NULL,NULL
    FROM sys.sequences q JOIN sys.schemas s ON s.schema_id=q.schema_id
    LEFT JOIN sys.extended_properties ep ON ep.class=1 AND ep.major_id=q.object_id AND ep.minor_id=0 AND ep.name='MS_Description'
    WHERE q.is_ms_shipped=0 ORDER BY s.name,q.name"#;
