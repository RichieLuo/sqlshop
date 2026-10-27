mod tabs;

mod grid;

mod database_tree;
mod navigation;
mod panel_layout;

mod sessions;

mod query_editor;

mod workspace;

mod cell_actions;

mod layout;

mod cell_editor;

mod file_open;
mod table_drafts;
mod table_rename;

use super::*;

fn grid_test_cell_position(output: &egui::FullOutput, column: &str, row: &str) -> egui::Pos2 {
    let text_rect = |label: &str| {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("grid label {label} should render"))
    };
    egui::pos2(text_rect(column).center().x, text_rect(row).center().y)
}

fn table_flow_fixture() -> (App, QueryTarget, CellUpdate) {
    let mut app = session_test_app();
    let target = add_test_session(&mut app, "A", "db_a");
    let table = app.sessions[&target.connection].tables[0].clone();
    let tab = &mut app.tabs[0];
    tab.target = Some(target.clone());
    tab.table_target = Some(TableTarget {
        connection: target.connection,
        database: "db_a".into(),
        schema: table.schema.clone(),
        name: table.name.clone(),
    });
    let mut page = table_flow_page();
    tab.output = Some(std::mem::take(&mut page.data.output));
    tab.table_data = Some(page.data);
    let change = CellUpdate {
        database: "db_a".into(),
        table,
        primary_keys: vec!["id".into()],
        key_values: vec![Some("7".into())],
        column: "note".into(),
        original: Some("original".into()),
        value: Some("changed".into()),
    };
    (app, target, change)
}

fn table_flow_page() -> sqlshop::db::TablePage {
    sqlshop::db::TablePage {
        data: TableData {
            output: QueryOutput {
                sets: vec![ResultSet {
                    columns: vec!["id".into(), "note".into()],
                    rows: vec![vec![Some("7".into()), Some("original".into())]],
                    ..Default::default()
                }],
                ..Default::default()
            },
            columns: [("id", true), ("note", false)]
                .into_iter()
                .map(|(name, primary_key)| sqlshop::model::TableColumn {
                    name: name.into(),
                    declared_type: "text".into(),
                    sql_type: "text".into(),
                    kind: sqlshop::model::CellKind::Text,
                    editable: true,
                    primary_key,
                })
                .collect(),
            primary_keys: vec!["id".into()],
            writable: true,
            keys: vec![vec![Some("7".into())]],
        },
        has_more: false,
        sql: "SELECT id,note FROM public.items LIMIT 201".into(),
        parameters: vec![],
    }
}

fn inline_grid_frame(app: &mut App, events: Vec<egui::Event>) -> egui::FullOutput {
    let ctx = app.context.clone();
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 820.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let actions = app.table_draft_action_state();
                let (editor, action) = result_grid_inline(
                    ui,
                    &mut app.tabs[0],
                    true,
                    app.cell_editor.as_mut(),
                    actions,
                );
                if let Some(editor) = editor {
                    app.cell_editor = Some(editor);
                }
                if let Some(GridAction::FinishCell { cancel, movement }) = action {
                    app.finish_inline_cell(cancel, movement);
                }
            });
        },
    )
}

fn click_inline_grid(app: &mut App, position: egui::Pos2) -> egui::FullOutput {
    for pressed in [true, false] {
        let output = inline_grid_frame(
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
        if !pressed {
            return output;
        }
    }
    unreachable!()
}

fn grid_text_position(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == label => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing grid label {label}"))
}

pub(super) fn session_test_app() -> App {
    let context = egui::Context::default();
    App {
        advanced_worker: sqlshop::advanced_worker::AdvancedWorker::new(context.clone()),
        compare_worker: sqlshop::compare_worker::CompareWorker::start(),
        backup_worker: sqlshop::backup_worker::BackupWorker::start(),
        job_scheduler: None,
        jobs_loaded: true,
        last_job_tick: std::time::Instant::now(),
        logo: context.load_texture(
            "test",
            egui::ColorImage::new([1, 1], vec![Color32::WHITE]),
            Default::default(),
        ),
        store: None,
        profiles: Vec::new(),
        redis_profiles: Vec::new(),
        redis_config_error: None,
        redis_test_worker: None,
        credentials: HashMap::new(),
        selected: None,
        sessions: HashMap::new(),
        context: context.clone(),
        query_sessions: HashMap::new(),
        browser_state: object_browser_ui::BrowserState::default(),
        browser_scope: None,
        browser_visible: false,
        inspector_visible: true,
        view: ViewLayout::default(),
        test_worker: Worker::start(|| {}),
        recovery_worker: sqlshop::recovery_worker::RecoveryWorker::start(|| {}),
        test_busy: false,
        expand_connection: None,
        expand_database: None,
        tree_selection: TreeSelection::default(),
        filter: String::new(),
        tabs: vec![QueryTab::new(1)],
        active_tab: 0,
        next_tab: 2,
        editor_height: None,
        status: String::new(),
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
        theme: egui::ThemePreference::System,
        screenshot_path: None,
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
        catalog: Catalog::default(),
        catalog_store: None,
        library_open: false,
        catalog_dialog: None,
    }
}

fn add_test_session(app: &mut App, name: &str, database: &str) -> QueryTarget {
    let profile = Profile {
        name: name.into(),
        ..Default::default()
    };
    let target = QueryTarget {
        schema: None,
        connection: profile.id,
        database: Some(database.into()),
    };
    let mut session = ConnectionSession::new(&app.context);
    session.connected = Some(profile.clone());
    session.active_database = target.database.clone();
    session.databases = vec![database.into()];
    session.tables = vec![Table {
        schema: "public".into(),
        name: format!("{database}_table"),
        kind: "BASE TABLE".into(),
    }];
    app.profiles.push(profile);
    app.sessions.insert(target.connection, session);
    target
}

fn table() -> Table {
    Table {
        schema: "public".into(),
        name: "users".into(),
        kind: "BASE TABLE".into(),
    }
}
