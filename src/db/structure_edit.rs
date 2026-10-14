use super::{Session, allowed_column_type, validate_identifier};
use crate::model::{Engine, Table};
use anyhow::{Result, ensure};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StructureChange {
    AddColumn {
        name: String,
        data_type: String,
        size: String,
    },
    AlterColumn {
        name: String,
        data_type: String,
        size: String,
        nullable: bool,
        /// Exact catalog row loaded by the designer, checked again before DDL.
        original_column: Vec<Option<String>>,
    },
    DropColumn {
        name: String,
    },
    CreateIndex {
        name: String,
        columns: Vec<String>,
    },
    DropIndex {
        name: String,
    },
}

fn field(row: &[Option<String>], index: usize) -> &str {
    row.get(index).and_then(Option::as_deref).unwrap_or("")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnEditInfo {
    pub data_type: String,
    pub size: String,
    pub nullable: bool,
}

/// Only simple catalog types whose attributes can be retained are editable.
pub fn column_edit_info(engine: Engine, row: &[Option<String>]) -> Result<ColumnEditInfo> {
    ensure!(
        row.len()
            >= match engine {
                Engine::MySql => 12,
                Engine::Postgres => 11,
                Engine::SqlServer => 16,
            },
        "字段元数据不完整，请刷新表结构"
    );
    match engine {
        Engine::MySql => {
            ensure!(
                field(row, 5).is_empty() && field(row, 8).is_empty(),
                "自增、生成或带扩展属性的字段暂不支持可视化修改"
            );
            ensure!(
                row.get(4).is_some_and(Option::is_none) && field(row, 9).is_empty(),
                "带默认值或注释的 MySQL 字段暂不支持可视化修改，以免丢失原属性；请使用 SQL"
            );
            ensure!(
                field(row, 10).is_empty(),
                "主键或索引字段暂不支持可视化修改；请检查依赖后使用 SQL"
            );
        }
        Engine::Postgres => {
            ensure!(
                field(row, 5).is_empty() && field(row, 6).is_empty(),
                "标识列或生成列暂不支持可视化修改"
            );
        }
        Engine::SqlServer => {
            ensure!(
                field(row, 4).is_empty() && field(row, 5).is_empty(),
                "带默认约束的 SQL Server 字段暂不支持可视化修改；请检查依赖后使用 SQL"
            );
            ensure!(
                field(row, 6).is_empty()
                    && field(row, 8).is_empty()
                    && field(row, 9).is_empty()
                    && field(row, 11) != "1"
                    && field(row, 12) != "1"
                    && field(row, 15) != "1"
                    && (field(row, 13).is_empty() || field(row, 13) == "NOT_APPLICABLE"),
                "计算、标识、稀疏、隐藏或带扩展属性的字段暂不支持可视化修改"
            );
        }
    }
    let original = field(row, 2).trim().to_ascii_uppercase();
    let (base, size) = match original.split_once('(') {
        Some((base, suffix)) => {
            let size = suffix
                .strip_suffix(')')
                .ok_or_else(|| anyhow::anyhow!("该字段类型暂不支持可视化修改：{original}"))?;
            ensure!(
                size.parse::<u32>().is_ok_and(|n| n > 0),
                "该字段类型参数暂不支持可视化修改：{original}"
            );
            (base.trim(), size)
        }
        None => (original.as_str(), ""),
    };
    let base = match base {
        "CHARACTER VARYING" => "VARCHAR",
        "CHARACTER" => "CHAR",
        "TIMESTAMP WITHOUT TIME ZONE" => "TIMESTAMP",
        "TIMESTAMP WITH TIME ZONE" => "TIMESTAMPTZ",
        "TIME WITHOUT TIME ZONE" => "TIME",
        "INTEGER" if engine == Engine::MySql => "INT",
        base => base,
    };
    let data_type = allowed_column_type(engine, base)?;
    let sized = sized_type(data_type);
    ensure!(
        size.is_empty()
            || sized
            || (engine == Engine::MySql && integer_rank(engine, data_type).is_some()),
        "该字段类型参数暂不支持可视化修改：{original}"
    );
    let size = if sized {
        size.to_owned()
    } else {
        String::new()
    };
    // Reuse the exact whitelist and length checks applied to ADD COLUMN.
    editable_type_sql(engine, data_type, &size)?;
    let nullable = match (engine, field(row, 3)) {
        (Engine::MySql, "YES") | (Engine::Postgres, "true") | (Engine::SqlServer, "1") => true,
        (Engine::MySql, "NO") | (Engine::Postgres, "false") | (Engine::SqlServer, "0") => false,
        _ => anyhow::bail!("字段可空状态不完整，请刷新表结构"),
    };
    Ok(ColumnEditInfo {
        data_type: data_type.to_owned(),
        size,
        nullable,
    })
}

fn sized_type(data_type: &str) -> bool {
    matches!(
        data_type,
        "VARCHAR" | "NVARCHAR" | "CHAR" | "NCHAR" | "VARBINARY" | "BINARY"
    )
}

fn integer_rank(engine: Engine, data_type: &str) -> Option<usize> {
    let types: &[&str] = match engine {
        Engine::MySql => &["TINYINT", "SMALLINT", "MEDIUMINT", "INT", "BIGINT"],
        Engine::Postgres => &["SMALLINT", "INTEGER", "BIGINT"],
        Engine::SqlServer => &["TINYINT", "SMALLINT", "INT", "BIGINT"],
    };
    types.iter().position(|ty| *ty == data_type)
}

fn editable_type_sql(engine: Engine, data_type: &str, size: &str) -> Result<String> {
    let table = Table {
        schema: "schema".into(),
        name: "table".into(),
        kind: "BASE TABLE".into(),
    };
    // The ADD generator is the existing authority for type and size safety.
    structure_change_sql(
        engine,
        &table,
        &StructureChange::AddColumn {
            name: "column".into(),
            data_type: data_type.into(),
            size: size.into(),
        },
    )?;
    let ty = allowed_column_type(engine, data_type)?;
    Ok(if sized_type(ty) {
        format!("{ty}({})", size.trim())
    } else {
        ty.to_owned()
    })
}

pub fn structure_change_sql(
    engine: Engine,
    table: &Table,
    change: &StructureChange,
) -> Result<String> {
    validate_identifier(&table.schema, "Schema 名称")?;
    validate_identifier(&table.name, "表名称")?;
    ensure!(
        table.kind.eq_ignore_ascii_case("BASE TABLE"),
        "仅支持普通数据表"
    );
    let qualified = format!(
        "{}.{}",
        engine.quote(&table.schema),
        engine.quote(&table.name)
    );
    Ok(match change {
        StructureChange::AddColumn {
            name,
            data_type,
            size,
        } => {
            validate_identifier(name, "字段名称")?;
            let data_type = allowed_column_type(engine, data_type)?;
            let size = size.trim();
            let sized = matches!(
                data_type,
                "VARCHAR" | "NVARCHAR" | "CHAR" | "NCHAR" | "VARBINARY" | "BINARY"
            );
            ensure!(
                size.is_empty() || sized,
                "该数据类型暂不支持指定长度；请选择不带长度的类型"
            );
            ensure!(
                !matches!(
                    (engine, data_type),
                    (Engine::MySql, "VARCHAR" | "VARBINARY" | "CHAR" | "BINARY")
                ) || size.parse::<u32>().is_ok_and(|n| n <= 255),
                "MySQL 字符或二进制列长度上限按 255 保守限制"
            );
            ensure!(!sized || !size.is_empty(), "该数据类型需要指定长度");
            ensure!(
                !matches!(
                    (engine, data_type),
                    (
                        Engine::MySql,
                        "TEXT" | "TINYTEXT" | "MEDIUMTEXT" | "LONGTEXT" | "BLOB" | "JSON"
                    ) | (Engine::SqlServer, "TEXT" | "NTEXT")
                ),
                "该类型暂不在可视化新增列中支持，请使用 SQL"
            );
            if sized {
                ensure!(
                    size.parse::<u32>().is_ok_and(|n| n > 0 && n <= 8000),
                    "长度必须为 1 至 8000 的整数"
                );
                if engine == Engine::SqlServer {
                    ensure!(
                        size.parse::<u32>().is_ok_and(|n| n
                            <= if matches!(data_type, "NVARCHAR" | "NCHAR") {
                                4000
                            } else {
                                8000
                            }),
                        "SQL Server 字段长度超过类型上限"
                    );
                }
            }
            let ty = if sized {
                format!("{data_type}({size})")
            } else {
                data_type.to_owned()
            };
            let nullable = if engine == Engine::Postgres {
                ""
            } else {
                " NULL"
            };
            format!(
                "ALTER TABLE {qualified} ADD {}{} {ty}{nullable}",
                if engine == Engine::Postgres {
                    "COLUMN "
                } else {
                    ""
                },
                engine.quote(name)
            )
        }
        StructureChange::AlterColumn {
            name,
            data_type,
            size,
            nullable,
            original_column,
        } => {
            validate_identifier(name, "字段名称")?;
            ensure!(
                field(original_column, 1) == name,
                "原始字段与修改目标不一致"
            );
            let original = column_edit_info(engine, original_column)?;
            let requested = allowed_column_type(engine, data_type)?;
            let requested = if engine == Engine::MySql && requested == "INTEGER" {
                "INT"
            } else {
                requested
            };
            let size = size.trim();
            let mut ty = editable_type_sql(engine, requested, size)?;
            let same_type = original.data_type == requested;
            ensure!(
                (same_type
                    && (!sized_type(requested)
                        || size.parse::<u32>()? >= original.size.parse::<u32>()?))
                    || integer_rank(engine, &original.data_type)
                        .zip(integer_rank(engine, requested))
                        .is_some_and(|(old, new)| new > old),
                "可视化修改仅支持同类型长度扩大、整数类型扩大及可空设置；缩短长度或类型转换请使用 SQL"
            );
            if engine == Engine::MySql && same_type && integer_rank(engine, requested).is_some() {
                // Retain legacy integer display width, including tinyint(1).
                ty = field(original_column, 2).to_owned();
            }
            let column = engine.quote(name);
            let null_sql = if *nullable { "NULL" } else { "NOT NULL" };
            match engine {
                Engine::MySql => {
                    let charset = field(original_column, 6);
                    let collation = field(original_column, 7);
                    if !charset.is_empty() {
                        validate_identifier(charset, "字符集")?;
                        ensure!(
                            matches!(requested, "VARCHAR" | "CHAR"),
                            "字符字段仅支持保持原类型并扩大长度；请使用 SQL"
                        );
                        ty.push_str(&format!(" CHARACTER SET {}", engine.quote(charset)));
                    }
                    if !collation.is_empty() {
                        validate_identifier(collation, "排序规则")?;
                        ty.push_str(&format!(" COLLATE {}", engine.quote(collation)));
                    }
                    format!("ALTER TABLE {qualified} MODIFY COLUMN {column} {ty} {null_sql}")
                }
                Engine::Postgres => {
                    let unchanged_type = same_type && original.size == size;
                    let mut changes = Vec::new();
                    if !unchanged_type {
                        let collation = field(original_column, 7);
                        if !collation.is_empty() {
                            ensure!(
                                matches!(requested, "VARCHAR" | "CHAR" | "TEXT"),
                                "此字段排序规则无法应用到所选类型"
                            );
                            // Catalog query produced quote_ident(schema).quote_ident(name).
                            // The full row is validated against a fresh server row before execution.
                            ty.push_str(&format!(" COLLATE {collation}"));
                        }
                        changes.push(format!("ALTER COLUMN {column} TYPE {ty}"));
                    }
                    changes.push(format!(
                        "ALTER COLUMN {column} {} NOT NULL",
                        if *nullable { "DROP" } else { "SET" }
                    ));
                    format!("ALTER TABLE {qualified} {}", changes.join(", "))
                }
                Engine::SqlServer => {
                    let collation = field(original_column, 10);
                    if !collation.is_empty() {
                        validate_identifier(collation, "排序规则")?;
                        ensure!(
                            matches!(requested, "VARCHAR" | "NVARCHAR" | "CHAR" | "NCHAR"),
                            "字符字段仅支持保持原类型并扩大长度；请使用 SQL"
                        );
                        ty.push_str(&format!(" COLLATE {}", engine.quote(collation)));
                    }
                    format!("ALTER TABLE {qualified} ALTER COLUMN {column} {ty} {null_sql}")
                }
            }
        }
        StructureChange::DropColumn { name } => {
            validate_identifier(name, "字段名称")?;
            ensure!(
                engine == Engine::Postgres,
                "MySQL / SQL Server 删除列可能隐式修改其他依赖，暂不支持可视化删除"
            );
            format!("ALTER TABLE {qualified} DROP COLUMN {}", engine.quote(name))
        }
        StructureChange::CreateIndex { name, columns } => {
            validate_identifier(name, "索引名称")?;
            ensure!(
                !columns.is_empty() && columns.len() <= 16,
                "索引需选择 1 至 16 列"
            );
            for (i, column) in columns.iter().enumerate() {
                validate_identifier(column, "索引列")?;
                ensure!(
                    !columns[..i].iter().any(|previous| previous == column),
                    "索引列不能重复"
                );
            }
            format!(
                "CREATE INDEX {} ON {qualified} ({})",
                engine.quote(name),
                columns
                    .iter()
                    .map(|column| engine.quote(column))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        StructureChange::DropIndex { name } => {
            validate_identifier(name, "索引名称")?;
            match engine {
                Engine::MySql => format!("DROP INDEX {} ON {qualified}", engine.quote(name)),
                Engine::Postgres => format!(
                    "DROP INDEX {}.{}",
                    engine.quote(&table.schema),
                    engine.quote(name)
                ),
                Engine::SqlServer => format!("DROP INDEX {} ON {qualified}", engine.quote(name)),
            }
        }
    })
}

impl Session {
    async fn ensure_column_alter_dependencies_safe(
        &mut self,
        table: &Table,
        name: &str,
    ) -> Result<()> {
        let sql = match self.profile.engine {
            Engine::MySql => {
                r#"SELECT 'constraint' FROM information_schema.KEY_COLUMN_USAGE
                WHERE (TABLE_SCHEMA=? AND TABLE_NAME=? AND COLUMN_NAME=?)
                UNION ALL SELECT 'referenced foreign key' FROM information_schema.KEY_COLUMN_USAGE
                WHERE REFERENCED_TABLE_SCHEMA=? AND REFERENCED_TABLE_NAME=? AND REFERENCED_COLUMN_NAME=?
                UNION ALL SELECT 'check constraint' FROM information_schema.TABLE_CONSTRAINTS
                WHERE TABLE_SCHEMA=? AND TABLE_NAME=? AND CONSTRAINT_TYPE='CHECK'
                UNION ALL SELECT 'generated column' FROM information_schema.COLUMNS
                WHERE TABLE_SCHEMA=? AND TABLE_NAME=? AND GENERATION_EXPRESSION<>''
                UNION ALL SELECT 'unsupported storage engine' FROM information_schema.TABLES
                WHERE TABLE_SCHEMA=? AND TABLE_NAME=? AND (ENGINE IS NULL OR ENGINE<>'InnoDB')"#
            }
            Engine::Postgres => {
                r#"SELECT 'dependency'::text FROM pg_attribute a
                JOIN pg_class c ON c.oid=a.attrelid JOIN pg_namespace n ON n.oid=c.relnamespace
                JOIN pg_depend d ON d.refclassid='pg_class'::regclass AND d.refobjid=c.oid AND d.refobjsubid=a.attnum
                WHERE n.nspname=$1 AND c.relname=$2 AND a.attname=$3 AND NOT a.attisdropped
                AND NOT (d.classid='pg_attrdef'::regclass AND EXISTS (SELECT 1 FROM pg_attrdef ad WHERE ad.oid=d.objid AND ad.adrelid=c.oid AND ad.adnum=a.attnum))
                UNION ALL SELECT 'partition or inheritance'::text FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
                WHERE n.nspname=$1 AND c.relname=$2 AND (c.relkind<>'r' OR c.relispartition OR EXISTS (SELECT 1 FROM pg_inherits i WHERE i.inhrelid=c.oid OR i.inhparent=c.oid))
                UNION ALL SELECT 'constraint'::text FROM pg_constraint con JOIN pg_class c ON c.oid=con.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum=ANY(con.conkey)
                WHERE n.nspname=$1 AND c.relname=$2 AND a.attname=$3
                UNION ALL SELECT 'referenced foreign key'::text FROM pg_constraint con JOIN pg_class c ON c.oid=con.confrelid JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum=ANY(con.confkey)
                WHERE n.nspname=$1 AND c.relname=$2 AND a.attname=$3"#
            }
            Engine::SqlServer => {
                r#"SELECT N'dependency' FROM sys.columns c JOIN sys.objects o ON o.object_id=c.object_id JOIN sys.schemas s ON s.schema_id=o.schema_id
                WHERE s.name=@P1 AND o.name=@P2 AND c.name=@P3 AND (
                    c.default_object_id<>0 OR c.rule_object_id<>0 OR c.is_filestream=1 OR c.is_column_set=1 OR c.encryption_type IS NOT NULL
                    OR EXISTS (SELECT 1 FROM sys.foreign_key_columns f WHERE (f.parent_object_id=c.object_id AND f.parent_column_id=c.column_id) OR (f.referenced_object_id=c.object_id AND f.referenced_column_id=c.column_id))
                    OR EXISTS (SELECT 1 FROM sys.check_constraints ck WHERE ck.parent_object_id=c.object_id)
                    OR EXISTS (SELECT 1 FROM sys.computed_columns cc WHERE cc.object_id=c.object_id)
                    OR EXISTS (SELECT 1 FROM sys.stats_columns st WHERE st.object_id=c.object_id AND st.column_id=c.column_id)
                    OR EXISTS (SELECT 1 FROM sys.sql_expression_dependencies d WHERE d.referenced_id=c.object_id AND (d.referenced_minor_id=c.column_id OR d.referenced_minor_id=0))
                    OR EXISTS (SELECT 1 FROM sys.tables t WHERE t.object_id=c.object_id AND (t.temporal_type<>0 OR t.is_memory_optimized=1 OR t.is_filetable=1)))"#
            }
        };
        let output = if self.profile.engine == Engine::MySql {
            self.parameter_query(
                sql,
                &[
                    &table.schema,
                    &table.name,
                    name,
                    &table.schema,
                    &table.name,
                    name,
                    &table.schema,
                    &table.name,
                    &table.schema,
                    &table.name,
                    &table.schema,
                    &table.name,
                ],
            )
            .await?
        } else {
            self.parameter_query(sql, &[&table.schema, &table.name, name])
                .await?
        };
        ensure!(
            output.sets.iter().all(|set| set.rows.is_empty()),
            "字段涉及约束、外键、索引、统计或特殊表依赖，暂不支持可视化修改；请检查依赖后使用 SQL"
        );
        if self.profile.engine == Engine::MySql {
            let mode = self
                .parameter_query("SELECT CAST(@@session.sql_mode AS CHAR)", &[])
                .await?;
            let mode = mode
                .sets
                .first()
                .and_then(|set| set.rows.first())
                .map(|row| field(row, 0))
                .unwrap_or("");
            ensure!(
                mode.split(',')
                    .any(|part| matches!(part.trim(), "STRICT_ALL_TABLES" | "STRICT_TRANS_TABLES")),
                "MySQL 当前会话未启用严格模式；为避免 NOT NULL 修改自动替换已有 NULL，未执行结构修改"
            );
        }
        Ok(())
    }

    pub async fn change_structure(
        &mut self,
        database: &str,
        table: &Table,
        change: &StructureChange,
        expected_sql: &str,
    ) -> Result<()> {
        self.ensure_edit_transaction_safe().await?;
        ensure!(
            self.current_database().await?.as_deref() == Some(database),
            "当前数据库与结构修改目标不符"
        );
        ensure!(
            self.tables()
                .await?
                .iter()
                .any(|item| item.schema == table.schema
                    && item.name == table.name
                    && item.kind.eq_ignore_ascii_case("BASE TABLE")),
            "目标不存在或不是普通数据表"
        );
        let engine = self.profile.engine;
        let sql = structure_change_sql(engine, table, change)?;
        ensure!(sql == expected_sql, "预览 SQL 与当前请求不一致，未执行");
        let structure = self.table_structure(table).await?;
        let columns = &structure.columns.rows;
        let indexes = &structure.indexes.rows;
        match change {
            StructureChange::AddColumn { name, .. } => {
                ensure!(
                    !columns.iter().any(|row| field(row, 1) == name),
                    "字段已存在"
                );
            }
            StructureChange::AlterColumn {
                name,
                original_column,
                ..
            } => {
                let current = columns
                    .iter()
                    .find(|row| field(row, 1) == name)
                    .ok_or_else(|| anyhow::anyhow!("字段不存在，请刷新表结构"))?;
                ensure!(
                    current == original_column,
                    "字段定义已被其他操作修改，未执行；请刷新表结构并重新编辑"
                );
                // Reject dependencies before engines can silently rebuild/drop them.
                // Expression/filtered indexes and CHECK constraints are treated
                // conservatively at table level when the metadata lacks column IDs.
                let indexed = match engine {
                    Engine::MySql => indexes
                        .iter()
                        .any(|row| field(row, 3) == name || !field(row, 10).is_empty()),
                    Engine::Postgres => false, // Exact pg_depend query below covers all index forms.
                    Engine::SqlServer => indexes
                        .iter()
                        .any(|row| field(row, 7) == name || !field(row, 10).is_empty()),
                };
                ensure!(
                    !indexed
                        && !structure
                            .foreign_keys
                            .rows
                            .iter()
                            .any(|row| field(row, 2) == name),
                    "字段涉及索引或外键，暂不支持可视化修改；请检查依赖后使用 SQL"
                );
                self.ensure_column_alter_dependencies_safe(table, name)
                    .await?;
            }
            StructureChange::DropColumn { name } => {
                if engine == Engine::Postgres {
                    let constraints = self.parameter_query(
                        "SELECT con.conname::text FROM pg_constraint con JOIN pg_class c ON c.oid=con.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum=ANY(con.conkey) WHERE n.nspname=$1 AND c.relname=$2 AND a.attname=$3",
                        &[&table.schema, &table.name, name],
                    ).await?;
                    ensure!(
                        constraints.sets.iter().all(|set| set.rows.is_empty()),
                        "字段涉及 CHECK 或其他约束，请使用 SQL 并自行检查依赖"
                    );
                }
                let row = columns
                    .iter()
                    .find(|row| field(row, 1) == name)
                    .ok_or_else(|| anyhow::anyhow!("字段不存在"))?;
                ensure!(columns.len() > 1, "不能删除表中最后一个字段");
                let indexed = match engine {
                    Engine::MySql => indexes
                        .iter()
                        .any(|index| field(index, 3) == name || field(index, 10).contains(name)),
                    Engine::Postgres => indexes.iter().any(|index| field(index, 4).contains(name)),
                    Engine::SqlServer => indexes.iter().any(|index| field(index, 7) == name),
                };
                ensure!(
                    !indexed
                        && !structure
                            .foreign_keys
                            .rows
                            .iter()
                            .any(|fk| field(fk, 2) == name),
                    "字段涉及索引或外键，请使用 SQL 并自行检查依赖"
                );
                match engine {
                    Engine::MySql => ensure!(
                        field(row, 8).is_empty()
                            && !field(row, 5).contains("auto_increment")
                            && field(row, 10) != "PRI"
                            && field(row, 10) != "UNI",
                        "生成列或关键字段不支持删除"
                    ),
                    Engine::Postgres => ensure!(
                        field(row, 5).is_empty() && field(row, 6).is_empty(),
                        "标识列或生成列不支持删除"
                    ),
                    Engine::SqlServer => ensure!(
                        field(row, 4).is_empty()
                            && field(row, 6).is_empty()
                            && field(row, 8).is_empty()
                            && field(row, 15) != "1",
                        "默认约束、计算列、标识列或隐藏列不支持删除"
                    ),
                }
                // Other dependencies are left to the engine's own RESTRICT/error behavior.
                // SQL Server default constraints are rejected above, never silently dropped.
            }
            StructureChange::CreateIndex {
                name,
                columns: selected,
            } => {
                ensure!(
                    !indexes.iter().any(|row| field(row, 0) == name),
                    "索引已存在"
                );
                ensure!(
                    selected
                        .iter()
                        .all(|name| columns.iter().any(|row| field(row, 1) == name)),
                    "索引列不存在"
                );
                for name in selected {
                    let row = columns.iter().find(|row| field(row, 1) == name).unwrap();
                    if engine == Engine::MySql {
                        ensure!(
                            !["text", "blob", "tinytext", "mediumtext", "longtext", "json"]
                                .iter()
                                .any(|ty| field(row, 2).to_ascii_lowercase().starts_with(ty)),
                            "该列类型需要前缀索引或不支持普通索引"
                        );
                    }
                }
            }
            StructureChange::DropIndex { name } => {
                let row = indexes
                    .iter()
                    .find(|row| field(row, 0) == name)
                    .ok_or_else(|| anyhow::anyhow!("索引不存在"))?;
                match engine {
                    Engine::MySql => ensure!(
                        name != "PRIMARY" && field(row, 1) == "1",
                        "仅支持删除非唯一、非主键普通索引"
                    ),
                    Engine::Postgres => ensure!(
                        field(row, 1) == "false"
                            && field(row, 2) == "false"
                            && field(row, 12).is_empty(),
                        "约束、主键及唯一索引不支持删除"
                    ),
                    Engine::SqlServer => ensure!(
                        field(row, 2) == "0"
                            && field(row, 3) == "0"
                            && field(row, 4) == "0"
                            && field(row, 1) == "NONCLUSTERED",
                        "仅支持删除非唯一非聚集普通索引"
                    ),
                }
            }
        }
        self.query_internal(&sql).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dialect_sql_quotes_names_and_rejects_unsafe_type_lengths() {
        for (engine, quoted, drop_index) in [
            (
                Engine::MySql,
                "`a``b`.`t``x`",
                "DROP INDEX `i``x` ON `a``b`.`t``x`",
            ),
            (
                Engine::Postgres,
                "\"a`b\".\"t`x\"",
                "DROP INDEX \"a`b\".\"i`x\"",
            ),
            (
                Engine::SqlServer,
                "[a`b].[t`x]",
                "DROP INDEX [i`x] ON [a`b].[t`x]",
            ),
        ] {
            let table = Table {
                schema: "a`b".into(),
                name: "t`x".into(),
                kind: "BASE TABLE".into(),
            };
            let drop_column = structure_change_sql(
                engine,
                &table,
                &StructureChange::DropColumn { name: "c".into() },
            );
            if engine == Engine::Postgres {
                assert_eq!(
                    drop_column.unwrap(),
                    format!("ALTER TABLE {quoted} DROP COLUMN {}", engine.quote("c"))
                );
            } else {
                assert!(drop_column.is_err());
            }
            assert_eq!(
                structure_change_sql(
                    engine,
                    &table,
                    &StructureChange::DropIndex { name: "i`x".into() }
                )
                .unwrap(),
                drop_index
            );
            assert!(
                structure_change_sql(
                    engine,
                    &table,
                    &StructureChange::AddColumn {
                        name: "c".into(),
                        data_type: "INT; DROP TABLE x".into(),
                        size: "".into()
                    }
                )
                .is_err()
            );
            assert!(
                structure_change_sql(
                    engine,
                    &table,
                    &StructureChange::AddColumn {
                        name: "c".into(),
                        data_type: "INT".into(),
                        size: "1);DROP TABLE x".into()
                    }
                )
                .is_err()
            );
        }
    }
}
