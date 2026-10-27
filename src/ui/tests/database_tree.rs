use super::*;

fn catalog(table_name: &str) -> sqlshop::object_catalog::ObjectCatalog {
    let mut catalog = object_browser_host::example_catalog();
    catalog.objects.truncate(1);
    catalog.objects[0].name = table_name.into();
    catalog
}

fn sidebar_frame(app: &mut App, time: f64, events: Vec<egui::Event>) -> egui::FullOutput {
    app.context.clone().run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 1000.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.sidebar(ui));
        },
    )
}

fn label_position(output: &egui::FullOutput, label: &str) -> Option<egui::Pos2> {
    output.shapes.iter().find_map(|shape| match &shape.shape {
        egui::Shape::Text(text) if text.galley.job.text == label => {
            Some(text.pos + text.galley.size() * 0.5)
        }
        _ => None,
    })
}

fn double_click(app: &mut App, time: f64, position: egui::Pos2) {
    for (index, pressed) in [true, false, true, false].into_iter().enumerate() {
        sidebar_frame(
            app,
            time + index as f64 * 0.05,
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
}

#[test]
fn switching_databases_keeps_both_expanded_table_lists() {
    for engine in Engine::ALL {
        check_expanded_table_lists(engine);
    }
}

fn check_expanded_table_lists(engine: Engine) {
    let mut app = session_test_app();
    let target = add_test_session(&mut app, "server", "db_a");
    let connection = target.connection;
    app.profiles[0].engine = engine;
    app.sessions
        .get_mut(&connection)
        .unwrap()
        .connected
        .as_mut()
        .unwrap()
        .engine = engine;
    app.sessions
        .get_mut(&connection)
        .unwrap()
        .databases
        .push("db_b".into());
    app.handle_response(
        connection,
        Response::ObjectCatalog {
            database: "db_a".into(),
            catalog: catalog("a_table"),
        },
    );
    app.expand_connection = Some(connection);
    app.expand_database = Some((connection, "db_a".into()));
    sidebar_frame(&mut app, 0.0, vec![]);
    let first = sidebar_frame(&mut app, 1.0, vec![]);
    assert!(label_position(&first, "a_table").is_some());

    app.handle_response(connection, Response::DatabaseSelected("db_b".into()));
    app.handle_response(
        connection,
        Response::ObjectCatalog {
            database: "db_b".into(),
            catalog: catalog("b_table"),
        },
    );
    sidebar_frame(&mut app, 2.0, vec![]);
    let switched = sidebar_frame(&mut app, 3.0, vec![]);
    assert!(label_position(&switched, "b_table").is_some());
    assert!(
        label_position(&switched, "a_table").is_some(),
        "opening another database must preserve the previous database's table list"
    );
    assert!(label_position(&switched, "打开数据库").is_none());
    app.filter = "a_table".into();
    sidebar_frame(&mut app, 4.0, vec![]);
    let searched = sidebar_frame(&mut app, 5.0, vec![]);
    assert!(
        label_position(&searched, "a_table").is_some(),
        "search must include tables in previously opened databases"
    );
}

#[test]
fn database_expansion_while_busy_opens_after_current_operation() {
    let mut app = session_test_app();
    let target = add_test_session(&mut app, "server", "db_a");
    let connection = target.connection;
    let session = app.sessions.get_mut(&connection).unwrap();
    session.databases.push("db_b".into());
    session.busy = true;
    app.expand_connection = Some(connection);
    sidebar_frame(&mut app, 0.0, vec![]);
    let output = sidebar_frame(&mut app, 1.0, vec![]);
    let position = label_position(&output, "db_b").unwrap();
    double_click(&mut app, 1.05, position);
    assert_eq!(app.sessions[&connection].active_database, target.database);
    app.sessions.get_mut(&connection).unwrap().busy = false;
    app.poll();
    assert!(
        app.sessions[&connection].busy,
        "explicit expansion must be dispatched once the connection becomes idle"
    );
}

#[test]
fn collapsing_a_waiting_database_cancels_its_open_request() {
    let mut app = session_test_app();
    let target = add_test_session(&mut app, "server", "db_a");
    let connection = target.connection;
    let session = app.sessions.get_mut(&connection).unwrap();
    session.databases.push("db_b".into());
    session.busy = true;
    app.expand_connection = Some(connection);
    sidebar_frame(&mut app, 0.0, vec![]);
    let output = sidebar_frame(&mut app, 1.0, vec![]);
    let position = label_position(&output, "db_b").unwrap();
    double_click(&mut app, 1.05, position);
    let waiting = sidebar_frame(&mut app, 2.0, vec![]);
    assert!(label_position(&waiting, "正在等待当前操作完成…").is_some());
    double_click(&mut app, 2.1, position);
    app.sessions.get_mut(&connection).unwrap().busy = false;
    app.poll();
    assert!(!app.sessions[&connection].busy);
    assert_eq!(app.sessions[&connection].active_database, target.database);
}

#[test]
fn programmatic_database_reveal_does_not_schedule_a_switch() {
    let mut app = session_test_app();
    let target = add_test_session(&mut app, "server", "db_a");
    let connection = target.connection;
    let session = app.sessions.get_mut(&connection).unwrap();
    session.databases.push("db_b".into());
    session.busy = true;
    app.expand_connection = Some(connection);
    app.expand_database = Some((connection, "db_b".into()));
    sidebar_frame(&mut app, 0.0, vec![]);
    sidebar_frame(&mut app, 1.0, vec![]);
    app.sessions.get_mut(&connection).unwrap().busy = false;
    app.poll();
    assert!(!app.sessions[&connection].busy);
    assert_eq!(app.sessions[&connection].active_database, target.database);
}

#[test]
fn waiting_database_switch_preserves_session_confirmation() {
    let mut app = session_test_app();
    let target = add_test_session(&mut app, "server", "db_a");
    let session = app.sessions.get_mut(&target.connection).unwrap();
    session.busy = true;
    session.raw_session_used = true;
    app.open_tree_database(target.connection, "db_b".into());
    app.sessions.get_mut(&target.connection).unwrap().busy = false;
    app.poll_database_opens();
    assert!(matches!(app.confirm_request.as_ref(),
        Some((connection, Request::SelectDatabase(database), _))
            if *connection == target.connection && database == "db_b"));
    assert!(!app.sessions[&target.connection].busy);
    assert_eq!(
        app.sessions[&target.connection].active_database,
        target.database
    );
}

#[test]
fn database_tree_reports_loading_failure_and_empty_catalogs() {
    let mut app = session_test_app();
    let target = add_test_session(&mut app, "server", "db_a");
    let connection = target.connection;
    app.expand_connection = Some(connection);
    app.expand_database = Some((connection, "db_a".into()));
    app.request_on(
        connection,
        Request::ObjectCatalog {
            database: "db_a".into(),
        },
        "loading",
    );
    sidebar_frame(&mut app, 0.0, vec![]);
    let loading = sidebar_frame(&mut app, 1.0, vec![]);
    assert!(label_position(&loading, "正在读取数据库对象…").is_some());
    app.handle_response(
        connection,
        Response::Error {
            message: "permission denied".into(),
            disconnected: false,
        },
    );
    let failed = sidebar_frame(&mut app, 2.0, vec![]);
    assert!(label_position(&failed, "读取失败：permission denied").is_some());
    assert!(label_position(&failed, "重试读取对象").is_some());
    app.handle_response(
        connection,
        Response::ObjectCatalog {
            database: "db_a".into(),
            catalog: Default::default(),
        },
    );
    let empty = sidebar_frame(&mut app, 3.0, vec![]);
    assert!(label_position(&empty, "暂无表或视图").is_some());
    assert!(label_position(&empty, "重试读取对象").is_none());
}

#[test]
fn reconnect_does_not_reuse_previous_connection_table_lists() {
    let mut app = session_test_app();
    let target = add_test_session(&mut app, "server", "db_a");
    let connection = target.connection;
    app.handle_response(
        connection,
        Response::ObjectCatalog {
            database: "db_a".into(),
            catalog: catalog("old_table"),
        },
    );
    app.expand_connection = Some(connection);
    app.expand_database = Some((connection, "db_a".into()));
    sidebar_frame(&mut app, 0.0, vec![]);
    let before = sidebar_frame(&mut app, 1.0, vec![]);
    assert!(label_position(&before, "old_table").is_some());
    app.handle_response(connection, Response::Disconnected);
    app.handle_response(connection, Response::Connected(connection));
    app.handle_response(connection, Response::Databases(vec!["db_a".into()]));
    let after = sidebar_frame(&mut app, 2.0, vec![]);
    assert!(label_position(&after, "old_table").is_none());
    assert!(label_position(&after, "db_a").is_some());
    assert!(
        label_position(&after, "打开数据库").is_none(),
        "a reconnected database must start collapsed instead of restoring an unloaded body"
    );
    let position = label_position(&after, "db_a").unwrap();
    double_click(&mut app, 2.1, position);
    assert!(app.sessions[&connection].busy);
    let opened = sidebar_frame(&mut app, 3.0, vec![]);
    assert!(label_position(&opened, "正在读取数据库对象…").is_some());
}

#[test]
fn restarted_app_does_not_restore_unloaded_database_expansion() {
    let mut app = session_test_app();
    let target = add_test_session(&mut app, "server", "db_a");
    let connection = target.connection;
    app.expand_connection = Some(connection);
    app.expand_database = Some((connection, "db_a".into()));
    sidebar_frame(&mut app, 0.0, vec![]);
    sidebar_frame(&mut app, 1.0, vec![]);

    let mut restarted = session_test_app();
    restarted.context = app.context.clone();
    restarted.profiles = app.profiles.clone();
    let mut session = ConnectionSession::new(&restarted.context);
    session.connected = Some(restarted.profiles[0].clone());
    session.databases = vec!["db_a".into()];
    restarted.sessions.insert(connection, session);
    restarted.expand_connection = Some(connection);
    sidebar_frame(&mut restarted, 2.0, vec![]);
    let restored = sidebar_frame(&mut restarted, 3.0, vec![]);
    assert!(label_position(&restored, "db_a").is_some());
    assert!(
        label_position(&restored, "打开数据库").is_none(),
        "reopening a connection must not restore database bodies without their contents"
    );
    assert!(
        !restarted.sessions[&connection].busy,
        "restoring a saved tree layout must not silently switch databases"
    );
}

#[test]
fn failed_database_open_displays_the_reason_on_its_node() {
    let mut app = session_test_app();
    let target = add_test_session(&mut app, "server", "db_a");
    let connection = target.connection;
    app.sessions
        .get_mut(&connection)
        .unwrap()
        .databases
        .push("db_b".into());
    app.open_tree_database(connection, "db_b".into());
    app.handle_response(
        connection,
        Response::Error {
            message: "database access denied".into(),
            disconnected: false,
        },
    );
    app.expand_connection = Some(connection);
    app.expand_database = Some((connection, "db_b".into()));
    sidebar_frame(&mut app, 0.0, vec![]);
    let failed = sidebar_frame(&mut app, 1.0, vec![]);
    assert!(label_position(&failed, "读取失败：database access denied").is_some());
    assert!(label_position(&failed, "重试打开数据库").is_some());
    assert_eq!(app.sessions[&connection].active_database, target.database);
}

#[test]
fn inactive_database_table_details_report_read_errors_without_switching_databases() {
    let mut app = session_test_app();
    let target = add_test_session(&mut app, "server", "db_a");
    let connection = target.connection;
    app.handle_response(
        connection,
        Response::ObjectCatalog {
            database: "db_a".into(),
            catalog: catalog("a_table"),
        },
    );
    let table = app.sessions[&connection].tables[0].clone();
    let session = app.sessions.get_mut(&connection).unwrap();
    session.active_database = Some("db_b".into());
    session.tables.clear();
    // This fixture has no credential store, so reads must report an error
    // without making a network connection or silently leaving a loading row.
    app.request_tree_details(vec![(connection, "db_a".into(), table.clone(), false)]);
    let entry = app.sessions[&connection]
        .tree_metadata
        .entries
        .get(&table_tree_ui::key("db_a", &table))
        .expect("a visible cached table must reach the metadata reader");
    assert!(!entry.loading);
    assert!(entry.error.is_some());
    assert_eq!(
        app.sessions[&connection].active_database.as_deref(),
        Some("db_b")
    );
}
