use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use sqlshop::{
    db::{ColumnFilter, ColumnSort, FilterOperator, TableBrowseOptions, TableStructure},
    model::{
        CellUpdate, Credentials, Engine, Profile, QueryOutput, SshAuth, Table, TableData,
        export_csv,
    },
    sql_editor::{byte_to_char_index, find_matches, highlight_sql, replace_all, replace_match},
    storage::Store,
    worker::{Request, Response, Worker},
    workspace::{
        HistoryEntry, QueryTarget, SavedTab, Workspace, WorkspaceStore, read_sql_file,
        write_sql_file,
    },
};
use std::{collections::HashMap, path::PathBuf};
use uuid::Uuid;

mod catalog_ui;
mod icons;
mod window_chrome;
mod workspace_ui;
use catalog_ui::CatalogDialog;
use sqlshop::catalog::{Catalog, CatalogStore};

#[derive(Clone, PartialEq, Eq)]
struct TableTarget {
    connection: Uuid,
    database: String,
    schema: String,
    name: String,
}

struct QueryTab {
    id: Uuid,
    name: String,
    sql: String,
    last_executed_sql: Option<String>,
    selection: String,
    output: Option<QueryOutput>,
    query_started: bool,
    source: String,
    result_index: usize,
    table_target: Option<TableTarget>,
    table_data: Option<TableData>,
    selected_cell: Option<(usize, usize)>,
    selected_rows: Option<(usize, usize)>,
    grid_width: f32,
    target: Option<QueryTarget>,
    file_path: Option<PathBuf>,
    saved_sql: String,
    browse: TableBrowseOptions,
    has_more: bool,
    filters_open: bool,
    browse_draft: workspace_ui::TableBrowseDraft,
    cell_anchor: Option<(usize, usize)>,
    find_open: bool,
    find_text: String,
    replace_text: String,
    find_case: bool,
    find_index: usize,
    select_range: Option<std::ops::Range<usize>>,
}

impl QueryTab {
    fn new(index: usize) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: format!("查询 {index}"),
            sql: "-- 在这里编写 SQL；⌘ / Ctrl + Enter 执行选中内容或整段 SQL\nSELECT 1 AS hello;"
                .into(),
            last_executed_sql: None,
            selection: String::new(),
            output: None,
            query_started: false,
            source: String::new(),
            result_index: 0,
            table_target: None,
            table_data: None,
            selected_cell: None,
            selected_rows: None,
            grid_width: 0.0,
            target: None,
            file_path: None,
            saved_sql:
                "-- 在这里编写 SQL；⌘ / Ctrl + Enter 执行选中内容或整段 SQL\nSELECT 1 AS hello;"
                    .into(),
            browse: TableBrowseOptions::default(),
            has_more: false,
            filters_open: false,
            browse_draft: workspace_ui::TableBrowseDraft::default(),
            cell_anchor: None,
            find_open: false,
            find_text: String::new(),
            replace_text: String::new(),
            find_case: false,
            find_index: 0,
            select_range: None,
        }
    }
}

struct ConnectionForm {
    profile: Profile,
    password: String,
    ssh_secret: String,
}

struct CellEditor {
    tab: Uuid,
    connection: Uuid,
    change: CellUpdate,
    text: String,
    is_null: bool,
    focus: bool,
}

#[derive(Clone, Copy)]
enum TabAction {
    Select(usize),
    New,
    Close(usize),
    CloseOthers(usize),
    CloseRight(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TreeNode {
    Connection(Uuid),
    Database(Uuid, String),
    Table(Uuid, String, Table),
}

// Tree focus is independent of the connected server and active query database.
#[derive(Default)]
struct TreeSelection {
    selected: Option<TreeNode>,
    rows: Vec<(egui::layers::ShapeIdx, egui::Rect, TreeNode)>,
}

impl TreeSelection {
    fn row(
        &mut self,
        response: &egui::Response,
        background: egui::layers::ShapeIdx,
        node: TreeNode,
    ) {
        if response.clicked() || response.secondary_clicked() {
            self.selected = Some(node.clone());
        }
        self.rows.push((background, response.rect, node));
    }

    fn paint(&mut self, ui: &egui::Ui) {
        // Paint after all clicks are handled, so even this frame has only one selection.
        for (background, mut rect, node) in self.rows.drain(..) {
            if self.selected.as_ref() == Some(&node) {
                rect.min.x = ui.max_rect().left();
                rect.max.x = ui.max_rect().right();
                ui.painter().set(
                    background,
                    egui::Shape::rect_filled(
                        rect,
                        ui.visuals().widgets.inactive.corner_radius,
                        accent(ui).gamma_multiply(if ui.visuals().dark_mode { 0.30 } else { 0.14 }),
                    ),
                );
            }
        }
    }
}

struct ConnectionSession {
    worker: Worker,
    connected: Option<Profile>,
    databases: Vec<String>,
    active_database: Option<String>,
    tables: Vec<Table>,
    busy: bool,
    running_tab: Option<Uuid>,
    raw_session_used: bool,
    status: String,
    error: bool,
}

impl ConnectionSession {
    fn new(ctx: &egui::Context) -> Self {
        let ctx = ctx.clone();
        Self {
            worker: Worker::start(move || ctx.request_repaint()),
            connected: None,
            databases: Vec::new(),
            active_database: None,
            tables: Vec::new(),
            busy: false,
            running_tab: None,
            raw_session_used: false,
            status: "未连接".into(),
            error: false,
        }
    }
}

pub struct App {
    logo: egui::TextureHandle,
    store: Option<Store>,
    profiles: Vec<Profile>,
    credentials: HashMap<Uuid, Credentials>,
    selected: Option<Uuid>,
    sessions: HashMap<Uuid, ConnectionSession>,
    context: egui::Context,
    test_worker: Worker,
    test_busy: bool,
    expand_connection: Option<Uuid>,
    expand_database: Option<(Uuid, String)>,
    tree_selection: TreeSelection,
    filter: String,
    tabs: Vec<QueryTab>,
    active_tab: usize,
    next_tab: usize,
    editor_height: Option<f32>,
    status: String,
    error: bool,
    form: Option<ConnectionForm>,
    confirm_delete: Option<Uuid>,
    export_path: Option<String>,
    cell_editor: Option<CellEditor>,
    theme: egui::ThemePreference,
    screenshot_path: Option<PathBuf>,
    frames: usize,
    workspace_store: Option<WorkspaceStore>,
    history: Vec<HistoryEntry>,
    history_open: bool,
    history_search: String,
    last_workspace: String,
    last_autosave: std::time::Instant,
    pending_history: HashMap<Uuid, HistoryEntry>,
    confirm_tab_action: Option<TabAction>,
    confirm_rebind: Option<(Uuid, QueryTarget)>,
    confirm_exit: bool,
    allow_exit: bool,
    structure: Option<(Uuid, TableStructure)>,
    confirm_request: Option<(Uuid, Request, String)>,
    catalog: Catalog,
    catalog_store: Option<CatalogStore>,
    library_open: bool,
    library_search: String,
    catalog_dialog: Option<CatalogDialog>,
}

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
            logo: crate::branding::load_texture(&cc.egui_ctx),
            store: None,
            profiles: Vec::new(),
            credentials: HashMap::new(),
            selected: None,
            sessions: HashMap::new(),
            context: cc.egui_ctx.clone(),
            test_worker: Worker::start(move || context.request_repaint()),
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
            export_path: None,
            cell_editor: None,
            theme,
            screenshot_path,
            frames: 0,
            workspace_store: None,
            history: Vec::new(),
            history_open: false,
            history_search: String::new(),
            last_workspace: String::new(),
            last_autosave: std::time::Instant::now(),
            pending_history: HashMap::new(),
            confirm_tab_action: None,
            confirm_rebind: None,
            confirm_exit: false,
            allow_exit: false,
            structure: None,
            confirm_request: None,
            catalog: Catalog::default(),
            catalog_store: None,
            library_open: false,
            library_search: String::new(),
            catalog_dialog: None,
        };
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
        if app.screenshot_path.is_none() {
            app.restore_workspace();
            app.restore_catalog();
        }
        if app.screenshot_path.is_some()
            && std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref() == Ok("ssh")
        {
            let mut profile = Profile::default();
            profile.ssh.enabled = true;
            profile.ssh.auth = SshAuth::PrivateKey;
            app.form = Some(ConnectionForm {
                profile,
                password: String::new(),
                ssh_secret: String::new(),
            });
        }
        if app.screenshot_path.is_some()
            && matches!(
                std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref(),
                Ok("catalog" | "results")
            )
        {
            let profile = Profile {
                name: "示例服务器（界面测试）".into(),
                ..Default::default()
            };
            app.selected = Some(profile.id);
            let mut session = ConnectionSession::new(&app.context);
            session.connected = Some(profile.clone());
            app.expand_connection = Some(profile.id);
            app.expand_database = Some((profile.id, "app_dev".into()));
            app.profiles = vec![profile];
            session.active_database = Some("app_dev".into());
            session.databases = vec!["analytics".into(), "app_dev".into(), "mysql".into()];
            session.tables = ["orders", "users"]
                .into_iter()
                .map(|name| Table {
                    schema: "app_dev".into(),
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
            if std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref() == Ok("results") {
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
                });
                tab.source = "示例服务器（界面测试） / app_dev".into();
                tab.table_data = Some(TableData {
                    output: QueryOutput::default(),
                    primary_keys: vec!["id".into()],
                    keys,
                    columns,
                });
                tab.query_started = true;
                tab.selected_cell = Some((1, 1));
                app.editor_height = Some(100.0);
            }
        }
        app
    }

    fn fail(&mut self, message: String) {
        self.status = message;
        self.error = true;
    }

    fn selected_session(&self) -> Option<&ConnectionSession> {
        self.selected.and_then(|id| self.sessions.get(&id))
    }

    fn session_busy(&self, id: Uuid) -> bool {
        self.sessions.get(&id).is_some_and(|s| s.busy)
    }

    fn target_ready(&self, target: &QueryTarget) -> bool {
        self.sessions
            .get(&target.connection)
            .is_some_and(|s| s.connected.is_some() && s.active_database == target.database)
    }

    fn execution_target(&self) -> Option<QueryTarget> {
        self.tabs[self.active_tab]
            .target
            .clone()
            .or_else(|| self.current_target())
    }

    fn can_execute(&self) -> bool {
        self.execution_target()
            .is_some_and(|t| self.target_ready(&t) && !self.session_busy(t.connection))
    }

    fn tab_busy(&self, index: usize) -> bool {
        self.tabs[index]
            .target
            .as_ref()
            .is_some_and(|t| self.session_busy(t.connection))
    }

    fn any_busy(&self) -> bool {
        self.test_busy || self.sessions.values().any(|s| s.busy)
    }

    fn request(&mut self, request: Request, status: &str) {
        if matches!(&request, Request::Test(..)) {
            if self.test_busy {
                return;
            }
            match self.test_worker.send(request) {
                Ok(()) => {
                    self.test_busy = true;
                    self.status = status.into();
                    self.error = false;
                }
                Err(error) => self.fail(error.to_string()),
            }
            return;
        }
        let connection = match &request {
            Request::Connect(profile, _) => Some(profile.id),
            Request::RunQuery { target, .. }
            | Request::ReadPage { target, .. }
            | Request::DescribeTable { target, .. }
            | Request::SaveCell { target, .. } => Some(target.connection),
            _ => self.selected,
        };
        if let Some(id) = connection {
            self.request_on(id, request, status);
        }
    }

    fn request_on(&mut self, id: Uuid, request: Request, status: &str) {
        self.request_on_impl(id, request, status, false);
    }

    fn request_on_impl(&mut self, id: Uuid, request: Request, status: &str, confirmed: bool) {
        if self.session_busy(id) {
            return;
        }
        if self
            .cell_editor
            .as_ref()
            .is_some_and(|e| e.connection == id)
            && matches!(
                &request,
                Request::Connect(..) | Request::Disconnect | Request::SelectDatabase(..)
            )
        {
            self.fail("请先保存或取消此连接的单元格修改".into());
            return;
        }
        let session = self
            .sessions
            .entry(id)
            .or_insert_with(|| ConnectionSession::new(&self.context));
        if !confirmed
            && session.raw_session_used
            && matches!(
                &request,
                Request::Connect(..) | Request::Disconnect | Request::SelectDatabase(..)
            )
        {
            self.confirm_request = Some((id, request, status.into()));
            return;
        }
        let tab = match &request {
            Request::RunQuery { tab, .. }
            | Request::ReadPage { tab, .. }
            | Request::DescribeTable { tab, .. }
            | Request::SaveCell { tab, .. } => Some(*tab),
            _ => None,
        };
        let raw = matches!(&request, Request::RunQuery { .. } | Request::Query(..));
        match session.worker.send(request) {
            Ok(()) => {
                session.busy = true;
                session.running_tab = tab;
                session.raw_session_used |= raw;
                session.error = false;
                session.status = status.into();
                self.error = false;
                self.status = status.into();
            }
            Err(error) => self.fail(error.to_string()),
        }
    }

    fn poll(&mut self) {
        while let Ok(response) = self.test_worker.receiver.try_recv() {
            self.test_busy = false;
            match response {
                Response::Tested => {
                    self.status = "连接测试成功（未改变现有会话）".into();
                    self.error = false;
                }
                Response::Error { message, .. } => self.fail(message),
                _ => {}
            }
        }
        let responses: Vec<_> = self
            .sessions
            .iter()
            .flat_map(|(id, session)| {
                session
                    .worker
                    .receiver
                    .try_iter()
                    .map(|response| (*id, response))
                    .collect::<Vec<_>>()
            })
            .collect();
        for (connection, response) in responses {
            self.handle_response(connection, response);
        }
    }

    fn handle_response(&mut self, connection: Uuid, response: Response) {
        let Some(mut session) = self.sessions.remove(&connection) else {
            return;
        };
        if let Response::DatabaseContext(database) = &response {
            if session.active_database != *database {
                session.active_database = database.clone();
                session.tables.clear();
            }
            self.sessions.insert(connection, session);
            return;
        }
        session.busy = false;
        self.error = false;
        let mut follow_up = None;
        match response {
            Response::DatabaseContext(_) => unreachable!(),
            Response::QueryResult {
                tab: id,
                output,
                database,
            } => {
                if let Some(entry) = self.pending_history.get(&id)
                    && let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id)
                {
                    tab.last_executed_sql = Some(entry.sql.clone());
                }
                self.finish_history(id, true);
                let changed = session.active_database != database;
                session.active_database = database.clone();
                self.status = format!(
                    "执行完成 · {} ms · {} 个结果集",
                    output.elapsed_ms,
                    output.sets.len()
                );
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.output = Some(output);
                    tab.result_index = 0;
                    // Only explicit SQL context changes rebind the executing query tab.
                    // Other tabs retain their original database and will reject a mismatch.
                    if tab.table_target.is_none()
                        && let Some(target) = &mut tab.target
                    {
                        target.database = database;
                    }
                }
                if changed {
                    session.tables.clear();
                    if session.active_database.is_some() {
                        follow_up = Some((Request::Tables, "数据库已切换 · 正在读取表目录…"));
                    }
                }
            }
            Response::TablePage { tab: id, mut page } => {
                self.status = format!(
                    "已读取表分页 · {} 行",
                    page.data
                        .output
                        .sets
                        .first()
                        .map_or(0, |set| set.rows.len())
                );
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.last_executed_sql = Some(page.execution_sql());
                    if !tab.dirty() {
                        tab.sql = page.sql;
                        tab.saved_sql = tab.sql.clone();
                    }
                    tab.output = Some(std::mem::take(&mut page.data.output));
                    tab.table_data = Some(page.data);
                    tab.has_more = page.has_more;
                    tab.result_index = 0;
                    tab.selected_cell = None;
                    tab.cell_anchor = None;
                    tab.selected_rows = None;
                }
            }
            Response::TableStructure { tab, structure } => {
                self.structure = Some((tab, structure));
                self.status = "表结构已读取".into();
            }
            Response::OperationError {
                tab,
                message,
                disconnected,
            } => {
                self.finish_history(tab, false);
                if disconnected {
                    session.connected = None;
                    session.active_database = None;
                    session.databases.clear();
                    session.tables.clear();
                    session.raw_session_used = false;
                }
                self.fail(message);
            }
            Response::Tested => self.status = "连接测试成功".into(),
            Response::Connected(id) => {
                session.raw_session_used = false;
                session.connected = self.profiles.iter().find(|p| p.id == id).cloned();
                session.databases.clear();
                session.active_database = None;
                session.tables.clear();
                self.expand_connection = Some(id);
                follow_up = Some((Request::Databases, "已连接服务器 · 正在读取数据库列表…"));
            }
            Response::Disconnected => {
                session.raw_session_used = false;
                session.connected = None;
                session.databases.clear();
                session.active_database = None;
                session.tables.clear();
                self.status = "已断开连接".into();
            }
            Response::Databases(databases) => {
                self.status = format!(
                    "已连接服务器 · {} 个数据库 · 点击数据库选择",
                    databases.len()
                );
                // Discovery must not silently alter the worker's database context.
                session.databases = databases;
            }
            Response::DatabaseSelected(database) => {
                session.active_database = Some(database.clone());
                session.tables.clear();
                self.expand_database = Some((connection, database));
                follow_up = Some((Request::Tables, "已选择数据库 · 正在读取表和视图…"));
            }
            Response::Tables(tables) => {
                self.status = format!("已连接 · {} 个表 / 视图", tables.len());
                session.tables = tables;
            }
            Response::TableData { tab, mut data } => {
                self.status = format!(
                    "已读取表数据 · {} 行",
                    data.output.sets.first().map_or(0, |set| set.rows.len())
                );
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab) {
                    tab.output = Some(std::mem::take(&mut data.output));
                    tab.table_data = Some(data);
                    tab.result_index = 0;
                }
            }
            Response::CellSaved { tab } => {
                if self.cell_editor.as_ref().is_some_and(|e| e.tab == tab) {
                    self.cell_editor = None;
                }
                self.status = "单元格已保存 · 正在刷新表数据…".into();
                if let Some(tab) = self.tabs.iter().find(|t| t.id == tab)
                    && let (Some(target), Some(table)) =
                        (tab.target.clone(), tab.table_target.as_ref())
                {
                    follow_up = Some((
                        Request::ReadPage {
                            tab: tab.id,
                            target,
                            table: Table {
                                schema: table.schema.clone(),
                                name: table.name.clone(),
                                kind: "BASE TABLE".into(),
                            },
                            options: tab.browse.clone(),
                        },
                        "正在刷新已保存的表数据…",
                    ));
                }
            }
            Response::Query { output, database } => {
                let changed = session.active_database != database;
                session.active_database = database;
                self.status = format!(
                    "执行完成 · {} ms · {} 个结果集",
                    output.elapsed_ms,
                    output.sets.len()
                );
                if let Some(id) = session.running_tab
                    && let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id)
                {
                    tab.output = Some(output);
                    tab.result_index = 0;
                }
                if changed {
                    session.tables.clear();
                }
            }
            Response::Error {
                message,
                disconnected,
            } => {
                if disconnected {
                    session.connected = None;
                    session.databases.clear();
                    session.active_database = None;
                    session.tables.clear();
                    session.raw_session_used = false;
                }
                self.fail(if disconnected {
                    format!("{message}\n连接已关闭；重新连接后可继续操作。")
                } else {
                    message
                });
            }
        }
        session.running_tab = None;
        session.status = self.status.clone();
        session.error = self.error;
        let name = self
            .profiles
            .iter()
            .find(|p| p.id == connection)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| connection.to_string());
        self.status = format!("{name} · {}", self.status);
        self.sessions.insert(connection, session);
        if let Some((request, status)) = follow_up {
            self.request_on(connection, request, status);
        }
    }

    fn edit_connection(&mut self, id: Uuid) {
        let Some(profile) = self.profiles.iter().find(|p| p.id == id).cloned() else {
            return;
        };
        let credentials = match self.credentials_for(&profile) {
            Ok(credentials) => credentials.unwrap_or_default(),
            Err(error) => {
                self.fail(error.to_string());
                return;
            }
        };
        self.form = Some(ConnectionForm {
            profile,
            password: credentials.database_password,
            ssh_secret: credentials.ssh_secret,
        });
    }

    fn credentials_for(&self, profile: &Profile) -> anyhow::Result<Option<Credentials>> {
        if let Some(credentials) = self.credentials.get(&profile.id) {
            return Ok(Some(credentials.clone()));
        }
        self.store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("配置目录不可用"))?
            .credentials(profile)
    }

    fn save_connection(
        &mut self,
        profile: Profile,
        credentials: Credentials,
    ) -> anyhow::Result<()> {
        profile.validate()?;
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("配置目录不可用"))?;
        let mut profiles = self.profiles.clone();
        if let Some(existing) = profiles.iter_mut().find(|p| p.id == profile.id) {
            *existing = profile.clone();
        } else {
            profiles.push(profile.clone());
        }
        store.save_credentials(&profile, &credentials)?;
        store.save(&profiles)?;
        self.selected = Some(profile.id);
        self.tree_selection.selected = Some(TreeNode::Connection(profile.id));
        self.credentials.insert(profile.id, credentials);
        self.profiles = profiles;
        self.status = "连接已保存".into();
        self.error = false;
        Ok(())
    }

    fn connect_selected(&mut self) {
        let Some(profile) = self
            .profiles
            .iter()
            .find(|p| Some(p.id) == self.selected)
            .cloned()
        else {
            return;
        };
        if self
            .sessions
            .get(&profile.id)
            .is_some_and(|s| s.connected.is_some() || s.busy)
        {
            self.status = "此连接已连接或正在操作；选择节点不会重连或断开其他会话".into();
            return;
        }
        let saved_credentials = match self.credentials_for(&profile) {
            Ok(credentials) => credentials,
            Err(error) => {
                self.fail(error.to_string());
                return;
            }
        };
        let missing_credentials = saved_credentials.is_none();
        let credentials = saved_credentials.unwrap_or_default();
        if missing_credentials
            || (!self.credentials.contains_key(&profile.id)
                && (!profile.remember_password
                    || (profile.ssh.enabled && !profile.ssh.remember_secret)))
        {
            self.form = Some(ConnectionForm {
                profile,
                password: credentials.database_password,
                ssh_secret: credentials.ssh_secret,
            });
            self.status = "请填写数据库 / SSH 凭据，勾选记住后保存在本机".into();
            return;
        }
        self.request(Request::Connect(profile, credentials), "正在连接数据库…");
    }

    fn load_table(&mut self, index: usize) {
        if self.tab_busy(index) || self.cell_editor.is_some() {
            return;
        }
        let Some(target) = self.tabs[index].table_target.clone() else {
            return;
        };
        let Some(session) = self.sessions.get(&target.connection) else {
            return;
        };
        let Some(profile) = session.connected.as_ref() else {
            return;
        };
        if session.active_database.as_ref() != Some(&target.database) {
            self.fail("请先选择此表所属连接的数据库；不会自动切库".into());
            return;
        }
        let table = session
            .tables
            .iter()
            .find(|table| table.schema == target.schema && table.name == target.name)
            .cloned()
            .unwrap_or(Table {
                schema: target.schema,
                name: target.name,
                kind: "BASE TABLE".into(),
            });
        let tab = &mut self.tabs[index];
        if !tab.dirty() {
            tab.sql = profile.engine.preview_sql(&table);
            tab.saved_sql = tab.sql.clone();
        }
        tab.selection.clear();
        tab.query_started = true;
        tab.output = None;
        tab.table_data = None;
        tab.selected_cell = None;
        tab.selected_rows = None;
        tab.source = format!("{} / {}", profile.name, target.database);
        tab.cell_anchor = None;
        let request = Request::ReadPage {
            tab: tab.id,
            target: QueryTarget {
                connection: target.connection,
                database: Some(target.database),
            },
            table,
            options: tab.browse.clone(),
        };
        self.request(request, "正在读取表数据…");
    }

    fn execute(&mut self) {
        if self.modal_open() {
            return;
        }
        let Some(current) = self.execution_target() else {
            self.fail("请先连接服务器并选择执行目标".into());
            return;
        };
        if !self.target_ready(&current) {
            self.fail("标签绑定连接未连接，或该连接的数据库与标签不符。请明确选择原数据库或确认改绑；不会自动切库。".into());
            return;
        }
        if self.session_busy(current.connection) {
            return;
        }
        if self.confirm_rebind.is_some()
            || self.confirm_tab_action.is_some()
            || self.confirm_request.is_some()
            || self.confirm_exit
        {
            return;
        }
        if !self.tabs[self.active_tab].bind_for_execution(&current) {
            self.fail("当前连接或数据库与此 SQL 标签绑定目标不同。请选择原目标，或点击“改绑到当前”并确认。".into());
            return;
        }
        let tab = &mut self.tabs[self.active_tab];
        let sql = if tab.selection.trim().is_empty() {
            tab.sql.clone()
        } else {
            tab.selection.clone()
        };
        if sql.trim().is_empty() {
            self.fail("SQL 不能为空".into());
            return;
        }
        tab.output = None;
        tab.table_data = None;
        tab.selected_cell = None;
        tab.selected_rows = None;
        tab.query_started = true;
        tab.source = self
            .sessions
            .get(&current.connection)
            .and_then(|s| s.connected.as_ref())
            .map(|p| {
                format!(
                    "{} / {}",
                    p.name,
                    current.database.as_deref().unwrap_or("服务器")
                )
            })
            .unwrap_or_default();
        let id = tab.id;
        self.pending_history.insert(
            id,
            HistoryEntry {
                sql: sql.clone(),
                target: current.clone(),
                source: tab.source.clone(),
                timestamp: chrono::Utc::now().timestamp(),
                success: false,
            },
        );
        let connection = current.connection;
        self.request(
            Request::RunQuery {
                tab: id,
                target: current,
                sql,
            },
            "正在执行 SQL…",
        );
        if !self.session_busy(connection) {
            self.finish_history(id, false);
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text("⌕  搜索数据库、数据表…")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        let filter = self.filter.to_lowercase();
        let mut to_connect = None;
        let mut to_edit = None;
        let mut to_delete = None;
        let mut disconnect = None;
        let mut refresh_databases = None;
        let mut refresh_tables = None;
        let mut to_select = None;
        let mut preview = None;
        let mut move_connection = None;
        let mut group_dialog = None;
        let mut buckets: Vec<_> = self
            .catalog
            .groups
            .iter()
            .map(|g| (Some(g.id), g.name.clone()))
            .collect();
        buckets.push((None, "未分组".into()));
        let expand_connection = self.expand_connection.take();
        let expand_database = self.expand_database.take();
        egui::ScrollArea::vertical()
            .id_salt("connection-tree")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (group_id, group_name) in &buckets {
                    let members: Vec<_> = self
                        .profiles
                        .iter()
                        .filter(|p| self.catalog.memberships.get(&p.id).copied() == *group_id)
                        .cloned()
                        .collect();
                    let expand_group =
                        expand_connection.is_some_and(|id| members.iter().any(|p| p.id == id));
                    let group_header =
                        egui::CollapsingHeader::new(format!("{} ({})", group_name, members.len()))
                            .id_salt(("connection-group", group_id))
                            .default_open(true)
                            .open(if expand_group || !filter.is_empty() {
                                Some(true)
                            } else {
                                None
                            })
                            .show(ui, |ui| {
                                for profile in members {
                                    let session = self.sessions.get(&profile.id);
                                    let connected = session.is_some_and(|s| s.connected.is_some());
                                    let busy = session.is_some_and(|s| s.busy);
                                    let databases =
                                        session.map(|s| s.databases.clone()).unwrap_or_default();
                                    let active_database =
                                        session.and_then(|s| s.active_database.clone());
                                    let catalog_tables =
                                        session.map(|s| s.tables.clone()).unwrap_or_default();
                                    let mut title = egui::text::LayoutJob::default();
                                    title.append(
                                        &profile.name,
                                        if connected {
                                            28.0
                                        } else {
                                            ui.spacing().indent + 28.0
                                        },
                                        egui::TextFormat::simple(
                                            egui::TextStyle::Button.resolve(ui.style()),
                                            ui.visuals().text_color(),
                                        ),
                                    );
                                    let background = ui.painter().add(egui::Shape::Noop);
                                    let header_response = if connected {
                                        egui::CollapsingHeader::new(title)
                                            .id_salt(("server", profile.id))
                                            .open(if expand_connection == Some(profile.id) {
                                                Some(true)
                                            } else {
                                                None
                                            })
                                            .show(ui, |ui| {
                                                ui.label(
                                                    RichText::new(format!(
                                                        "{} · {}:{}{}",
                                                        profile.engine.label(),
                                                        profile.host,
                                                        profile.port,
                                                        if profile.ssh.enabled {
                                                            " · SSH"
                                                        } else {
                                                            ""
                                                        }
                                                    ))
                                                    .small()
                                                    .weak(),
                                                );
                                                if databases.is_empty() {
                                                    ui.label(
                                                        RichText::new(if busy {
                                                            "正在读取数据库列表…"
                                                        } else {
                                                            "未发现可访问的数据库"
                                                        })
                                                        .weak(),
                                                    );
                                                }
                                                for database in &databases {
                                                    let active =
                                                        active_database.as_ref() == Some(database);
                                                    let matches_database =
                                                        database.to_lowercase().contains(&filter);
                                                    if !filter.is_empty()
                                                        && !matches_database
                                                        && !active
                                                    {
                                                        continue;
                                                    }
                                                    let title = RichText::new(database)
                                                        .color(ui.visuals().text_color());
                                                    let background =
                                                        ui.painter().add(egui::Shape::Noop);
                                                    let database_header =
                                                egui::CollapsingHeader::new(title)
                                                    .id_salt(("database", profile.id, database))
                                                    .open(
                                                        if expand_database.as_ref()
                                                            == Some(&(profile.id, database.clone()))
                                                        {
                                                            Some(true)
                                                        } else {
                                                            None
                                                        },
                                                    )
                                                    .show(ui, |ui| {
                                                        if !active {
                                                            ui.label(
                                                                RichText::new(
                                                                    "点击数据库名称加载表和视图",
                                                                )
                                                                .small()
                                                                .weak(),
                                                            );
                                                            return;
                                                        }
                                                        let tables: Vec<_> = catalog_tables
                                                            .iter()
                                                            .filter(|table| {
                                                                matches_database
                                                                    || format!(
                                                                        "{}.{}",
                                                                        table.schema, table.name
                                                                    )
                                                                    .to_lowercase()
                                                                    .contains(&filter)
                                                            })
                                                            .collect();
                                                        if profile.engine == Engine::MySql {
                                                            if let Some(table) = table_buttons(
                                                                ui,
                                                                &tables,
                                                                &mut self.tree_selection,
                                                                profile.id,
                                                                database,
                                                                busy,
                                                            ) {
                                                                preview = Some((
                                                                    profile.id,
                                                                    database.clone(),
                                                                    profile.engine,
                                                                    table,
                                                                ));
                                                            }
                                                        } else {
                                                            let mut schemas: Vec<_> = tables
                                                                .iter()
                                                                .map(|table| &table.schema)
                                                                .collect();
                                                            schemas.sort();
                                                            schemas.dedup();
                                                            for schema in schemas {
                                                                egui::CollapsingHeader::new(schema)
                                                                    .id_salt((
                                                                        profile.id, database,
                                                                        schema,
                                                                    ))
                                                                    .default_open(true)
                                                                    .show(ui, |ui| {
                                                                        let schema_tables: Vec<_> =
                                                                            tables
                                                                                .iter()
                                                                                .copied()
                                                                                .filter(|table| {
                                                                                    &table.schema
                                                                                        == schema
                                                                                })
                                                                                .collect();
                                                                        if let Some(table) =
                                                                            table_buttons(
                                                                                ui,
                                                                                &schema_tables,
                                                                                &mut self
                                                                                    .tree_selection,
                                                                                profile.id,
                                                                                database,
                                                                                busy,
                                                                            )
                                                                        {
                                                                            preview = Some((
                                                                                profile.id,
                                                                                database.clone(),
                                                                                profile.engine,
                                                                                table,
                                                                            ));
                                                                        }
                                                                    });
                                                            }
                                                        }
                                                        if catalog_tables.is_empty() {
                                                            ui.label(
                                                                RichText::new(if busy {
                                                                    "正在读取表目录…"
                                                                } else {
                                                                    "没有表或视图"
                                                                })
                                                                .weak(),
                                                            );
                                                        } else if tables.is_empty() {
                                                            ui.label(
                                                                RichText::new("没有匹配的表或视图")
                                                                    .weak(),
                                                            );
                                                        }
                                                    });
                                                    self.tree_selection.row(
                                                        &database_header.header_response,
                                                        background,
                                                        TreeNode::Database(
                                                            profile.id,
                                                            database.clone(),
                                                        ),
                                                    );
                                                    if database_header.header_response.clicked()
                                                        && !busy
                                                        && !active
                                                    {
                                                        to_select =
                                                            Some((profile.id, database.clone()));
                                                    }
                                                    database_header.header_response.context_menu(
                                                        |ui| {
                                                            if ui
                                                                .add_enabled(
                                                                    !busy && !active,
                                                                    egui::Button::new("选择数据库"),
                                                                )
                                                                .clicked()
                                                            {
                                                                to_select = Some((
                                                                    profile.id,
                                                                    database.clone(),
                                                                ));
                                                                ui.close();
                                                            }
                                                            if ui
                                                                .add_enabled(
                                                                    !busy && active,
                                                                    egui::Button::new(
                                                                        "刷新表和视图",
                                                                    ),
                                                                )
                                                                .clicked()
                                                            {
                                                                refresh_tables = Some(profile.id);
                                                                ui.close();
                                                            }
                                                        },
                                                    );
                                                }
                                            })
                                            .header_response
                                    } else {
                                        ui.add_sized(
                                            [ui.available_width(), ui.spacing().interact_size.y],
                                            egui::Button::new(title).frame(false).right_text(""),
                                        )
                                    };
                                    icons::paint_connection(
                                        ui,
                                        egui::Rect::from_center_size(
                                            egui::pos2(
                                                header_response.rect.left()
                                                    + ui.spacing().indent
                                                    + 10.0,
                                                header_response.rect.center().y,
                                            ),
                                            egui::vec2(20.0, 20.0),
                                        ),
                                        profile.engine,
                                        connected,
                                    );
                                    header_response.clone().on_hover_text(format!(
                                        "{} · {}",
                                        profile.engine.label(),
                                        if connected { "已连接" } else { "未连接" },
                                    ));
                                    self.tree_selection.row(
                                        &header_response,
                                        background,
                                        TreeNode::Connection(profile.id),
                                    );
                                    if header_response.clicked()
                                        || header_response.secondary_clicked()
                                    {
                                        self.selected = Some(profile.id);
                                    }
                                    if header_response.double_clicked() && !busy && !connected {
                                        to_connect = Some(profile.id);
                                    }
                                    header_response.context_menu(|ui| {
                                        if connected {
                                            if ui
                                                .add_enabled(!busy, egui::Button::new("断开连接"))
                                                .clicked()
                                            {
                                                disconnect = Some(profile.id);
                                                ui.close();
                                            }
                                            if ui
                                                .add_enabled(
                                                    !busy,
                                                    egui::Button::new("刷新数据库列表"),
                                                )
                                                .clicked()
                                            {
                                                refresh_databases = Some(profile.id);
                                                ui.close();
                                            }
                                        } else if ui
                                            .add_enabled(!busy, egui::Button::new("连接服务器"))
                                            .clicked()
                                        {
                                            to_connect = Some(profile.id);
                                            ui.close();
                                        }
                                        ui.separator();
                                        ui.add_enabled_ui(
                                            !busy && self.catalog_store.is_some(),
                                            |ui| {
                                                ui.menu_button("移动到分组", |ui| {
                                                    for (id, name) in &buckets {
                                                        if ui
                                                            .selectable_label(
                                                                self.catalog
                                                                    .memberships
                                                                    .get(&profile.id)
                                                                    .copied()
                                                                    == *id,
                                                                name,
                                                            )
                                                            .clicked()
                                                        {
                                                            move_connection =
                                                                Some((profile.id, *id));
                                                            ui.close();
                                                        }
                                                    }
                                                });
                                            },
                                        );
                                        if ui
                                            .add_enabled(!busy, egui::Button::new("编辑连接"))
                                            .clicked()
                                        {
                                            to_edit = Some(profile.id);
                                            ui.close();
                                        }
                                        if ui
                                            .add_enabled(!busy, egui::Button::new("删除连接…"))
                                            .clicked()
                                        {
                                            to_delete = Some(profile.id);
                                            ui.close();
                                        }
                                    });
                                }
                                if self.profiles.iter().all(|p| {
                                    self.catalog.memberships.get(&p.id).copied() != *group_id
                                }) {
                                    ui.label(
                                        RichText::new("空分组 · 右键连接可移入").small().weak(),
                                    );
                                }
                            });
                    if let Some(id) = group_id {
                        group_header.header_response.context_menu(|ui| {
                            ui.add_enabled_ui(self.catalog_store.is_some(), |ui| {
                                if ui.button("重命名分组…").clicked() {
                                    group_dialog = Some(CatalogDialog::Group {
                                        id: Some(*id),
                                        name: group_name.clone(),
                                    });
                                    ui.close();
                                }
                                if ui.button("删除分组…").clicked() {
                                    group_dialog =
                                        Some(CatalogDialog::DeleteGroup(*id, group_name.clone()));
                                    ui.close();
                                }
                            });
                        });
                    }
                }
                self.tree_selection.paint(ui);
                if let Some(node) = &self.tree_selection.selected {
                    self.selected = Some(match node {
                        TreeNode::Connection(id)
                        | TreeNode::Database(id, _)
                        | TreeNode::Table(id, _, _) => *id,
                    });
                }
                if self.profiles.is_empty() {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("⌁").size(38.0).color(accent(ui)));
                        ui.label(RichText::new("还没有数据库连接").size(16.0).strong());
                        ui.label(RichText::new("新建连接，开始探索你的数据").small().weak());
                    });
                }
            });
        if let Some((connection, group)) = move_connection {
            self.move_to_group(connection, group);
        }
        if let Some(dialog) = group_dialog {
            self.catalog_dialog = Some(dialog);
        }
        if let Some(id) = to_edit {
            self.selected = Some(id);
            self.edit_connection(id);
        }
        if let Some(id) = to_delete {
            self.selected = Some(id);
            self.confirm_delete = Some(id);
        }
        if let Some(id) = disconnect {
            self.request_on(id, Request::Disconnect, "正在断开连接…");
        }
        if let Some(id) = refresh_databases {
            self.request_on(id, Request::Databases, "正在刷新数据库列表…");
        }
        if let Some(id) = refresh_tables {
            self.request_on(id, Request::Tables, "正在刷新表目录…");
        }
        if let Some(id) = to_connect {
            self.selected = Some(id);
            self.connect_selected();
        }
        if let Some((id, database)) = to_select {
            self.selected = Some(id);
            self.request_on(id, Request::SelectDatabase(database), "正在选择数据库…");
        }
        if let Some((connection, database, engine, table)) = preview {
            self.selected = Some(connection);
            let target = TableTarget {
                connection,
                database: database.clone(),
                schema: table.schema.clone(),
                name: table.name.clone(),
            };
            let (index, created) =
                open_table_tab(&mut self.tabs, &mut self.next_tab, target, engine, &table);
            self.active_tab = index;
            if created {
                self.load_table(index);
            }
        }
    }

    fn workspace(&mut self, ui: &mut egui::Ui) {
        if self.tabs[self.active_tab].table_target.is_none() {
            self.workspace_content(ui);
            return;
        }
        // Reserve pagination before rendering content, including failed or empty queries.
        let (content, footer) = table_workspace_rects(ui.available_rect_before_wrap());
        ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(content));
            self.workspace_content(ui);
        });
        ui.scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(footer));
            self.table_pagination(ui);
        });
    }

    fn workspace_content(&mut self, ui: &mut egui::Ui) {
        let busy = self.tab_busy(self.active_tab);
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.spacing_mut().button_padding.y = 4.0;
        ui.spacing_mut().interact_size.y = 24.0;
        ui.add_space(4.0);
        if let Some(action) =
            query_tabs(ui, &self.tabs, self.active_tab, self.cell_editor.is_some())
        {
            self.change_tab(action);
        }
        ui.add_space(4.0);
        ui.separator();
        let table_tab = self.tabs[self.active_tab].table_target.is_some();
        self.workspace_tools(ui);
        ui.add_space(4.0);
        let show_results = table_tab
            || self.tabs[self.active_tab].query_started
            || self.tabs[self.active_tab].output.is_some();
        if !table_tab {
            sql_editor(
                ui,
                &mut self.tabs[self.active_tab],
                &mut self.editor_height,
                show_results,
            );
            if self.cell_editor.is_none()
                && self.form.is_none()
                && self.confirm_delete.is_none()
                && self.export_path.is_none()
                && self.confirm_rebind.is_none()
                && self.confirm_tab_action.is_none()
                && self.confirm_request.is_none()
                && !self.confirm_exit
                && !self.history_open
                && self.structure.is_none()
                && ui.is_enabled()
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter))
            {
                self.execute();
            }
        }
        if !show_results {
            return;
        }
        ui.add_space(4.0);
        let tab = &mut self.tabs[self.active_tab];
        let Some(output) = &tab.output else {
            ui.label(
                RichText::new(
                    if self
                        .sessions
                        .values()
                        .any(|s| s.running_tab == Some(tab.id))
                    {
                        "正在执行查询…"
                    } else {
                        "暂无查询结果，执行状态见底部状态栏。"
                    },
                )
                .weak(),
            );
            return;
        };
        if output.sets.len() > 1 {
            ui.horizontal(|ui| {
                for index in 0..output.sets.len() {
                    ui.selectable_value(&mut tab.result_index, index, format!("#{}", index + 1));
                }
            });
        }
        let Some(set) = output.sets.get(tab.result_index) else {
            ui.label("执行完成，没有行结果；SQL Server 批处理不报告受影响行数。");
            return;
        };
        if !table_tab {
            ui.horizontal(|ui| {
                ui.strong("查询结果");
                ui.label(format!(
                    "{} 行 · {} 列 · {} ms",
                    set.rows.len(),
                    set.columns.len(),
                    output.elapsed_ms
                ))
                .on_hover_text(&tab.source);
                if let Some(affected) = set.affected {
                    ui.label(RichText::new(format!("数据库报告行数: {affected}")).weak());
                }
                if !set.columns.is_empty() && ui.button("导出 CSV").clicked() {
                    self.export_path = Some(default_csv_export_path());
                }
            });
        }
        if set.truncated {
            ui.colored_label(
                Color32::from_rgb(235, 180, 80),
                "结果超过 1000 行，仅展示和导出前 1000 行；使用 LIMIT / TOP 缩小查询范围。",
            );
        }
        if set.columns.is_empty() {
            ui.label("语句已完成，无行数据。MySQL / PostgreSQL 空结果暂不显示列名。");
            return;
        }
        let context_matches = tab.table_target.as_ref().is_some_and(|target| {
            self.sessions.get(&target.connection).is_some_and(|s| {
                s.connected.is_some() && s.active_database.as_ref() == Some(&target.database)
            })
        });
        if let Some(data) = &tab.table_data {
            let reason = if !context_matches {
                Some("此表来自其他连接或数据库，选择对应数据库后可编辑。")
            } else if data.primary_keys.is_empty() {
                Some("只读：此表没有主键，无法准确定位要修改的行。")
            } else if !data.columns.iter().any(|column| column.editable) {
                Some("只读：视图、生成列或当前表类型不支持编辑。")
            } else {
                None
            };
            if let Some(reason) = reason {
                ui.label(RichText::new(reason).small().weak());
            }
        }
        if let Some(editor) = result_grid(
            ui,
            tab,
            context_matches && !busy && self.cell_editor.is_none(),
        ) {
            self.cell_editor = Some(editor);
        }
    }

    fn cell_editor_window(&mut self, ctx: &egui::Context) {
        let busy = self
            .cell_editor
            .as_ref()
            .is_some_and(|e| self.session_busy(e.connection));
        let Some(editor) = &mut self.cell_editor else {
            return;
        };
        let mut open = true;
        let mut save = false;
        let mut cancel = false;
        egui::Window::new(format!("编辑单元格 · {}", editor.change.column))
            .id(egui::Id::new("cell-editor"))
            .open(&mut open)
            .collapsible(false)
            .default_width(560.0)
            .show(ctx, |ui| {
                ui.label(format!(
                    "{} / {}.{}",
                    editor.change.database, editor.change.table.schema, editor.change.table.name
                ));
                ui.add_enabled_ui(!busy, |ui| {
                    ui.checkbox(&mut editor.is_null, "设为 NULL");
                    let response = ui.add_enabled(
                        !editor.is_null,
                        egui::TextEdit::multiline(&mut editor.text)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .desired_rows(6),
                    );
                    if editor.focus {
                        response.request_focus();
                        editor.focus = false;
                    }
                    ui.label(
                        RichText::new("保存后立即写入这一行 · 空文本与 NULL 分开处理")
                            .small()
                            .weak(),
                    );
                    ui.horizontal(|ui| {
                        save = ui.button("保存修改").clicked();
                        cancel = ui.button("取消").clicked();
                    });
                });
                if busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("正在保存…");
                    });
                }
                if self.error {
                    ui.colored_label(Color32::from_rgb(220, 90, 90), &self.status);
                }
            });
        if !busy
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            cancel = true;
        }
        if !busy
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter))
        {
            save = true;
        }
        if (!open || cancel) && !busy {
            self.cell_editor = None;
            return;
        }
        if save && !busy {
            if !self.sessions.get(&editor.connection).is_some_and(|s| {
                s.connected.is_some() && s.active_database.as_ref() == Some(&editor.change.database)
            }) {
                self.fail("连接或数据库已切换，请回到此表所属数据库后再保存".into());
                return;
            }
            let mut change = editor.change.clone();
            change.value = if editor.is_null {
                None
            } else {
                Some(editor.text.clone())
            };
            if change.value == change.original {
                self.cell_editor = None;
                return;
            }
            let tab = editor.tab;
            let target = QueryTarget {
                connection: editor.connection,
                database: Some(change.database.clone()),
            };
            self.request(
                Request::SaveCell {
                    tab,
                    target,
                    change,
                },
                "正在保存单元格修改…",
            );
        }
    }

    fn connection_window(&mut self, ctx: &egui::Context) {
        let busy = self.test_busy
            || self
                .form
                .as_ref()
                .is_some_and(|f| self.session_busy(f.profile.id));
        let Some(form) = &mut self.form else {
            return;
        };
        let mut open = true;
        let mut save = false;
        let mut connect = false;
        let mut test = false;
        let mut selection_error = None;
        egui::Window::new("数据库连接")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width((ctx.content_rect().width() - 120.0).clamp(620.0, 860.0))
            .min_width(620.0)
            .default_pos(egui::pos2(
                ((ctx.content_rect().width() - 860.0) * 0.5).max(24.0),
                64.0,
            ))
            .show(ctx, |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    ui.label(
                        RichText::new("CONNECTION SETTINGS")
                            .size(11.0)
                            .color(accent(ui)),
                    );
                    connection_heading(ui, form);
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt("connection-fields")
                        .max_height((ctx.content_rect().height() - 340.0).max(180.0))
                        .show(ui, |ui| {
                            if ui.available_width() >= 820.0 {
                                ui.columns(2, |columns| {
                                    columns[0].group(|ui| database_fields(ui, form));
                                    columns[1].group(|ui| {
                                        selection_error = ssh_fields(ui, form);
                                    });
                                });
                            } else {
                                ui.group(|ui| database_fields(ui, form));
                                ui.add_space(10.0);
                                ui.group(|ui| {
                                    selection_error = ssh_fields(ui, form);
                                });
                            }
                            ui.add_space(10.0);
                            ui.group(|ui| {
                                ui.set_min_width(ui.available_width());
                                ui.strong("数据库 TLS");
                                database_tls_settings(ui, &mut form.profile);
                            });
                        });
                    ui.add_space(10.0);
                    ui.separator();
                    ui.horizontal(|ui| {
                        test = ui.button("测试连接").clicked();
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            connect = ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("保存并连接").color(Color32::WHITE),
                                    )
                                    .fill(accent(ui)),
                                )
                                .clicked();
                            save = ui.button("保存").clicked();
                        });
                    });
                });
                if busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("正在处理…");
                    });
                }
                if self.error {
                    ui.colored_label(ui.visuals().error_fg_color, &self.status);
                } else {
                    ui.label(RichText::new(&self.status).small().weak());
                }
            });
        let profile = form.profile.clone();
        let credentials = Credentials {
            database_password: form.password.clone(),
            ssh_secret: form.ssh_secret.clone(),
        };
        if !open {
            self.form = None;
        }
        if let Some(error) = selection_error {
            self.fail(error);
        }
        if test {
            match profile.validate() {
                Ok(()) => self.request(
                    Request::Test(profile.clone(), credentials.clone()),
                    "正在测试连接…",
                ),
                Err(error) => self.fail(error.to_string()),
            }
        }
        if save || connect {
            match self.save_connection(profile.clone(), credentials.clone()) {
                Ok(()) => {
                    self.form = None;
                    if connect {
                        self.request(Request::Connect(profile, credentials), "正在连接数据库…");
                    }
                }
                Err(error) => self.fail(format!("{error:#}")),
            }
        }
    }

    fn delete_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.confirm_delete else {
            return;
        };
        let busy = self.session_busy(id);
        let mut cancel = false;
        let mut delete = false;
        egui::Window::new("删除本机连接")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("将删除此连接配置及保存的密码，数据库中的数据不受影响。");
                if self.sessions.get(&id).is_some_and(|s| s.connected.is_some()) {
                    ui.label("此连接仍已连接。删除会关闭这一会话，可能回滚未提交事务；其他连接不受影响。请先自行 COMMIT / ROLLBACK。");
                }
                ui.horizontal(|ui| {
                    cancel = ui.button("取消").clicked();
                    delete = ui
                        .add_enabled(!busy, egui::Button::new("删除连接"))
                        .clicked();
                });
            });
        if cancel {
            self.confirm_delete = None;
        }
        if delete {
            let profiles: Vec<_> = self
                .profiles
                .iter()
                .filter(|p| p.id != id)
                .cloned()
                .collect();
            let result = (|| -> anyhow::Result<()> {
                let store = self
                    .store
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("配置目录不可用"))?;
                store.forget_credentials(id)?;
                store.save(&profiles)?;
                Ok(())
            })();
            match result {
                Ok(()) => {
                    self.profiles = profiles;
                    self.credentials.remove(&id);
                    self.selected = self.profiles.first().map(|p| p.id);
                    self.tree_selection.selected = self.selected.map(TreeNode::Connection);
                    self.confirm_delete = None;
                    // Deletion explicitly closes only this connection's session.
                    self.sessions.remove(&id);
                }
                Err(error) => self.fail(format!("{error:#}")),
            }
        }
    }

    fn export_window(&mut self, ctx: &egui::Context) {
        let Some(path) = &mut self.export_path else {
            return;
        };
        let mut open = true;
        let mut export = false;
        egui::Window::new("导出当前结果集")
            .open(&mut open)
            .collapsible(false)
            .default_width(500.0)
            .show(ctx, |ui| {
                ui.label("保存路径");
                ui.add(egui::TextEdit::singleline(path).desired_width(f32::INFINITY));
                ui.label(
                    RichText::new("导出当前展示的行；NULL 写为文字 NULL，已有同名文件会被覆盖。")
                        .small()
                        .weak(),
                );
                export = ui.button("保存 CSV").clicked();
            });
        let destination = PathBuf::from(path.as_str());
        if !open {
            self.export_path = None;
        }
        if export {
            let tab = &self.tabs[self.active_tab];
            let result = tab
                .output
                .as_ref()
                .and_then(|o| o.sets.get(tab.result_index))
                .ok_or_else(|| anyhow::anyhow!("没有可导出的结果集"))
                .and_then(|set| export_csv(set, &destination));
            match result {
                Ok(()) => {
                    self.export_path = None;
                    self.status = format!("已导出: {}", destination.display());
                    self.error = false;
                }
                Err(error) => self.fail(format!("导出失败: {error:#}")),
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        apply_theme(ctx);
        egui::TopBottomPanel::top("title")
            .exact_height(window_chrome::TITLE_HEIGHT)
            .frame(egui::Frame::new().fill(chrome(ctx)))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.horizontal(|ui| {
                    ui.set_height(window_chrome::TITLE_HEIGHT);
                    let brand = ui.allocate_ui_with_layout(
                        egui::vec2(160.0, window_chrome::TITLE_HEIGHT),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add_space(16.0);
                            ui.add(
                                egui::Image::new(&self.logo)
                                    .fit_to_exact_size(egui::vec2(28.0, 28.0))
                                    .alt_text("SQLShop 猞猁 Logo"),
                            );
                            ui.add_space(8.0);
                            ui.label(RichText::new("SQLShop").strong().size(18.0));
                        },
                    );
                    let brand_drag = ui.interact(
                        brand.response.rect,
                        ui.id().with("title-brand-drag"),
                        egui::Sense::click_and_drag(),
                    );
                    window_chrome::drag_response(&brand_drag);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        window_chrome::controls(ui);
                        ui.add_space(16.0);
                        let previous = self.theme;
                        egui::ComboBox::from_id_salt("theme")
                            .width(90.0)
                            .selected_text(match self.theme {
                                egui::ThemePreference::System => "跟随系统",
                                egui::ThemePreference::Light => "浅色",
                                egui::ThemePreference::Dark => "深色",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.theme,
                                    egui::ThemePreference::System,
                                    "跟随系统",
                                );
                                ui.selectable_value(
                                    &mut self.theme,
                                    egui::ThemePreference::Light,
                                    "浅色",
                                );
                                ui.selectable_value(
                                    &mut self.theme,
                                    egui::ThemePreference::Dark,
                                    "深色",
                                );
                            });
                        if self.theme != previous {
                            ctx.set_theme(self.theme);
                        }
                        ui.add_space(16.0);
                        ui.label(
                            RichText::new(
                                if let Some(profile) =
                                    self.selected_session().and_then(|s| s.connected.as_ref())
                                {
                                    format!("● {}", profile.name)
                                } else {
                                    "○ 未连接".into()
                                },
                            )
                            .color(
                                if self
                                    .selected_session()
                                    .is_some_and(|s| s.connected.is_some())
                                {
                                    accent(ui)
                                } else {
                                    ui.visuals().weak_text_color()
                                },
                            ),
                        );
                        window_chrome::drag_area(ui);
                    });
                });
            });
        self.command_bar(ctx);
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(chrome(ctx))
                    .inner_margin(egui::Margin::symmetric(18, 8)),
            )
            .show(ctx, |ui| {
                let target_session = self
                    .execution_target()
                    .and_then(|t| self.sessions.get(&t.connection));
                let message = target_session
                    .map(|s| s.status.as_str())
                    .unwrap_or(&self.status);
                let error = target_session.map_or(self.error, |s| s.error);
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{} 个连接 · {} 个操作中",
                        self.sessions
                            .values()
                            .filter(|s| s.connected.is_some())
                            .count(),
                        self.sessions.values().filter(|s| s.busy).count()
                    ));
                    let width = (ui.available_width() * 0.35).min(420.0);
                    let status = if message == self.status {
                        message.to_owned()
                    } else {
                        format!("{message} · {}", self.status)
                    };
                    let color = if error {
                        Color32::from_rgb(240, 120, 120)
                    } else {
                        ui.visuals().weak_text_color()
                    };
                    ui.add_sized(
                        [width, 22.0],
                        egui::Label::new(RichText::new(&status).small().color(color)).truncate(),
                    )
                    .on_hover_text(status);
                    ui.separator();
                    self.executed_sql_footer(ui);
                });
            });
        let modal = self.modal_open();
        egui::SidePanel::left("sidebar")
            .default_width(302.0)
            .width_range(265.0..=450.0)
            .frame(
                egui::Frame::new()
                    .fill(chrome(ctx))
                    .inner_margin(egui::Margin::symmetric(16, 8)),
            )
            .show(ctx, |ui| {
                ui.add_enabled_ui(!modal, |ui| self.sidebar(ui));
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(canvas(ctx))
                    .inner_margin(egui::Margin::symmetric(22, 10)),
            )
            .show(ctx, |ui| {
                ui.add_enabled_ui(!modal, |ui| self.workspace(ui));
            });
        self.connection_window(ctx);
        self.delete_window(ctx);
        self.export_window(ctx);
        self.cell_editor_window(ctx);
        self.workspace_windows(ctx);
        self.catalog_windows(ctx);
        window_chrome::paint_border(ctx);
        if self.screenshot_path.is_none() {
            if ctx.input(|i| i.viewport().close_requested())
                && !self.allow_exit
                && (self.any_busy()
                    || self.sessions.values().any(|s| s.raw_session_used)
                    || self.tabs.iter().any(QueryTab::dirty)
                    || !self.autosave_workspace(true))
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.confirm_exit = true;
            }
            if !self.allow_exit {
                self.autosave_workspace(false);
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }
        // Optional native-render smoke test: capture this viewport, then exit.
        if let Some(path) = &self.screenshot_path {
            self.frames += 1;
            if self.frames == 3 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            }
            let screenshot = ctx.input(|input| {
                input.events.iter().find_map(|event| {
                    if let egui::Event::Screenshot { image, .. } = event {
                        Some(image.clone())
                    } else {
                        None
                    }
                })
            });
            if let Some(screenshot) = screenshot {
                let bytes: Vec<u8> = screenshot
                    .pixels
                    .iter()
                    .flat_map(|pixel| pixel.to_array())
                    .collect();
                match image::save_buffer(
                    path,
                    &bytes,
                    screenshot.width() as u32,
                    screenshot.height() as u32,
                    image::ColorType::Rgba8,
                ) {
                    Ok(()) => eprintln!("Saved viewport screenshot: {}", path.display()),
                    Err(error) => eprintln!("Screenshot failed: {error}"),
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
        }
    }
}

fn table_workspace_rects(available: egui::Rect) -> (egui::Rect, egui::Rect) {
    let footer_top = (available.bottom() - 32.0).max(available.top());
    let content = egui::Rect::from_min_max(
        available.min,
        egui::pos2(available.right(), (footer_top - 4.0).max(available.top())),
    );
    let footer = egui::Rect::from_min_max(egui::pos2(available.left(), footer_top), available.max);
    (content, footer)
}

fn default_csv_export_path() -> String {
    let directory = directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    directory
        .join(format!(
            "sqlshop-{}.csv",
            chrono::Local::now().format("%Y%m%d%H%M%S")
        ))
        .display()
        .to_string()
}

fn result_copy_controls(ui: &mut egui::Ui, tab: &QueryTab, close_menu: bool) {
    let Some(set) = tab
        .output
        .as_ref()
        .and_then(|output| output.sets.get(tab.result_index))
    else {
        return;
    };
    if ui
        .add_enabled(
            tab.selected_cell.is_some() || tab.selected_rows.is_some(),
            egui::Button::new("复制选区 (TSV)"),
        )
        .on_hover_text("单击行号选择整行；单击单元格选择区域；Shift + 单击扩展选区")
        .clicked()
    {
        let text = if let Some((anchor, end)) = tab.selected_rows {
            selected_tsv(
                set,
                Some((anchor, 0)),
                Some((end, set.columns.len().saturating_sub(1))),
            )
        } else {
            selected_tsv(set, tab.cell_anchor, tab.selected_cell)
        };
        ui.ctx().copy_text(text);
        if close_menu {
            ui.close();
        }
    }
    if ui.button("复制全部 (TSV)").clicked() {
        ui.ctx().copy_text(selected_tsv(
            set,
            Some((0, 0)),
            Some((
                set.rows.len().saturating_sub(1),
                set.columns.len().saturating_sub(1),
            )),
        ));
        if close_menu {
            ui.close();
        }
    }
}

const GRID_HEADER_HEIGHT: f32 = 52.0;

fn result_grid(ui: &mut egui::Ui, tab: &mut QueryTab, can_edit: bool) -> Option<CellEditor> {
    let set = tab.output.as_ref()?.sets.get(tab.result_index)?;
    let available_width = ui.available_width();
    let reset_widths = (tab.grid_width - available_width).abs() > 2.0;
    tab.grid_width = available_width;
    let mut editor = None;
    let target = tab.table_target.as_ref();
    let metadata = tab.table_data.as_ref();
    if tab.table_target.is_none() {
        ui.horizontal(|ui| result_copy_controls(ui, tab, false));
    }
    egui::Frame::new()
        .fill(surface(ui))
        .corner_radius(6)
        .stroke(egui::Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            ui.visuals_mut().selection.bg_fill = accent(ui).gamma_multiply(0.10);
            let height = ui.available_height().max(0.0);
            let body_height =
                (height - GRID_HEADER_HEIGHT - ui.spacing().scroll.bar_width).max(0.0);
            egui::ScrollArea::horizontal()
                .id_salt(("result-grid-scroll", tab.id, tab.result_index))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let widths_id = ui
                        .id()
                        .with(("grid-column-widths", tab.id, tab.result_index));
                    let mut widths = ui
                        .data_mut(|data| data.get_temp::<Vec<f32>>(widths_id))
                        .filter(|widths| !reset_widths && widths.len() == set.columns.len())
                        .unwrap_or_else(|| {
                            vec![
                                ((available_width - 44.0) / set.columns.len().max(1) as f32)
                                    .max(110.0);
                                set.columns.len()
                            ]
                        });
                    let mut header_rects = Vec::new();
                    let mut builder = TableBuilder::new(ui)
                        .id_salt(("data-grid", tab.id, tab.result_index))
                        .striped(true)
                        .resizable(false)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .min_scrolled_height(body_height)
                        .max_scroll_height(body_height)
                        .column(Column::exact(44.0));
                    for width in &widths {
                        builder = builder.column(Column::exact(*width).clip(true));
                    }
                    if reset_widths {
                        builder.reset();
                    }
                    builder
                        .header(GRID_HEADER_HEIGHT, |mut header| {
                            header.col(|ui| {
                                grid_header_cell(ui, "#", "行号");
                            });
                            for (index, name) in set.columns.iter().enumerate() {
                                header.col(|ui| {
                                    let column = metadata.and_then(|data| data.columns.get(index));
                                    let sql_type = column
                                        .map(|column| column.declared_type.as_str())
                                        .or_else(|| set.column_types.get(index).map(String::as_str))
                                        .filter(|sql_type| !sql_type.is_empty())
                                        .unwrap_or("类型未知");
                                    let response = grid_header_cell(ui, name, sql_type);
                                    header_rects.push(response.rect);
                                    let key = if column.is_some_and(|column| column.primary_key) {
                                        " · 主键"
                                    } else {
                                        ""
                                    };
                                    response.on_hover_text(format!(
                                        "{name} · {sql_type}{key}\n拖动表头分隔线调整列宽"
                                    ));
                                });
                            }
                        })
                        .body(|body| {
                            body.rows(32.0, set.rows.len(), |mut row| {
                                let row_index = row.index();
                                row.set_selected(row_in_selection(tab.selected_rows, row_index));
                                row.col(|ui| {
                                    let response = grid_cell(
                                        ui,
                                        RichText::new((row_index + 1).to_string()).weak(),
                                        false,
                                        false,
                                    );
                                    if response.clicked() {
                                        tab.selected_rows = Some(row_selection(
                                            tab.selected_rows,
                                            row_index,
                                            ui.input(|i| i.modifiers.shift),
                                        ));
                                        tab.selected_cell = None;
                                        tab.cell_anchor = None;
                                    }
                                    response
                                        .on_hover_text("单击选中整行 · Shift + 单击连续选择多行");
                                });
                                for (column_index, cell) in set.rows[row_index].iter().enumerate() {
                                    row.col(|ui| {
                                        let value = cell.as_deref().unwrap_or("NULL");
                                        let text = if cell.is_none() {
                                            RichText::new(value).italics().weak()
                                        } else {
                                            RichText::new(value)
                                        };
                                        let response = grid_cell(
                                            ui,
                                            text,
                                            cell_in_selection(
                                                tab.cell_anchor,
                                                tab.selected_cell,
                                                row_index,
                                                column_index,
                                            ),
                                            false,
                                        );
                                        if response.clicked() {
                                            let extend = ui.input(|i| i.modifiers.shift);
                                            if extend && tab.selected_rows.is_some() {
                                                tab.selected_rows = Some(row_selection(
                                                    tab.selected_rows,
                                                    row_index,
                                                    true,
                                                ));
                                            } else {
                                                tab.selected_rows = None;
                                                if !extend || tab.cell_anchor.is_none() {
                                                    tab.cell_anchor =
                                                        Some((row_index, column_index));
                                                }
                                                tab.selected_cell = Some((row_index, column_index));
                                            }
                                        }
                                        let editable = can_edit
                                            && metadata.is_some_and(|data| {
                                                !data.primary_keys.is_empty()
                                                    && data
                                                        .columns
                                                        .get(column_index)
                                                        .is_some_and(|column| column.editable)
                                                    && data
                                                        .columns
                                                        .iter()
                                                        .filter(|column| column.primary_key)
                                                        .all(|column| {
                                                            column.kind
                                                            != sqlshop::model::CellKind::Unsupported
                                                        })
                                            });
                                        let begin_edit = response.double_clicked() && editable;
                                        let mut menu_edit = false;
                                        let hint = if editable {
                                            format!(
                                                "{value}
双击编辑 · 保存后更新数据库并刷新"
                                            )
                                        } else if target.is_none() {
                                            format!(
                                                "{value}
SQL 查询结果只读；双击左侧表名打开可编辑的表数据"
                                            )
                                        } else {
                                            value.to_owned()
                                        };
                                        response.on_hover_text(hint).context_menu(|ui| {
                                            if ui.button("复制单元格").clicked() {
                                                ui.ctx().copy_text(value.into());
                                                ui.close();
                                            }
                                            if ui
                                                .add_enabled(
                                                    editable,
                                                    egui::Button::new("编辑单元格…"),
                                                )
                                                .clicked()
                                            {
                                                menu_edit = true;
                                                ui.close();
                                            }
                                        });
                                        if (begin_edit || menu_edit)
                                            && let (Some(target), Some(data)) = (target, metadata)
                                        {
                                            editor = Some(CellEditor {
                                                tab: tab.id,
                                                connection: target.connection,
                                                change: CellUpdate {
                                                    database: target.database.clone(),
                                                    table: Table {
                                                        schema: target.schema.clone(),
                                                        name: target.name.clone(),
                                                        kind: "BASE TABLE".into(),
                                                    },
                                                    primary_keys: data.primary_keys.clone(),
                                                    key_values: data.keys[row_index].clone(),
                                                    column: set.columns[column_index].clone(),
                                                    original: cell.clone(),
                                                    value: cell.clone(),
                                                },
                                                text: cell.clone().unwrap_or_default(),
                                                is_null: cell.is_none(),
                                                focus: true,
                                            });
                                        }
                                    });
                                }
                            });
                        });
                    resize_grid_columns(ui, widths_id, &header_rects, &mut widths);
                    ui.data_mut(|data| data.insert_temp(widths_id, widths));
                });
        });
    editor
}

fn resize_grid_columns(
    ui: &mut egui::Ui,
    id: egui::Id,
    headers: &[egui::Rect],
    widths: &mut [f32],
) {
    // Keep resize hitboxes in the header, away from the body scroll bar.
    let radius = ui.style().interaction.resize_grab_radius_side;
    for (index, (header, width)) in headers.iter().zip(widths).enumerate() {
        let rect = egui::Rect::from_min_max(
            egui::pos2(header.right() - radius, header.top()),
            egui::pos2(header.right() + radius, header.bottom()),
        );
        let response = ui
            .interact(rect, id.with(index), egui::Sense::drag())
            .on_hover_and_drag_cursor(egui::CursorIcon::ResizeColumn);
        if response.dragged() {
            *width = (*width + response.drag_delta().x).max(110.0);
        }
    }
}

fn row_selection(previous: Option<(usize, usize)>, row: usize, extend: bool) -> (usize, usize) {
    let anchor = if extend {
        previous.map_or(row, |(anchor, _)| anchor)
    } else {
        row
    };
    (anchor, row)
}

fn row_in_selection(selection: Option<(usize, usize)>, row: usize) -> bool {
    selection.is_some_and(|(anchor, end)| (anchor.min(end)..=anchor.max(end)).contains(&row))
}

fn cell_in_selection(
    anchor: Option<(usize, usize)>,
    end: Option<(usize, usize)>,
    row: usize,
    column: usize,
) -> bool {
    let Some((r1, c1)) = end else { return false };
    let (r0, c0) = anchor.unwrap_or((r1, c1));
    (r0.min(r1)..=r0.max(r1)).contains(&row) && (c0.min(c1)..=c0.max(c1)).contains(&column)
}

fn selected_tsv(
    set: &sqlshop::model::ResultSet,
    anchor: Option<(usize, usize)>,
    end: Option<(usize, usize)>,
) -> String {
    let Some((r1, c1)) = end else {
        return String::new();
    };
    let (r0, c0) = anchor.unwrap_or((r1, c1));
    let rows: Vec<Vec<String>> = set
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| value.clone().unwrap_or_else(|| "NULL".into()))
                .collect()
        })
        .collect();
    sqlshop::sql_editor::copy_tsv(&rows, r0.min(r1)..=r0.max(r1), c0.min(c1)..=c0.max(c1))
}

fn grid_cell_border(ui: &egui::Ui, rect: egui::Rect) {
    let color = if ui.visuals().dark_mode {
        Color32::from_rgb(70, 84, 94)
    } else {
        Color32::from_rgb(210, 220, 224)
    };
    let stroke = egui::Stroke::new(1.0, color);
    let right = rect.right() - 0.5;
    let bottom = rect.bottom() - 0.5;
    ui.painter().line_segment(
        [
            egui::pos2(right, rect.top()),
            egui::pos2(right, rect.bottom()),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            egui::pos2(rect.left(), bottom),
            egui::pos2(rect.right(), bottom),
        ],
        stroke,
    );
}

fn grid_header_cell(ui: &mut egui::Ui, name: &str, sql_type: &str) -> egui::Response {
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .rect_filled(rect, 0, accent(ui).gamma_multiply(0.07));
    let response = ui.allocate_rect(rect, egui::Sense::click());
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(8.0, 5.0)))
            .layout(egui::Layout::top_down(egui::Align::Min)),
        |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(
                egui::Label::new(RichText::new(name).strong())
                    .selectable(false)
                    .truncate(),
            );
            ui.add(
                egui::Label::new(RichText::new(sql_type).size(11.0).weak())
                    .selectable(false)
                    .truncate(),
            );
        },
    );
    grid_cell_border(ui, rect);
    response
}

fn grid_cell(ui: &mut egui::Ui, text: RichText, selected: bool, header: bool) -> egui::Response {
    let rect = ui.available_rect_before_wrap();
    if header || selected {
        ui.painter().rect_filled(
            rect,
            0,
            accent(ui).gamma_multiply(if selected { 0.16 } else { 0.07 }),
        );
    }
    let response = ui.add_sized(
        rect.size(),
        egui::Label::new(text)
            .selectable(false)
            .truncate()
            .sense(egui::Sense::click()),
    );
    grid_cell_border(ui, rect);
    if selected {
        ui.painter().rect_stroke(
            rect,
            0,
            egui::Stroke::new(1.0, accent(ui)),
            egui::StrokeKind::Inside,
        );
    }
    response
}

fn apply_tab_action(
    tabs: &mut Vec<QueryTab>,
    active: &mut usize,
    next: &mut usize,
    action: TabAction,
) {
    let active_id = tabs[*active].id;
    match action {
        TabAction::Select(index) => *active = index,
        TabAction::New => {
            tabs.push(QueryTab::new(*next));
            *next += 1;
            *active = tabs.len() - 1;
        }
        TabAction::Close(index) => {
            tabs.remove(index);
            if tabs.is_empty() {
                tabs.push(QueryTab::new(*next));
                *next += 1;
            }
            *active = tabs
                .iter()
                .position(|tab| tab.id == active_id)
                .unwrap_or_else(|| index.min(tabs.len() - 1));
        }
        TabAction::CloseOthers(index) => {
            let keep = tabs[index].id;
            tabs.retain(|tab| tab.id == keep);
            *active = 0;
        }
        TabAction::CloseRight(index) => {
            tabs.truncate(index + 1);
            *active = tabs
                .iter()
                .position(|tab| tab.id == active_id)
                .unwrap_or(index);
        }
    }
}

fn query_tabs(
    ui: &mut egui::Ui,
    tabs: &[QueryTab],
    active: usize,
    busy: bool,
) -> Option<TabAction> {
    let mut action = None;
    let active_id = tabs[active].id;
    let reveal_active = ui
        .ctx()
        .data(|data| data.get_temp::<Uuid>(egui::Id::new("visible-query-tab")) != Some(active_id));
    egui::Frame::new()
        .fill(if ui.visuals().dark_mode {
            Color32::from_rgb(24, 28, 34)
        } else {
            Color32::from_rgb(238, 242, 245)
        })
        .inner_margin(egui::Margin {
            left: 2,
            right: 2,
            top: 1,
            bottom: 1,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Reserve the new-query button so it stays visible even with many tabs.
                let width = (ui.available_width() - 48.0).max(80.0);
                egui::ScrollArea::horizontal()
                    .id_salt("query-tabs")
                    .max_width(width)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for (index, tab) in tabs.iter().enumerate() {
                                if index > 0 {
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(1.0, 20.0),
                                        egui::Sense::hover(),
                                    );
                                    let color = if ui.visuals().dark_mode {
                                        Color32::from_rgb(90, 103, 115)
                                    } else {
                                        Color32::from_rgb(170, 182, 193)
                                    };
                                    ui.painter().line_segment(
                                        [rect.center_top(), rect.center_bottom()],
                                        egui::Stroke::new(1.0, color),
                                    );
                                }
                                ui.push_id(tab.id, |ui| {
                                    egui::Frame::new()
                                        .fill(if index == active {
                                            surface(ui)
                                        } else {
                                            Color32::TRANSPARENT
                                        })
                                        .stroke(egui::Stroke::NONE)
                                        .corner_radius(egui::CornerRadius {
                                            nw: 5,
                                            ne: 5,
                                            sw: 5,
                                            se: 5,
                                        })
                                        .inner_margin(egui::Margin::symmetric(4, 1))
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = 2.0;
                                                let response = ui.add_sized(
                                                    egui::vec2(170.0, 24.0),
                                                    egui::Button::new(
                                                        RichText::new(format!(
                                                            "{}{}",
                                                            tab.name,
                                                            if tab.dirty() { " •" } else { "" }
                                                        ))
                                                        .size(13.0)
                                                        .color(if index == active {
                                                            accent(ui)
                                                        } else if ui.visuals().dark_mode {
                                                            Color32::from_rgb(190, 201, 211)
                                                        } else {
                                                            ui.visuals().text_color()
                                                        }),
                                                    )
                                                    .frame(false)
                                                    .truncate(),
                                                );
                                                if index == active && reveal_active {
                                                    response
                                                        .scroll_to_me(Some(egui::Align::Center));
                                                }
                                                response.clone().on_hover_text(
                                                    if let Some(target) = &tab.table_target {
                                                        format!(
                                                            "{} / {}.{}\n右键管理标签页 · 中键关闭",
                                                            target.database,
                                                            target.schema,
                                                            target.name
                                                        )
                                                    } else {
                                                        format!(
                                                            "{}\n右键管理标签页 · 中键关闭",
                                                            tab.name
                                                        )
                                                    },
                                                );
                                                if response.clicked() {
                                                    action = Some(TabAction::Select(index));
                                                }
                                                if !busy
                                                    && response
                                                        .clicked_by(egui::PointerButton::Middle)
                                                {
                                                    action = Some(TabAction::Close(index));
                                                }
                                                response.context_menu(|ui| {
                                                    if ui
                                                        .add_enabled(
                                                            !busy,
                                                            egui::Button::new("关闭标签页"),
                                                        )
                                                        .clicked()
                                                    {
                                                        action = Some(TabAction::Close(index));
                                                        ui.close();
                                                    }
                                                    if ui
                                                        .add_enabled(
                                                            !busy && tabs.len() > 1,
                                                            egui::Button::new("关闭其他标签页"),
                                                        )
                                                        .clicked()
                                                    {
                                                        action =
                                                            Some(TabAction::CloseOthers(index));
                                                        ui.close();
                                                    }
                                                    if ui
                                                        .add_enabled(
                                                            !busy && index + 1 < tabs.len(),
                                                            egui::Button::new("关闭右侧标签页"),
                                                        )
                                                        .clicked()
                                                    {
                                                        action = Some(TabAction::CloseRight(index));
                                                        ui.close();
                                                    }
                                                });
                                                if ui
                                                    .add_enabled(
                                                        !busy,
                                                        egui::Button::new("×").frame(false),
                                                    )
                                                    .on_hover_text("关闭标签页")
                                                    .clicked()
                                                {
                                                    action = Some(TabAction::Close(index));
                                                }
                                            });
                                        });
                                });
                            }
                        });
                    });
                if ui.button("＋").on_hover_text("新建 SQL 查询").clicked() {
                    action = Some(TabAction::New);
                }
            });
        });
    ui.ctx().data_mut(|data| {
        data.insert_temp(egui::Id::new("visible-query-tab"), active_id);
    });
    action
}

fn sql_editor(
    ui: &mut egui::Ui,
    tab: &mut QueryTab,
    height: &mut Option<f32>,
    show_results: bool,
) -> egui::Response {
    let minimum = if ui.available_height() < 320.0 {
        60.0
    } else {
        100.0
    };
    let maximum = (ui.available_height() - 180.0).max(minimum);
    let editor_height = if show_results {
        height
            .unwrap_or(ui.available_height() * 0.45)
            .clamp(minimum, maximum)
    } else {
        (ui.available_height() - 2.0).max(100.0)
    };
    let editor = egui::Frame::new()
        .fill(surface(ui))
        .corner_radius(10)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            egui::ScrollArea::both()
                .id_salt(("editor", tab.id))
                .min_scrolled_height(editor_height - 28.0)
                .max_height(editor_height - 28.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut layouter =
                        |ui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap_width: f32| {
                            let job = highlight_sql(text.as_str(), ui.visuals().dark_mode);
                            ui.fonts_mut(|fonts| fonts.layout_job(job))
                        };
                    let mut output = egui::TextEdit::multiline(&mut tab.sql)
                        .id_salt(tab.id)
                        .layouter(&mut layouter)
                        .font(egui::TextStyle::Monospace)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(1)
                        .frame(false)
                        .show(ui);
                    if let Some(range) = tab.select_range.take() {
                        let selected = egui::text::CCursorRange::two(
                            egui::text::CCursor::new(byte_to_char_index(&tab.sql, range.start)),
                            egui::text::CCursor::new(byte_to_char_index(&tab.sql, range.end)),
                        );
                        output.state.cursor.set_char_range(Some(selected));
                        output.state.store(ui.ctx(), output.response.id);
                        output.response.request_focus();
                    }
                    tab.selection = output
                        .cursor_range
                        .map(|range| {
                            let range = range.as_sorted_char_range();
                            tab.sql
                                .chars()
                                .skip(range.start)
                                .take(range.len())
                                .collect()
                        })
                        .unwrap_or_default();
                });
        });
    if !show_results {
        return editor.response;
    }
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 8.0),
        egui::Sense::click_and_drag(),
    );
    let response = response
        .on_hover_cursor(egui::CursorIcon::ResizeVertical)
        .on_hover_text("上下拖动，调整 SQL 编辑区与结果区高度；双击恢复默认");
    let color = if response.hovered() || response.dragged() {
        accent(ui)
    } else {
        ui.visuals().weak_text_color().gamma_multiply(0.45)
    };
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        egui::Stroke::new(1.0, color),
    );
    ui.painter().hline(
        rect.center().x - 18.0..=rect.center().x + 18.0,
        rect.center().y,
        egui::Stroke::new(3.0, color),
    );
    if response.dragged() {
        *height = Some((editor_height + response.drag_delta().y).clamp(minimum, maximum));
    }
    if response.double_clicked() {
        *height = None;
    }
    response
}

fn open_table_tab(
    tabs: &mut Vec<QueryTab>,
    next_tab: &mut usize,
    target: TableTarget,
    engine: Engine,
    table: &Table,
) -> (usize, bool) {
    if let Some(index) = tabs
        .iter()
        .position(|tab| tab.table_target.as_ref() == Some(&target))
    {
        return (index, false);
    }
    let mut tab = QueryTab::new(*next_tab);
    tab.name = table.name.clone();
    tab.sql = engine.preview_sql(table);
    tab.target = Some(QueryTarget {
        connection: target.connection,
        database: Some(target.database.clone()),
    });
    tab.saved_sql = tab.sql.clone();
    tab.table_target = Some(target);
    *next_tab += 1;
    tabs.push(tab);
    (tabs.len() - 1, true)
}

fn table_buttons(
    ui: &mut egui::Ui,
    tables: &[&Table],
    selection: &mut TreeSelection,
    connection: Uuid,
    database: &str,
    busy: bool,
) -> Option<Table> {
    let mut preview = None;
    for table in tables {
        let background = ui.painter().add(egui::Shape::Noop);
        let button = egui::Button::new((
            RichText::new(&table.name).color(ui.visuals().text_color()),
            egui::Atom::grow(),
        ))
        .frame(false)
        .min_size(egui::vec2(
            ui.available_width(),
            ui.spacing().interact_size.y,
        ));
        let response = ui
            .add(button)
            .on_hover_text(if table.kind.contains("VIEW") {
                "视图 · 单击选中，双击打开（前 200 行）"
            } else {
                "数据表 · 单击选中，双击打开（前 200 行）"
            });
        selection.row(
            &response,
            background,
            TreeNode::Table(connection, database.to_owned(), (*table).clone()),
        );
        if response.double_clicked() && !busy {
            preview = Some((*table).clone());
        }
    }
    preview
}

fn connection_heading(ui: &mut egui::Ui, form: &mut ConnectionForm) {
    ui.horizontal(|ui| {
        ui.label("连接名称");
        ui.add(
            egui::TextEdit::singleline(&mut form.profile.name)
                .hint_text("例如：开发数据库")
                .desired_width((ui.available_width() - 20.0).min(440.0)),
        );
    });
    let previous = form.profile.engine;
    ui.horizontal_wrapped(|ui| {
        ui.label("数据库类型");
        for engine in Engine::ALL {
            ui.selectable_value(&mut form.profile.engine, engine, engine.label());
        }
    });
    if previous != form.profile.engine {
        let defaults = |engine: Engine| match engine {
            Engine::MySql => "root",
            Engine::Postgres => "postgres",
            Engine::SqlServer => "sa",
        };
        let old_user = defaults(previous);
        let new_user = defaults(form.profile.engine);
        if form.profile.port == previous.port() {
            form.profile.port = form.profile.engine.port();
        }
        if form.profile.username == old_user {
            form.profile.username = new_user.into();
        }
        if form.profile.name == format!("本地 {}", previous.label()) {
            form.profile.name = format!("本地 {}", form.profile.engine.label());
        }
    }
}

fn database_fields(ui: &mut egui::Ui, form: &mut ConnectionForm) {
    ui.set_min_width(ui.available_width());
    ui.strong("数据库");
    ui.add_space(6.0);
    let input_width = (ui.available_width() - 90.0).clamp(140.0, 330.0);
    egui::Grid::new("connection-form")
        .num_columns(2)
        .spacing([14.0, 12.0])
        .show(ui, |ui| {
            ui.label("主机");
            ui.add(egui::TextEdit::singleline(&mut form.profile.host).desired_width(input_width));
            ui.end_row();
            ui.label("端口");
            ui.add(egui::DragValue::new(&mut form.profile.port).range(1..=65535));
            ui.end_row();
            ui.label("用户名");
            ui.add(
                egui::TextEdit::singleline(&mut form.profile.username).desired_width(input_width),
            );
            ui.end_row();
            ui.label("密码");
            ui.add(
                egui::TextEdit::singleline(&mut form.password)
                    .password(true)
                    .desired_width(input_width),
            );
            ui.end_row();
        });
    ui.add_space(8.0);
    ui.checkbox(
        &mut form.profile.remember_password,
        "记住数据库密码（本机保存）",
    );
    let hint = if form.profile.ssh.enabled {
        "主机和端口填写 SSH 服务器能访问的数据库地址。"
    } else {
        "连接后展开服务器，在数据库列表中选择要使用的库。"
    };
    ui.label(RichText::new(hint).small().weak());
}

fn ssh_fields(ui: &mut egui::Ui, form: &mut ConnectionForm) -> Option<String> {
    ui.set_min_width(ui.available_width());
    ui.strong("SSH 隧道");
    ui.checkbox(&mut form.profile.ssh.enabled, "通过 SSH 隧道连接");
    let mut selection_error = None;
    let input_width = (ui.available_width() - 90.0).clamp(140.0, 330.0);
    ui.add_enabled_ui(form.profile.ssh.enabled, |ui| {
        egui::Grid::new("ssh-form")
            .num_columns(2)
            .spacing([14.0, 10.0])
            .show(ui, |ui| {
                ui.label("主机");
                ui.add(
                    egui::TextEdit::singleline(&mut form.profile.ssh.host)
                        .desired_width(input_width),
                );
                ui.end_row();
                ui.label("端口");
                ui.add(egui::DragValue::new(&mut form.profile.ssh.port).range(1..=65535));
                ui.end_row();
                ui.label("用户名");
                ui.add(
                    egui::TextEdit::singleline(&mut form.profile.ssh.username)
                        .desired_width(input_width),
                );
                ui.end_row();
                ui.label("认证方式");
                let previous = form.profile.ssh.auth;
                egui::ComboBox::from_id_salt("ssh-auth")
                    .selected_text(match previous {
                        SshAuth::Password => "密码",
                        SshAuth::PrivateKey => "私钥",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut form.profile.ssh.auth, SshAuth::Password, "密码");
                        ui.selectable_value(
                            &mut form.profile.ssh.auth,
                            SshAuth::PrivateKey,
                            "私钥",
                        );
                    });
                if previous != form.profile.ssh.auth {
                    form.ssh_secret.clear();
                }
                ui.end_row();
                if form.profile.ssh.auth == SshAuth::PrivateKey {
                    ui.label("私钥文件");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut form.profile.ssh.private_key)
                                .desired_width((input_width - 100.0).max(90.0)),
                        )
                        .on_hover_text(&form.profile.ssh.private_key);
                        if ui.button("选择文件…").clicked() {
                            match choose_private_key(&form.profile.ssh.private_key) {
                                Ok(Some(path)) => {
                                    form.profile.ssh.private_key = path.display().to_string()
                                }
                                Ok(None) => {}
                                Err(error) => selection_error = Some(format!("{error:#}")),
                            }
                        }
                    });
                    ui.end_row();
                }
                ui.label(if form.profile.ssh.auth == SshAuth::Password {
                    "密码"
                } else {
                    "私钥口令"
                });
                ui.add(
                    egui::TextEdit::singleline(&mut form.ssh_secret)
                        .password(true)
                        .desired_width(input_width)
                        .hint_text(if form.profile.ssh.auth == SshAuth::PrivateKey {
                            "未加密的私钥可留空"
                        } else {
                            ""
                        }),
                );
                ui.end_row();
            });
        ui.add_space(6.0);
        ui.checkbox(
            &mut form.profile.ssh.remember_secret,
            "记住 SSH 密码 / 私钥口令（本机保存）",
        );
        egui::CollapsingHeader::new("SSH 主机校验")
            .default_open(false)
            .show(ui, |ui| {
                ui.label("known_hosts 文件");
                ui.add(
                    egui::TextEdit::singleline(&mut form.profile.ssh.known_hosts)
                        .desired_width(input_width),
                );
                ui.label("服务器指纹（可选，填写后以此指纹校验）");
                ui.add(
                    egui::TextEdit::singleline(&mut form.profile.ssh.fingerprint)
                        .hint_text("SHA256:…")
                        .desired_width(input_width),
                );
                ui.label(
                    RichText::new("首次连接的测试结果会显示指纹；核实后填入此处。")
                        .small()
                        .weak(),
                );
            });
    });
    selection_error
}

fn database_tls_settings(ui: &mut egui::Ui, profile: &mut Profile) {
    ui.checkbox(&mut profile.tls, "启用 TLS 加密");
    if !profile.tls {
        profile.trust_certificate = false;
    }
    ui.add_enabled(
        profile.tls,
        egui::Checkbox::new(&mut profile.trust_certificate, "信任服务器证书（跳过证书校验）"),
    ).on_hover_text("保留数据库 TLS 加密，但跳过数据库证书校验；请先确认服务器可信。此选项不改变 SSH 主机密钥校验。");
    if profile.ssh.enabled
        && profile.tls
        && !profile.trust_certificate
        && profile.engine != Engine::SqlServer
    {
        ui.label(
            RichText::new("严格数据库 TLS 校验要求证书包含 127.0.0.1；SSH 不会自动改变 TLS 设置。")
                .small(),
        );
    }
}

fn choose_private_key(current: &str) -> anyhow::Result<Option<PathBuf>> {
    let current = sqlshop::ssh::expand_path(current)?;
    let directory = if current.is_dir() {
        Some(current.clone())
    } else {
        current
            .parent()
            .filter(|path| path.is_dir())
            .map(PathBuf::from)
    }
    .or_else(|| {
        directories::UserDirs::new().map(|user| {
            let ssh = user.home_dir().join(".ssh");
            if ssh.is_dir() {
                ssh
            } else {
                user.home_dir().to_path_buf()
            }
        })
    });
    let mut dialog = rfd::FileDialog::new().set_title("选择 SSH 私钥文件（不要选择 .pub 公钥）");
    if let Some(directory) = directory {
        dialog = dialog.set_directory(directory);
    }
    // Private keys frequently have no extension; do not filter them out.
    dialog
        .pick_file()
        .map(|path| sqlshop::ssh::private_key_path(&path.display().to_string()))
        .transpose()
}

fn confirmation_dialog(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    content: impl FnOnce(&mut egui::Ui),
) {
    let dark = ctx.style().visuals.dark_mode;
    egui::Modal::new(egui::Id::new(id))
        .backdrop_color(Color32::from_black_alpha(if dark { 140 } else { 65 }))
        .frame(
            egui::Frame::popup(&ctx.style())
                .fill(chrome(ctx))
                .stroke(egui::Stroke::new(
                    1.0,
                    if dark {
                        Color32::from_rgb(52, 65, 74)
                    } else {
                        Color32::from_rgb(225, 232, 234)
                    },
                ))
                .corner_radius(16)
                .inner_margin(24),
        )
        .show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 80.0).clamp(240.0, 440.0));
            ui.spacing_mut().item_spacing = egui::vec2(12.0, 10.0);
            ui.label(RichText::new(title).size(21.0).strong());
            ui.add_space(6.0);
            content(ui);
        });
}

fn dialog_button(ui: &mut egui::Ui, label: &str, danger: bool) -> egui::Response {
    let fill = if danger {
        if ui.visuals().dark_mode {
            Color32::from_rgb(180, 65, 68)
        } else {
            Color32::from_rgb(192, 61, 65)
        }
    } else {
        accent(ui)
    };
    ui.add(
        egui::Button::new(RichText::new(label).color(Color32::WHITE))
            .fill(fill)
            .corner_radius(8)
            .min_size(egui::vec2(100.0, 36.0)),
    )
}

fn accent(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(96, 216, 179)
    } else {
        Color32::from_rgb(13, 130, 105)
    }
}

fn chrome(ctx: &egui::Context) -> Color32 {
    if ctx.style().visuals.dark_mode {
        Color32::from_rgb(21, 28, 35)
    } else {
        Color32::from_rgb(255, 255, 255)
    }
}

fn canvas(ctx: &egui::Context) -> Color32 {
    if ctx.style().visuals.dark_mode {
        Color32::from_rgb(15, 21, 27)
    } else {
        Color32::from_rgb(245, 248, 249)
    }
}

fn surface(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(25, 34, 42)
    } else {
        Color32::WHITE
    }
}

fn apply_theme(ctx: &egui::Context) {
    ctx.style_mut(|style| {
        let dark = style.visuals.dark_mode;
        let text = if dark {
            Color32::from_rgb(230, 236, 240)
        } else {
            Color32::from_rgb(35, 45, 53)
        };
        style.visuals.weak_text_color = Some(if dark {
            Color32::from_rgb(170, 184, 194)
        } else {
            Color32::from_rgb(86, 101, 111)
        });
        for widgets in [
            &mut style.visuals.widgets.noninteractive,
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
            &mut style.visuals.widgets.open,
        ] {
            widgets.fg_stroke.color = text;
        }
        let accent = if dark {
            Color32::from_rgb(96, 216, 179)
        } else {
            Color32::from_rgb(13, 130, 105)
        };
        let control = if dark {
            Color32::from_rgb(35, 46, 55)
        } else {
            Color32::from_rgb(237, 243, 244)
        };
        style.visuals.selection.bg_fill = accent.gamma_multiply(0.35);
        style.visuals.selection.stroke.color = accent;
        style.visuals.hyperlink_color = accent;
        style.visuals.widgets.inactive.bg_fill = control;
        style.visuals.widgets.inactive.weak_bg_fill = control;
        style.visuals.widgets.hovered.bg_fill = if dark {
            Color32::from_rgb(46, 61, 68)
        } else {
            Color32::from_rgb(222, 235, 233)
        };
        style.visuals.widgets.active.bg_fill = accent.gamma_multiply(if dark { 0.5 } else { 0.25 });
        style.visuals.widgets.noninteractive.bg_stroke.color = Color32::TRANSPARENT;
        style.visuals.widgets.inactive.bg_stroke.color = Color32::TRANSPARENT;
        style.visuals.window_fill = if dark {
            Color32::from_rgb(25, 34, 42)
        } else {
            Color32::WHITE
        };
        style.visuals.panel_fill = if dark {
            Color32::from_rgb(15, 21, 27)
        } else {
            Color32::from_rgb(245, 248, 249)
        };
    });
}

fn install_font(ctx: &egui::Context) {
    let custom = std::env::var("SQLSHOP_FONT").ok();
    let candidates = [
        custom.as_deref().unwrap_or(""),
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "C:/Windows/Fonts/msyh.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("cjk".into(), egui::FontData::from_owned(bytes).into());
            // Use the system's regular UI font instead of egui's light Latin font.
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "cjk".into());
            // Keep SQL and other monospace text aligned, with CJK as a fallback.
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("cjk".into());
            ctx.set_fonts(fonts);
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_text_colors_remain_readable_in_light_and_dark_modes() {
        let ctx = egui::Context::default();
        for (visuals, text, secondary) in [
            (
                egui::Visuals::light(),
                Color32::from_rgb(35, 45, 53),
                Color32::from_rgb(86, 101, 111),
            ),
            (
                egui::Visuals::dark(),
                Color32::from_rgb(230, 236, 240),
                Color32::from_rgb(170, 184, 194),
            ),
        ] {
            ctx.set_visuals(visuals);
            apply_theme(&ctx);
            let style = ctx.style();
            assert_eq!(style.visuals.text_color(), text);
            assert_eq!(style.visuals.weak_text_color(), secondary);
            assert_eq!(style.visuals.widgets.inactive.fg_stroke.color, text);
            assert_eq!(style.visuals.widgets.hovered.fg_stroke.color, text);
        }
    }

    fn session_test_app() -> App {
        let context = egui::Context::default();
        App {
            logo: context.load_texture(
                "test",
                egui::ColorImage::new([1, 1], vec![Color32::WHITE]),
                Default::default(),
            ),
            store: None,
            profiles: Vec::new(),
            credentials: HashMap::new(),
            selected: None,
            sessions: HashMap::new(),
            context: context.clone(),
            test_worker: Worker::start(|| {}),
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
            export_path: None,
            cell_editor: None,
            theme: egui::ThemePreference::System,
            screenshot_path: None,
            frames: 0,
            workspace_store: None,
            history: Vec::new(),
            history_open: false,
            history_search: String::new(),
            last_workspace: String::new(),
            last_autosave: std::time::Instant::now(),
            pending_history: HashMap::new(),
            confirm_tab_action: None,
            confirm_rebind: None,
            confirm_exit: false,
            allow_exit: false,
            structure: None,
            confirm_request: None,
            catalog: Catalog::default(),
            catalog_store: None,
            library_open: false,
            library_search: String::new(),
            catalog_dialog: None,
        }
    }

    fn add_test_session(app: &mut App, name: &str, database: &str) -> QueryTarget {
        let profile = Profile {
            name: name.into(),
            ..Default::default()
        };
        let target = QueryTarget {
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
            connection: b.connection,
            database: Some("other".into()),
        };
        app.select_query_target(other.clone());
        assert!(!app.can_execute());
        assert_eq!(app.sessions[&b.connection].active_database, b.database);
        app.sessions.get_mut(&b.connection).unwrap().busy = true;
        app.select_query_target(a);
        assert_eq!(app.execution_target(), Some(other));
    }

    #[test]
    fn query_dropdown_supports_offline_targets_without_auto_connecting() {
        let mut app = session_test_app();
        let profile = Profile::default();
        let target = QueryTarget {
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
    fn executed_sql_footer_handles_empty_and_long_parameterized_sql() {
        let mut app = session_test_app();
        let context = app.context.clone();
        for sql in [
            None,
            Some(format!(
                "SELECT {}\n-- 1: \"罗子言\"",
                "column, ".repeat(300)
            )),
        ] {
            app.tabs[0].last_executed_sql = sql;
            let _ = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 480.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::TopBottomPanel::bottom("test-sql-footer").show(ctx, |ui| {
                        app.executed_sql_footer(ui);
                        assert!(ui.min_rect().height() < 100.0);
                        assert!(ui.min_rect().width() <= 640.0);
                    });
                },
            );
        }
    }

    #[test]
    fn executed_table_sql_is_independent_of_editor_and_failed_refresh() {
        let mut app = session_test_app();
        let target = add_test_session(&mut app, "A", "db_a");
        let id = app.tabs[0].id;
        app.tabs[0].sql = "SELECT 'unsaved draft'".into();
        let page = sqlshop::db::TablePage {
            data: TableData {
                output: QueryOutput::default(),
                columns: vec![],
                primary_keys: vec![],
                keys: vec![],
            },
            has_more: false,
            sql: "SELECT * FROM items WHERE name = $1 LIMIT 201 OFFSET 200".into(),
            parameters: vec!["罗子言".into()],
        };
        let expected = page.execution_sql();
        app.tabs.push(QueryTab::new(2));
        app.active_tab = 1;
        app.handle_response(target.connection, Response::TablePage { tab: id, page });
        assert_eq!(
            app.tabs[0].last_executed_sql.as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(app.tabs[0].sql, "SELECT 'unsaved draft'");
        assert!(app.tabs[1].last_executed_sql.is_none());
        app.tabs[0].browse.offset = 400;
        app.handle_response(
            target.connection,
            Response::OperationError {
                tab: id,
                message: "failed refresh".into(),
                disconnected: false,
            },
        );
        assert_eq!(
            app.tabs[0].last_executed_sql.as_deref(),
            Some(expected.as_str())
        );
    }

    #[test]
    fn bound_tabs_and_session_busy_are_independent_of_sidebar_focus() {
        let mut app = session_test_app();
        let a = add_test_session(&mut app, "A", "db_a");
        let b = add_test_session(&mut app, "B", "db_b");
        app.selected = Some(a.connection);
        app.tabs[0].target = Some(a.clone());
        let tab_a = app.tabs[0].id;
        app.sessions.get_mut(&a.connection).unwrap().busy = true;
        app.sessions.get_mut(&a.connection).unwrap().running_tab = Some(tab_a);
        app.selected = Some(b.connection);
        assert_eq!(app.execution_target(), Some(a.clone()));
        assert!(!app.can_execute());
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
        assert!(app.can_execute()); // Still executes A even with sidebar B selected.
        app.sessions.get_mut(&a.connection).unwrap().active_database = Some("other".into());
        assert!(!app.can_execute()); // Never silently switches databases.
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
    fn connection_scroll_area_uses_full_sidebar_width() {
        for width in [265.0, 302.0, 450.0] {
            let ctx = egui::Context::default();
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::SidePanel::left("test-sidebar")
                    .exact_width(width)
                    .show(ctx, |ui| {
                        let available = ui.available_rect_before_wrap();
                        let scroll = egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for _ in 0..100 {
                                    ui.label("短表名");
                                }
                            });
                        let gap = available.right() - scroll.inner_rect.right();
                        assert!(gap >= 0.0 && gap <= ui.spacing().scroll.allocated_width() + 1.0);
                        assert!(scroll.inner_rect.width() > width - 50.0);
                    });
            });
        }
    }

    #[test]
    fn table_toolbar_is_one_row_and_filters_start_collapsed() {
        let mut app = session_test_app();
        assert!(!app.tabs[0].filters_open);
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let toolbar = ui.scope(|ui| app.table_controls(ui));
                assert!(toolbar.response.rect.height() <= 36.0);
                app.tabs[0].filters_open = true;
                let expanded = ui.scope(|ui| app.table_controls(ui));
                assert!(expanded.response.rect.height() > toolbar.response.rect.height());
            });
        });
    }

    #[test]
    fn row_selection_extends_from_anchor_in_both_directions() {
        assert_eq!(row_selection(None, 4, false), (4, 4));
        assert_eq!(row_selection(None, 4, true), (4, 4));
        assert_eq!(row_selection(Some((4, 6)), 1, true), (4, 1));
        assert_eq!(row_selection(Some((4, 6)), 1, false), (1, 1));
        for row in 0..7 {
            assert_eq!(row_in_selection(Some((4, 1)), row), (1..=4).contains(&row));
        }
        assert!(!row_in_selection(None, 0));
    }

    #[test]
    fn table_footer_stays_below_content_and_inside_window() {
        for size in [egui::vec2(1280.0, 820.0), egui::vec2(900.0, 600.0)] {
            let ctx = egui::Context::default();
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let available = ui.available_rect_before_wrap();
                        let (content, footer) = table_workspace_rects(available);
                        assert!(content.bottom() < footer.top());
                        assert!(available.contains_rect(footer));
                        assert_eq!(footer.height(), 32.0);
                        // A child with oversized content cannot move the reserved footer.
                        ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
                            ui.set_clip_rect(ui.clip_rect().intersect(content));
                            ui.allocate_space(egui::vec2(content.width(), 2000.0));
                        });
                        ui.scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| {
                            assert_eq!(ui.max_rect(), footer);
                            assert!(ui.max_rect().bottom() <= size.y);
                        });
                    });
                },
            );
        }
    }

    #[test]
    fn offline_sql_binds_once_and_rejects_other_contexts() {
        let mut tab = QueryTab::new(1);
        let target = QueryTarget {
            connection: Uuid::new_v4(),
            database: Some("dev".into()),
        };
        assert!(tab.bind_for_execution(&target));
        assert!(tab.bind_for_execution(&target));
        for other in [
            QueryTarget {
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

    #[test]
    fn grid_headers_render_names_types_and_visible_cell_borders_in_both_themes() {
        for dark in [false, true] {
            let ctx = egui::Context::default();
            ctx.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(180.0, GRID_HEADER_HEIGHT),
                        egui::Sense::hover(),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                        grid_header_cell(ui, "CustomerName", "varchar(100)");
                    });
                });
            });
            let texts: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| {
                    if let egui::epaint::Shape::Text(text) = &shape.shape {
                        Some(text.galley.job.text.as_str())
                    } else {
                        None
                    }
                })
                .collect();
            assert!(texts.contains(&"CustomerName"));
            assert!(texts.contains(&"varchar(100)"));
            let borders: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| {
                    if let egui::epaint::Shape::LineSegment { stroke, .. } = &shape.shape {
                        Some(stroke)
                    } else {
                        None
                    }
                })
                .collect();
            assert!(borders.len() >= 2);
            assert!(borders.iter().all(|stroke| stroke.color.a() == 255));
        }
    }

    #[test]
    fn grid_scrollbar_drag_does_not_resize_columns() {
        for dark in [false, true] {
            let ctx = egui::Context::default();
            ctx.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            let mut widths = vec![302.0, 110.0];
            let mut frame = |events| {
                let mut offset = 0.0;
                let mut header = egui::Rect::NOTHING;
                let mut body_rect = egui::Rect::NOTHING;
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(316.0, 300.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                            egui::ScrollArea::horizontal()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    let mut headers = Vec::new();
                                    let scroll = TableBuilder::new(ui)
                                        .resizable(false)
                                        .column(Column::exact(widths[0]).clip(true))
                                        .column(Column::exact(widths[1]).clip(true))
                                        .min_scrolled_height(200.0)
                                        .max_scroll_height(200.0)
                                        .header(GRID_HEADER_HEIGHT, |mut row| {
                                            for _ in 0..2 {
                                                row.col(|ui| {
                                                    headers.push(
                                                        grid_header_cell(ui, "column", "int").rect,
                                                    );
                                                });
                                            }
                                        })
                                        .body(|body| {
                                            body.rows(32.0, 100, |mut row| {
                                                for _ in 0..2 {
                                                    row.col(|ui| {
                                                        ui.label("value");
                                                    });
                                                }
                                            });
                                        });
                                    offset = scroll.state.offset.y;
                                    body_rect = scroll.inner_rect;
                                    header = headers[0];
                                    resize_grid_columns(
                                        ui,
                                        ui.id().with("test-widths"),
                                        &headers,
                                        &mut widths,
                                    );
                                });
                        });
                    },
                );
                (
                    output.platform_output.cursor_icon,
                    offset,
                    header,
                    body_rect,
                    widths.clone(),
                )
            };
            let (_, _, header, body, _) = frame(Vec::new());
            let start = egui::pos2(310.0, body.top() + 6.0);
            let (cursor, _, _, _, _) = frame(vec![egui::Event::PointerMoved(start)]);
            assert_ne!(cursor, egui::CursorIcon::ResizeColumn);
            frame(vec![egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }]);
            let end = start + egui::vec2(0.0, 60.0);
            let (_, offset, _, _, after) = frame(vec![egui::Event::PointerMoved(end)]);
            assert!(
                offset > 0.0,
                "the vertical scrollbar must receive the drag: header={header:?}, body={body:?}, cursor={cursor:?}, offset={offset}"
            );
            assert_eq!(after, vec![302.0, 110.0]);
            frame(vec![egui::Event::PointerButton {
                pos: end,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            let start = egui::pos2(header.right() - 3.0, header.center().y);
            let (cursor, _, _, _, _) = frame(vec![egui::Event::PointerMoved(start)]);
            assert_eq!(cursor, egui::CursorIcon::ResizeColumn);
            frame(vec![egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }]);
            let (_, _, _, _, after) = frame(vec![egui::Event::PointerMoved(
                start - egui::vec2(40.0, 0.0),
            )]);
            assert!(
                after[0] < 302.0,
                "header dragging must still resize the column"
            );
        }
    }

    #[test]
    fn dirty_sql_and_rectangular_clipboard_preserve_unicode_and_null() {
        let mut tab = QueryTab::new(1);
        assert!(!tab.dirty());
        tab.sql.push_str("\nSELECT '中文';");
        assert!(tab.dirty());
        tab.saved_sql = tab.sql.clone();
        assert!(!tab.dirty());
        let set = sqlshop::model::ResultSet {
            columns: vec!["a".into(), "b".into()],
            rows: vec![
                vec![Some("中文".into()), None],
                vec![Some("a\tb".into()), Some(String::new())],
            ],
            ..Default::default()
        };
        assert_eq!(
            selected_tsv(&set, Some((1, 1)), Some((0, 0))),
            "中文\tNULL\n\"a\tb\"\t"
        );
        assert_eq!(
            selected_tsv(&set, Some((1, 0)), Some((1, set.columns.len() - 1))),
            "\"a\tb\"\t"
        );
        assert!(cell_in_selection(Some((2, 2)), Some((0, 0)), 1, 1));
        assert!(!cell_in_selection(Some((2, 2)), Some((0, 0)), 3, 1));
    }

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
                connection: target.connection,
                database: Some(target.database.clone())
            })
        );
        assert!(!tabs[index].bind_for_execution(&QueryTarget {
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
        apply_tab_action(&mut tabs, &mut active, &mut next, TabAction::CloseRight(1));
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[active].id, active_id);
        apply_tab_action(&mut tabs, &mut active, &mut next, TabAction::CloseOthers(1));
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[active].id, active_id);
        apply_tab_action(&mut tabs, &mut active, &mut next, TabAction::Close(0));
        assert_eq!(tabs.len(), 1);
        assert_eq!(active, 0);
        assert_eq!(tabs[0].name, "查询 5");
        assert_eq!(next, 6);
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

    #[test]
    fn dragging_splitter_resizes_editor_and_reserves_result_space() {
        let ctx = egui::Context::default();
        let mut tab = QueryTab::new(1);
        let mut height = Some(180.0);
        let mut frame = |events| {
            let mut splitter = egui::Rect::NOTHING;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        splitter = sql_editor(ui, &mut tab, &mut height, true).rect;
                    });
                },
            );
            (splitter, height)
        };
        let (rect, _) = frame(Vec::new());
        let start = rect.center();
        frame(vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        let (_, grown) = frame(vec![egui::Event::PointerMoved(
            start + egui::vec2(0.0, 60.0),
        )]);
        assert!(grown.unwrap() > 180.0);
        let (_, clamped) = frame(vec![egui::Event::PointerMoved(
            start + egui::vec2(0.0, 1000.0),
        )]);
        assert!(clamped.unwrap() <= 420.0);
        let (_, shrunk) = frame(vec![egui::Event::PointerMoved(
            start - egui::vec2(0.0, 1000.0),
        )]);
        assert_eq!(shrunk, Some(100.0));
        frame(vec![egui::Event::PointerButton {
            pos: start - egui::vec2(0.0, 1000.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        let (rect, _) = frame(Vec::new());
        let reset_position = rect.center();
        for _ in 0..2 {
            frame(vec![
                egui::Event::PointerMoved(reset_position),
                egui::Event::PointerButton {
                    pos: reset_position,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            frame(vec![egui::Event::PointerButton {
                pos: reset_position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]);
        }
        assert_eq!(frame(Vec::new()).1, None);
    }

    fn table() -> Table {
        Table {
            schema: "public".into(),
            name: "users".into(),
            kind: "BASE TABLE".into(),
        }
    }

    #[test]
    fn tree_selection_moves_between_connection_database_and_table() {
        let ctx = egui::Context::default();
        let connection = Uuid::new_v4();
        let table = table();
        let nodes = [
            TreeNode::Connection(connection),
            TreeNode::Database(connection, "app_dev".into()),
            TreeNode::Table(connection, "app_dev".into(), table.clone()),
        ];
        let mut selection = TreeSelection::default();
        let mut positions = [egui::Pos2::ZERO; 3];
        let mut time = 0.0;
        let mut frame = |click: Option<(usize, bool, egui::PointerButton)>| {
            time += 0.1;
            let events = click.map_or_else(Vec::new, |(index, pressed, button)| {
                let pos = positions[index];
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            });
            let mut fill = egui::Color32::TRANSPARENT;
            let mut row_right = 0.0;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        for (index, label) in ["server", "app_dev"].into_iter().enumerate() {
                            let background = ui.painter().add(egui::Shape::Noop);
                            let header = egui::CollapsingHeader::new(
                                RichText::new(label).color(ui.visuals().text_color()),
                            )
                            .show(ui, |_| {});
                            positions[index] = header.header_response.rect.center();
                            selection.row(
                                &header.header_response,
                                background,
                                nodes[index].clone(),
                            );
                        }
                        // An active database still has selectable children while work is busy.
                        table_buttons(ui, &[&table], &mut selection, connection, "app_dev", true);
                        positions[2] = selection.rows.last().unwrap().1.center();
                        fill = accent(ui).gamma_multiply(if ui.visuals().dark_mode {
                            0.30
                        } else {
                            0.14
                        });
                        row_right = ui.max_rect().right();
                        selection.paint(ui);
                    });
                },
            );
            let table_text = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == table.name => Some(text),
                    _ => None,
                })
                .expect("table label must be rendered");
            assert!(
                table_text.pos.x < positions[2].x - 100.0,
                "table label stays left-aligned"
            );
            let backgrounds = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect) if rect.fill == fill => Some(rect),
                    _ => None,
                })
                .inspect(|rect| {
                    assert_eq!(
                        rect.rect.right(),
                        row_right,
                        "selection fills the row width"
                    );
                })
                .count();
            (selection.selected.clone(), backgrounds)
        };
        assert_eq!(frame(None), (None, 0));
        // Returning to the already-active database after a table click must move the
        // background back to the database, not keep either parent highlighted.
        for index in [0, 1, 2, 1, 2, 0] {
            frame(Some((index, true, egui::PointerButton::Primary)));
            assert_eq!(
                frame(Some((index, false, egui::PointerButton::Primary))),
                (Some(nodes[index].clone()), 1),
            );
        }
        frame(Some((1, true, egui::PointerButton::Secondary)));
        assert_eq!(
            frame(Some((1, false, egui::PointerButton::Secondary))),
            (Some(nodes[1].clone()), 1),
        );
    }

    #[test]
    fn tree_selection_background_covers_nested_indentation() {
        let ctx = egui::Context::default();
        let node = TreeNode::Connection(Uuid::new_v4());
        let mut selection = TreeSelection {
            selected: Some(node.clone()),
            ..Default::default()
        };
        let mut bounds = egui::Rect::NOTHING;
        let mut fill = Color32::TRANSPARENT;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                bounds = ui.max_rect();
                fill = accent(ui).gamma_multiply(if ui.visuals().dark_mode { 0.30 } else { 0.14 });
                ui.indent("group", |ui| {
                    ui.indent("database", |ui| {
                        let background = ui.painter().add(egui::Shape::Noop);
                        let response = ui.button("nested table");
                        assert!(response.rect.left() > bounds.left());
                        selection.row(&response, background, node.clone());
                    });
                });
                selection.paint(ui);
            });
        });
        let background = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.fill == fill => Some(rect),
                _ => None,
            })
            .expect("selected row background must be rendered");
        assert_eq!(background.rect.left(), bounds.left());
        assert_eq!(background.rect.right(), bounds.right());
    }

    #[test]
    fn tree_node_identity_includes_connection_database_and_schema() {
        let connection = Uuid::new_v4();
        let node = TreeNode::Table(connection, "app_dev".into(), table());
        assert_ne!(
            node,
            TreeNode::Table(Uuid::new_v4(), "app_dev".into(), table())
        );
        assert_ne!(
            node,
            TreeNode::Table(connection, "app_prod".into(), table())
        );
        let mut other_schema = table();
        other_schema.schema = "archive".into();
        assert_ne!(
            node,
            TreeNode::Table(connection, "app_dev".into(), other_schema)
        );
    }

    #[test]
    fn table_single_click_selects_and_double_click_opens() {
        let ctx = egui::Context::default();
        let table = table();
        let connection = Uuid::new_v4();
        let mut selection = TreeSelection::default();
        let mut frame = |time, pressed: Option<bool>, busy| {
            let position = egui::pos2(20.0, 18.0);
            let mut events = vec![egui::Event::PointerMoved(position)];
            if let Some(pressed) = pressed {
                events.push(egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let mut opened = None;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        opened = table_buttons(
                            ui,
                            &[&table],
                            &mut selection,
                            connection,
                            "app_dev",
                            busy,
                        );
                        selection.paint(ui);
                    });
                },
            );
            (opened, selection.selected.clone())
        };
        frame(0.0, None, false);
        frame(0.02, Some(true), false);
        let (opened, selected) = frame(0.03, Some(false), false);
        assert!(opened.is_none());
        assert_eq!(
            selected,
            Some(TreeNode::Table(connection, "app_dev".into(), table.clone()))
        );
        frame(0.12, Some(true), false);
        assert_eq!(frame(0.13, Some(false), false).0, Some(table.clone()));
        // Selection remains usable during work, but cannot launch a query.
        frame(1.0, Some(true), true);
        assert!(frame(1.01, Some(false), true).0.is_none());
        frame(1.1, Some(true), true);
        assert!(frame(1.11, Some(false), true).0.is_none());
    }

    #[test]
    fn reopening_table_preserves_tab_and_distinguishes_database_and_schema() {
        let table = table();
        let target = TableTarget {
            connection: Uuid::new_v4(),
            database: "app_dev".into(),
            schema: table.schema.clone(),
            name: table.name.clone(),
        };
        let mut tabs = vec![QueryTab::new(1)];
        let mut next = 2;
        let (index, created) = open_table_tab(
            &mut tabs,
            &mut next,
            target.clone(),
            Engine::Postgres,
            &table,
        );
        assert!(created);
        let id = tabs[index].id;
        tabs[index].sql = "SELECT id FROM public.users LIMIT 10".into();
        tabs[index].output = Some(QueryOutput::default());
        let reopened = open_table_tab(
            &mut tabs,
            &mut next,
            target.clone(),
            Engine::Postgres,
            &table,
        );
        assert_eq!(reopened, (index, false));
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[index].id, id);
        assert_eq!(tabs[index].sql, "SELECT id FROM public.users LIMIT 10");
        assert!(tabs[index].output.is_some());
        for distinct in [
            TableTarget {
                connection: Uuid::new_v4(),
                ..target.clone()
            },
            TableTarget {
                database: "app_prod".into(),
                ..target.clone()
            },
            TableTarget {
                schema: "audit".into(),
                ..target.clone()
            },
        ] {
            assert!(open_table_tab(&mut tabs, &mut next, distinct, Engine::Postgres, &table).1);
        }
        tabs.remove(index);
        let (index, created) =
            open_table_tab(&mut tabs, &mut next, target, Engine::Postgres, &table);
        assert!(created);
        assert_ne!(tabs[index].id, id);
    }
}
