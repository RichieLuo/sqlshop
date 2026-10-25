use super::*;

#[test]
fn completion_columns_require_matching_bound_connected_database_and_generation() {
    let mut app = session_test_app();
    let a = add_test_session(&mut app, "A", "db_a");
    let b = add_test_session(&mut app, "B", "db_b");
    app.tabs[0].target = Some(a.clone());
    let table = app.sessions[&a.connection].tables[0].clone();
    let session = app.sessions.get_mut(&a.connection).unwrap();
    session.column_cache.insert(
        (table.schema.clone(), table.name.clone()),
        vec!["私有字段".into()],
    );
    assert_eq!(app.completion_columns(), ["私有字段"]);
    app.tabs[0].target = Some(b.clone());
    assert!(app.completion_columns().is_empty());
    app.tabs[0].target = Some(a.clone());
    app.sessions.get_mut(&a.connection).unwrap().active_database = Some("other".into());
    assert!(app.completion_columns().is_empty());
    let session = app.sessions.get_mut(&a.connection).unwrap();
    session.active_database = a.database.clone();
    let old_generation = session.completion_generation;
    session.clear_columns();
    assert_ne!(session.completion_generation, old_generation);
    assert!(app.completion_columns().is_empty());
    app.sessions.get_mut(&a.connection).unwrap().connected = None;
    assert!(app.completion_columns().is_empty());
}

#[test]
fn semantic_completion_metadata_is_scoped_to_bound_live_database() {
    let mut app = session_test_app();
    let a = add_test_session(&mut app, "A", "db_a");
    let b = add_test_session(&mut app, "B", "db_b");
    app.tabs[0].target = Some(a.clone());
    let session = app.sessions.get_mut(&a.connection).unwrap();
    session.column_cache.insert(
        ("public".into(), "db_a_table".into()),
        vec!["private_column".into()],
    );
    let metadata = app.semantic_completion_tables();
    assert_eq!(metadata[0].name, "db_a_table");
    assert_eq!(metadata[0].columns, ["private_column"]);
    app.tabs[0].target = Some(b);
    assert_eq!(app.semantic_completion_tables()[0].name, "db_b_table");
    assert!(app.semantic_completion_tables()[0].columns.is_empty());
    app.tabs[0].target = Some(a.clone());
    app.sessions.get_mut(&a.connection).unwrap().active_database = Some("other".into());
    assert!(app.semantic_completion_tables().is_empty());
    let session = app.sessions.get_mut(&a.connection).unwrap();
    session.active_database = a.database.clone();
    session.clear_columns();
    assert!(app.semantic_completion_tables()[0].columns.is_empty());
    app.sessions.get_mut(&a.connection).unwrap().connected = None;
    assert!(app.semantic_completion_tables().is_empty());
}

#[test]
fn completion_tables_require_matching_bound_connected_database_and_schema() {
    let mut app = session_test_app();
    let a = add_test_session(&mut app, "A", "db_a");
    let b = add_test_session(&mut app, "B", "db_b");
    app.tabs[0].target = Some(a.clone());
    assert_eq!(app.completion_tables(), ["db_a_table"]);
    app.tabs[0].target = Some(b.clone());
    assert_eq!(app.completion_tables(), ["db_b_table"]);
    app.tabs[0].target = Some(a.clone());
    app.sessions.get_mut(&a.connection).unwrap().active_database = Some("other".into());
    assert!(app.completion_tables().is_empty());
    let session = app.sessions.get_mut(&a.connection).unwrap();
    session.active_database = a.database.clone();
    session.tables.push(Table {
        schema: "other_schema".into(),
        name: "db_a_table".into(),
        kind: "BASE TABLE".into(),
    });
    // The same table name in two schemas is offered once.
    assert_eq!(app.completion_tables(), ["db_a_table"]);
    app.tabs[0].target.as_mut().unwrap().schema = Some("other_schema".into());
    assert_eq!(app.completion_tables(), ["db_a_table"]);
    app.tabs[0].target.as_mut().unwrap().schema = Some("missing".into());
    assert!(app.completion_tables().is_empty());
    app.sessions.get_mut(&a.connection).unwrap().connected = None;
    assert!(app.completion_tables().is_empty());
}

#[test]
fn execution_scope_preserves_target_and_confirms_only_the_requested_sql() {
    let mut app = session_test_app();
    let a = add_test_session(&mut app, "A", "db_a");
    let b = add_test_session(&mut app, "B", "db_b");
    app.selected = Some(b.connection);
    app.tabs[0].target = Some(a.clone());
    let first = "UPDATE items SET note = '中文;值' WHERE id = 1;";
    let second = "DELETE FROM items WHERE id = 2;";
    let sql = format!("{first}\n{second}");
    app.tabs[0].sql = sql.clone();
    app.tabs[0].selection = first.into();
    app.tabs[0].cursor = first.chars().count() + 5;
    app.tabs[0].output = Some(QueryOutput::default());

    for (scope, expected) in [
        (SqlExecutionScope::CurrentStatement, second),
        (SqlExecutionScope::All, sql.as_str()),
        (SqlExecutionScope::SelectionOrAll, first),
    ] {
        app.execute_scope(scope);
        let pending = app.confirm_sql.take().expect("writes require confirmation");
        assert_eq!(pending.sql, expected);
        assert_eq!(pending.target, a);
        assert_eq!(pending.connection_name, "A");
        assert!(app.tabs[0].output.is_some());
        assert!(app.pending_history.is_empty());
        assert!(!app.sessions[&a.connection].busy);
        assert!(!app.sessions[&b.connection].busy);
    }
    assert_eq!(app.tabs[0].sql, sql);
    assert_eq!(app.tabs[0].selection, first);
}

#[test]
fn current_statement_shortcut_does_not_fall_back_to_full_document_or_edit_sql() {
    let mut app = session_test_app();
    let a = add_test_session(&mut app, "A", "db_a");
    app.sessions.get_mut(&a.connection).unwrap().tables.clear();
    app.tabs[0].target = Some(a);
    let sql = "UPDATE items SET note = '中文;值' WHERE id = 1;\nDELETE FROM items WHERE id = 2;";
    let cursor = sql.find("DELETE").unwrap();
    app.tabs[0].sql = sql.into();
    app.tabs[0].select_range = Some(cursor..cursor);
    let ctx = app.context.clone();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| app.workspace(ui));
    });
    let modifiers = egui::Modifiers {
        shift: true,
        ..egui::Modifiers::COMMAND
    };
    let _ = ctx.run(
        egui::RawInput {
            modifiers,
            events: vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.workspace(ui));
        },
    );
    assert_eq!(app.tabs[0].sql, sql);
    assert_eq!(
        app.confirm_sql.as_ref().unwrap().sql,
        "DELETE FROM items WHERE id = 2;"
    );
    assert!(app.pending_history.is_empty());
}

#[test]
fn rejected_current_statement_keeps_results_and_does_not_dispatch() {
    let mut app = session_test_app();
    let a = add_test_session(&mut app, "A", "db_a");
    app.tabs[0].target = Some(a.clone());
    app.tabs[0].output = Some(QueryOutput::default());
    for sql in [
        "-- 只有注释",
        "SELECT 'unterminated",
        "DELIMITER $$\nSELECT 1$$",
    ] {
        app.tabs[0].sql = sql.into();
        app.tabs[0].cursor = 0;
        app.execute_scope(SqlExecutionScope::CurrentStatement);
        assert!(app.error, "{sql}");
        assert!(app.confirm_sql.is_none());
        assert!(app.tabs[0].output.is_some());
        assert!(app.pending_history.is_empty());
        assert!(!app.sessions[&a.connection].busy);
    }
}

#[test]
fn executed_sql_footer_handles_empty_and_long_parameterized_sql() {
    let mut app = session_test_app();
    let context = app.context.clone();
    for sql in [
        None,
        Some(format!(
            "SELECT {}\n-- 1: \"罗子言\"",
            "column, ".repeat(300)
        )),
    ] {
        app.tabs[0].last_executed_sql = sql;
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 480.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::TopBottomPanel::bottom("test-sql-footer").show(ctx, |ui| {
                    app.executed_sql_footer(ui);
                    assert!(ui.min_rect().height() < 100.0);
                    assert!(ui.min_rect().width() <= 640.0);
                });
            },
        );
    }
}

#[test]
fn executed_table_sql_is_independent_of_editor_and_failed_refresh() {
    let mut app = session_test_app();
    let target = add_test_session(&mut app, "A", "db_a");
    let id = app.tabs[0].id;
    app.tabs[0].sql = "SELECT 'unsaved draft'".into();
    let page = sqlshop::db::TablePage {
        data: TableData {
            output: QueryOutput::default(),
            columns: vec![],
            primary_keys: vec![],
            writable: false,
            keys: vec![],
        },
        has_more: false,
        sql: "SELECT * FROM items WHERE name = $1 LIMIT 201 OFFSET 200".into(),
        parameters: vec!["罗子言".into()],
    };
    let expected = page.execution_sql();
    app.tabs.push(QueryTab::new(2));
    app.active_tab = 1;
    app.handle_response(target.connection, Response::TablePage { tab: id, page });
    assert_eq!(
        app.tabs[0].last_executed_sql.as_deref(),
        Some(expected.as_str())
    );
    assert_eq!(app.tabs[0].sql, "SELECT 'unsaved draft'");
    assert!(app.tabs[1].last_executed_sql.is_none());
    app.tabs[0].browse.offset = 400;
    app.handle_response(
        target.connection,
        Response::OperationError {
            tab: id,
            message: "failed refresh".into(),
            disconnected: false,
        },
    );
    assert_eq!(
        app.tabs[0].last_executed_sql.as_deref(),
        Some(expected.as_str())
    );
}
