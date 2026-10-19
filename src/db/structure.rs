use super::{Session, TableStructure};
use crate::model::{Engine, ResultSet, Table};
use anyhow::{Context, Result};

fn mysql_index_sql(visible: bool, expression: bool) -> String {
    let visible = if visible {
        "CAST(IS_VISIBLE AS CHAR)"
    } else {
        "NULL"
    };
    let expression = if expression {
        "CAST(EXPRESSION AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci"
    } else {
        "NULL"
    };
    format!(
        r#"SELECT CAST(INDEX_NAME AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci, CAST(NON_UNIQUE AS CHAR),
        CAST(SEQ_IN_INDEX AS CHAR), CAST(COLUMN_NAME AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci, CAST(COLLATION AS CHAR),
        CAST(SUB_PART AS CHAR), CAST(NULLABLE AS CHAR), CAST(INDEX_TYPE AS CHAR), CAST(INDEX_COMMENT AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
        {visible} AS visible, {expression} AS expression
        FROM information_schema.STATISTICS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY INDEX_NAME, SEQ_IN_INDEX"#
    )
}

impl Session {
    async fn metadata_set(
        &mut self,
        table: &Table,
        sql: &str,
        names: &[&str],
    ) -> Result<ResultSet> {
        // Metadata is not clipped to ROW_LIMIT: silently truncating indexes or
        // foreign keys would falsely present an incomplete table definition.
        let mut output = self
            .parameter_query(sql, &[&table.schema, &table.name])
            .await?;
        let mut set = output.sets.remove(0);
        set.columns = names.iter().map(|s| (*s).to_owned()).collect();
        Ok(set)
    }

    pub async fn table_structure(&mut self, table: &Table) -> Result<TableStructure> {
        // Also verifies visibility/existence before constructing identifiers.
        self.table_columns(table).await?;
        let mut structure = match self.profile.engine {
            Engine::MySql => self.mysql_structure(table).await?,
            Engine::Postgres => self.postgres_structure(table).await?,
            Engine::SqlServer => self.sqlserver_structure(table).await?,
        };
        let sql = match self.profile.engine {
            Engine::MySql => {
                "SELECT CAST(CONSTRAINT_NAME AS CHAR CHARACTER SET utf8mb4), CAST(CONSTRAINT_TYPE AS CHAR) FROM information_schema.TABLE_CONSTRAINTS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY CONSTRAINT_NAME"
            }
            Engine::Postgres => {
                "SELECT con.conname::text, CASE con.contype WHEN 'p' THEN 'PRIMARY KEY' WHEN 'u' THEN 'UNIQUE' WHEN 'f' THEN 'FOREIGN KEY' WHEN 'c' THEN 'CHECK' ELSE con.contype::text END, pg_get_constraintdef(con.oid,true)::text FROM pg_constraint con JOIN pg_class c ON c.oid=con.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2 ORDER BY con.conname"
            }
            Engine::SqlServer => {
                "SELECT o.name, CASE o.type WHEN 'PK' THEN N'PRIMARY KEY' WHEN 'UQ' THEN N'UNIQUE' WHEN 'F' THEN N'FOREIGN KEY' WHEN 'C' THEN N'CHECK' ELSE o.type_desc END, CASE WHEN o.type='C' THEN OBJECT_DEFINITION(o.object_id) END FROM sys.objects o JOIN sys.objects t ON t.object_id=o.parent_object_id JOIN sys.schemas s ON s.schema_id=t.schema_id WHERE s.name=@P1 AND t.name=@P2 AND o.type IN ('PK','UQ','F','C') ORDER BY o.name"
            }
        };
        let labels: &[&str] = if self.profile.engine == Engine::MySql {
            &["name", "kind"]
        } else {
            &["name", "kind", "definition"]
        };
        structure.constraints = self.metadata_set(table, sql, labels).await?;
        Ok(structure)
    }

    async fn mysql_structure(&mut self, table: &Table) -> Result<TableStructure> {
        let columns = self.metadata_set(table, r#"SELECT
            CAST(ORDINAL_POSITION AS CHAR), CAST(COLUMN_NAME AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
            CAST(COLUMN_TYPE AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci, CAST(IS_NULLABLE AS CHAR),
            CAST(COLUMN_DEFAULT AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci, CAST(EXTRA AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
            CAST(CHARACTER_SET_NAME AS CHAR), CAST(COLLATION_NAME AS CHAR),
            CAST(GENERATION_EXPRESSION AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci, CAST(COLUMN_COMMENT AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
            CAST(COLUMN_KEY AS CHAR), CAST(PRIVILEGES AS CHAR)
            FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY ORDINAL_POSITION"#,
            &["ordinal", "name", "type", "nullable", "default", "extra", "character_set", "collation", "generation_expression", "comment", "key", "privileges"]).await?;
        // Optional STATISTICS fields differ across MySQL releases and compatible servers.
        // Probe the actual catalog rather than guessing capabilities from a version string.
        let capabilities = self.parameter_query(
            "SELECT CAST(COLUMN_NAME AS CHAR) FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME=?",
            &["information_schema", "STATISTICS"],
        ).await?;
        let has_column = |name: &str| {
            capabilities
                .sets
                .iter()
                .flat_map(|set| &set.rows)
                .filter_map(|row| row.first().and_then(Option::as_deref))
                .any(|column| column.eq_ignore_ascii_case(name))
        };
        let index_sql = mysql_index_sql(has_column("IS_VISIBLE"), has_column("EXPRESSION"));
        let indexes = self
            .metadata_set(
                table,
                &index_sql,
                &[
                    "name",
                    "non_unique",
                    "ordinal",
                    "column",
                    "direction",
                    "prefix_length",
                    "nullable",
                    "type",
                    "comment",
                    "visible",
                    "expression",
                ],
            )
            .await?;
        let foreign_keys = self.metadata_set(table, r#"SELECT CAST(k.CONSTRAINT_NAME AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci, CAST(k.ORDINAL_POSITION AS CHAR),
            CAST(k.COLUMN_NAME AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci, CAST(k.REFERENCED_TABLE_SCHEMA AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
            CAST(k.REFERENCED_TABLE_NAME AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci, CAST(k.REFERENCED_COLUMN_NAME AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci,
            CAST(r.UPDATE_RULE AS CHAR), CAST(r.DELETE_RULE AS CHAR), CAST(r.MATCH_OPTION AS CHAR)
            FROM information_schema.KEY_COLUMN_USAGE k JOIN information_schema.REFERENTIAL_CONSTRAINTS r
            ON r.CONSTRAINT_SCHEMA=k.CONSTRAINT_SCHEMA AND r.TABLE_NAME=k.TABLE_NAME AND r.CONSTRAINT_NAME=k.CONSTRAINT_NAME
            WHERE k.TABLE_SCHEMA=? AND k.TABLE_NAME=? AND k.REFERENCED_TABLE_NAME IS NOT NULL ORDER BY k.CONSTRAINT_NAME,k.ORDINAL_POSITION"#,
            &["name", "ordinal", "column", "referenced_schema", "referenced_table", "referenced_column", "on_update", "on_delete", "match"]).await?;
        let sql = format!(
            "SHOW CREATE TABLE {}.{}",
            Engine::MySql.quote(&table.schema),
            Engine::MySql.quote(&table.name)
        );
        let output = self.query_internal(&sql).await?;
        let ddl = output
            .sets
            .first()
            .and_then(|s| s.rows.first())
            .and_then(|r| r.get(1))
            .and_then(Option::as_ref)
            .context("SHOW CREATE 未返回定义")?
            .clone();
        Ok(TableStructure {
            constraints: ResultSet::default(),
            columns,
            indexes,
            foreign_keys,
            ddl,
        })
    }

    async fn postgres_structure(&mut self, table: &Table) -> Result<TableStructure> {
        let columns = self.metadata_set(table, r#"SELECT a.attnum::text, a.attname::text, format_type(a.atttypid,a.atttypmod)::text,
            (NOT a.attnotnull)::text, pg_get_expr(d.adbin,d.adrelid)::text, a.attidentity::text, a.attgenerated::text,
            CASE WHEN a.attcollation<>0 THEN quote_ident(cn.nspname)||'.'||quote_ident(co.collname) END,
            col_description(a.attrelid,a.attnum)::text, a.attstorage::text, a.attstattarget::text
            FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid JOIN pg_namespace n ON n.oid=c.relnamespace
            LEFT JOIN pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum
            LEFT JOIN pg_collation co ON co.oid=a.attcollation LEFT JOIN pg_namespace cn ON cn.oid=co.collnamespace
            WHERE n.nspname=$1 AND c.relname=$2 AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum"#,
            &["ordinal", "name", "type", "nullable", "default_or_generation", "identity", "generated", "collation", "comment", "storage", "statistics_target"]).await?;
        let indexes = self.metadata_set(table, r#"SELECT ic.relname::text, i.indisprimary::text, i.indisunique::text, am.amname::text,
            pg_get_indexdef(i.indexrelid)::text, pg_get_expr(i.indpred,i.indrelid)::text,
            i.indisvalid::text, i.indisready::text, i.indisclustered::text, i.indisreplident::text,
            ts.spcname::text, array_to_string(ic.reloptions,',')::text, con.conname::text
            FROM pg_index i JOIN pg_class c ON c.oid=i.indrelid JOIN pg_namespace n ON n.oid=c.relnamespace
            JOIN pg_class ic ON ic.oid=i.indexrelid JOIN pg_am am ON am.oid=ic.relam
            LEFT JOIN pg_tablespace ts ON ts.oid=ic.reltablespace LEFT JOIN pg_constraint con ON con.conindid=i.indexrelid AND con.contype IN ('p','u','x')
            WHERE n.nspname=$1 AND c.relname=$2 ORDER BY ic.relname"#,
            &["name", "primary", "unique", "method", "definition", "predicate", "valid", "ready", "clustered", "replica_identity", "tablespace", "options", "constraint"]).await?;
        let foreign_keys = self.metadata_set(table, r#"SELECT con.conname::text, k.position::text, a.attname::text, rn.nspname::text,
            rc.relname::text, ra.attname::text, pg_get_constraintdef(con.oid,true)::text,
            con.condeferrable::text, con.condeferred::text, con.convalidated::text
            FROM pg_constraint con JOIN pg_class c ON c.oid=con.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace
            JOIN pg_class rc ON rc.oid=con.confrelid JOIN pg_namespace rn ON rn.oid=rc.relnamespace
            CROSS JOIN LATERAL unnest(con.conkey,con.confkey) WITH ORDINALITY k(local_num,remote_num,position)
            JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum=k.local_num
            JOIN pg_attribute ra ON ra.attrelid=rc.oid AND ra.attnum=k.remote_num
            WHERE n.nspname=$1 AND c.relname=$2 AND con.contype='f' ORDER BY con.conname,k.position"#,
            &["name", "ordinal", "column", "referenced_schema", "referenced_table", "referenced_column", "definition", "deferrable", "initially_deferred", "validated"]).await?;
        let definitions = self.metadata_set(table, r#"SELECT con.conname::text, pg_get_constraintdef(con.oid,true)::text
            FROM pg_constraint con JOIN pg_class c ON c.oid=con.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace
            WHERE n.nspname=$1 AND c.relname=$2 AND con.contype IN ('p','u','c','f','x') ORDER BY con.conname"#, &["name", "definition"]).await?;
        let relation = self.metadata_set(table, r#"SELECT c.relkind::text, CASE WHEN c.relkind IN ('v','m') THEN pg_get_viewdef(c.oid,true) END
            FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2"#, &["kind", "view_definition"]).await?;
        let name = qualified(Engine::Postgres, table);
        let mut ddl = "-- Reconstructed PostgreSQL DDL, NOT a lossless original definition.\n-- Rebuild limitations: sequence ownership/options/current values, identity options, partitions/inheritance,\n-- relation options/tablespaces, storage/compression, grants/owner, comments, triggers, RLS/policies,\n-- replica identity, dependencies and extensions require separate review/export (use pg_dump).\n".to_owned();
        let kind = relation.rows.first().map(|r| field(r, 0)).unwrap_or("");
        if matches!(kind, "v" | "m") {
            ddl.push_str(&format!(
                "CREATE {}VIEW {name} AS\n{};\n",
                if kind == "m" { "MATERIALIZED " } else { "" },
                field(&relation.rows[0], 1).trim_end_matches(';')
            ));
        } else {
            let mut parts = Vec::new();
            for row in &columns.rows {
                let mut part = format!(
                    "  {} {}",
                    Engine::Postgres.quote(field(row, 1)),
                    field(row, 2)
                );
                if !field(row, 7).is_empty() {
                    part.push_str(&format!(" COLLATE {}", field(row, 7)));
                }
                if !field(row, 6).is_empty() {
                    part.push_str(&format!(
                        " GENERATED ALWAYS AS ({}) {}",
                        field(row, 4),
                        if field(row, 6) == "v" {
                            "VIRTUAL"
                        } else {
                            "STORED"
                        }
                    ));
                } else if !field(row, 5).is_empty() {
                    part.push_str(if field(row, 5) == "a" {
                        " GENERATED ALWAYS AS IDENTITY"
                    } else {
                        " GENERATED BY DEFAULT AS IDENTITY"
                    });
                } else if !field(row, 4).is_empty() {
                    part.push_str(&format!(" DEFAULT {}", field(row, 4)));
                }
                if field(row, 3) == "false" {
                    part.push_str(" NOT NULL");
                }
                parts.push(part);
            }
            for row in &definitions.rows {
                parts.push(format!(
                    "  CONSTRAINT {} {}",
                    Engine::Postgres.quote(field(row, 0)),
                    field(row, 1)
                ));
            }
            ddl.push_str(&format!(
                "CREATE TABLE {name} (\n{}\n);\n",
                parts.join(",\n")
            ));
        }
        for row in &indexes.rows {
            if field(row, 12).is_empty() {
                ddl.push_str(field(row, 4));
                ddl.push_str(";\n");
            }
        }
        Ok(TableStructure {
            constraints: ResultSet::default(),
            columns,
            indexes,
            foreign_keys,
            ddl,
        })
    }

    async fn sqlserver_structure(&mut self, table: &Table) -> Result<TableStructure> {
        let columns = self.metadata_set(table, r#"SELECT CONVERT(nvarchar(20),c.column_id), c.name,
            CASE WHEN t.is_user_defined=1 THEN QUOTENAME(SCHEMA_NAME(t.schema_id))+'.'+QUOTENAME(t.name) ELSE t.name +
            CASE WHEN t.name IN ('varchar','char','varbinary','binary','nvarchar','nchar') THEN '('+CASE WHEN c.max_length=-1 THEN 'max' ELSE CONVERT(varchar(10),CASE WHEN t.name IN ('nvarchar','nchar') THEN c.max_length/2 ELSE c.max_length END) END+')'
            WHEN t.name IN ('decimal','numeric') THEN '('+CONVERT(varchar(10),c.precision)+','+CONVERT(varchar(10),c.scale)+')'
            WHEN t.name IN ('datetime2','datetimeoffset','time') THEN '('+CONVERT(varchar(10),c.scale)+')' ELSE '' END END,
            CONVERT(nvarchar(1),c.is_nullable), dc.name, dc.definition, cc.definition, CONVERT(nvarchar(1),cc.is_persisted),
            CONVERT(nvarchar(100),ic.seed_value), CONVERT(nvarchar(100),ic.increment_value), c.collation_name,
            CONVERT(nvarchar(1),c.is_rowguidcol), CONVERT(nvarchar(1),c.is_sparse), c.generated_always_type_desc,
            CONVERT(nvarchar(max),ep.value), CONVERT(nvarchar(1),c.is_hidden)
            FROM sys.columns c JOIN sys.objects o ON o.object_id=c.object_id JOIN sys.schemas s ON s.schema_id=o.schema_id
            JOIN sys.types t ON t.user_type_id=c.user_type_id
            LEFT JOIN sys.default_constraints dc ON dc.object_id=c.default_object_id
            LEFT JOIN sys.computed_columns cc ON cc.object_id=c.object_id AND cc.column_id=c.column_id
            LEFT JOIN sys.identity_columns ic ON ic.object_id=c.object_id AND ic.column_id=c.column_id
            LEFT JOIN sys.extended_properties ep ON ep.major_id=c.object_id AND ep.minor_id=c.column_id AND ep.class=1 AND ep.name='MS_Description'
            WHERE s.name=@P1 AND o.name=@P2 ORDER BY c.column_id"#,
            &["ordinal", "name", "type", "nullable", "default_name", "default", "computed", "persisted", "identity_seed", "identity_increment", "collation", "rowguid", "sparse", "generated_always", "comment", "hidden"]).await?;
        let indexes = self.metadata_set(table, r#"SELECT i.name, i.type_desc, CONVERT(nvarchar(1),i.is_unique), CONVERT(nvarchar(1),i.is_primary_key),
            CONVERT(nvarchar(1),i.is_unique_constraint), CONVERT(nvarchar(20),k.index_column_id), CONVERT(nvarchar(20),k.key_ordinal),
            c.name, CONVERT(nvarchar(1),k.is_descending_key), CONVERT(nvarchar(1),k.is_included_column), i.filter_definition,
            CONVERT(nvarchar(1),i.is_disabled), CONVERT(nvarchar(3),i.fill_factor), ds.name,
            CONVERT(nvarchar(1),i.ignore_dup_key), CONVERT(nvarchar(1),i.allow_row_locks), CONVERT(nvarchar(1),i.allow_page_locks)
            FROM sys.indexes i JOIN sys.objects o ON o.object_id=i.object_id JOIN sys.schemas s ON s.schema_id=o.schema_id
            LEFT JOIN sys.index_columns k ON k.object_id=i.object_id AND k.index_id=i.index_id
            LEFT JOIN sys.columns c ON c.object_id=k.object_id AND c.column_id=k.column_id
            LEFT JOIN sys.data_spaces ds ON ds.data_space_id=i.data_space_id
            WHERE s.name=@P1 AND o.name=@P2 AND i.index_id>0 ORDER BY i.index_id,k.index_column_id"#,
            &["name", "type", "unique", "primary", "unique_constraint", "index_ordinal", "key_ordinal", "column", "descending", "included", "filter", "disabled", "fill_factor", "data_space", "ignore_dup_key", "allow_row_locks", "allow_page_locks"]).await?;
        let foreign_keys = self.metadata_set(table, r#"SELECT f.name, CONVERT(nvarchar(20),k.constraint_column_id), c.name, rs.name, ro.name, rc.name,
            f.update_referential_action_desc, f.delete_referential_action_desc, CONVERT(nvarchar(1),f.is_disabled),
            CONVERT(nvarchar(1),f.is_not_trusted), CONVERT(nvarchar(1),f.is_not_for_replication)
            FROM sys.foreign_keys f JOIN sys.objects o ON o.object_id=f.parent_object_id JOIN sys.schemas s ON s.schema_id=o.schema_id
            JOIN sys.foreign_key_columns k ON k.constraint_object_id=f.object_id
            JOIN sys.columns c ON c.object_id=k.parent_object_id AND c.column_id=k.parent_column_id
            JOIN sys.objects ro ON ro.object_id=k.referenced_object_id JOIN sys.schemas rs ON rs.schema_id=ro.schema_id
            JOIN sys.columns rc ON rc.object_id=ro.object_id AND rc.column_id=k.referenced_column_id
            WHERE s.name=@P1 AND o.name=@P2 ORDER BY f.name,k.constraint_column_id"#,
            &["name", "ordinal", "column", "referenced_schema", "referenced_table", "referenced_column", "on_update", "on_delete", "disabled", "not_trusted", "not_for_replication"]).await?;
        let checks = self.metadata_set(table, r#"SELECT cc.name, cc.definition, CONVERT(nvarchar(1),cc.is_not_trusted), CONVERT(nvarchar(1),cc.is_disabled), CONVERT(nvarchar(1),cc.is_not_for_replication)
            FROM sys.check_constraints cc JOIN sys.objects o ON o.object_id=cc.parent_object_id JOIN sys.schemas s ON s.schema_id=o.schema_id
            WHERE s.name=@P1 AND o.name=@P2 ORDER BY cc.name"#, &["name", "definition", "not_trusted", "disabled", "not_for_replication"]).await?;
        let view = self.metadata_set(table, r#"SELECT o.type, m.definition FROM sys.objects o JOIN sys.schemas s ON s.schema_id=o.schema_id
            LEFT JOIN sys.sql_modules m ON m.object_id=o.object_id WHERE s.name=@P1 AND o.name=@P2"#, &["type", "definition"]).await?;
        let name = qualified(Engine::SqlServer, table);
        let q = |s: &str| Engine::SqlServer.quote(s);
        let mut ddl = "-- Reconstructed SQL Server DDL, NOT a lossless original definition.\n-- Rebuild limitations: temporal/ledger/graph/memory-optimized/partition/filegroup/compression options,\n-- columnstore/XML/spatial/fulltext indexes, encryption/masking, rules/bound defaults,\n-- permissions/owner, triggers, extended properties, sequence/type dependencies and identity current values\n-- require separate review/export using SQL Server scripting tools. Disabled/trust/replication state is in metadata.\n".to_owned();
        if view.rows.first().is_some_and(|r| field(r, 0).trim() == "V") {
            ddl.push_str(
                view.rows
                    .first()
                    .map(|r| field(r, 1))
                    .filter(|s| !s.is_empty())
                    .context("无权读取视图定义，或视图已加密")?,
            );
            return Ok(TableStructure {
                constraints: ResultSet::default(),
                columns,
                indexes,
                foreign_keys,
                ddl,
            });
        }
        let mut parts = Vec::new();
        for row in &columns.rows {
            let mut part = format!("  {}", q(field(row, 1)));
            if !field(row, 6).is_empty() {
                part.push_str(&format!(
                    " AS {}{}",
                    field(row, 6),
                    if field(row, 7) == "1" {
                        " PERSISTED"
                    } else {
                        ""
                    }
                ));
            } else {
                part.push_str(&format!(" {}", field(row, 2)));
                if !field(row, 10).is_empty() {
                    part.push_str(&format!(" COLLATE {}", q(field(row, 10))));
                }
                if !field(row, 8).is_empty() {
                    part.push_str(&format!(" IDENTITY({},{})", field(row, 8), field(row, 9)));
                }
                if field(row, 11) == "1" {
                    part.push_str(" ROWGUIDCOL");
                }
                if field(row, 12) == "1" {
                    part.push_str(" SPARSE");
                }
                part.push_str(if field(row, 3) == "1" {
                    " NULL"
                } else {
                    " NOT NULL"
                });
                if !field(row, 5).is_empty() {
                    part.push_str(&format!(
                        " CONSTRAINT {} DEFAULT {}",
                        q(field(row, 4)),
                        field(row, 5)
                    ));
                }
            }
            parts.push(part);
        }
        for row in &checks.rows {
            parts.push(format!(
                "  CONSTRAINT {} CHECK {}{}",
                q(field(row, 0)),
                if field(row, 4) == "1" {
                    "NOT FOR REPLICATION "
                } else {
                    ""
                },
                field(row, 1)
            ));
        }
        ddl.push_str(&format!(
            "CREATE TABLE {name} (\n{}\n);\n",
            parts.join(",\n")
        ));
        // Group index rows; all constituent columns and INCLUDE columns remain
        // visible in metadata even for index families we cannot reconstruct.
        let mut names = Vec::new();
        for row in &indexes.rows {
            if !names.contains(&field(row, 0)) {
                names.push(field(row, 0));
            }
        }
        for index_name in names {
            let rows = indexes
                .rows
                .iter()
                .filter(|r| field(r, 0) == index_name)
                .collect::<Vec<_>>();
            let first = rows[0];
            let kind = field(first, 1);
            if !matches!(kind, "CLUSTERED" | "NONCLUSTERED") {
                ddl.push_str(&format!(
                    "-- Unsupported index family: {} ({}); see index metadata.\n",
                    q(index_name).replace(['\r', '\n'], " "),
                    kind
                ));
                continue;
            }
            let mut keys = rows
                .iter()
                .filter(|r| field(r, 6) != "0" && !field(r, 6).is_empty())
                .collect::<Vec<_>>();
            keys.sort_by_key(|r| field(r, 6).parse::<u32>().unwrap_or(0));
            let keys = keys
                .iter()
                .map(|r| {
                    format!(
                        "{} {}",
                        q(field(r, 7)),
                        if field(r, 8) == "1" { "DESC" } else { "ASC" }
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            if field(first, 3) == "1" || field(first, 4) == "1" {
                ddl.push_str(&format!(
                    "ALTER TABLE {name} ADD CONSTRAINT {} {} {kind} ({keys});\n",
                    q(index_name),
                    if field(first, 3) == "1" {
                        "PRIMARY KEY"
                    } else {
                        "UNIQUE"
                    }
                ));
            } else {
                ddl.push_str(&format!(
                    "CREATE {}{kind} INDEX {} ON {name} ({keys})",
                    if field(first, 2) == "1" {
                        "UNIQUE "
                    } else {
                        ""
                    },
                    q(index_name)
                ));
                let include = rows
                    .iter()
                    .filter(|r| field(r, 9) == "1")
                    .map(|r| q(field(r, 7)))
                    .collect::<Vec<_>>();
                if !include.is_empty() {
                    ddl.push_str(&format!(" INCLUDE ({})", include.join(", ")));
                }
                if !field(first, 10).is_empty() {
                    ddl.push_str(&format!(" WHERE {}", field(first, 10)));
                }
                ddl.push_str(";\n");
            }
        }
        let mut names = Vec::new();
        for row in &foreign_keys.rows {
            if !names.contains(&field(row, 0)) {
                names.push(field(row, 0));
            }
        }
        for fk_name in names {
            let rows = foreign_keys
                .rows
                .iter()
                .filter(|r| field(r, 0) == fk_name)
                .collect::<Vec<_>>();
            let first = rows[0];
            let local = rows
                .iter()
                .map(|r| q(field(r, 2)))
                .collect::<Vec<_>>()
                .join(", ");
            let remote = rows
                .iter()
                .map(|r| q(field(r, 5)))
                .collect::<Vec<_>>()
                .join(", ");
            ddl.push_str(&format!("ALTER TABLE {name} WITH {} ADD CONSTRAINT {} FOREIGN KEY ({local}) REFERENCES {}.{} ({remote}) ON UPDATE {} ON DELETE {}{};\n",if field(first,9)=="1" {"NOCHECK"}else{"CHECK"},q(fk_name),q(field(first,3)),q(field(first,4)),field(first,6).replace('_'," "),field(first,7).replace('_'," "),if field(first,10)=="1" {" NOT FOR REPLICATION"}else{""}));
            if field(first, 8) == "1" {
                ddl.push_str(&format!(
                    "ALTER TABLE {name} NOCHECK CONSTRAINT {};\n",
                    q(fk_name)
                ));
            }
        }
        Ok(TableStructure {
            constraints: ResultSet::default(),
            columns,
            indexes,
            foreign_keys,
            ddl,
        })
    }
}
fn field(row: &[Option<String>], i: usize) -> &str {
    row.get(i).and_then(Option::as_deref).unwrap_or("")
}
fn qualified(engine: Engine, table: &Table) -> String {
    format!(
        "{}.{}",
        engine.quote(&table.schema),
        engine.quote(&table.name)
    )
}

#[cfg(test)]
mod tests {
    use super::mysql_index_sql;

    #[test]
    fn mysql_index_query_only_references_available_optional_columns() {
        for visible in [false, true] {
            for expression in [false, true] {
                let sql = mysql_index_sql(visible, expression);
                assert_eq!(sql.contains("CAST(IS_VISIBLE AS CHAR)"), visible);
                assert_eq!(sql.contains("CAST(EXPRESSION AS CHAR"), expression);
                assert_eq!(sql.contains("NULL AS visible"), !visible);
                assert_eq!(sql.contains("NULL AS expression"), !expression);
                assert!(sql.contains("TABLE_SCHEMA=? AND TABLE_NAME=?"));
            }
        }
    }
}
