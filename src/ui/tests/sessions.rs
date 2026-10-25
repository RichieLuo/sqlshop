use super::*;

#[test]
fn query_dropdown_changes_target_without_changing_sql_or_other_sessions() {
    let mut app = session_test_app();
    let a = add_test_session(&mut app, "A", "db_a");
    let b = add_test_session(&mut app, "B", "db_b");
    app.selected = Some(a.connection);
    app.tabs[0].target = Some(a.clone());
    app.tabs[0].sql = "SELECT 'keep my draft'".into();
    app.tabs[0].output = Some(QueryOutput::default());
    app.tabs[0].query_started = true;
    app.tabs[0].last_executed_sql = Some("SELECT 'old result'".into());
    app.select_query_target(b.clone());
    assert_eq!(app.execution_target(), Some(b.clone()));
    assert_eq!(app.selected, Some(a.connection));
    assert_eq!(app.tabs[0].sql, "SELECT 'keep my draft'");
    assert!(app.tabs[0].output.is_none());
    assert!(app.tabs[0].last_executed_sql.is_none());
    assert!(!app.tabs[0].query_started);
    assert!(app.can_execute());
    assert_eq!(app.sessions[&a.connection].active_database, a.database);
    let other = QueryTarget {
        schema: None,
        connection: b.connection,
        database: Some("other".into()),
    };
    app.select_query_target(other.clone());
    assert!(app.can_execute());
    assert_eq!(app.sessions[&b.connection].active_database, b.database);
    app.sessions.get_mut(&b.connection).unwrap().busy = true;
    app.select_query_target(a.clone());
    assert_eq!(app.execution_target(), Some(a));
}

#[test]
fn query_dropdown_supports_offline_targets_without_auto_connecting() {
    let mut app = session_test_app();
    let profile = Profile::default();
    let target = QueryTarget {
        schema: None,
        connection: profile.id,
        database: None,
    };
    app.profiles.push(profile);
    app.select_query_target(target.clone());
    assert_eq!(app.execution_target(), Some(target));
    assert!(app.sessions.is_empty());
    assert!(!app.can_execute());
    let context = app.context.clone();
    let _ = context.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| app.workspace_tools(ui));
    });
    assert!(app.sessions.is_empty());
}

#[test]
fn bound_tabs_and_session_busy_are_independent_of_sidebar_focus() {
    let mut app = session_test_app();
    let a = add_test_session(&mut app, "A", "db_a");
    let b = add_test_session(&mut app, "B", "db_b");
    app.selected = Some(a.connection);
    app.tabs[0].target = Some(a.clone());
    let tab_a = app.tabs[0].id;
    app.query_sessions.insert(
        tab_a,
        query_session_ui::QuerySessionState {
            worker: sqlshop::worker::QueryWorker::start(tab_a, || {}),
            profile: app.profiles[0].clone(),
            target: a.clone(),
            busy: true,
            connected: true,
            user_sql: true,
            schemas_target: None,
            schemas: Vec::new(),
            schemas_loading: false,
            schemas_error: None,
            execution: None,
            cancellable: false,
            cancel_requested: false,
            transaction: Default::default(),
            transaction_verified: false,
        },
    );
    app.sessions.get_mut(&a.connection).unwrap().busy = true;
    app.sessions.get_mut(&a.connection).unwrap().running_tab = Some(tab_a);
    app.selected = Some(b.connection);
    assert_eq!(app.execution_target(), Some(a.clone()));
    assert!(!app.can_execute());
    app.browser_visible = true;
    app.browser_scope = Some(object_browser_ui::BrowserScope {
        connection: b.connection,
        database: b.database.clone(),
        schema: None,
    });
    app.change_tab(TabAction::New);
    assert_eq!(app.execution_target(), Some(b.clone()));
    assert!(app.can_execute());
    assert!(app.any_busy());
    app.change_tab(TabAction::Select(0));
    assert_eq!(app.active_tab, 0); // A's execution does not lock tab navigation.
    app.change_tab(TabAction::Close(0));
    assert_eq!(app.tabs.len(), 2); // In-flight tab cannot be discarded.
    app.change_tab(TabAction::Close(1));
    assert_eq!(app.tabs.len(), 1); // Other idle tabs remain closable.
    assert_eq!(app.sessions[&a.connection].tables[0].name, "db_a_table");
    assert_eq!(app.sessions[&b.connection].tables[0].name, "db_b_table");
    app.sessions.get_mut(&a.connection).unwrap().busy = false;
    app.query_sessions.get_mut(&tab_a).unwrap().busy = false;
    assert!(app.can_execute()); // Still executes A even with sidebar B selected.
    app.sessions.get_mut(&a.connection).unwrap().active_database = Some("other".into());
    assert!(app.can_execute()); // Management browsing cannot move this query session.
    assert_eq!(app.execution_target(), Some(a));
}

#[test]
fn out_of_order_results_and_disconnect_only_touch_originating_session() {
    let mut app = session_test_app();
    let a = add_test_session(&mut app, "A", "db_a");
    let b = add_test_session(&mut app, "B", "db_b");
    app.tabs[0].target = Some(a.clone());
    app.tabs.push(QueryTab::new(2));
    app.tabs[1].target = Some(b.clone());
    let ids = [app.tabs[0].id, app.tabs[1].id];
    for (target, id) in [(&a, ids[0]), (&b, ids[1])] {
        let session = app.sessions.get_mut(&target.connection).unwrap();
        session.busy = true;
        session.running_tab = Some(id);
        session.raw_session_used = true;
        app.pending_history.insert(
            id,
            HistoryEntry {
                sql: "SELECT 1".into(),
                target: target.clone(),
                source: id.to_string(),
                timestamp: 1,
                success: false,
            },
        );
    }
    app.selected = Some(b.connection);
    app.active_tab = 1;
    for (target, id, elapsed) in [(&b, ids[1], 2), (&a, ids[0], 1)] {
        app.handle_response(
            target.connection,
            Response::QueryResult {
                tab: id,
                database: target.database.clone(),
                output: QueryOutput {
                    elapsed_ms: elapsed,
                    messages: Vec::new(),
                    ..Default::default()
                },
            },
        );
    }
    assert_eq!(app.tabs[0].output.as_ref().unwrap().elapsed_ms, 1);
    assert_eq!(app.tabs[1].output.as_ref().unwrap().elapsed_ms, 2);
    assert!(
        app.tabs
            .iter()
            .all(|tab| tab.last_executed_sql.as_deref() == Some("SELECT 1"))
    );
    assert_eq!(app.history.len(), 2);
    assert!(app.history.iter().all(|h| h.success));
    assert!(app.pending_history.is_empty());
    app.handle_response(b.connection, Response::Disconnected);
    assert!(app.sessions[&b.connection].connected.is_none());
    assert!(app.sessions[&a.connection].connected.is_some());
    assert!(app.sessions[&a.connection].raw_session_used);
    assert_eq!(app.sessions[&a.connection].active_database, a.database);
    assert_eq!(app.sessions[&a.connection].tables[0].name, "db_a_table");
    assert_eq!(app.tabs[1].target, Some(b));
}

#[test]
fn database_context_isolation_and_transaction_confirmation_keep_connection_id() {
    let mut app = session_test_app();
    let a = add_test_session(&mut app, "A", "db_a");
    let b = add_test_session(&mut app, "B", "db_b");
    app.selected = Some(b.connection);
    app.tabs[0].target = Some(a.clone());
    {
        let session = app.sessions.get_mut(&a.connection).unwrap();
        session.busy = true;
        session.raw_session_used = true;
        session.running_tab = Some(app.tabs[0].id);
    }
    app.handle_response(
        a.connection,
        Response::DatabaseContext(Some("changed_by_sql".into())),
    );
    assert!(app.sessions[&a.connection].busy); // Context notification is not operation completion.
    assert!(app.sessions[&a.connection].tables.is_empty());
    assert_eq!(app.sessions[&b.connection].active_database, b.database);
    assert_eq!(app.tabs[0].target, Some(a.clone())); // Failed batch cannot silently retarget a tab.
    let tab = app.tabs[0].id;
    app.handle_response(
        a.connection,
        Response::OperationError {
            tab,
            message: "failed batch; session retained".into(),
            disconnected: false,
        },
    );
    assert!(!app.sessions[&a.connection].busy);
    app.request_on(
        a.connection,
        Request::SelectDatabase("db_a".into()),
        "switch",
    );
    assert!(
        matches!(app.confirm_request.as_ref(), Some((id, Request::SelectDatabase(db), _)) if *id == a.connection && db == "db_a")
    );
    assert!(app.sessions[&a.connection].raw_session_used); // Warning remains until session really ends.
    assert_eq!(app.sessions[&b.connection].tables[0].name, "db_b_table");
    app.confirm_request = None;
    app.handle_response(
        a.connection,
        Response::Error {
            message: "connection failed".into(),
            disconnected: true,
        },
    );
    assert!(app.sessions[&a.connection].connected.is_none());
    assert!(app.sessions[&b.connection].connected.is_some());
    assert!(!app.sessions[&a.connection].raw_session_used);
}

#[test]
fn offline_sql_binds_once_and_rejects_other_contexts() {
    let mut tab = QueryTab::new(1);
    let target = QueryTarget {
        schema: None,
        connection: Uuid::new_v4(),
        database: Some("dev".into()),
    };
    assert!(tab.bind_for_execution(&target));
    assert!(tab.bind_for_execution(&target));
    for other in [
        QueryTarget {
            schema: None,
            connection: Uuid::new_v4(),
            ..target.clone()
        },
        QueryTarget {
            database: Some("prod".into()),
            ..target.clone()
        },
        QueryTarget {
            database: None,
            ..target.clone()
        },
    ] {
        assert!(!tab.bind_for_execution(&other));
        assert_eq!(tab.target, Some(target.clone()));
    }
}
