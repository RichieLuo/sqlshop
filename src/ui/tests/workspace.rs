use super::*;

#[test]
fn workspace_borrowed_comparison_tracks_all_persisted_fields() {
    let mut app = session_test_app();
    let mut saved = app.workspace_snapshot();
    assert!(app.workspace_matches(&saved));
    macro_rules! changed {
        ($change:expr) => {
            $change;
            assert!(!app.workspace_matches(&saved), "{}", stringify!($change));
            saved = app.workspace_snapshot();
            assert!(app.workspace_matches(&saved));
        };
    }
    changed!(app.tabs[0].sql.push_str("SELECT '草稿'"));
    changed!(app.tabs[0].saved_sql.push_str("SELECT 1"));
    changed!(app.tabs[0].name.push_str(" renamed"));
    changed!(app.tabs[0].file_path = Some(PathBuf::from("draft.sql")));
    changed!(app.tabs[0].id = Uuid::new_v4());
    changed!(
        app.tabs[0].target = Some(QueryTarget {
            connection: Uuid::new_v4(),
            database: Some("db".into()),
            schema: Some("public".into()),
        })
    );
    changed!(app.tabs[0].target.as_mut().unwrap().schema = Some("other".into()));
    changed!(app.tabs[0].target.as_mut().unwrap().database = None);
    changed!(app.tabs[0].target.as_mut().unwrap().connection = Uuid::new_v4());
    changed!(app.next_tab += 1);
    changed!(app.editor_height = Some(280.0));
    changed!(app.view.navigation ^= true);
    changed!(app.inspector_visible ^= true);
    changed!(app.view.information_force ^= true);
    changed!(app.view.toolbar ^= true);
    changed!(app.view.status ^= true);
    changed!(app.view.focus ^= true);
    changed!(app.view.connected_only ^= true);
    changed!(app.theme = egui::ThemePreference::Light);
    changed!(app.theme = egui::ThemePreference::Dark);
    changed!(app.theme = egui::ThemePreference::System);
    changed!(app.history.push(HistoryEntry {
        sql: "SELECT '历史'".into(),
        target: app.tabs[0].target.clone().unwrap(),
        source: "SQL 1".into(),
        timestamp: 1,
        success: false,
    }));
    changed!(app.history[0].sql.push_str("; SELECT 2"));
    changed!(app.history[0].target.schema = Some("history_schema".into()));
    changed!(app.history[0].source.push_str(" renamed"));
    changed!(app.history[0].timestamp += 1);
    changed!(app.history[0].success = true);
    changed!(app.history.clear());
    changed!(app.tabs.push(QueryTab::new(2)));
    changed!(app.active_tab = 1);
    changed!(app.tabs.swap(0, 1));
    changed!(app.active_tab = 0);
    changed!(app.tabs.pop());
}

#[test]
fn workspace_borrowed_comparison_uses_filtered_tab_order_and_active_mapping() {
    let mut app = session_test_app();
    let mut clean_table = QueryTab::new(2);
    clean_table.table_target = Some(TableTarget {
        connection: Uuid::new_v4(),
        database: "db".into(),
        schema: "public".into(),
        name: "items".into(),
    });
    // A clean table browser is omitted even when it contains generated SQL.
    clean_table.sql = "SELECT * FROM items".into();
    clean_table.saved_sql = clean_table.sql.clone();
    let mut management = QueryTab::new(3);
    management.jobs = Some(management_ui::JobsDocument::new(None));
    let mut definition = QueryTab::new(4);
    definition.definition = Some(object_browser_host::DefinitionDocument {
        connection: Uuid::new_v4(),
        database: "db".into(),
        object: object_browser_host::example_catalog().objects.remove(0),
        sql: Some("CREATE TABLE items (id int)".into()),
        error: None,
        stale: false,
    });
    app.tabs
        .extend([clean_table, management, definition, QueryTab::new(5)]);
    app.active_tab = 4;
    let saved = app.workspace_snapshot();
    assert_eq!(saved.tabs.len(), 2);
    assert_eq!(saved.active_tab, 1);
    assert!(app.workspace_matches(&saved));
    app.tabs[2].sql.push_str("ignored management edit");
    app.tabs[3].sql.push_str("ignored definition edit");
    assert!(app.workspace_matches(&saved));
    app.tabs.swap(1, 2);
    assert!(app.workspace_matches(&saved));
    app.active_tab = 2; // Omitted table maps to persisted index zero.
    assert!(!app.workspace_matches(&saved));
    let omitted_active = app.workspace_snapshot();
    assert_eq!(omitted_active.active_tab, 0);
    app.active_tab = 1; // Switching between omitted tabs does not change it.
    assert!(app.workspace_matches(&omitted_active));
    app.tabs[2].sql.push_str(" WHERE id = 1");
    assert!(!app.workspace_matches(&omitted_active));
    let dirty_table = app.workspace_snapshot();
    assert_eq!(dirty_table.tabs.len(), 3);
    assert!(app.workspace_matches(&dirty_table));
    app.tabs[2].saved_sql = app.tabs[2].sql.clone();
    assert!(app.workspace_matches(&omitted_active));
    app.tabs.clear();
    let mut omitted = QueryTab::new(6);
    omitted.jobs = Some(management_ui::JobsDocument::new(None));
    app.tabs.push(omitted);
    app.active_tab = 0;
    let empty = app.workspace_snapshot();
    assert!(empty.tabs.is_empty());
    assert_eq!(empty.active_tab, 0);
    assert!(app.workspace_matches(&empty));
}

#[test]
fn workspace_autosave_first_save_and_idle_skip_preserve_cached_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = session_test_app();
    app.workspace_store = Some(WorkspaceStore::new(directory.path().to_owned()));
    app.tabs[0].sql = "SELECT 'large unchanged draft'".repeat(1000);
    assert!(app.last_workspace.is_none());
    assert!(app.autosave_workspace(true));
    assert_eq!(
        app.workspace_store.as_ref().unwrap().load().unwrap(),
        app.workspace_snapshot()
    );
    let sql_ptr = app.last_workspace.as_ref().unwrap().tabs[0].sql.as_ptr();
    // An attempted rewrite now fails: an idle save must never touch the store.
    let path = directory.path().join("workspace.json");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    app.last_autosave = std::time::Instant::now() - std::time::Duration::from_secs(3);
    assert!(app.autosave_workspace(false));
    assert!(app.autosave_workspace(true));
    assert_eq!(
        app.last_workspace.as_ref().unwrap().tabs[0].sql.as_ptr(),
        sql_ptr
    );
    assert!(path.is_dir());
}

#[test]
fn workspace_autosave_failed_save_retries_without_advancing_cache() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workspace.json");
    let mut app = session_test_app();
    app.workspace_store = Some(WorkspaceStore::new(directory.path().to_owned()));
    std::fs::create_dir(&path).unwrap();
    assert!(!app.autosave_workspace(true));
    assert!(app.last_workspace.is_none());
    std::fs::remove_dir(&path).unwrap();
    assert!(app.autosave_workspace(true));
    let previous = app.last_workspace.clone();
    app.tabs[0].sql = "SELECT 'retry me'".into();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(!app.autosave_workspace(true));
    assert_eq!(app.last_workspace, previous);
    assert!(!app.workspace_matches(app.last_workspace.as_ref().unwrap()));
    std::fs::remove_dir(&path).unwrap();
    app.last_autosave = std::time::Instant::now() - std::time::Duration::from_secs(3);
    assert!(app.autosave_workspace(false));
    assert_eq!(
        app.workspace_store.as_ref().unwrap().load().unwrap(),
        app.workspace_snapshot()
    );
    assert!(app.workspace_matches(app.last_workspace.as_ref().unwrap()));
}

#[test]
fn workspace_autosave_force_bypasses_interval_but_screenshot_and_disabled_store_remain_safe() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = session_test_app();
    app.workspace_store = Some(WorkspaceStore::new(directory.path().to_owned()));
    assert!(app.autosave_workspace(true));
    let previous = app.last_workspace.clone();
    app.tabs[0].sql = "SELECT 'force'".into();
    assert!(app.autosave_workspace(false));
    assert_eq!(app.last_workspace, previous);
    assert!(app.autosave_workspace(true));
    assert_ne!(app.last_workspace, previous);
    let previous = app.last_workspace.clone();
    app.tabs[0].sql.push_str("; SELECT 2");
    app.screenshot_path = Some(directory.path().join("screenshot.png"));
    assert!(app.autosave_workspace(true));
    assert_eq!(app.last_workspace, previous);
    app.screenshot_path = None;
    app.workspace_store = None;
    assert!(!app.autosave_workspace(true));
    assert_eq!(app.last_workspace, previous);
}

#[test]
fn workspace_autosave_corrupt_store_stays_disabled_and_preserves_original() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workspace.json");
    std::fs::write(&path, b"{corrupt").unwrap();
    let mut app = session_test_app();
    app.store = Some(Store {
        directory: directory.path().to_owned(),
    });
    app.restore_workspace();
    assert!(app.workspace_store.is_none());
    assert!(!app.autosave_workspace(true));
    assert!(app.last_workspace.is_none());
    assert_eq!(std::fs::read(&path).unwrap(), b"{corrupt");
}
