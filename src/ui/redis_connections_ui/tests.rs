use super::*;
use sqlshop::redis_database::RedisDatabase;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    time::{Duration, Instant},
};

struct TestDirectory(PathBuf);
impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("sqlshop-redis-ui-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn store(&self) -> Store {
        Store {
            directory: self.0.clone(),
        }
    }
}
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn app_with_redis() -> (App, RedisProfile) {
    let mut app = super::super::tests::session_test_app();
    let profile = RedisProfile::default();
    app.redis_profiles.push(profile.clone());
    app.credentials.insert(profile.id, Credentials::default());
    (app, profile)
}

fn labels(output: &egui::FullOutput) -> Vec<String> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.job.text.clone()),
            _ => None,
        })
        .collect()
}

fn render_tree(app: &mut App, events: Vec<egui::Event>, time: f64) -> egui::FullOutput {
    let context = app.context.clone();
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 1200.0),
            )),
            events,
            time: Some(time),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.sidebar(ui));
        },
    )
}

#[test]
fn unified_new_connection_has_four_types_and_anonymous_redis_defaults() {
    let mut app = super::super::tests::session_test_app();
    app.new_connection();
    let form = app.form.as_mut().unwrap();
    assert_eq!(form.kind, ConnectionKind::Sql(Engine::MySql));
    assert_eq!(ConnectionKind::ALL.len(), 4);
    assert!(ConnectionKind::ALL.contains(&ConnectionKind::Redis));
    form.password = "sql-only-secret".into();
    form.set_kind(ConnectionKind::Redis);
    assert_eq!(form.redis_profile.port, 6379);
    assert_eq!(form.redis_profile.database, 0);
    assert!(form.redis_profile.username.is_empty() && form.password.is_empty());
    assert!(!form.redis_profile.tls);
    assert!(form.redis_profile.validate().is_ok());
    let context = app.context.clone();
    let output = context.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            connection_heading(ui, form);
            database_fields(ui, form);
        });
    });
    let rendered = labels(&output);
    let text = rendered.join("\n");
    assert!(text.contains("数据库类型") && text.contains("Redis"));
    assert!(
        !rendered
            .iter()
            .any(|label| matches!(label.as_str(), "数据库" | "默认数据库" | "DB 索引"))
    );
    assert!(rendered.iter().any(|label| label == "记住密码"));
    assert!(!text.contains("连接时不选 DB"));
}

#[test]
fn legacy_redis_form_has_no_database_field() {
    let context = egui::Context::default();
    let mut form = ConnectionForm::redis(
        RedisProfile {
            database: 27,
            ..Default::default()
        },
        Credentials::default(),
        true,
    );
    let output = context.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            redis_database_fields(ui, &mut form);
        });
    });
    let rendered = labels(&output);
    assert!(
        !rendered
            .iter()
            .any(|label| matches!(label.as_str(), "数据库" | "默认数据库" | "DB 索引" | "27"))
    );
    assert_eq!(
        form.redis_profile.database, 27,
        "rendering the form does not mutate legacy configuration"
    );
}

#[test]
fn saved_profiles_cannot_cross_sql_redis_type_boundary() {
    let mut sql = ConnectionForm::sql(Profile::default(), Credentials::default(), true);
    sql.set_kind(ConnectionKind::Redis);
    assert_eq!(sql.kind, ConnectionKind::Sql(Engine::MySql));
    sql.set_kind(ConnectionKind::Sql(Engine::Postgres));
    assert_eq!(sql.profile.port, 5432);
    assert_eq!(sql.profile.username, "postgres");
    let mut redis = ConnectionForm::redis(RedisProfile::default(), Credentials::default(), true);
    redis.set_kind(ConnectionKind::Sql(Engine::MySql));
    assert_eq!(redis.kind, ConnectionKind::Redis);
}

#[test]
fn redis_save_load_delete_preserve_sql_files_and_credentials() {
    let directory = TestDirectory::new();
    let mut app = super::super::tests::session_test_app();
    app.store = Some(directory.store());
    let sql = Profile::default();
    app.save_connection(
        sql.clone(),
        Credentials {
            database_password: "sql-secret".into(),
            ssh_secret: String::new(),
        },
    )
    .unwrap();
    let original = std::fs::read(directory.0.join("connections.json")).unwrap();
    assert!(app.expand_connection.is_none());
    let profile = RedisProfile::default();
    app.save_redis_connection(profile.clone(), Credentials::default())
        .unwrap();
    assert!(
        app.expand_connection.is_none(),
        "saving must not expand a disconnected connection"
    );
    assert_eq!(
        app.tree_selection.selected,
        Some(TreeNode::Connection(profile.id))
    );
    assert_eq!(
        app.store
            .as_ref()
            .unwrap()
            .redis_credentials(&profile)
            .unwrap()
            .unwrap()
            .database_password,
        ""
    );
    let mut loaded = super::super::tests::session_test_app();
    loaded.store = Some(directory.store());
    loaded.restore_redis_connections();
    assert_eq!(loaded.selected, Some(profile.id));
    assert!(!loaded.redis_profiles[0].tls);
    assert!(loaded.open_redis_connection(profile.id, false));
    assert!(!loaded.redis_document(profile.id).unwrap().is_connected());
    assert!(app.open_redis_connection(profile.id, false));
    app.delete_redis_connection(profile.id).unwrap();
    assert!(app.redis_document(profile.id).is_none());
    assert!(app.store.as_ref().unwrap().load_redis().unwrap().is_empty());
    assert_eq!(app.selected, Some(sql.id));
    assert_eq!(
        std::fs::read(directory.0.join("connections.json")).unwrap(),
        original
    );
    assert_eq!(
        app.store
            .as_ref()
            .unwrap()
            .credentials(&sql)
            .unwrap()
            .unwrap()
            .database_password,
        "sql-secret"
    );
}

#[test]
fn broken_redis_config_only_disables_redis_and_preserves_catalog_membership() {
    let directory = TestDirectory::new();
    let mut app = super::super::tests::session_test_app();
    app.store = Some(directory.store());
    let redis = RedisProfile::default();
    let mut catalog = Catalog::default();
    let group = catalog.save_group(None, "混合连接").unwrap();
    catalog.move_connection(redis.id, Some(group)).unwrap();
    CatalogStore::new(directory.0.clone())
        .save(&catalog)
        .unwrap();
    std::fs::write(directory.0.join("redis-connections.json"), b"{broken").unwrap();
    app.restore_redis_connections();
    app.restore_catalog();
    assert!(app.redis_config_error.is_some());
    assert_eq!(app.catalog.memberships.get(&redis.id), Some(&group));
    app.save_connection(Profile::default(), Credentials::default())
        .unwrap();
    assert!(
        app.save_redis_connection(redis, Credentials::default())
            .is_err()
    );
    assert_eq!(
        std::fs::read(directory.0.join("redis-connections.json")).unwrap(),
        b"{broken"
    );
}

#[test]
fn mixed_tree_filters_groups_search_and_connected_only() {
    let (mut app, redis) = app_with_redis();
    let sql = Profile::default();
    app.profiles.push(sql.clone());
    let group = app.catalog.save_group(None, "生产").unwrap();
    app.catalog.move_connection(redis.id, Some(group)).unwrap();
    app.catalog.move_connection(sql.id, Some(group)).unwrap();
    assert_eq!(
        app.connection_tree_members(Some(group), "", false, false)
            .len(),
        2
    );
    assert_eq!(
        app.connection_tree_members(Some(group), "redis", false, false)
            .len(),
        1
    );
    assert_eq!(
        app.connection_tree_members(Some(group), "生产", true, false)
            .len(),
        2
    );
    assert!(
        app.connection_tree_members(Some(group), "", false, true)
            .is_empty()
    );
    assert!(app.open_redis_connection(redis.id, false));
    app.redis_document_mut(redis.id)
        .unwrap()
        .set_test_state(true, false);
    let connected = app.connection_tree_members(Some(group), "", false, true);
    assert_eq!(connected.len(), 1);
    assert_eq!(connected[0].id(), redis.id);
}

#[test]
fn disconnected_redis_tree_has_no_invented_databases_or_sql_navigation() {
    let (mut app, mut profile) = app_with_redis();
    profile.database = 27;
    app.redis_profiles[0] = profile.clone();
    app.browser_visible = false;
    app.expand_connection = Some(profile.id);
    app.context.style_mut(|style| style.animation_time = 0.0);
    let mut output = render_tree(&mut app, vec![], 0.0);
    for time in [0.1, 0.2] {
        output = render_tree(&mut app, vec![], time);
    }
    let rendered = labels(&output);
    let text = rendered.join("\n");
    assert!(rendered.contains(&profile.name), "{text}");
    assert!(
        !text.contains("本地 Redis · Redis"),
        "Redis uses its icon, not a suffix"
    );
    for redundant in [
        "尚未连接",
        "打开并连接",
        "连接后读取全部真实数据库",
        "localhost:6379",
    ] {
        assert!(!text.contains(redundant), "{text}");
    }
    assert!(
        !rendered.iter().any(|label| label.starts_with("db")),
        "{text}"
    );
    assert!(!text.contains("还没有数据库连接"));
    let position = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == profile.name => {
                Some(text.pos + egui::vec2(35.0, 6.0))
            }
            _ => None,
        })
        .unwrap();
    let pointer = |pressed| egui::Event::PointerButton {
        pos: position,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    render_tree(
        &mut app,
        vec![egui::Event::PointerMoved(position), pointer(true)],
        0.3,
    );
    render_tree(&mut app, vec![pointer(false)], 0.4);
    assert_eq!(app.selected, Some(profile.id));
    assert!(app.browser_scope.is_none() && !app.browser_visible);
    assert!(app.redis_document(profile.id).is_none());
    assert!(app.sessions.is_empty() && app.query_sessions.is_empty());
    assert!(
        app.connection_tree_members(None, "db27", false, false)
            .is_empty()
    );
    assert!(
        app.connection_tree_members(None, "db0", false, false)
            .is_empty()
    );
}

#[test]
fn sql_connection_tree_omits_inline_status_buttons_and_endpoint_details() {
    for engine in Engine::ALL {
        let mut app = super::super::tests::session_test_app();
        let profile = Profile {
            engine,
            name: format!("精简 {} 连接", engine.label()),
            host: "tree-only.example.test".into(),
            port: engine.port(),
            ..Default::default()
        };
        app.profiles.push(profile.clone());
        app.selected = Some(profile.id);
        app.expand_connection = Some(profile.id);
        app.context.style_mut(|style| style.animation_time = 0.0);
        render_tree(&mut app, vec![], 0.0);
        let output = render_tree(&mut app, vec![], 0.1);
        let rendered = labels(&output);
        assert!(rendered.contains(&profile.name));
        for redundant in ["尚未连接", "正在连接…", "连接"] {
            assert!(!rendered.iter().any(|label| label == redundant));
        }
        assert!(!rendered.iter().any(|label| label.contains(&profile.host)));
        assert!(
            app.sessions.is_empty(),
            "rendering an expanded node must not connect"
        );

        let mut session = ConnectionSession::new(&app.context);
        session.connected = Some(profile.clone());
        session.databases = vec!["example_database".into()];
        app.sessions.insert(profile.id, session);
        let output = render_tree(&mut app, vec![], 0.2);
        let rendered = labels(&output);
        assert!(rendered.iter().any(|label| label == "example_database"));
        assert!(!rendered.iter().any(|label| label.contains(&profile.host)));
    }
}

#[test]
fn legacy_database_is_not_a_navigation_or_initial_database_target() {
    let (mut app, mut profile) = app_with_redis();
    profile.database = 27;
    app.redis_profiles[0] = profile.clone();
    assert!(app.open_redis_connection(profile.id, false));
    let doc = app.redis_document(profile.id).unwrap();
    assert_eq!(doc.database_index(), 0);
    assert!(doc.databases().is_empty());
    assert!(!app.select_redis_database(profile.id, 27));
    assert!(
        !app.redis_document(profile.id).unwrap().busy,
        "legacy DB must not trigger CONNECT or SELECT"
    );
    assert!(
        app.connection_tree_members(None, "db27", false, false)
            .is_empty()
    );
    let directory = TestDirectory::new();
    app.store = Some(directory.store());
    app.save_redis_connection(profile, Credentials::default())
        .unwrap();
    assert_eq!(app.redis_profiles[0].database, 0);
    assert_eq!(
        app.store.as_ref().unwrap().load_redis().unwrap()[0].database,
        0
    );
}

#[test]
fn connected_tree_renders_all_32_real_databases_including_empty_and_only_actual_current() {
    let (mut app, mut profile) = app_with_redis();
    profile.database = 27;
    app.redis_profiles[0] = profile.clone();
    assert!(app.open_redis_connection(profile.id, false));
    let doc = app.redis_document_mut(profile.id).unwrap();
    doc.set_test_state(true, false);
    doc.set_test_databases(
        (0..32)
            .map(|index| RedisDatabase {
                index,
                keys: Some(if index == 7 { 11 } else { 0 }),
            })
            .collect(),
        true,
        None,
    );
    app.expand_connection = Some(profile.id);
    app.context.style_mut(|style| style.animation_time = 0.0);
    let mut output = render_tree(&mut app, vec![], 0.0);
    for time in [0.1, 0.2] {
        output = render_tree(&mut app, vec![], time);
    }
    let rendered = labels(&output);
    let text = rendered.join("\n");
    for index in 0..32 {
        let expected = format!(
            "db{index}{} · {} 个键",
            if index == 0 { " · 当前" } else { "" },
            if index == 7 { 11 } else { 0 }
        );
        assert!(rendered.contains(&expected), "missing {expected}: {text}");
    }
    assert_eq!(
        rendered
            .iter()
            .filter(|label| label.starts_with("db") && label.contains(" · 当前"))
            .count(),
        1
    );
    assert!(!text.contains("db27 · 当前") && !text.contains("配置"));
    for redundant in ["已连接", "尚未连接", "打开并连接", "localhost:6379"] {
        assert!(!rendered.iter().any(|label| label == redundant), "{text}");
    }
    assert_eq!(
        app.connection_tree_members(None, "db31", false, false)
            .len(),
        1
    );
    assert_eq!(
        app.connection_tree_members(None, "db31", false, true).len(),
        1
    );
    assert!(
        app.connection_tree_members(None, "db32", false, false)
            .is_empty()
    );
}

#[test]
fn partial_database_snapshot_explains_missing_empty_databases_without_filling_gaps() {
    let (mut app, mut profile) = app_with_redis();
    profile.database = 15;
    app.redis_profiles[0] = profile.clone();
    assert!(app.open_redis_connection(profile.id, false));
    let doc = app.redis_document_mut(profile.id).unwrap();
    doc.set_test_state(true, false);
    doc.set_test_databases(
        vec![
            RedisDatabase {
                index: 0,
                keys: None,
            },
            RedisDatabase {
                index: 5,
                keys: Some(3),
            },
        ],
        false,
        Some("CONFIG GET databases 返回 NOPERM；INFO keyspace 仅确认非空库及当前库。".into()),
    );
    app.expand_connection = Some(profile.id);
    app.context.style_mut(|style| style.animation_time = 0.0);
    let mut output = render_tree(&mut app, vec![], 0.0);
    for time in [0.1, 0.2] {
        output = render_tree(&mut app, vec![], time);
    }
    let rendered = labels(&output);
    let text = rendered.join("\n");
    let databases: Vec<_> = rendered
        .iter()
        .filter(|label| label.starts_with("db"))
        .map(String::as_str)
        .collect();
    assert_eq!(databases, ["db0 · 当前", "db5 · 3 个键"]);
    assert!(
        text.contains("数据库列表不完整")
            && text.contains("无法列出全部空 DB")
            && text.contains("NOPERM"),
        "{text}"
    );
    assert!(
        !text.contains("db0 · 当前 · 0 个键"),
        "an unknown count must not be shown as empty"
    );
    assert!(
        app.connection_tree_members(None, "db15", false, false)
            .is_empty()
    );
    assert_eq!(
        app.connection_tree_members(None, "db5", false, false).len(),
        1
    );
}

#[test]
fn database_single_click_only_selects_and_double_click_requests_select_without_sql() {
    let (mut app, profile) = app_with_redis();
    assert!(app.open_redis_connection(profile.id, false));
    let doc = app.redis_document_mut(profile.id).unwrap();
    doc.set_test_state(true, false);
    doc.set_test_databases(
        vec![
            RedisDatabase {
                index: 0,
                keys: Some(0),
            },
            RedisDatabase {
                index: 7,
                keys: Some(0),
            },
        ],
        true,
        None,
    );
    app.expand_connection = Some(profile.id);
    app.context.style_mut(|style| style.animation_time = 0.0);
    let mut output = render_tree(&mut app, vec![], 0.0);
    for time in [0.1, 0.2] {
        output = render_tree(&mut app, vec![], time);
    }
    let position = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == "db7 · 0 个键" => {
                Some(text.pos + egui::vec2(20.0, 6.0))
            }
            _ => None,
        })
        .unwrap();
    let pointer = |pressed| egui::Event::PointerButton {
        pos: position,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    render_tree(
        &mut app,
        vec![egui::Event::PointerMoved(position), pointer(true)],
        0.3,
    );
    render_tree(&mut app, vec![pointer(false)], 0.35);
    assert_eq!(
        app.tree_selection.selected,
        Some(TreeNode::Database(profile.id, "7".into()))
    );
    assert!(!app.redis_document(profile.id).unwrap().busy);
    assert_eq!(app.redis_document(profile.id).unwrap().database_index(), 0);
    assert!(app.browser_scope.is_none() && !app.browser_visible);
    render_tree(&mut app, vec![pointer(true)], 0.4);
    render_tree(&mut app, vec![pointer(false)], 0.45);
    let doc = app.redis_document(profile.id).unwrap();
    assert!(
        doc.busy,
        "double click must send SELECT to the mocked disconnected worker, not connect to a real server"
    );
    assert_eq!(
        doc.database_index(),
        0,
        "current DB changes only after a successful SELECT response"
    );
    assert!(app.browser_scope.is_none() && !app.browser_visible);
    assert!(app.sessions.is_empty() && app.query_sessions.is_empty());
}

#[test]
fn refresh_database_list_uses_redis_document_and_respects_busy_dirty_guards() {
    let (mut app, profile) = app_with_redis();
    assert!(!app.refresh_redis_databases(profile.id));
    assert!(app.redis_document(profile.id).is_none());
    assert!(app.open_redis_connection(profile.id, false));
    assert!(!app.refresh_redis_databases(profile.id));
    let doc = app.redis_document_mut(profile.id).unwrap();
    doc.set_test_state(true, true);
    assert!(!app.refresh_redis_databases(profile.id));
    let doc = app.redis_document_mut(profile.id).unwrap();
    doc.set_test_state(true, false);
    doc.busy = true;
    assert!(!app.refresh_redis_databases(profile.id));
    app.redis_document_mut(profile.id).unwrap().busy = false;
    assert!(app.refresh_redis_databases(profile.id));
    assert!(app.redis_document(profile.id).unwrap().busy);
    assert!(app.sessions.is_empty() && app.query_sessions.is_empty());
}

#[test]
fn redis_open_reuses_bound_tab_and_edit_updates_title() {
    let directory = TestDirectory::new();
    let (mut app, mut profile) = app_with_redis();
    app.store = Some(directory.store());
    assert!(app.open_redis_connection(profile.id, false));
    let tab = app.tabs[app.active_tab].id;
    let count = app.tabs.len();
    assert!(app.open_redis_connection(profile.id, false));
    assert_eq!(app.tabs.len(), count);
    assert_eq!(app.tabs[app.active_tab].id, tab);
    assert!(app.tabs[app.active_tab].target.is_none());
    profile.name = "改名后的缓存".into();
    app.save_redis_connection(profile.clone(), Credentials::default())
        .unwrap();
    assert_eq!(app.tabs[app.active_tab].name, "Redis · 改名后的缓存");
    assert_eq!(
        app.redis_document(profile.id).unwrap().bound_profile().name,
        profile.name
    );
    assert!(app.sessions.is_empty() && app.query_sessions.is_empty());
}

#[test]
fn dirty_busy_and_live_profile_guards_preserve_documents_and_configuration() {
    let directory = TestDirectory::new();
    let (mut app, profile) = app_with_redis();
    app.store = Some(directory.store());
    app.save_redis_connection(profile.clone(), Credentials::default())
        .unwrap();
    app.open_redis_connection(profile.id, false);
    let original = std::fs::read(directory.0.join("redis-connections.json")).unwrap();
    for (busy, dirty, connected) in [
        (true, false, true),
        (false, true, true),
        (false, true, false),
    ] {
        let doc = app.redis_document_mut(profile.id).unwrap();
        doc.set_test_state(connected, dirty);
        doc.busy = busy;
        assert!(!app.open_redis_connection(profile.id, true));
        assert!(!app.select_redis_database(profile.id, 1));
        app.disconnect_connection(profile.id);
        assert!(app.delete_redis_connection(profile.id).is_err());
        assert!(
            app.save_redis_connection(profile.clone(), Credentials::default())
                .is_err()
        );
        app.edit_connection(profile.id);
        assert!(app.form.is_none());
        assert!(app.redis_document(profile.id).is_some());
        assert_eq!(
            std::fs::read(directory.0.join("redis-connections.json")).unwrap(),
            original
        );
    }
    let doc = app.redis_document_mut(profile.id).unwrap();
    doc.busy = false;
    doc.set_test_state(true, false);
    assert!(
        app.save_redis_connection(profile.clone(), Credentials::default())
            .is_err()
    );
    app.redis_document_mut(profile.id)
        .unwrap()
        .set_test_state(false, false);
    app.edit_connection(profile.id);
    assert_eq!(app.form.as_ref().unwrap().kind, ConnectionKind::Redis);
}

#[test]
fn nonremembered_credentials_prompt_but_anonymous_defaults_open() {
    let (mut app, mut profile) = app_with_redis();
    profile.remember_password = false;
    app.redis_profiles[0] = profile.clone();
    app.credentials.clear();
    let directory = TestDirectory::new();
    app.store = Some(directory.store());
    assert!(!app.open_redis_connection(profile.id, false));
    assert_eq!(app.form.as_ref().unwrap().kind, ConnectionKind::Redis);
    assert!(app.redis_document(profile.id).is_none());
    app.form = None;
    app.redis_profiles[0].remember_password = true;
    assert!(app.open_redis_connection(profile.id, false));
    assert!(app.form.is_none());
}

#[test]
fn corrupt_secrets_never_replace_existing_redis_configuration() {
    let directory = TestDirectory::new();
    let (mut app, profile) = app_with_redis();
    app.store = Some(directory.store());
    app.save_redis_connection(profile.clone(), Credentials::default())
        .unwrap();
    let original = std::fs::read(directory.0.join("redis-connections.json")).unwrap();
    std::fs::write(directory.0.join("credentials.enc"), b"corrupt secrets").unwrap();
    app.credentials.clear();
    app.edit_connection(profile.id);
    assert!(app.form.is_none() && app.error);
    assert!(!app.open_redis_connection(profile.id, false));
    assert!(
        app.save_redis_connection(profile, Credentials::default())
            .is_err()
    );
    assert_eq!(
        std::fs::read(directory.0.join("redis-connections.json")).unwrap(),
        original
    );
    assert_eq!(
        std::fs::read(directory.0.join("credentials.enc")).unwrap(),
        b"corrupt secrets"
    );
}

#[test]
fn redis_title_counts_do_not_enable_sql_sessions() {
    let (mut app, profile) = app_with_redis();
    app.open_redis_connection(profile.id, false);
    app.redis_document_mut(profile.id)
        .unwrap()
        .set_test_state(true, false);
    assert_eq!(app.selected_connected_name(), Some(profile.name.as_str()));
    assert_eq!(app.connection_activity_counts(), (1, 0));
    assert!(app.redis_ready(profile.id));
    assert!(app.selected_session().is_none());
    app.redis_document_mut(profile.id).unwrap().busy = true;
    assert_eq!(app.connection_activity_counts(), (1, 1));
    assert!(!app.redis_ready(profile.id));
}

#[test]
fn temporary_test_worker_only_pings_then_drops_without_scan_tab_or_save() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let profile = RedisProfile {
        host: "127.0.0.1".into(),
        port: listener.local_addr().unwrap().port(),
        ..Default::default()
    };
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request = String::new();
        for _ in 0..3 {
            reader.read_line(&mut request).unwrap();
        }
        assert_eq!(request, "*1\r\n$4\r\nPING\r\n");
        stream.write_all(b"+PONG\r\n").unwrap();
        let mut extra = [0u8; 1];
        assert_eq!(
            reader.read(&mut extra).unwrap(),
            0,
            "temporary test must close without SCAN"
        );
    });
    let (mut app, existing) = app_with_redis();
    app.open_redis_connection(existing.id, false);
    app.redis_document_mut(existing.id)
        .unwrap()
        .set_test_state(true, false);
    let tab_count = app.tabs.len();
    let directory = TestDirectory::new();
    app.store = Some(directory.store());
    app.test_redis_connection(profile, Credentials::default())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(6);
    while app.redis_test_worker.is_some() && Instant::now() < deadline {
        app.poll_redis_connection_test();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        app.redis_test_worker.is_none(),
        "test worker did not complete"
    );
    assert!(!app.error, "{}", app.status);
    assert_eq!(app.tabs.len(), tab_count);
    assert_eq!(app.redis_profiles.len(), 1);
    assert!(app.redis_document(existing.id).unwrap().is_connected());
    assert!(!directory.0.join("redis-connections.json").exists());
    server.join().unwrap();
}
