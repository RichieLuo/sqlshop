use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use sqlshop::{
    model::{
        CellUpdate, Credentials, Engine, Profile, QueryOutput, SshAuth, Table, TableData,
        export_csv,
    },
    storage::Store,
    worker::{Request, Response, Worker},
};
use std::{collections::HashMap, path::PathBuf};
use uuid::Uuid;

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
    selection: String,
    output: Option<QueryOutput>,
    query_started: bool,
    source: String,
    result_index: usize,
    table_target: Option<TableTarget>,
    table_data: Option<TableData>,
    selected_cell: Option<(usize, usize)>,
    grid_width: f32,
}

impl QueryTab {
    fn new(index: usize) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: format!("查询 {index}"),
            sql: "-- 在这里编写 SQL；⌘ / Ctrl + Enter 执行选中内容或整段 SQL\nSELECT 1 AS hello;"
                .into(),
            selection: String::new(),
            output: None,
            query_started: false,
            source: String::new(),
            result_index: 0,
            table_target: None,
            table_data: None,
            selected_cell: None,
            grid_width: 0.0,
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

pub struct App {
    logo: egui::TextureHandle,
    store: Option<Store>,
    profiles: Vec<Profile>,
    credentials: HashMap<Uuid, Credentials>,
    selected: Option<Uuid>,
    connected: Option<Profile>,
    databases: Vec<String>,
    active_database: Option<String>,
    expand_connection: Option<Uuid>,
    expand_database: Option<String>,
    tables: Vec<Table>,
    selected_table: Option<Table>,
    filter: String,
    tabs: Vec<QueryTab>,
    active_tab: usize,
    next_tab: usize,
    editor_height: Option<f32>,
    running_tab: Option<Uuid>,
    worker: Worker,
    busy: bool,
    status: String,
    error: bool,
    form: Option<ConnectionForm>,
    confirm_delete: Option<Uuid>,
    export_path: Option<String>,
    cell_editor: Option<CellEditor>,
    theme: egui::ThemePreference,
    screenshot_path: Option<PathBuf>,
    frames: usize,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
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
            connected: None,
            databases: Vec::new(),
            active_database: None,
            expand_connection: None,
            expand_database: None,
            tables: Vec::new(),
            selected_table: None,
            filter: String::new(),
            tabs: vec![QueryTab::new(1)],
            active_tab: 0,
            next_tab: 2,
            editor_height: None,
            running_tab: None,
            worker: Worker::start(move || context.request_repaint()),
            busy: false,
            status: "就绪 · 添加连接即可开始".into(),
            error: false,
            form: None,
            confirm_delete: None,
            export_path: None,
            cell_editor: None,
            theme,
            screenshot_path,
            frames: 0,
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
            app.connected = Some(profile.clone());
            app.expand_connection = Some(profile.id);
            app.expand_database = Some("app_dev".into());
            app.profiles = vec![profile];
            app.active_database = Some("app_dev".into());
            app.databases = vec!["analytics".into(), "app_dev".into(), "mysql".into()];
            app.tables = ["orders", "users"]
                .into_iter()
                .map(|name| Table {
                    schema: "app_dev".into(),
                    name: name.into(),
                    kind: "BASE TABLE".into(),
                })
                .collect();
            for table in &app.tables {
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
            app.status = "数据库树界面测试 · 示例数据，未建立真实连接".into();
            if std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref() == Ok("results") {
                let tab = &mut app.tabs[app.active_tab];
                let columns: Vec<_> = ["id", "name", "email", "note", "amount", "created_at"]
                    .into_iter()
                    .enumerate()
                    .map(|(index, name)| sqlshop::model::TableColumn {
                        name: name.into(),
                        sql_type: "文本".into(),
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

    fn request(&mut self, request: Request, status: &str) {
        if self.busy {
            return;
        }
        match self.worker.send(request) {
            Ok(()) => {
                self.busy = true;
                self.error = false;
                self.status = status.into();
            }
            Err(error) => self.fail(error.to_string()),
        }
    }

    fn poll(&mut self) {
        while let Ok(response) = self.worker.receiver.try_recv() {
            self.busy = false;
            self.error = false;
            match response {
                Response::Tested => self.status = "连接测试成功".into(),
                Response::Connected(id) => {
                    self.connected = self.profiles.iter().find(|p| p.id == id).cloned();
                    self.databases.clear();
                    self.active_database = None;
                    self.tables.clear();
                    self.selected_table = None;
                    self.expand_connection = Some(id);
                    self.request(Request::Databases, "已连接服务器 · 正在读取数据库列表…");
                }
                Response::Disconnected => {
                    self.connected = None;
                    self.databases.clear();
                    self.active_database = None;
                    self.tables.clear();
                    self.selected_table = None;
                    self.status = "已断开连接".into();
                }
                Response::Databases(databases) => {
                    self.status = format!(
                        "已连接服务器 · {} 个数据库 · 点击数据库选择",
                        databases.len()
                    );
                    if self
                        .active_database
                        .as_ref()
                        .is_some_and(|name| !databases.contains(name))
                    {
                        self.active_database = None;
                        self.tables.clear();
                        self.selected_table = None;
                    }
                    self.databases = databases;
                }
                Response::DatabaseSelected(database) => {
                    self.active_database = Some(database.clone());
                    self.tables.clear();
                    self.selected_table = None;
                    self.expand_database = Some(database);
                    self.request(Request::Tables, "已选择数据库 · 正在读取表和视图…");
                }
                Response::Tables(tables) => {
                    if self
                        .selected_table
                        .as_ref()
                        .is_some_and(|table| !tables.contains(table))
                    {
                        self.selected_table = None;
                    }
                    self.status = format!("已连接 · {} 个表 / 视图", tables.len());
                    self.tables = tables;
                }
                Response::TableData { tab, mut data } => {
                    self.running_tab = None;
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
                    self.cell_editor = None;
                    self.status = "单元格已保存 · 正在刷新表数据…".into();
                    if let Some(index) = self.tabs.iter().position(|t| t.id == tab) {
                        self.load_table(index);
                    }
                }
                Response::Query { output, database } => {
                    let database_changed = self.active_database != database;
                    self.active_database = database;
                    self.status = format!(
                        "执行完成 · {} ms · {} 个结果集",
                        output.elapsed_ms,
                        output.sets.len()
                    );
                    if let Some(id) = self.running_tab.take()
                        && let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id)
                    {
                        tab.output = Some(output);
                        tab.result_index = 0;
                    }
                    if database_changed {
                        self.tables.clear();
                        self.selected_table = None;
                        if let Some(database) = &self.active_database {
                            self.expand_database = Some(database.clone());
                            self.request(Request::Tables, "数据库已切换 · 正在读取表和视图…");
                        }
                    }
                }
                Response::Error {
                    message,
                    disconnected,
                } => {
                    if disconnected {
                        self.connected = None;
                        self.databases.clear();
                        self.active_database = None;
                        self.tables.clear();
                        self.selected_table = None;
                    }
                    self.running_tab = None;
                    self.fail(if disconnected {
                        format!("{message}\n连接已关闭；重新连接后可继续操作。")
                    } else {
                        message
                    });
                }
            }
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
        if self.busy || self.cell_editor.is_some() {
            return;
        }
        let Some(target) = self.tabs[index].table_target.clone() else {
            return;
        };
        let Some(profile) = self.connected.as_ref() else {
            return;
        };
        if profile.id != target.connection
            || self.active_database.as_ref() != Some(&target.database)
        {
            self.fail("请先选择此表所属的连接和数据库".into());
            return;
        }
        let table = self
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
        tab.sql = profile.engine.preview_sql(&table);
        tab.selection.clear();
        tab.query_started = true;
        tab.output = None;
        tab.table_data = None;
        tab.selected_cell = None;
        tab.source = format!("{} / {}", profile.name, target.database);
        self.running_tab = Some(tab.id);
        let request = Request::ReadTable {
            tab: tab.id,
            database: target.database,
            table,
        };
        self.request(request, "正在读取表数据…");
    }

    fn execute(&mut self) {
        if self.busy
            || self.connected.is_none()
            || self.form.is_some()
            || self.confirm_delete.is_some()
            || self.export_path.is_some()
            || self.cell_editor.is_some()
        {
            return;
        }
        if let Some(target) = &self.tabs[self.active_tab].table_target
            && (self.connected.as_ref().map(|profile| profile.id) != Some(target.connection)
                || self.active_database.as_ref() != Some(&target.database))
        {
            self.fail(format!(
                "此表标签属于 {} / {}.{}，请先在左侧选择对应连接和数据库再运行。",
                target.database, target.schema, target.name
            ));
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
        tab.query_started = true;
        tab.source = self
            .connected
            .as_ref()
            .map(|p| {
                format!(
                    "{} / {}",
                    p.name,
                    self.active_database.as_deref().unwrap_or("服务器")
                )
            })
            .unwrap_or_default();
        self.running_tab = Some(tab.id);
        self.request(Request::Query(sql), "正在执行 SQL…");
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.label(
            RichText::new("EXPLORER  /  数据资源")
                .size(11.0)
                .color(accent(ui)),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("连接管理").size(19.0).strong());
            if ui
                .add_enabled(
                    !self.busy,
                    egui::Button::new(RichText::new("＋ 新建连接").color(Color32::WHITE))
                        .fill(accent(ui)),
                )
                .clicked()
            {
                self.form = Some(ConnectionForm {
                    profile: Profile::default(),
                    password: String::new(),
                    ssh_secret: String::new(),
                });
            }
        });
        ui.add_space(8.0);
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
        let mut disconnect = false;
        let mut refresh_databases = false;
        let mut refresh_tables = false;
        let mut to_select = None;
        let mut preview = None;
        let expand_connection = self.expand_connection.take();
        let expand_database = self.expand_database.take();
        egui::ScrollArea::vertical()
            .id_salt("connection-tree")
            .show(ui, |ui| {
                for profile in self.profiles.clone() {
                    let connected = self.connected.as_ref().is_some_and(|p| p.id == profile.id);
                    let title = format!("{}  {}", if connected { "●" } else { "○" }, profile.name);
                    let title = if self.selected == Some(profile.id) {
                        RichText::new(title).strong().color(accent(ui))
                    } else {
                        RichText::new(title)
                    };
                    let header = egui::CollapsingHeader::new(title)
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
                                    if profile.ssh.enabled { " · SSH" } else { "" }
                                ))
                                .small()
                                .weak(),
                            );
                            if !connected {
                                ui.label(
                                    RichText::new("双击连接，或右键选择连接服务器")
                                        .small()
                                        .weak(),
                                );
                                return;
                            }
                            if self.databases.is_empty() {
                                ui.label(
                                    RichText::new(if self.busy {
                                        "正在读取数据库列表…"
                                    } else {
                                        "未发现可访问的数据库"
                                    })
                                    .weak(),
                                );
                            }
                            for database in &self.databases {
                                let active = self.active_database.as_ref() == Some(database);
                                let matches_database = database.to_lowercase().contains(&filter);
                                if !filter.is_empty() && !matches_database && !active {
                                    continue;
                                }
                                let title = if active {
                                    RichText::new(format!("●  {database}"))
                                        .strong()
                                        .color(accent(ui))
                                } else {
                                    RichText::new(database)
                                };
                                let database_header = egui::CollapsingHeader::new(title)
                                    .id_salt(("database", profile.id, database))
                                    .open(if expand_database.as_ref() == Some(database) {
                                        Some(true)
                                    } else {
                                        None
                                    })
                                    .show(ui, |ui| {
                                        if !active {
                                            ui.label(
                                                RichText::new("点击数据库名称加载表和视图")
                                                    .small()
                                                    .weak(),
                                            );
                                            return;
                                        }
                                        let tables: Vec<_> = self
                                            .tables
                                            .iter()
                                            .filter(|table| {
                                                matches_database
                                                    || format!("{}.{}", table.schema, table.name)
                                                        .to_lowercase()
                                                        .contains(&filter)
                                            })
                                            .collect();
                                        if profile.engine == Engine::MySql {
                                            if let Some(table) = table_buttons(
                                                ui,
                                                &tables,
                                                &mut self.selected_table,
                                                self.busy,
                                            ) {
                                                preview = Some(table);
                                            }
                                        } else {
                                            let mut schemas: Vec<_> =
                                                tables.iter().map(|table| &table.schema).collect();
                                            schemas.sort();
                                            schemas.dedup();
                                            for schema in schemas {
                                                egui::CollapsingHeader::new(schema)
                                                    .id_salt((profile.id, database, schema))
                                                    .default_open(true)
                                                    .show(ui, |ui| {
                                                        let schema_tables: Vec<_> = tables
                                                            .iter()
                                                            .copied()
                                                            .filter(|table| &table.schema == schema)
                                                            .collect();
                                                        if let Some(table) = table_buttons(
                                                            ui,
                                                            &schema_tables,
                                                            &mut self.selected_table,
                                                            self.busy,
                                                        ) {
                                                            preview = Some(table);
                                                        }
                                                    });
                                            }
                                        }
                                        if self.tables.is_empty() {
                                            ui.label(
                                                RichText::new(if self.busy {
                                                    "正在读取表目录…"
                                                } else {
                                                    "没有表或视图"
                                                })
                                                .weak(),
                                            );
                                        } else if tables.is_empty() {
                                            ui.label(RichText::new("没有匹配的表或视图").weak());
                                        }
                                    });
                                if database_header.header_response.clicked()
                                    && !self.busy
                                    && !active
                                {
                                    to_select = Some(database.clone());
                                }
                                database_header.header_response.context_menu(|ui| {
                                    if ui
                                        .add_enabled(
                                            !self.busy && !active,
                                            egui::Button::new("选择数据库"),
                                        )
                                        .clicked()
                                    {
                                        to_select = Some(database.clone());
                                        ui.close();
                                    }
                                    if ui
                                        .add_enabled(
                                            !self.busy && active,
                                            egui::Button::new("刷新表和视图"),
                                        )
                                        .clicked()
                                    {
                                        refresh_tables = true;
                                        ui.close();
                                    }
                                });
                            }
                        });
                    if header.header_response.clicked()
                        || header.header_response.secondary_clicked()
                    {
                        self.selected = Some(profile.id);
                    }
                    if header.header_response.double_clicked() && !self.busy && !connected {
                        to_connect = Some(profile.id);
                    }
                    header.header_response.context_menu(|ui| {
                        if connected {
                            if ui
                                .add_enabled(!self.busy, egui::Button::new("断开连接"))
                                .clicked()
                            {
                                disconnect = true;
                                ui.close();
                            }
                            if ui
                                .add_enabled(!self.busy, egui::Button::new("刷新数据库列表"))
                                .clicked()
                            {
                                refresh_databases = true;
                                ui.close();
                            }
                        } else if ui
                            .add_enabled(!self.busy, egui::Button::new("连接服务器"))
                            .clicked()
                        {
                            to_connect = Some(profile.id);
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(!self.busy, egui::Button::new("编辑连接"))
                            .clicked()
                        {
                            to_edit = Some(profile.id);
                            ui.close();
                        }
                        if ui
                            .add_enabled(!self.busy, egui::Button::new("删除连接…"))
                            .clicked()
                        {
                            to_delete = Some(profile.id);
                            ui.close();
                        }
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
        if let Some(id) = to_edit {
            self.selected = Some(id);
            self.edit_connection(id);
        }
        if let Some(id) = to_delete {
            self.selected = Some(id);
            self.confirm_delete = Some(id);
        }
        if disconnect {
            self.request(Request::Disconnect, "正在断开连接…");
        }
        if refresh_databases {
            self.request(Request::Databases, "正在刷新数据库列表…");
        }
        if refresh_tables {
            self.request(Request::Tables, "正在刷新表目录…");
        }
        if let Some(id) = to_connect {
            self.selected = Some(id);
            self.connect_selected();
        }
        if let Some(database) = to_select {
            self.request(Request::SelectDatabase(database), "正在选择数据库…");
        }
        if let Some(table) = preview
            && let Some(profile) = &self.connected
            && let Some(database) = &self.active_database
        {
            let target = TableTarget {
                connection: profile.id,
                database: database.clone(),
                schema: table.schema.clone(),
                name: table.name.clone(),
            };
            let (index, created) = open_table_tab(
                &mut self.tabs,
                &mut self.next_tab,
                target,
                profile.engine,
                &table,
            );
            self.active_tab = index;
            if created {
                self.load_table(index);
            }
        }
    }

    fn workspace(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.label(
            RichText::new("WORKSPACE  /  SQL 编辑器")
                .size(11.0)
                .color(accent(ui)),
        );
        ui.add_space(7.0);
        if let Some(action) = query_tabs(
            ui,
            &self.tabs,
            self.active_tab,
            self.busy || self.cell_editor.is_some(),
        ) {
            apply_tab_action(
                &mut self.tabs,
                &mut self.active_tab,
                &mut self.next_tab,
                action,
            );
        }
        ui.add_space(4.0);
        ui.separator();
        if let Some(profile) = &self.connected {
            ui.label(
                RichText::new(format!(
                    "●  {}   /   {}",
                    profile.name,
                    self.active_database.as_deref().unwrap_or("请选择数据库")
                ))
                .color(accent(ui)),
            );
        } else {
            ui.label(RichText::new("○  未连接数据库   /   可先编写 SQL，连接后执行").weak());
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.busy && self.connected.is_some(),
                    egui::Button::new(RichText::new("▶  运行查询").color(Color32::WHITE))
                        .fill(accent(ui)),
                )
                .clicked()
            {
                self.execute();
            }
            ui.label(
                RichText::new("⌘ / Ctrl + Enter  ·  优先执行选中内容")
                    .small()
                    .weak(),
            );
            if self.busy {
                ui.spinner();
            }
            if self.tabs[self.active_tab].table_target.is_some()
                && ui
                    .add_enabled(
                        !self.busy && self.cell_editor.is_none(),
                        egui::Button::new("刷新表数据"),
                    )
                    .clicked()
            {
                self.load_table(self.active_tab);
            }
        });
        ui.add_space(4.0);
        let show_results =
            self.tabs[self.active_tab].query_started || self.tabs[self.active_tab].output.is_some();
        sql_editor(
            ui,
            &mut self.tabs[self.active_tab],
            &mut self.editor_height,
            show_results,
        );
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter)) {
            self.execute();
        }
        if !show_results {
            return;
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("查询结果").size(17.0).strong());
            ui.label(RichText::new("RESULTS").size(10.0).color(accent(ui)));
        });
        let tab = &mut self.tabs[self.active_tab];
        let Some(output) = &tab.output else {
            ui.label(
                RichText::new(if self.running_tab == Some(tab.id) {
                    "正在执行查询…"
                } else {
                    "暂无查询结果，执行状态见底部状态栏。"
                })
                .weak(),
            );
            return;
        };
        ui.horizontal(|ui| {
            ui.label(format!("结果 · {} ms", output.elapsed_ms));
            ui.label(RichText::new(&tab.source).weak());
            for index in 0..output.sets.len() {
                ui.selectable_value(&mut tab.result_index, index, format!("#{}", index + 1));
            }
        });
        let Some(set) = output.sets.get(tab.result_index) else {
            ui.label("执行完成，没有行结果；SQL Server 批处理不报告受影响行数。");
            return;
        };
        ui.horizontal(|ui| {
            ui.label(format!("{} 行 · {} 列", set.rows.len(), set.columns.len()));
            if let Some(affected) = set.affected {
                ui.label(RichText::new(format!("数据库报告行数: {affected}")).weak());
            }
            if !set.columns.is_empty() && ui.button("导出 CSV").clicked() {
                let directory = directories::UserDirs::new()
                    .and_then(|d| d.download_dir().map(PathBuf::from))
                    .unwrap_or_else(|| PathBuf::from("."));
                self.export_path = Some(
                    directory
                        .join(format!(
                            "sqlshop-{}.csv",
                            chrono::Local::now().format("%Y%m%d%H%M%S")
                        ))
                        .display()
                        .to_string(),
                );
            }
        });
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
            self.connected.as_ref().map(|p| p.id) == Some(target.connection)
                && self.active_database.as_ref() == Some(&target.database)
        });
        if let Some(data) = &tab.table_data {
            ui.label(
                RichText::new(if !context_matches {
                    "此表来自其他连接或数据库，选择对应数据库后可编辑。"
                } else if data.primary_keys.is_empty() {
                    "只读：此表没有主键，无法准确定位要修改的行。"
                } else if !data.columns.iter().any(|column| column.editable) {
                    "只读：视图、生成列或当前表类型不支持编辑。"
                } else {
                    "双击单元格编辑 · 保存后更新数据库并刷新 · 拖动表头分隔线调整列宽"
                })
                .small()
                .weak(),
            );
        } else {
            ui.label(
                RichText::new("SQL 查询结果只读 · 双击左侧表名打开可编辑的表数据")
                    .small()
                    .weak(),
            );
        }
        if let Some(editor) = result_grid(
            ui,
            tab,
            context_matches && !self.busy && self.cell_editor.is_none(),
        ) {
            self.cell_editor = Some(editor);
        }
    }

    fn cell_editor_window(&mut self, ctx: &egui::Context) {
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
                ui.add_enabled_ui(!self.busy, |ui| {
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
                if self.busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("正在保存…");
                    });
                }
                if self.error {
                    ui.colored_label(Color32::from_rgb(220, 90, 90), &self.status);
                }
            });
        if !self.busy
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            cancel = true;
        }
        if !self.busy
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter))
        {
            save = true;
        }
        if (!open || cancel) && !self.busy {
            self.cell_editor = None;
            return;
        }
        if save && !self.busy {
            if self.connected.as_ref().map(|p| p.id) != Some(editor.connection)
                || self.active_database.as_ref() != Some(&editor.change.database)
            {
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
            self.request(Request::UpdateCell { tab, change }, "正在保存单元格修改…");
        }
    }

    fn connection_window(&mut self, ctx: &egui::Context) {
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
                ui.add_enabled_ui(!self.busy, |ui| {
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
                if self.busy {
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
        let mut cancel = false;
        let mut delete = false;
        egui::Window::new("删除本机连接")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("将删除此连接配置及保存的密码，数据库中的数据不受影响。");
                ui.horizontal(|ui| {
                    cancel = ui.button("取消").clicked();
                    delete = ui
                        .add_enabled(!self.busy, egui::Button::new("删除连接"))
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
                    self.confirm_delete = None;
                    if self.connected.as_ref().is_some_and(|p| p.id == id) {
                        self.request(Request::Disconnect, "正在断开连接…");
                    }
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
            .frame(
                egui::Frame::new()
                    .fill(chrome(ctx))
                    .inner_margin(egui::Margin::symmetric(20, 10)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Image::new(&self.logo)
                            .fit_to_exact_size(egui::vec2(36.0, 36.0))
                            .alt_text("SQLShop 猞猁 Logo"),
                    );
                    ui.label(RichText::new("SQLShop").strong().size(21.0));
                    ui.add_space(8.0);
                    ui.label(RichText::new("/  数据库工作台").small().weak());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
                        ui.label(
                            RichText::new(if let Some(profile) = &self.connected {
                                format!("● {}", profile.name)
                            } else {
                                "○ 未连接".into()
                            })
                            .color(if self.connected.is_some() {
                                accent(ui)
                            } else {
                                ui.visuals().weak_text_color()
                            }),
                        );
                    });
                });
            });
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(chrome(ctx))
                    .inner_margin(egui::Margin::symmetric(18, 8)),
            )
            .show(ctx, |ui| {
                if self.error {
                    ui.colored_label(Color32::from_rgb(240, 120, 120), &self.status);
                } else {
                    ui.label(RichText::new(format!("●  {}", self.status)).small().weak());
                }
            });
        egui::SidePanel::left("sidebar")
            .default_width(295.0)
            .width_range(265.0..=450.0)
            .frame(
                egui::Frame::new()
                    .fill(chrome(ctx))
                    .inner_margin(egui::Margin::symmetric(16, 8)),
            )
            .show(ctx, |ui| self.sidebar(ui));
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(canvas(ctx))
                    .inner_margin(egui::Margin::symmetric(22, 10)),
            )
            .show(ctx, |ui| self.workspace(ui));
        self.connection_window(ctx);
        self.delete_window(ctx);
        self.export_window(ctx);
        self.cell_editor_window(ctx);
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

fn result_grid(ui: &mut egui::Ui, tab: &mut QueryTab, can_edit: bool) -> Option<CellEditor> {
    let set = tab.output.as_ref()?.sets.get(tab.result_index)?;
    let available_width = ui.available_width();
    let reset_widths = (tab.grid_width - available_width).abs() > 2.0;
    tab.grid_width = available_width;
    let mut editor = None;
    let target = tab.table_target.as_ref();
    let metadata = tab.table_data.as_ref();
    egui::Frame::new()
        .fill(surface(ui))
        .corner_radius(6)
        .stroke(egui::Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            let height = ui.available_height().max(80.0);
            egui::ScrollArea::horizontal()
                .id_salt(("result-grid-scroll", tab.id, tab.result_index))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let builder = TableBuilder::new(ui)
                        .id_salt(("data-grid", tab.id, tab.result_index))
                        .striped(true)
                        .resizable(true)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .min_scrolled_height((height - 34.0).max(40.0))
                        .max_scroll_height((height - 34.0).max(40.0))
                        .column(Column::exact(44.0))
                        .columns(
                            Column::remainder().at_least(110.0).clip(true),
                            set.columns.len(),
                        );
                    if reset_widths {
                        builder.reset();
                    }
                    builder
                        .header(34.0, |mut header| {
                            header.col(|ui| {
                                grid_cell(ui, RichText::new("#").strong(), false, true);
                            });
                            for (index, name) in set.columns.iter().enumerate() {
                                header.col(|ui| {
                                    let response =
                                        grid_cell(ui, RichText::new(name).strong(), false, true);
                                    let hint = metadata
                                        .and_then(|data| data.columns.get(index))
                                        .map(|column| {
                                            format!(
                                                "{} · {}{}",
                                                column.name,
                                                column.sql_type,
                                                if column.primary_key { " · 主键" } else { "" }
                                            )
                                        })
                                        .unwrap_or_else(|| name.clone());
                                    response.on_hover_text(hint);
                                });
                            }
                        })
                        .body(|body| {
                            body.rows(32.0, set.rows.len(), |mut row| {
                                let row_index = row.index();
                                row.col(|ui| {
                                    grid_cell(
                                        ui,
                                        RichText::new((row_index + 1).to_string()).weak(),
                                        false,
                                        false,
                                    );
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
                                            tab.selected_cell == Some((row_index, column_index)),
                                            false,
                                        );
                                        if response.clicked() {
                                            tab.selected_cell = Some((row_index, column_index));
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
                                        response.on_hover_text(value).context_menu(|ui| {
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
                });
        });
    editor
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
            .truncate()
            .sense(egui::Sense::click()),
    );
    let stroke = egui::Stroke::new(1.0, ui.visuals().weak_text_color().gamma_multiply(0.15));
    ui.painter()
        .line_segment([rect.right_top(), rect.right_bottom()], stroke);
    ui.painter()
        .line_segment([rect.left_bottom(), rect.right_bottom()], stroke);
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
                        ui.push_id(tab.id, |ui| {
                            egui::Frame::new()
                                .fill(if index == active {
                                    surface(ui)
                                } else {
                                    Color32::TRANSPARENT
                                })
                                .stroke(if index == active {
                                    egui::Stroke::new(1.0, accent(ui))
                                } else {
                                    egui::Stroke::NONE
                                })
                                .corner_radius(6)
                                .inner_margin(egui::Margin::symmetric(4, 2))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 2.0;
                                        let response = ui.add_sized(
                                            egui::vec2(150.0, 30.0),
                                            egui::Button::new(&tab.name).frame(false).truncate(),
                                        );
                                        if index == active && reveal_active {
                                            response.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        response.clone().on_hover_text(
                                            if let Some(target) = &tab.table_target {
                                                format!(
                                                    "{} / {}.{}\n右键管理标签页 · 中键关闭",
                                                    target.database, target.schema, target.name
                                                )
                                            } else {
                                                format!("{}\n右键管理标签页 · 中键关闭", tab.name)
                                            },
                                        );
                                        if response.clicked() {
                                            action = Some(TabAction::Select(index));
                                        }
                                        if !busy && response.clicked_by(egui::PointerButton::Middle)
                                        {
                                            action = Some(TabAction::Close(index));
                                        }
                                        response.context_menu(|ui| {
                                            if ui
                                                .add_enabled(!busy, egui::Button::new("关闭标签页"))
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
                                                action = Some(TabAction::CloseOthers(index));
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
                                            .add_enabled(!busy, egui::Button::new("×").frame(false))
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
    let maximum = (ui.available_height() - 180.0).max(100.0);
    let editor_height = if show_results {
        height
            .unwrap_or(ui.available_height() * 0.45)
            .clamp(100.0, maximum)
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
                    let output = egui::TextEdit::multiline(&mut tab.sql)
                        .id_salt(tab.id)
                        .font(egui::TextStyle::Monospace)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(1)
                        .frame(false)
                        .show(ui);
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
        *height = Some((editor_height + response.drag_delta().y).clamp(100.0, maximum));
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
    tab.table_target = Some(target);
    *next_tab += 1;
    tabs.push(tab);
    (tabs.len() - 1, true)
}

fn table_buttons(
    ui: &mut egui::Ui,
    tables: &[&Table],
    selected: &mut Option<Table>,
    busy: bool,
) -> Option<Table> {
    let mut preview = None;
    for table in tables {
        let response = ui
            .add(
                egui::Button::new(&table.name)
                    .frame(false)
                    .selected(selected.as_ref() == Some(*table)),
            )
            .on_hover_text(if table.kind.contains("VIEW") {
                "视图 · 单击选中，双击打开（前 200 行）"
            } else {
                "数据表 · 单击选中，双击打开（前 200 行）"
            });
        if response.clicked() {
            *selected = Some((*table).clone());
        }
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
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(family).or_default().push("cjk".into());
            }
            ctx.set_fonts(fonts);
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn table_single_click_selects_and_double_click_opens() {
        let ctx = egui::Context::default();
        let table = table();
        let mut selected = None;
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
                        opened = table_buttons(ui, &[&table], &mut selected, busy);
                    });
                },
            );
            (opened, selected.clone())
        };
        frame(0.0, None, false);
        frame(0.02, Some(true), false);
        let (opened, selected) = frame(0.03, Some(false), false);
        assert!(opened.is_none());
        assert_eq!(selected, Some(table.clone()));
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
