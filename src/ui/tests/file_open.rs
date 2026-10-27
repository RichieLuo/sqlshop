use super::*;

#[test]
fn sql_shell_open_with_an_offline_profile_stays_disconnected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("离线 查询.sql");
    std::fs::write(&path, "DROP TABLE must_not_execute;").unwrap();
    let mut app = session_test_app();
    let profile = Profile::default();
    let target = QueryTarget {
        connection: profile.id,
        database: Some("offline_database".into()),
        schema: None,
    };
    app.selected = Some(profile.id);
    app.profiles.push(profile);
    app.tabs[0].target = Some(target.clone());

    assert!(app.open_sql_path(&path));

    assert!(app.sessions.is_empty());
    assert!(app.query_sessions.is_empty());
    assert!(app.history.is_empty());
    assert_eq!(app.tabs[app.active_tab].target, Some(target));
    assert!(!app.tabs[app.active_tab].query_started);
    assert!(app.tabs[app.active_tab].output.is_none());
}

#[test]
fn sql_shell_open_preserves_busy_draft_and_never_connects_or_executes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("中文 空格 查询.SQL");
    let sql = "DROP TABLE keep_me; SELECT '只载入，不能运行';";
    std::fs::write(&path, format!("\u{feff}{sql}")).unwrap();
    let mut app = session_test_app();
    let target = add_test_session(&mut app, "保留连接", "original_database");
    app.selected = Some(target.connection);
    app.tabs[0].target = Some(target.clone());
    app.tabs[0].sql = "UPDATE old_draft SET value = 1; -- unsaved".into();
    let original_sql = app.tabs[0].sql.clone();
    let original_id = app.tabs[0].id;
    let session = app.sessions.get_mut(&target.connection).unwrap();
    session.busy = true;
    session.running_tab = Some(original_id);

    app.open_shell_paths(std::slice::from_ref(&path));

    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.tabs[0].id, original_id);
    assert_eq!(app.tabs[0].sql, original_sql);
    assert_eq!(app.tabs[0].target, Some(target.clone()));
    let opened = &app.tabs[app.active_tab];
    assert_eq!(opened.sql, sql);
    assert_eq!(opened.saved_sql, sql);
    assert_eq!(opened.file_path.as_ref(), Some(&path));
    assert!(!opened.query_started);
    assert!(opened.output.is_none());
    assert!(app.query_sessions.is_empty());
    assert!(app.history.is_empty());
    assert!(app.sessions[&target.connection].busy);
    assert_eq!(
        app.sessions[&target.connection].running_tab,
        Some(original_id)
    );
    assert_eq!(
        app.sessions[&target.connection].active_database,
        target.database
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        format!("\u{feff}{sql}")
    );
}

#[test]
fn reopening_sql_reuses_canonical_file_and_keeps_unsaved_changes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("重复 草稿.sql");
    std::fs::write(&path, "SELECT 1;").unwrap();
    let mut app = session_test_app();
    assert!(app.open_sql_path(&path));
    let index = app.active_tab;
    let id = app.tabs[index].id;
    app.tabs[index].sql = "SELECT 'unsaved editor text';".into();
    std::fs::write(&path, "DELETE FROM outside_change;").unwrap();
    app.active_tab = 0;
    let alias_directory = directory.path().join("别名");
    std::fs::create_dir(&alias_directory).unwrap();
    let alias = alias_directory.join("..").join("重复 草稿.sql");

    assert!(app.open_sql_path(&alias));

    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.active_tab, index);
    assert_eq!(app.tabs[index].id, id);
    assert_eq!(app.tabs[index].sql, "SELECT 'unsaved editor text';");
    assert_eq!(app.tabs[index].saved_sql, "SELECT 1;");
    assert!(app.query_sessions.is_empty());
}

#[test]
fn shell_opens_sql_dump_above_the_former_16_mib_limit_without_truncation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("large.sql");
    let sql = format!("{}SELECT '文件末尾 🦀';", "-- dump\n".repeat(2_100_000));
    assert!(sql.len() > 16 * 1024 * 1024);
    std::fs::write(&path, &sql).unwrap();
    let mut app = session_test_app();

    app.open_shell_paths(std::slice::from_ref(&path));

    assert!(!app.error, "{}", app.status);
    assert_eq!(app.tabs.len(), 2);
    let tab = &app.tabs[app.active_tab];
    assert_eq!(tab.sql, sql);
    assert_eq!(tab.saved_sql, sql);
    assert_eq!(tab.file_path.as_ref(), Some(&path));
    assert!(!tab.query_started);
    assert!(app.sessions.is_empty());
    assert!(app.query_sessions.is_empty());
}

#[test]
fn invalid_utf8_and_oversized_sql_leave_existing_documents_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let bad_utf8 = directory.path().join("invalid.sql");
    std::fs::write(&bad_utf8, [0xff, 0xfe, 0xfd]).unwrap();
    let too_large = directory.path().join("oversized.sql");
    std::fs::File::create(&too_large)
        .unwrap()
        .set_len(sqlshop::workspace::MAX_SQL_FILE_BYTES as u64 + 1)
        .unwrap();
    let mut app = session_test_app();
    app.tabs[0].sql = "keep this draft".into();
    let id = app.tabs[0].id;
    for path in [bad_utf8, too_large, directory.path().join("missing.sql")] {
        assert!(!app.open_sql_path(&path));
        assert!(app.error);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].id, id);
        assert_eq!(app.tabs[0].sql, "keep this draft");
        assert!(app.sessions.is_empty());
        assert!(app.query_sessions.is_empty());
    }
}

#[test]
fn shell_batch_keeps_unsupported_file_feedback_after_later_sql_opens() {
    let directory = tempfile::tempdir().unwrap();
    let sql = directory.path().join("one.sql");
    let json = directory.path().join("通用 JSON.json");
    std::fs::write(&sql, "SELECT 'unexecuted';").unwrap();
    std::fs::write(&json, "{\"unchanged\":true}").unwrap();
    let mut app = session_test_app();

    app.open_shell_paths(&[json.clone(), sql]);

    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.tabs[app.active_tab].sql, "SELECT 'unexecuted';");
    assert!(!app.tabs[app.active_tab].query_started);
    assert!(app.error);
    assert!(app.status.contains("不支持此文件类型"));
    assert!(app.status.contains("通用 JSON.json"));
    assert_eq!(
        std::fs::read_to_string(json).unwrap(),
        "{\"unchanged\":true}"
    );
}

#[test]
fn sqlite_shell_open_only_loads_version_and_catalog_and_reuses_busy_document() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("中文 数据库.db3");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch("CREATE TABLE keep_me(id INTEGER); INSERT INTO keep_me VALUES (42);")
            .unwrap();
    }
    let bytes_before = std::fs::read(&path).unwrap();
    let mut app = session_test_app();
    app.tabs[0].sql = "SELECT 'original unsaved SQL';".into();

    app.open_shell_paths(std::slice::from_ref(&path));

    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.tabs[0].sql, "SELECT 'original unsaved SQL';");
    let index = app.active_tab;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while app.tabs[index]
        .local_database
        .as_ref()
        .unwrap()
        .pending
        .is_some()
    {
        app.tabs[index].local_database.as_mut().unwrap().poll();
        assert!(
            std::time::Instant::now() < deadline,
            "SQLite metadata response timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let doc = app.tabs[index].local_database.as_mut().unwrap();
    assert!(doc.error.is_none(), "{:?}", doc.error);
    assert!(doc.objects.iter().any(|(name, _, _)| name == "keep_me"));
    assert_eq!(doc.sql, "SELECT sqlite_version() AS SQLite;");
    assert!(!doc.in_transaction);
    let busy = Uuid::new_v4();
    doc.sql = "DROP TABLE keep_me; -- not executed".into();
    doc.pending = Some(busy);
    doc.in_transaction = true;
    let original_id = app.tabs[index].id;
    app.active_tab = 0;

    assert!(app.open_local_database_path(&path));

    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.active_tab, index);
    assert_eq!(app.tabs[index].id, original_id);
    let doc = app.tabs[index].local_database.as_ref().unwrap();
    assert_eq!(doc.pending, Some(busy));
    assert!(doc.in_transaction);
    assert_eq!(doc.sql, "DROP TABLE keep_me; -- not executed");
    assert!(app.query_sessions.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), bytes_before);
    let connection = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT id FROM keep_me", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        42
    );
}

#[test]
fn sqlite_shell_open_rejects_missing_empty_non_database_and_directory_paths() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.sqlite");
    let empty = directory.path().join("empty.sqlite3");
    let invalid = directory.path().join("not a SQLite database.db");
    std::fs::write(&empty, b"").unwrap();
    std::fs::write(&invalid, b"ordinary data file unchanged").unwrap();
    let mut app = session_test_app();

    for path in [&missing, &empty, &invalid, directory.path()] {
        assert!(!app.open_local_database_path(path));
        assert!(app.error);
        assert_eq!(app.tabs.len(), 1);
        assert!(app.tabs[0].local_database.is_none());
    }
    assert!(!missing.exists());
    assert!(std::fs::read(empty).unwrap().is_empty());
    assert_eq!(
        std::fs::read(invalid).unwrap(),
        b"ordinary data file unchanged"
    );
}

#[test]
fn manual_sqlite_open_keeps_support_for_existing_empty_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("手动打开 空白.sqlite");
    std::fs::write(&path, b"").unwrap();
    let mut app = session_test_app();

    assert!(app.open_local_database_document(path.canonicalize().unwrap()));

    let index = app.active_tab;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while app.tabs[index]
        .local_database
        .as_ref()
        .unwrap()
        .pending
        .is_some()
    {
        app.tabs[index].local_database.as_mut().unwrap().poll();
        assert!(
            std::time::Instant::now() < deadline,
            "SQLite metadata response timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let doc = app.tabs[index].local_database.as_ref().unwrap();
    assert!(doc.error.is_none(), "{:?}", doc.error);
    assert!(doc.output.is_some());
    assert!(doc.objects.is_empty());
    assert!(path.is_file());
}
