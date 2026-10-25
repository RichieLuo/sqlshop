use super::*;

fn library_app(directory: &std::path::Path) -> App {
    let mut app = super::super::tests::session_test_app();
    app.browser_visible = false;
    app.catalog_store = Some(CatalogStore::new(directory.into()));
    app
}

fn save_new_query(app: &mut App, name: &str) -> SavedQuery {
    app.save_query_command(false);
    let mut dialog = app
        .catalog_dialog
        .take()
        .expect("name dialog for new query");
    let CatalogDialog::Query { name: title, .. } = &mut dialog else {
        panic!("query dialog")
    };
    *title = name.into();
    assert!(app.apply_catalog_dialog(&dialog));
    app.catalog.queries.last().unwrap().clone()
}

#[test]
fn named_query_save_update_and_restart_preserve_identity_sql_and_target() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = library_app(dir.path());
    let target = Some(QueryTarget {
        connection: Uuid::new_v4(),
        database: Some("业务库".into()),
        schema: Some("sales".into()),
    });
    app.tabs[0].sql = "SELECT '查询内容';".into();
    app.tabs[0].target = target.clone();
    let original = save_new_query(&mut app, "每日报表");
    assert!(!app.tabs[0].dirty());
    assert!(app.tabs[0].saved_sql.is_empty() && app.tabs[0].file_path.is_none());
    app.tabs[0].sql = "SELECT '更新内容';".into();
    assert!(app.tabs[0].dirty());
    app.save_query_command(false);
    assert!(app.catalog_dialog.is_none());
    assert_eq!(app.catalog.queries.len(), 1);
    assert_eq!(app.catalog.queries[0].id, original.id);
    assert!(!app.tabs[0].dirty());
    let stored = CatalogStore::new(dir.path().into()).load().unwrap();
    assert_eq!(stored.queries[0].sql, "SELECT '更新内容';");
    assert_eq!(stored.queries[0].target, target);

    WorkspaceStore::new(dir.path().into())
        .save(&app.workspace_snapshot())
        .unwrap();
    let mut restored = library_app(dir.path());
    restored.store = Some(Store {
        directory: dir.path().into(),
    });
    restored.restore_workspace();
    restored.restore_catalog();
    assert_eq!(
        restored.tabs[0].saved_query.as_ref().unwrap().id,
        original.id
    );
    assert_eq!(restored.tabs[0].target, target);
    assert!(!restored.tabs[0].dirty());
    assert!(restored.query_sessions.is_empty());
    assert!(!restored.tabs[0].query_started);
    restored.tabs[0].sql = "SELECT '重启后修改';".into();
    restored.save_query_command(false);
    assert_eq!(restored.catalog.queries.len(), 1);
    assert_eq!(restored.catalog.queries[0].sql, "SELECT '重启后修改';");
}

#[test]
fn opening_named_query_reuses_its_editor_and_keeps_unsaved_changes() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = library_app(dir.path());
    app.tabs[0].sql = "SELECT 1".into();
    let saved = save_new_query(&mut app, "常用查询");
    app.tabs[0].sql = "SELECT 2".into();
    app.open_saved_query(saved);
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.tabs[0].sql, "SELECT 2");
    assert!(app.tabs[0].dirty());
    assert!(app.query_sessions.is_empty());
}

#[test]
fn opening_updated_query_uses_current_contents_and_keeps_old_target_as_a_draft() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = library_app(dir.path());
    app.tabs[0].sql = "SELECT 'old'".into();
    app.tabs[0].target = Some(QueryTarget {
        connection: Uuid::new_v4(),
        database: Some("old_db".into()),
        schema: None,
    });
    let old_target = app.tabs[0].target.clone();
    let saved = save_new_query(&mut app, "目标保护");
    let mut newer = QueryTab::new(2);
    newer.sql = "SELECT 'new'".into();
    newer.target = Some(QueryTarget {
        connection: Uuid::new_v4(),
        database: Some("new_db".into()),
        schema: None,
    });
    app.tabs.push(newer);
    app.active_tab = 1;
    app.save_to_library(Some(saved.id));
    let dialog = app.catalog_dialog.take().unwrap();
    assert!(app.apply_catalog_dialog(&dialog));
    assert!(app.tabs[0].saved_query_stale && app.tabs[0].dirty());
    assert_eq!(app.tabs[0].target, old_target);
    app.active_tab = 0;
    app.open_saved_query(app.catalog.queries[0].clone());
    assert_eq!(app.active_tab, 1);
    assert_eq!(app.tabs[1].sql, "SELECT 'new'");
    assert_eq!(
        app.tabs[1].target.as_ref().unwrap().database.as_deref(),
        Some("new_db")
    );
    assert_eq!(app.tabs[0].sql, "SELECT 'old'");
}

#[test]
fn file_save_shortcut_writes_the_file_even_when_also_saved_in_the_library() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = library_app(dir.path());
    app.tabs[0].sql = "SELECT 1".into();
    let named = save_new_query(&mut app, "文件和查询");
    let file = dir.path().join("query.sql");
    app.tabs[0].file_path = Some(file.clone());
    app.tabs[0].sql = "SELECT 2".into();
    app.save_query_command(false);
    assert_eq!(std::fs::read_to_string(file).unwrap(), "SELECT 2");
    assert_eq!(app.catalog.queries[0], named);
    assert!(!app.tabs[0].dirty());
}

#[test]
fn editor_save_shortcut_opens_naming_for_an_untitled_query() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = library_app(dir.path());
    app.tabs[0].sql = "SELECT 1".into();
    let ctx = app.context.clone();
    let modifiers = egui::Modifiers::COMMAND;
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1200.0, 800.0),
        )),
        modifiers,
        events: vec![egui::Event::Key {
            key: egui::Key::S,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }],
        ..Default::default()
    };
    let _ = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| app.workspace_tools(ui));
    });
    assert!(matches!(
        app.catalog_dialog,
        Some(CatalogDialog::Query { id: None, .. })
    ));
    assert!(app.catalog.queries.is_empty());
    assert!(!app.tabs[0].query_started);
}

#[test]
fn stale_named_query_never_overwrites_another_editors_newer_save() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = library_app(dir.path());
    app.tabs[0].sql = "SELECT 1".into();
    let saved = save_new_query(&mut app, "并发编辑");
    app.tabs.push(query_tab_from_saved(saved, Uuid::new_v4()));
    app.tabs[0].sql = "SELECT 2".into();
    app.save_query_command(false);
    app.active_tab = 1;
    app.tabs[1].sql = "SELECT 3".into();
    app.save_query_command(false);
    assert!(app.error);
    assert_eq!(app.catalog.queries[0].sql, "SELECT 2");
    assert_eq!(
        CatalogStore::new(dir.path().into()).load().unwrap().queries[0].sql,
        "SELECT 2"
    );
    assert_eq!(app.tabs[1].sql, "SELECT 3");
    assert!(app.tabs[1].dirty());
    app.save_query_command(true);
    let mut dialog = app.catalog_dialog.take().unwrap();
    let CatalogDialog::Query { name, .. } = &mut dialog else {
        panic!()
    };
    *name = "并发编辑副本".into();
    assert!(app.apply_catalog_dialog(&dialog));
    assert_eq!(app.catalog.queries.len(), 2);
    assert!(!app.tabs[1].dirty());
}

#[test]
fn save_dialog_binds_its_source_and_failed_write_keeps_the_draft_dirty() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = library_app(dir.path());
    app.tabs[0].sql = "SELECT 1".into();
    app.save_query_command(false);
    let dialog = app.catalog_dialog.take().unwrap();
    app.tabs.push(QueryTab::new(2));
    app.active_tab = 1;
    assert!(app.apply_catalog_dialog(&dialog));
    assert!(app.tabs[0].saved_query.is_some());
    assert!(app.tabs[1].saved_query.is_none());
    app.tabs[1].sql = "SELECT 2".into();
    app.save_query_command(false);
    let dialog = app.catalog_dialog.take().unwrap();
    std::fs::write(dir.path().join("catalog.json"), b"{corrupt").unwrap();
    assert!(!app.apply_catalog_dialog(&dialog));
    assert_eq!(app.catalog.queries.len(), 1);
    assert!(app.tabs[1].saved_query.is_none());
    assert!(app.tabs[1].dirty());
}

#[test]
fn library_save_never_marks_an_existing_sql_file_as_saved() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = library_app(dir.path());
    app.tabs[0].sql = "SELECT 2".into();
    app.tabs[0].saved_sql = "SELECT 1".into();
    app.tabs[0].file_path = Some(dir.path().join("query.sql"));
    app.save_to_library(None);
    let dialog = app.catalog_dialog.take().unwrap();
    assert!(app.apply_catalog_dialog(&dialog));
    assert!(app.tabs[0].dirty());
    assert_eq!(app.tabs[0].saved_sql, "SELECT 1");
    assert!(!dir.path().join("query.sql").exists());
}

#[test]
fn library_open_preserves_missing_target_without_executing_or_claiming_a_file_save() {
    let target = Some(QueryTarget {
        schema: None,
        connection: Uuid::new_v4(),
        database: Some("生产".into()),
    });
    let tab = query_tab_from_saved(
        SavedQuery {
            id: Uuid::new_v4(),
            name: "危险查询".into(),
            sql: "DELETE FROM items".into(),
            target: target.clone(),
        },
        Uuid::new_v4(),
    );
    assert_eq!(tab.target, target);
    assert!(!tab.query_started);
    assert!(tab.output.is_none() && tab.file_path.is_none() && tab.table_target.is_none());
    assert!(!tab.dirty());
    assert!(tab.saved_sql.is_empty());
    assert!(tab.selection.is_empty());
}
