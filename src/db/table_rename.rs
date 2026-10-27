use super::{EditRejected, Session, validate_identifier};
use crate::model::{Engine, Table};
use anyhow::{Context, Result, ensure};

pub fn rename_table_sql(engine: Engine, table: &Table, new_name: &str) -> Result<String> {
    validate_identifier(&table.schema, "Schema 名称")?;
    validate_identifier(&table.name, "原表名称")?;
    validate_identifier(new_name, "新表名称")?;
    ensure!(
        table.kind.eq_ignore_ascii_case("BASE TABLE"),
        "只能重命名普通数据表"
    );
    ensure!(table.name != new_name, "新表名称与原表名称相同");
    // PostgreSQL silently truncates long identifiers; never report a different
    // name from the one actually stored by the server.
    ensure!(
        match engine {
            Engine::MySql => new_name.chars().count() <= 64,
            Engine::Postgres => new_name.len() <= 63,
            Engine::SqlServer => new_name.encode_utf16().count() <= 128,
        },
        "新表名称超过数据库支持的长度"
    );
    let source = format!(
        "{}.{}",
        engine.quote(&table.schema),
        engine.quote(&table.name)
    );
    Ok(match engine {
        Engine::MySql => format!(
            "RENAME TABLE {source} TO {}.{};",
            engine.quote(&table.schema),
            engine.quote(new_name)
        ),
        Engine::Postgres => format!("ALTER TABLE {source} RENAME TO {};", engine.quote(new_name)),
        Engine::SqlServer => format!(
            "EXEC sys.sp_rename N'{}', N'{}', N'OBJECT';",
            source.replace('\'', "''"),
            new_name.replace('\'', "''")
        ),
    })
}

impl Session {
    pub async fn rename_table(
        &mut self,
        database: &str,
        table: &Table,
        new_name: &str,
    ) -> Result<()> {
        let sql = rename_table_sql(self.profile.engine, table, new_name)
            .map_err(|error| EditRejected(error.to_string()))?;
        self.ensure_edit_transaction_safe().await?;
        if self.current_database().await?.as_deref() != Some(database) {
            return Err(EditRejected(
                "当前数据库与重命名表的目标数据库不符，请先选择目标数据库".into(),
            )
            .into());
        }
        let tables = self.tables().await?;
        if !tables
            .iter()
            .any(|existing| existing == table && existing.kind.eq_ignore_ascii_case("BASE TABLE"))
        {
            return Err(EditRejected("目标不存在或不是普通数据表，未执行重命名".into()).into());
        }
        if tables
            .iter()
            .any(|existing| existing.schema == table.schema && existing.name == new_name)
        {
            return Err(EditRejected("新表名称已存在，未执行重命名".into()).into());
        }
        self.query_internal(&sql)
            .await
            .context("重命名数据表失败；如连接中断，请刷新目录核实实际表名后再操作")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_sql_quotes_identifiers_and_sqlserver_literals() {
        let table = Table {
            schema: "s`\"]'".into(),
            name: "t`\"]'".into(),
            kind: "BASE TABLE".into(),
        };
        let new_name = "new`\"]'; DROP TABLE other;--";
        assert_eq!(
            rename_table_sql(Engine::MySql, &table, new_name).unwrap(),
            "RENAME TABLE `s``\"]'`.`t``\"]'` TO `s``\"]'`.`new``\"]'; DROP TABLE other;--`;"
        );
        assert_eq!(
            rename_table_sql(Engine::Postgres, &table, new_name).unwrap(),
            "ALTER TABLE \"s`\"\"]'\".\"t`\"\"]'\" RENAME TO \"new`\"\"]'; DROP TABLE other;--\";"
        );
        assert_eq!(
            rename_table_sql(Engine::SqlServer, &table, new_name).unwrap(),
            "EXEC sys.sp_rename N'[s`\"]]''].[t`\"]]'']', N'new`\"]''; DROP TABLE other;--', N'OBJECT';"
        );
    }

    #[test]
    fn rename_rejects_invalid_unchanged_view_and_truncated_names() {
        let mut table = Table {
            schema: "public".into(),
            name: "items".into(),
            kind: "BASE TABLE".into(),
        };
        for engine in Engine::ALL {
            for name in ["", "  ", "items", "bad\0name"] {
                assert!(rename_table_sql(engine, &table, name).is_err());
            }
            table.kind = "VIEW".into();
            assert!(rename_table_sql(engine, &table, "renamed").is_err());
            table.kind = "BASE TABLE".into();
        }
        assert!(rename_table_sql(Engine::Postgres, &table, &"表".repeat(22)).is_err());
        assert!(rename_table_sql(Engine::Postgres, &table, &"表".repeat(21)).is_ok());
        assert!(rename_table_sql(Engine::MySql, &table, &"a".repeat(65)).is_err());
        assert!(rename_table_sql(Engine::SqlServer, &table, &"a".repeat(129)).is_err());
    }
}
