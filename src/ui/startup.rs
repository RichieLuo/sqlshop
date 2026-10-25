use super::*;

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        window_chrome::configure_window(cc);
        install_font(&cc.egui_ctx);
        let screenshot_path = std::env::var_os("SQLSHOP_SCREENSHOT").map(PathBuf::from);
        let theme = if screenshot_path.is_some() {
            match std::env::var("SQLSHOP_SCREENSHOT_THEME").as_deref() {
                Ok("light") => egui::ThemePreference::Light,
                Ok("dark") => egui::ThemePreference::Dark,
                _ => egui::ThemePreference::System,
            }
        } else {
            egui::ThemePreference::System
        };
        cc.egui_ctx.set_theme(theme);
        cc.egui_ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(10.0, 10.0);
            style.spacing.button_padding = egui::vec2(13.0, 8.0);
            style.spacing.interact_size.y = 30.0;
            style.visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(8);
            style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(8);
            style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(8);
            style.visuals.widgets.active.corner_radius = egui::CornerRadius::same(8);
            style.visuals.window_corner_radius = egui::CornerRadius::same(14);
            style.visuals.window_shadow = egui::epaint::Shadow::NONE;
        });
        let context = cc.egui_ctx.clone();
        let mut app = Self {
            advanced_worker: sqlshop::advanced_worker::AdvancedWorker::new(cc.egui_ctx.clone()),
            compare_worker: sqlshop::compare_worker::CompareWorker::start(),
            backup_worker: sqlshop::backup_worker::BackupWorker::start(),
            job_scheduler: None,
            jobs_loaded: false,
            last_job_tick: std::time::Instant::now(),
            logo: crate::branding::load_texture(&cc.egui_ctx),
            store: None,
            profiles: Vec::new(),
            redis_profiles: Vec::new(),
            redis_config_error: None,
            redis_test_worker: None,
            credentials: HashMap::new(),
            selected: None,
            sessions: HashMap::new(),
            query_sessions: HashMap::new(),
            browser_state: object_browser_ui::BrowserState::default(),
            browser_scope: None,
            browser_visible: true,
            inspector_visible: true,
            view: ViewLayout::default(),
            context: cc.egui_ctx.clone(),
            test_worker: Worker::start(move || context.request_repaint()),
            recovery_worker: sqlshop::recovery_worker::RecoveryWorker::start({
                let context = cc.egui_ctx.clone();
                move || context.request_repaint()
            }),
            test_busy: false,
            expand_connection: None,
            expand_database: None,
            tree_selection: TreeSelection::default(),
            filter: String::new(),
            tabs: vec![QueryTab::new(1)],
            active_tab: 0,
            next_tab: 2,
            editor_height: None,
            status: "就绪 · 添加连接即可开始".into(),
            error: false,
            form: None,
            confirm_delete: None,
            schema_dialog: None,
            database_create_pending: None,
            table_create: None,
            table_create_pending: None,
            export_path: None,
            export_rows: None,
            delete_rows: None,
            cell_editor: None,
            csv_import: None,
            theme,
            screenshot_path,
            frames: 0,
            workspace_store: None,
            history: Vec::new(),
            table_grid_layouts: Vec::new(),
            history_open: false,
            history_search: String::new(),
            last_workspace: None,
            last_autosave: std::time::Instant::now(),
            pending_history: HashMap::new(),
            confirm_tab_action: None,
            confirm_rebind: None,
            confirm_exit: false,
            allow_exit: false,
            confirm_request: None,
            confirm_sql: None,
            catalog: Catalog::default(),
            catalog_store: None,
            library_open: false,
            library_search: String::new(),
            catalog_dialog: None,
        };
        if app.screenshot_path.is_none() {
            match Store::local() {
                Ok(store) => match store.load() {
                    Ok(profiles) => {
                        app.selected = profiles.first().map(|p| p.id);
                        app.profiles = profiles;
                        app.store = Some(store);
                    }
                    Err(error) => app.fail(format!("{error:#}")),
                },
                Err(error) => app.fail(format!("{error:#}")),
            }
            app.restore_redis_connections();
            app.restore_workspace();
            app.restore_catalog();
        }
        if app.screenshot_path.is_some()
            && std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref() == Ok("ssh")
        {
            let mut profile = Profile::default();
            profile.ssh.enabled = true;
            profile.ssh.auth = SshAuth::PrivateKey;
            app.form = Some(ConnectionForm::sql(profile, Credentials::default(), false));
        }
        if app.screenshot_path.is_some()
            && std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref() == Ok("redis-connection")
        {
            app.form = Some(ConnectionForm::redis(
                RedisProfile::default(),
                Credentials::default(),
                false,
            ));
        }
        if app.screenshot_path.is_some()
            && matches!(
                std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref(),
                Ok("catalog"
                    | "tree-details"
                    | "tree-document"
                    | "grid-selection"
                    | "browser"
                    | "browser-offline"
                    | "objects-rich"
                    | "objects-icons"
                    | "object-definition"
                    | "query"
                    | "query-offline"
                    | "results"
                    | "query-messages"
                    | "query-error"
                    | "query-error-results"
                    | "query-error-results-grid"
                    | "query-schema"
                    | "query-sqlserver"
                    | "focus"
                    | "grid-browse"
                    | "grid-filters"
                    | "structure"
                    | "structure-edit"
                    | "structure-preview"
                    | "structure-indexes"
                    | "structure-existing-edit"
                    | "structure-existing-preview"
                    | "structure-advanced"
                    | "inline-insert"
                    | "inline-empty"
                    | "cell-edit"
                    | "cell-edit-expanded"
                    | "staged"
                    | "stale")
            )
        {
            let browser_fixture = matches!(
                std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref(),
                Ok("browser"
                    | "tree-details"
                    | "tree-document"
                    | "browser-offline"
                    | "objects-rich"
                    | "objects-icons"
                    | "object-definition"
                    | "query-offline"
                    | "query-messages"
                    | "query-schema"
                    | "query-sqlserver"
                    | "query-error"
                    | "query-error-results"
                    | "query-error-results-grid")
            );
            let sqlserver_fixture =
                std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref() == Ok("query-sqlserver");
            let profile = Profile {
                name: "示例服务器（界面测试）".into(),
                engine: if sqlserver_fixture {
                    Engine::SqlServer
                } else if browser_fixture {
                    Engine::Postgres
                } else {
                    Engine::MySql
                },
                port: if sqlserver_fixture {
                    1433
                } else if browser_fixture {
                    5432
                } else {
                    3306
                },
                ..Default::default()
            };
            app.selected = Some(profile.id);
            let mut session = ConnectionSession::new(&app.context);
            session.connected = Some(profile.clone());
            session.status = "界面示例 · 未建立真实连接".into();
            app.expand_connection = Some(profile.id);
            app.expand_database = Some((profile.id, "app_dev".into()));
            app.profiles = vec![profile];
            session.active_database = Some("app_dev".into());
            session.databases = vec![
                "analytics".into(),
                "app_dev".into(),
                if sqlserver_fixture {
                    "master".into()
                } else if browser_fixture {
                    "postgres".into()
                } else {
                    "mysql".into()
                },
            ];
            session.tables = ["orders", "users"]
                .into_iter()
                .map(|name| Table {
                    schema: if sqlserver_fixture {
                        "dbo".into()
                    } else if browser_fixture {
                        "public".into()
                    } else {
                        "app_dev".into()
                    },
                    name: name.into(),
                    kind: "BASE TABLE".into(),
                })
                .collect();
            for table in &session.tables {
                let target = TableTarget {
                    connection: app.profiles[0].id,
                    database: "app_dev".into(),
                    schema: table.schema.clone(),
                    name: table.name.clone(),
                };
                let (index, _) = open_table_tab(
                    &mut app.tabs,
                    &mut app.next_tab,
                    target,
                    app.profiles[0].engine,
                    table,
                );
                app.active_tab = index;
            }
            app.sessions.insert(app.profiles[0].id, session);
            app.status = "数据库树界面测试 · 示例数据，未建立真实连接".into();
            app.browser_scope = Some(object_browser_ui::BrowserScope {
                connection: app.profiles[0].id,
                database: Some("app_dev".into()),
                schema: None,
            });
            app.browser_state.set_scope(app.browser_scope.as_ref());
            app.browser_visible = matches!(
                std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref(),
                Ok("catalog" | "browser")
            );
            if std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref() == Ok("query") {
                app.active_tab = 0;
                app.tabs[0].target = Some(QueryTarget {
                    schema: None,
                    connection: app.profiles[0].id,
                    database: Some("app_dev".into()),
                });
                app.tabs[0].sql =
                    "SELECT id, name, email\nFROM users\nWHERE id > 0\nORDER BY id\nLIMIT 200;"
                        .into();
                app.tabs[0].saved_sql = app.tabs[0].sql.clone();
            }
            if matches!(
                std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref(),
                Ok("query-messages"
                    | "query-error"
                    | "query-error-results"
                    | "query-error-results-grid"
                    | "query-schema"
                    | "query-sqlserver"
                    | "focus")
            ) {
                app.active_tab = 0;
                app.browser_visible = false;
                let tab = &mut app.tabs[0];
                tab.target = Some(QueryTarget {
                    connection: app.profiles[0].id,
                    database: Some("app_dev".into()),
                    schema: browser_fixture
                        .then(|| if sqlserver_fixture { "dbo" } else { "public" }.into()),
                });
                tab.sql = if sqlserver_fixture {
                    "SELECT id, name FROM [dbo].[users] WHERE id = 0;"
                } else {
                    "SELECT id, name FROM users WHERE id = 0;"
                }
                .into();
                tab.saved_sql = tab.sql.clone();
                tab.source = format!(
                    "示例服务器 / app_dev / {} · 未建立真实连接",
                    if sqlserver_fixture { "dbo" } else { "public" }
                );
                tab.last_executed_sql = Some(tab.sql.clone());
                tab.last_executed_document = Some(tab.sql.clone());
                tab.last_executed_char_start = Some(0);
                if std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref() == Ok("query-messages") {
                    tab.query_started = true;
                    tab.output = Some(QueryOutput {
                        sets: vec![
                            ResultSet {
                                affected: Some(3),
                                ..Default::default()
                            },
                            ResultSet {
                                columns: vec!["id".into(), "name".into()],
                                column_types: vec!["bigint".into(), "varchar".into()],
                                ..Default::default()
                            },
                        ],
                        elapsed_ms: 12,
                        messages: vec![sqlshop::model::QueryMessage {
                            severity: "NOTICE".into(),
                            code: Some("00000".into()),
                            text: "处理完成：更新 3 条记录（界面示例）".into(),
                            ..Default::default()
                        }],
                    });
                    app.context.data_mut(|data| {
                        data.insert_temp(egui::Id::new(("query-result-messages", tab.id)), true)
                    });
                }
                if matches!(
                    std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref(),
                    Ok("query-error" | "query-error-results" | "query-error-results-grid")
                ) {
                    tab.sql = "SELECT missing_column
FROM users;"
                        .into();
                    tab.saved_sql = tab.sql.clone();
                    tab.last_executed_document = Some(tab.sql.clone());
                    tab.last_executed_sql = Some(tab.sql.clone());
                    tab.query_started = true;
                    tab.query_elapsed_ms = Some(17);
                    tab.query_error =
                        Some("column \"missing_column\" does not exist（离线示例）".into());
                    tab.query_diagnostic = Some(sqlshop::model::QueryDiagnostic {
                        code: Some("42703".into()),
                        position: Some(sqlshop::model::QueryErrorPosition {
                            line: 1,
                            column: Some(8),
                            char_offset: 7,
                        }),
                        ..Default::default()
                    });
                    if std::env::var("SQLSHOP_SCREENSHOT_VIEW")
                        .unwrap_or_default()
                        .starts_with("query-error-results")
                    {
                        tab.sql =
                            "SELECT id, name FROM users;\nSELECT missing_column FROM users;".into();
                        tab.saved_sql = tab.sql.clone();
                        tab.last_executed_document = Some(tab.sql.clone());
                        tab.last_executed_sql = Some(tab.sql.clone());
                        tab.query_diagnostic.as_mut().unwrap().position =
                            Some(sqlshop::model::QueryErrorPosition {
                                line: 2,
                                column: Some(8),
                                char_offset: "SELECT id, name FROM users;\nSELECT ".chars().count(),
                            });
                        tab.output = Some(QueryOutput {
                            sets: vec![ResultSet {
                                columns: vec!["id".into(), "name".into()],
                                column_types: vec!["bigint".into(), "varchar".into()],
                                rows: vec![vec![
                                    Some("1".into()),
                                    Some("完整返回的前序结果".into()),
                                ]],
                                ..Default::default()
                            }],
                            elapsed_ms: 17,
                            messages: Vec::new(),
                        });
                        if std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref()
                            == Ok("query-error-results")
                        {
                            query_result_ui::show_error_view(&app.context, tab.id);
                        }
                    }
                }
                if std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref() == Ok("focus") {
                    app.view.focus = true;
                }
            }
            if std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref() == Ok("browser") {
                let tables = &mut app.sessions.get_mut(&app.profiles[0].id).unwrap().tables;
                tables.push(Table {
                    schema: "public".into(),
                    name: "active_users".into(),
                    kind: "VIEW".into(),
                });
                tables.push(Table {
                    schema: "audit".into(),
                    name: "events".into(),
                    kind: "BASE TABLE".into(),
                });
                app.browser_state.selected = Some(object_browser_ui::BrowserItem::Object {
                    connection: app.profiles[0].id,
                    database: "app_dev".into(),
                    table: tables[1].clone(),
                });
            }

            if matches!(
                std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref(),
                Ok("results"
                    | "tree-document"
                    | "grid-selection"
                    | "grid-browse"
                    | "grid-filters"
                    | "inline-insert"
                    | "inline-empty"
                    | "cell-edit"
                    | "cell-edit-expanded"
                    | "staged"
                    | "stale")
            ) {
                let tab = &mut app.tabs[app.active_tab];
                let columns: Vec<_> = ["id", "name", "email", "note", "amount", "created_at"]
                    .into_iter()
                    .enumerate()
                    .map(|(index, name)| sqlshop::model::TableColumn {
                        name: name.into(),
                        sql_type: "文本".into(),
                        declared_type: [
                            "bigint",
                            "varchar(100)",
                            "varchar(256)",
                            "text",
                            "decimal(20,4)",
                            "datetime2(3)",
                        ][index]
                            .into(),
                        kind: sqlshop::model::CellKind::Text,
                        primary_key: index == 0,
                        editable: true,
                    })
                    .collect();
                let rows: Vec<Vec<Option<String>>> = vec![
                    vec![
                        Some("1".into()),
                        Some("小云".into()),
                        Some("xiaoyun@example.com".into()),
                        None,
                        Some("128.50".into()),
                        Some("2026-10-12 13:25:03".into()),
                    ],
                    vec![
                        Some("2".into()),
                        Some("小林".into()),
                        Some("xiaolin@example.com".into()),
                        Some("支持双击编辑单元格".into()),
                        Some("60.00".into()),
                        Some("2026-10-12 13:26:15".into()),
                    ],
                    vec![
                        Some("3".into()),
                        Some("小陈".into()),
                        Some("xiaocheng@example.com".into()),
                        Some(String::new()),
                        Some("240.80".into()),
                        Some("2026-10-12 13:30:22".into()),
                    ],
                ];
                let keys = rows.iter().map(|row| vec![row[0].clone()]).collect();
                tab.output = Some(QueryOutput {
                    sets: vec![sqlshop::model::ResultSet {
                        columns: columns.iter().map(|c| c.name.clone()).collect(),
                        column_types: columns.iter().map(|c| c.declared_type.clone()).collect(),
                        rows,
                        ..Default::default()
                    }],
                    elapsed_ms: 12,
                    messages: Vec::new(),
                });
                tab.source = "示例服务器（界面测试） / app_dev".into();
                tab.table_data = Some(TableData {
                    output: QueryOutput::default(),
                    primary_keys: vec!["id".into()],
                    writable: true,
                    keys,
                    columns,
                });
                tab.query_started = true;
                tab.selected_cell = Some((1, 1));
                app.editor_height = Some(100.0);
                if matches!(
                    std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref(),
                    Ok("inline-insert" | "inline-empty")
                ) {
                    tab.table_data.as_mut().unwrap().columns[0].editable = false;
                    let mut draft = table_insert_ui::InsertDraft::new(6);
                    draft.values[1] = InsertValue::Text("新用户".into());
                    draft.values[2] = InsertValue::Text("new@example.com".into());
                    draft.values[3] = InsertValue::Null;
                    draft.values[4] = InsertValue::Text("0.00".into());
                    tab.selected_insert = Some(draft.id);
                    tab.selected_cell = None;
                    tab.insert_drafts.push(draft);
                    let mut second = table_insert_ui::InsertDraft::new(6);
                    second.values[1] = InsertValue::Text(String::new());
                    second.values[3] = InsertValue::Text(String::new());
                    tab.insert_drafts.push(second);
                    if std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref() == Ok("inline-empty") {
                        tab.output.as_mut().unwrap().sets[0].rows.clear();
                        tab.table_data.as_mut().unwrap().keys.clear();
                    } else {
                        let set = &tab.output.as_ref().unwrap().sets[0];
                        tab.stage_cell(CellUpdate {
                            database: "app_dev".into(),
                            table: Table {
                                schema: "app_dev".into(),
                                name: "users".into(),
                                kind: "BASE TABLE".into(),
                            },
                            primary_keys: vec!["id".into()],
                            key_values: tab.table_data.as_ref().unwrap().keys[0].clone(),
                            column: "name".into(),
                            original: set.rows[0][1].clone(),
                            value: Some("暂存修改".into()),
                        });
                    }
                    app.status =
                        "网格内新增草稿示例 · 默认值、NULL、空串独立 · 未连接真实数据库".into();
                }
                if matches!(
                    std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref(),
                    Ok("grid-browse" | "grid-filters")
                ) {
                    tab.browse.sorts = vec![
                        ColumnSort {
                            column: "id".into(),
                            descending: false,
                        },
                        ColumnSort {
                            column: "name".into(),
                            descending: true,
                        },
                    ];
                    tab.browse.filters = vec![ColumnFilter {
                        column: "name".into(),
                        operator: FilterOperator::IsNotNull,
                        value: None,
                    }];
                    tab.browse_draft.sync_sorts(&tab.browse.sorts);
                    tab.browse_draft
                        .add_filter("name".into(), FilterOperator::IsNotNull, None);
                    tab.filters_open =
                        std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref() == Ok("grid-filters");
                    if tab.filters_open {
                        tab.browse_draft
                            .add_filter("note".into(), FilterOperator::IsNull, None);
                    }
                    app.status = "表格筛选与排序界面示例 · 未连接真实数据库".into();
                }
                if std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref() == Ok("staged") {
                    for (row, column, value) in [
                        (0, 0, Some("101")),
                        (0, 1, Some("暂存姓名")),
                        (1, 2, None),
                        (1, 3, Some("")),
                    ] {
                        let set = &tab.output.as_ref().unwrap().sets[0];
                        tab.stage_cell(CellUpdate {
                            database: "app_dev".into(),
                            table: Table {
                                schema: "app_dev".into(),
                                name: "users".into(),
                                kind: "BASE TABLE".into(),
                            },
                            primary_keys: vec!["id".into()],
                            key_values: tab.table_data.as_ref().unwrap().keys[row].clone(),
                            column: set.columns[column].clone(),
                            original: set.rows[row][column].clone(),
                            value: value.map(str::to_owned),
                        });
                    }
                    app.status = "暂存编辑界面测试 · 网格显示草稿，复制/导出使用数据库快照 · 未连接真实数据库".into();
                }
                if std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref() == Ok("stale") {
                    tab.table_write_saved("已成功提交 4 格修改；刷新失败".into());
                    app.status = "示例写入已成功；刷新失败，显示过期快照。请重新刷新后编辑 · 未连接真实数据库".into();
                    app.error = true;
                }
                if matches!(
                    std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref(),
                    Ok("cell-edit" | "cell-edit-expanded")
                ) {
                    tab.selected_rows = None;
                    tab.selected_cell = Some((1, 3));
                    tab.cell_anchor = Some((1, 3));
                    app.cell_editor = Some(CellEditor {
                        tab: tab.id,
                        connection: app.profiles[0].id,
                        change: CellUpdate {
                            database: "app_dev".into(),
                            table: Table {
                                schema: "app_dev".into(),
                                name: "users".into(),
                                kind: "BASE TABLE".into(),
                            },
                            primary_keys: vec!["id".into()],
                            key_values: vec![Some("2".into())],
                            column: "note".into(),
                            original: Some("支持双击编辑单元格".into()),
                            value: None,
                        },
                        text: "这里直接填写记录备注".into(),
                        is_null: false,
                        focus: true,
                        expanded: false,
                    });
                    if std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref()
                        == Ok("cell-edit-expanded")
                    {
                        let editor = app.cell_editor.as_mut().unwrap();
                        editor.expanded = true;
                        editor.text =
                            "第一行记录备注\n这里可输入长文本和换行，暂存后再统一提交。".into();
                    }
                }
            }
        }
        if app.screenshot_path.is_some()
            && matches!(
                std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref(),
                Ok("browser-offline" | "query-offline")
            )
        {
            let connection = app.profiles[0].id;
            if let Some(session) = app.sessions.get_mut(&connection) {
                session.connected = None;
                session.active_database = None;
                session.databases.clear();
                session.tables.clear();
                session.object_catalog = None;
                session.catalog_loading = false;
                session.status = "界面示例 · 未连接".into();
            }
            app.tabs.truncate(1);
            app.tabs[0].target = None;
            app.active_tab = 0;
            app.browser_scope = Some(object_browser_ui::BrowserScope {
                connection,
                database: None,
                schema: None,
            });
            app.browser_state.set_scope(app.browser_scope.as_ref());
            app.browser_visible =
                std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref() == Ok("browser-offline");
        }
        if app.screenshot_path.is_some()
            && matches!(
                std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref(),
                Ok("structure"
                    | "structure-edit"
                    | "structure-preview"
                    | "structure-indexes"
                    | "structure-existing-edit"
                    | "structure-existing-preview"
                    | "structure-advanced")
            )
        {
            let source = app.tabs[app.active_tab].table_target.clone().unwrap();
            let table = Table {
                schema: source.schema,
                name: source.name,
                kind: "BASE TABLE".into(),
            };
            app.open_structure_document(source.connection, source.database, table.clone());
            let mode = std::env::var("SQLSHOP_SCREENSHOT_VIEW").unwrap();
            let target = app.tabs[app.active_tab].target.clone().unwrap();
            let mut doc = structure_ui::DesignDocument::example(&table, target, &mode);
            if mode == "structure-indexes" {
                doc.section = structure_ui::DesignSection::Indexes;
            }
            app.tabs[app.active_tab].design = Some(doc);
        }
        if app.screenshot_path.is_some()
            && matches!(
                std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref(),
                Ok("objects-rich" | "objects-icons" | "object-definition")
            )
        {
            let catalog = object_browser_host::example_catalog();
            let connection = app.profiles[0].id;
            let users = catalog
                .objects
                .iter()
                .find(|object| object.name == "customers")
                .unwrap()
                .as_table()
                .unwrap();
            if let Some(session) = app.sessions.get_mut(&connection) {
                session.tables = catalog
                    .objects
                    .iter()
                    .filter_map(|object| object.as_table())
                    .collect();
                session.object_catalog = Some(catalog.clone());
            }
            app.browser_visible = true;
            app.browser_state.selected = Some(object_browser_ui::BrowserItem::Object {
                connection,
                database: "app_dev".into(),
                table: users,
            });
            if std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref() == Ok("objects-icons") {
                app.browser_state.set_icon_view();
            }
            if std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref() == Ok("object-definition") {
                let object = catalog
                    .objects
                    .iter()
                    .find(|object| object.name == "calculate_total")
                    .unwrap()
                    .clone();
                app.open_object_definition(connection, "app_dev".into(), object);
                app.tabs[app.active_tab].definition.as_mut().unwrap().sql = Some("-- Offline layout example; no database connection.\nCREATE OR REPLACE FUNCTION public.calculate_total(amount numeric, tax_rate numeric)\nRETURNS numeric\nLANGUAGE sql\nIMMUTABLE\nAS $function$\n  SELECT round(amount * (1 + tax_rate), 2);\n$function$;".into());
            }
        }
        app.management_fixture();
        app.table_tree_fixture();
        app
    }
}
