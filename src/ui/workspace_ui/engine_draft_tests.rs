use super::*;

fn engine_tabs(directory: &std::path::Path) -> Vec<SavedTab> {
    [
        (
            "SQLite · 资料",
            "DROP TABLE keep_me;",
            SavedEngineDocument::Sqlite {
                database_path: directory.join("not-opened.sqlite"),
            },
        ),
        (
            "Oracle · 业务库",
            "UPDATE items SET value = '草稿';",
            SavedEngineDocument::Oracle {
                username: "app_user".into(),
                connect_string: "offline.invalid:1521/service".into(),
                schema: "APP".into(),
            },
        ),
    ]
    .into_iter()
    .map(|(name, sql, document)| SavedTab {
        id: Uuid::new_v4(),
        name: name.into(),
        sql: sql.into(),
        saved_sql: "SELECT 1".into(),
        file_path: Some(directory.join(format!("{name}.sql"))),
        target: None,
        saved_query: None,
        document: Some(document),
    })
    .collect()
}

fn restored_app(directory: &std::path::Path) -> App {
    let mut app = super::super::tests::session_test_app();
    let mut workspace = app.workspace_snapshot();
    workspace.tabs = engine_tabs(directory);
    workspace.active_tab = 1;
    workspace.next_tab = 3;
    WorkspaceStore::new(directory.to_owned())
        .save(&workspace)
        .unwrap();
    app.store = Some(Store {
        directory: directory.to_owned(),
    });
    app.restore_workspace();
    app
}

fn click_save_and_exit(app: &mut App) {
    let context = app.context.clone();
    let mut time = 0.0;
    let mut frame = |events| {
        time += 0.1;
        context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 700.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |context| app.workspace_windows(context),
        )
    };
    frame(vec![]);
    let output = frame(vec![]);
    let position = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "保存草稿并退出" => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
            }
            _ => None,
        })
        .expect("save-and-exit button must be available for native SQL drafts");
    for pressed in [true, false] {
        frame(vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
    }
}

#[test]
fn engine_drafts_save_on_exit_and_restore_with_correct_engine_and_baseline() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = restored_app(directory.path());
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.active_tab, 1);
    assert!(app.tabs[0].local_database.is_some());
    assert!(app.tabs[1].oracle.is_some());
    assert!(
        app.tabs
            .iter()
            .all(|tab| tab.target.is_none() && tab.saved_query.is_none() && tab.sql.is_empty())
    );
    let before = app.workspace_snapshot();
    assert!(app.workspace_matches(&before));
    app.tabs[0]
        .local_database
        .as_mut()
        .unwrap()
        .sql
        .push_str(" -- continued editing");
    assert!(!app.workspace_matches(&before));
    app.confirm_exit = true;
    click_save_and_exit(&mut app);
    assert!(app.allow_exit);
    let saved = app.workspace_store.as_ref().unwrap().load().unwrap();
    assert_eq!(saved.tabs, app.workspace_snapshot().tabs);
    assert_eq!(saved.active_tab, 1);
    assert!(!directory.path().join("not-opened.sqlite").exists());
    let mut reopened = super::super::tests::session_test_app();
    reopened.store = Some(Store {
        directory: directory.path().to_owned(),
    });
    reopened.restore_workspace();
    assert_eq!(reopened.workspace_snapshot().tabs, saved.tabs);
    assert!(reopened.tabs.iter().all(QueryTab::document_dirty));
    assert!(
        reopened.tabs[0]
            .local_database
            .as_ref()
            .unwrap()
            .pending
            .is_none()
    );
    let oracle = reopened.tabs[1].oracle.as_ref().unwrap();
    assert!(!oracle.connected && !oracle.busy && !oracle.may_have_transaction());
    assert!(!reopened.any_busy());
    assert!(reopened.query_sessions.is_empty());
}

#[test]
fn engine_drafts_keep_close_confirmation_and_failed_exit_save_preserves_input() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = restored_app(directory.path());
    for index in [0, 1] {
        app.change_tab(TabAction::Close(index));
        assert!(matches!(app.confirm_tab_action, Some(TabAction::Close(i)) if i == index));
        assert_eq!(app.tabs.len(), 2);
        app.confirm_tab_action = None;
    }
    let original = app.workspace_snapshot();
    let path = directory.path().join("workspace.json");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    app.confirm_exit = true;
    click_save_and_exit(&mut app);
    assert!(!app.allow_exit);
    assert!(app.confirm_exit);
    assert_eq!(app.workspace_snapshot(), original);
    assert!(path.is_dir());
}
