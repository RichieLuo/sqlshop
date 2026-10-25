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
            name: "note".into(),
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
        schema: "a`\"]".into(),
        name: "b`\"]".into(),
        kind: "BASE TABLE".into(),
    }
}
#[test]
fn insert_quotes_identifiers_and_binds_null_text_and_default_for_three_engines() {
    for engine in Engine::ALL {
        let names = vec!["id".into(), "note".into()];
        let (sql, params) = insert_statement(
            engine,
            &table(),
            &columns(),
            &names,
            &[
                InsertValue::Default,
                InsertValue::Text("NULL' ; DROP TABLE x".into()),
            ],
        )
        .unwrap();
        assert!(sql.contains(&engine.quote("note")));
        assert!(sql.contains(&engine.quote(&table().name)));
        assert!(!sql.contains("DROP TABLE"));
        assert_eq!(params, vec![Some("NULL' ; DROP TABLE x".into())]);
        let (sql, params) = insert_statement(
            engine,
            &table(),
            &columns(),
            &names,
            &[InsertValue::Default, InsertValue::Null],
        )
        .unwrap();
        assert_eq!(params, vec![None]);
        assert_eq!(sql.matches(&placeholder(engine, 1)).count(), 1);
        assert!(
            insert_statement(
                engine,
                &table(),
                &columns(),
                &["note; DROP".into()],
                &[InsertValue::Null]
            )
            .is_err()
        );
        assert!(
            insert_statement(
                engine,
                &table(),
                &columns(),
                &names,
                &[InsertValue::Null, InsertValue::Null]
            )
            .is_ok()
        );
        assert!(
            insert_statement(
                engine,
                &table(),
                &columns(),
                &["note".into(), "note".into()],
                &[InsertValue::Null, InsertValue::Null]
            )
            .is_err()
        );
    }
}
#[test]
fn batch_conflict_sql_preserves_original_null_and_parameter_order() {
    for engine in Engine::ALL {
        let change = CellUpdate {
            database: "db".into(),
            table: table(),
            primary_keys: vec!["id".into()],
            key_values: vec![Some("7".into())],
            column: "note".into(),
            original: None,
            value: Some("NULL".into()),
        };
        let (sql, params) = update_row_statement(engine, &columns(), &change, &[&change]).unwrap();
        assert_eq!(params, vec![Some("NULL".into()), Some("7".into()), None]);
        assert!(!sql.contains("'NULL'"));
        assert!(sql.contains(&engine.quote("note")));
        assert!(sql.contains(&placeholder(engine, 3)));
        let mut invalid = change.clone();
        invalid.primary_keys = vec!["injected".into()];
        assert!(update_row_statement(engine, &columns(), &change, &[&invalid]).is_err());
        assert!(rejected_update(Ok::<u64, &str>(0)).0.contains("修改未保存"));
    }
}

#[test]
fn sqlserver_rejects_full_binary_and_utf16_values_before_preparing_writes() {
    let mut column = columns()[1].clone();
    for (kind, sql_type, oversized, fitting) in [
        (CellKind::Text, "nvarchar(5)", "😀😀😀", "😀😀x"),
        (CellKind::Text, "varchar(5)", "abcdef", "abcde"),
        (CellKind::Binary, "varbinary(2)", "0x010203", "0x0102"),
        (CellKind::Binary, "binary(2)", "0x010203", "0x01"),
    ] {
        column.kind = kind;
        column.sql_type = sql_type.into();
        column.declared_type = sql_type.into();
        let columns = [column.clone()];
        let names = [column.name.clone()];
        assert!(
            insert_statement(
                Engine::SqlServer,
                &table(),
                &columns,
                &names,
                &[InsertValue::Text(oversized.into())]
            )
            .is_err()
        );
        assert!(
            insert_statement(
                Engine::SqlServer,
                &table(),
                &columns,
                &names,
                &[InsertValue::Text(fitting.into())]
            )
            .is_ok()
        );
    }
}
