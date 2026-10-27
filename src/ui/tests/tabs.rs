use super::*;

#[test]
fn table_tabs_bind_their_own_context() {
    let table = table();
    let target = TableTarget {
        connection: Uuid::new_v4(),
        database: "dev".into(),
        schema: table.schema.clone(),
        name: table.name.clone(),
    };
    let mut tabs = vec![QueryTab::new(1)];
    let mut next = 2;
    let (index, _) = open_table_tab(
        &mut tabs,
        &mut next,
        target.clone(),
        Engine::Postgres,
        &table,
    );
    assert_eq!(
        tabs[index].target,
        Some(QueryTarget {
            schema: Some(target.schema.clone()),
            connection: target.connection,
            database: Some(target.database.clone())
        })
    );
    assert!(!tabs[index].bind_for_execution(&QueryTarget {
        schema: None,
        connection: target.connection,
        database: Some("prod".into())
    }));
    assert_eq!(tabs[index].table_target.as_ref().unwrap().database, "dev");
}

#[test]
fn closing_tabs_keeps_the_active_query_and_results() {
    let mut tabs: Vec<_> = (1..=4).map(QueryTab::new).collect();
    let mut active = 2;
    let mut next = 5;
    let active_id = tabs[active].id;
    tabs[active].sql = "SELECT '保留我的查询'".into();
    tabs[active].output = Some(QueryOutput::default());
    apply_tab_action(&mut tabs, &mut active, &mut next, TabAction::Close(0));
    assert_eq!(active, 1);
    assert_eq!(tabs[active].id, active_id);
    assert_eq!(tabs[active].sql, "SELECT '保留我的查询'");
    assert!(tabs[active].output.is_some());
    assert_eq!(next, 5);
    apply_tab_action(&mut tabs, &mut active, &mut next, TabAction::CloseRight(1));
    assert_eq!(tabs.len(), 2);
    assert_eq!(tabs[active].id, active_id);
    apply_tab_action(&mut tabs, &mut active, &mut next, TabAction::CloseOthers(1));
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[active].id, active_id);
    assert_eq!(next, 5);
    apply_tab_action(&mut tabs, &mut active, &mut next, TabAction::Close(0));
    assert!(tabs.is_empty());
    assert_eq!(active, 0);
    assert_eq!(next, 1);
    for action in [
        TabAction::Close(0),
        TabAction::CloseOthers(0),
        TabAction::CloseRight(0),
        TabAction::Select(0),
    ] {
        apply_tab_action(&mut tabs, &mut active, &mut next, action);
        assert!(tabs.is_empty());
        assert_eq!(next, 1);
    }

    apply_tab_action(&mut tabs, &mut active, &mut next, TabAction::New);
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].name, "查询 1");
    assert_eq!(next, 2);
    assert_ne!(tabs[0].id, active_id);
}

#[test]
fn closing_active_tab_selects_a_neighbor() {
    let mut tabs: Vec<_> = (1..=4).map(QueryTab::new).collect();
    let mut active = 3;
    let mut next = 5;
    let neighbor = tabs[2].id;
    apply_tab_action(&mut tabs, &mut active, &mut next, TabAction::Close(3));
    assert_eq!(tabs[active].id, neighbor);
    // Closing to the right of an inactive tab removes the old active tab.
    apply_tab_action(&mut tabs, &mut active, &mut next, TabAction::CloseRight(0));
    assert_eq!(active, 0);
    assert_eq!(tabs.len(), 1);
}

fn tab_workspace_frame(app: &mut App, events: Vec<egui::Event>) -> egui::FullOutput {
    let context = app.context.clone();
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 820.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            app.workspace_shortcuts(ctx);
            app.command_bar(ctx);
            egui::SidePanel::right("test-inspector").show(ctx, |ui| app.object_inspector(ui));
            egui::CentralPanel::default().show(ctx, |ui| app.workspace(ui));
            app.workspace_windows(ctx);
            app.workspace_status();
        },
    )
}

fn click_tab_workspace_label(app: &mut App, label: &str) {
    let output = tab_workspace_frame(app, Vec::new());
    let position = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == label => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing {label}"));
    for pressed in [true, false] {
        tab_workspace_frame(
            app,
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
fn clicking_last_query_close_returns_to_objects_and_restarts_numbering() {
    let mut app = session_test_app();
    install_font(&app.context);
    let target = add_test_session(&mut app, "connection", "database");
    app.selected = Some(target.connection);
    app.tabs[0].name = "查询 175".into();
    app.next_tab = 176;
    tab_workspace_frame(&mut app, Vec::new());
    let output = tab_workspace_frame(&mut app, Vec::new());
    let title = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == "查询 175" => Some(text),
            _ => None,
        })
        .expect("query tab should render");
    let close = egui::pos2(
        title.pos.x - 29.0 + (title.galley.size().x + 58.0).clamp(104.0, 184.0) - 14.0,
        title.pos.y + title.galley.size().y * 0.5,
    );
    for _ in 0..3 {
        for pressed in [true, false] {
            tab_workspace_frame(
                &mut app,
                vec![
                    egui::Event::PointerMoved(close),
                    egui::Event::PointerButton {
                        pos: close,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert!(app.tabs.is_empty());
        assert!(app.browser_visible);
        assert_eq!(app.next_tab, 1);
        assert!(!app.can_execute());
        assert!(!app.can_save_query());
        assert!(!app.save_sql(false));
    }
    app.change_tab(TabAction::New);
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.tabs[0].name, "查询 1");
    assert_eq!(app.tabs[0].target, Some(target));
    assert!(!app.browser_visible);
    tab_workspace_frame(&mut app, Vec::new());
    app.change_tab(TabAction::New);
    assert_eq!(app.tabs[1].name, "查询 2");
    app.change_tab(TabAction::Close(1));
    assert_eq!(app.next_tab, 3);
    app.change_tab(TabAction::Close(0));
    app.change_tab(TabAction::New);
    assert_eq!(app.tabs[0].name, "查询 1");
}

#[test]
fn closing_last_dirty_query_requires_confirmation() {
    let mut app = session_test_app();
    app.tabs[0].sql = "SELECT 'unsaved'".into();
    let id = app.tabs[0].id;
    app.change_tab(TabAction::Close(0));
    assert!(matches!(app.confirm_tab_action, Some(TabAction::Close(0))));
    assert_eq!(app.tabs[0].id, id);
    let action = app.confirm_tab_action.take().unwrap();
    assert!(app.apply_ui_tab_action(action));
    assert!(app.tabs.is_empty());
    assert!(app.browser_visible);
    assert_eq!(app.next_tab, 1);
}

#[test]
fn objects_workspace_can_create_tables_and_open_jobs_without_query_tabs() {
    let mut app = session_test_app();
    let target = add_test_session(&mut app, "connection", "database");
    app.selected = Some(target.connection);
    app.change_tab(TabAction::Close(0));
    app.handle_browser_action(object_browser_ui::BrowserAction::CreateTable {
        connection: target.connection,
        database: target.database.clone().unwrap(),
        schema: None,
    });
    assert!(app.table_create.is_some());
    assert!(app.browser_visible);
    tab_workspace_frame(&mut app, Vec::new());
    tab_workspace_frame(&mut app, Vec::new());
    assert!(app.tabs.is_empty());
    assert_eq!(app.next_tab, 1);

    app.table_create = None;
    click_tab_workspace_label(&mut app, "工具");
    click_tab_workspace_label(&mut app, "SQL 任务调度…");
    assert_eq!(app.tabs.len(), 1);
    assert!(app.tabs[0].jobs.is_some());
    assert!(!app.browser_visible);
    tab_workspace_frame(&mut app, Vec::new());
}
