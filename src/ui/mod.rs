mod query_safety;
use query_safety::*;

mod commands;
mod connections;
mod dialogs;
mod editor;
mod events;
mod grid;
mod shell;
mod startup;
mod theme;
mod workspace;
use connections::*;
use editor::sql_editor;
use grid::*;
use theme::*;

#[cfg(test)]
mod tests;

use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use sqlshop::{
    db::{
        ColumnFilter, ColumnSort, FilterOperator, StructureChange, TableBrowseOptions,
        TableStructure, structure_change_sql,
    },
    model::{
        CellUpdate, CreateTableColumn, Credentials, Engine, InsertValue, Profile, QueryOutput,
        ResultSet, SshAuth, Table, TableData, export_csv, export_csv_rows, parse_nullable_csv,
    },
    sql_editor::{
        byte_to_char_index, char_to_byte_index, current_statement_range, find_matches,
        highlight_sql, insert_completion, replace_all, replace_match,
    },
    storage::Store,
    worker::{Request, Response, Worker},
    workspace::{
        HistoryEntry, QueryTarget, SavedTab, Workspace, WorkspaceStore, read_sql_file,
        write_sql_file,
    },
};
use std::{collections::HashMap, path::PathBuf};
use uuid::Uuid;

mod advanced_ui;
mod backup_ui;
mod catalog_ui;
mod compare_ui;
mod grid_browse_ui;
mod grid_columns_ui;
mod grid_selection_ui;
mod icons;
mod local_database_ui;
mod management_ui;
mod object_browser_host;
mod object_browser_ui;
mod object_design_ui;
mod oracle_database_ui;
mod query_result_ui;
mod query_session_ui;
mod recovery_ui;
mod redis_connections_ui;
mod redis_ui;
use redis_connections_ui::{ConnectionKind, TreeConnection};
use sqlshop::redis_database::{RedisProfile, RedisWorker};
mod structure_ui;
mod tab_bar_ui;
mod table_insert_ui;
mod table_tree_ui;
mod tree_navigation_ui;
mod window_chrome;
mod workspace_ui;
use catalog_ui::CatalogDialog;
use sqlshop::catalog::{Catalog, CatalogStore, SavedQuery};

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
    last_executed_document: Option<String>,
    last_executed_char_start: Option<usize>,
    selection_char_start: Option<usize>,
    selection: String,
    cursor: usize,
    focus_editor: bool,
    query_epoch: u64,
    query_error: Option<String>,
    query_diagnostic: Option<sqlshop::model::QueryDiagnostic>,
    query_elapsed_ms: Option<u128>,
    output: Option<QueryOutput>,
    query_started: bool,
    source: String,
    result_index: usize,
    table_target: Option<TableTarget>,
    table_kind: Option<String>,
    design: Option<structure_ui::DesignDocument>,
    definition: Option<object_browser_host::DefinitionDocument>,
    advanced: Option<sqlshop::advanced::AdvancedDocument>,
    backup: Option<backup_ui::BackupDocument>,
    local_database: Option<local_database_ui::LocalDocument>,
    oracle: Option<oracle_database_ui::OracleDocument>,
    redis: Option<redis_ui::RedisDocument>,
    jobs: Option<management_ui::JobsDocument>,
    compare: Option<compare_ui::CompareDocument>,
    table_data: Option<TableData>,
    table_snapshot_stale: bool,
    table_read_pending: bool,
    table_write_notice: Option<String>,
    table_write_pending: bool,
    recovery: Option<recovery_ui::RecoveryDocument>,
    recovery_epoch: u64,
    recovery_manager_epoch: Option<u64>,
    recovery_profile: Option<Profile>,
    recovery_reconnect: Option<recovery_ui::RecoveryAction>,
    recovery_disconnect_pending: bool,
    selected_cell: Option<(usize, usize)>,
    selected_rows: Option<(usize, usize)>,
    grid_layouts: HashMap<usize, sqlshop::workspace::GridColumnLayout>,
    grid_scroll_row: Option<usize>,
    target: Option<QueryTarget>,
    file_path: Option<PathBuf>,
    saved_sql: String,
    saved_query: Option<SavedQuery>,
    saved_query_stale: bool,
    browse: TableBrowseOptions,
    has_more: bool,
    filters_open: bool,
    browse_draft: workspace_ui::TableBrowseDraft,
    staged_cells: Vec<CellUpdate>,
    insert_drafts: Vec<table_insert_ui::InsertDraft>,
    selected_insert: Option<Uuid>,
    insert_focus: Option<Uuid>,
    cell_anchor: Option<(usize, usize)>,
    find_open: bool,
    find_text: String,
    replace_text: String,
    find_case: bool,
    find_index: usize,
    select_range: Option<std::ops::Range<usize>>,
    completion_selected: usize,
    completion_dismissed: Option<(String, usize)>,
}

impl QueryTab {
    fn new(index: usize) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: format!("查询 {index}"),
            sql: "".into(),
            last_executed_sql: None,
            last_executed_document: None,
            last_executed_char_start: None,
            selection_char_start: None,
            selection: String::new(),
            cursor: 0,
            focus_editor: true,
            query_epoch: 0,
            query_error: None,
            query_diagnostic: None,
            query_elapsed_ms: None,
            output: None,
            query_started: false,
            source: String::new(),
            result_index: 0,
            table_target: None,
            table_kind: None,
            design: None,
            definition: None,
            advanced: None,
            backup: None,
            local_database: None,
            oracle: None,
            redis: None,
            jobs: None,
            compare: None,
            table_data: None,
            table_snapshot_stale: false,
            table_read_pending: false,
            table_write_notice: None,
            table_write_pending: false,
            recovery: None,
            recovery_epoch: 0,
            recovery_manager_epoch: None,
            recovery_profile: None,
            recovery_reconnect: None,
            recovery_disconnect_pending: false,
            selected_cell: None,
            selected_rows: None,
            grid_layouts: HashMap::new(),
            grid_scroll_row: None,
            target: None,
            file_path: None,
            saved_sql: "".into(),
            saved_query: None,
            saved_query_stale: false,
            browse: TableBrowseOptions::default(),
            has_more: false,
            filters_open: false,
            browse_draft: workspace_ui::TableBrowseDraft::default(),
            staged_cells: Vec::new(),
            insert_drafts: Vec::new(),
            selected_insert: None,
            insert_focus: None,
            cell_anchor: None,
            find_open: false,
            find_text: String::new(),
            replace_text: String::new(),
            find_case: false,
            find_index: 0,
            select_range: None,
            completion_selected: 0,
            completion_dismissed: None,
        }
    }
    fn sql_dirty(&self) -> bool {
        if self.file_path.is_none()
            && let Some(saved) = &self.saved_query
        {
            self.saved_query_stale || self.sql != saved.sql || self.target != saved.target
        } else {
            self.sql != self.saved_sql
        }
    }

    fn stage_cell(&mut self, change: CellUpdate) {
        if let Some(previous) = self.staged_cells.iter_mut().find(|previous| {
            previous.key_values == change.key_values && previous.column == change.column
        }) {
            previous.value = change.value;
        } else if change.value != change.original {
            self.staged_cells.push(change);
        }
        self.staged_cells
            .retain(|change| change.value != change.original);
    }

    fn table_write_saved(&mut self, notice: String) {
        self.table_snapshot_stale = true;
        self.table_write_notice = Some(notice);
    }
}

#[cfg(test)]
mod sql_risk_tests;

#[derive(Clone, Copy)]
enum SqlExecutionScope {
    SelectionOrAll,
    CurrentStatement,
    All,
}

struct PendingSqlExecution {
    tab: Uuid,
    target: QueryTarget,
    sql: String,
    session_epoch: u64,
    connection_name: String,
    profile: Profile,
    document_sql: String,
    document_char_start: Option<usize>,
}

struct ConnectionForm {
    kind: ConnectionKind,
    saved_kind: Option<ConnectionKind>,
    profile: Profile,
    redis_profile: RedisProfile,
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
    expanded: bool,
}

struct CsvImportForm {
    tab: Uuid,
    target: QueryTarget,
    table: Table,
    headers: Vec<String>,
    mapping: Vec<Option<String>>,
    rows: Vec<Vec<InsertValue>>,
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
    Group(Option<Uuid>),
    Connection(Uuid),
    Database(Uuid, String),
    Schema(Uuid, String, String),
    Table(Uuid, String, Table),
    TablePart(
        Uuid,
        String,
        Table,
        table_tree_ui::TablePart,
        Option<String>,
    ),
}

enum SchemaDialogAction {
    CreateDatabase {
        connection: Uuid,
    },
    DropDatabase {
        connection: Uuid,
        database: String,
    },
    DropTable {
        connection: Uuid,
        database: String,
        table: Table,
    },
}

struct SchemaDialog {
    action: SchemaDialogAction,
    name: String,
}

struct TableCreateForm {
    connection: Uuid,
    database: String,
    schema: String,
    engine: Engine,
    name: String,
    columns: Vec<CreateTableColumn>,
}

fn new_table_create_form(
    connection: Uuid,
    database: String,
    schema: String,
    engine: Engine,
) -> TableCreateForm {
    TableCreateForm {
        connection,
        database,
        schema,
        engine,
        name: String::new(),
        columns: vec![CreateTableColumn {
            name: "id".into(),
            data_type: match engine {
                Engine::MySql => "INT",
                Engine::Postgres => "INTEGER",
                Engine::SqlServer => "INT",
            }
            .into(),
            nullable: false,
            primary_key: true,
            auto_increment: true,
            ..Default::default()
        }],
    }
}

fn column_type_options(engine: Engine) -> &'static [&'static str] {
    sqlshop::db::column_type_options(engine)
}

// Tree focus is independent of the connected server and active query database.
#[derive(Default)]
struct TreeSelection {
    navigator: tree_navigation_ui::NavigationState,
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
                        accent(ui).gamma_multiply(if ui.visuals().dark_mode { 0.30 } else { 0.22 }),
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
    object_catalog: Option<sqlshop::object_catalog::ObjectCatalog>,
    catalog_loading: bool,
    catalog_error: Option<String>,
    column_cache: HashMap<(String, String), Vec<String>>,
    tree_metadata: table_tree_ui::TreeMetadata,
    completion_pending: Option<(QueryTarget, Table, u64)>,
    completion_generation: u64,
    completion_pause: bool,
    busy: bool,
    running_tab: Option<Uuid>,
    raw_session_used: bool,
    epoch: u64,
    status: String,
    error: bool,
}

impl ConnectionSession {
    fn clear_columns(&mut self) {
        self.column_cache.clear();
        self.tree_metadata.entries.clear();
        self.completion_pending = None;
        self.completion_pause = false;
        self.completion_generation = self.completion_generation.wrapping_add(1);
    }

    fn columns_for(&self, target: &QueryTarget) -> Vec<String> {
        if !self.target_matches(target) {
            return Vec::new();
        }
        let mut columns = Vec::new();
        for table in self.tables.iter().filter(|table| {
            target
                .schema
                .as_ref()
                .is_none_or(|schema| schema == &table.schema)
        }) {
            if let Some(names) = self
                .column_cache
                .get(&(table.schema.clone(), table.name.clone()))
            {
                for name in names {
                    if !columns.contains(name) {
                        columns.push(name.clone());
                    }
                }
            }
        }
        columns
    }

    /// Table and view names offered as completion candidates; the table list
    /// is already loaded for the object browser, so no extra fetch is needed.
    #[cfg(test)]
    fn tables_for(&self, target: &QueryTarget) -> Vec<String> {
        if !self.target_matches(target) {
            return Vec::new();
        }
        let mut names = Vec::new();
        for table in self.tables.iter().filter(|table| {
            target
                .schema
                .as_ref()
                .is_none_or(|schema| schema == &table.schema)
        }) {
            if !names
                .iter()
                .any(|name: &String| name.eq_ignore_ascii_case(&table.name))
            {
                names.push(table.name.clone());
            }
        }
        names
    }

    fn target_matches(&self, target: &QueryTarget) -> bool {
        self.connected.as_ref().map(|profile| profile.id) == Some(target.connection)
            && self.active_database == target.database
            && target.database.is_some()
    }

    fn new(ctx: &egui::Context) -> Self {
        let tree_metadata = table_tree_ui::TreeMetadata::new(ctx);
        let ctx = ctx.clone();
        Self {
            worker: Worker::start(move || ctx.request_repaint()),
            connected: None,
            databases: Vec::new(),
            active_database: None,
            tables: Vec::new(),
            object_catalog: None,
            catalog_loading: false,
            catalog_error: None,
            column_cache: HashMap::new(),
            tree_metadata,
            completion_pending: None,
            completion_generation: 0,
            completion_pause: false,
            busy: false,
            running_tab: None,
            raw_session_used: false,
            epoch: 0,
            status: "未连接".into(),
            error: false,
        }
    }
}

type DeleteRowsPending = (Uuid, TableTarget, Vec<String>, Vec<Vec<Option<String>>>);

#[derive(Clone, Copy)]
struct ViewLayout {
    navigation: bool,
    toolbar: bool,
    status: bool,
    focus: bool,
    tree_generation: u64,
    connected_only: bool,
    information_force: bool,
}
impl Default for ViewLayout {
    fn default() -> Self {
        Self {
            navigation: true,
            toolbar: true,
            status: true,
            focus: false,
            tree_generation: 0,
            connected_only: false,
            information_force: false,
        }
    }
}

pub struct App {
    advanced_worker: sqlshop::advanced_worker::AdvancedWorker,
    compare_worker: sqlshop::compare_worker::CompareWorker,
    backup_worker: sqlshop::backup_worker::BackupWorker,
    job_scheduler: Option<sqlshop::backup_worker::JobScheduler>,
    jobs_loaded: bool,
    last_job_tick: std::time::Instant,
    logo: egui::TextureHandle,
    store: Option<Store>,
    profiles: Vec<Profile>,
    redis_profiles: Vec<RedisProfile>,
    redis_config_error: Option<String>,
    redis_test_worker: Option<RedisWorker>,
    credentials: HashMap<Uuid, Credentials>,
    selected: Option<Uuid>,
    sessions: HashMap<Uuid, ConnectionSession>,
    query_sessions: HashMap<Uuid, query_session_ui::QuerySessionState>,
    browser_state: object_browser_ui::BrowserState,
    browser_scope: Option<object_browser_ui::BrowserScope>,
    browser_visible: bool,
    inspector_visible: bool,
    view: ViewLayout,
    context: egui::Context,
    test_worker: Worker,
    recovery_worker: sqlshop::recovery_worker::RecoveryWorker,
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
    schema_dialog: Option<SchemaDialog>,
    database_create_pending: Option<(Uuid, String)>,
    table_create: Option<TableCreateForm>,
    table_create_pending: Option<Uuid>,
    export_path: Option<String>,
    export_rows: Option<(Uuid, Vec<usize>)>,
    delete_rows: Option<DeleteRowsPending>,
    cell_editor: Option<CellEditor>,
    csv_import: Option<CsvImportForm>,
    theme: egui::ThemePreference,
    screenshot_path: Option<PathBuf>,
    frames: usize,
    workspace_store: Option<WorkspaceStore>,
    history: Vec<HistoryEntry>,
    table_grid_layouts: Vec<sqlshop::workspace::TableGridLayout>,
    history_open: bool,
    history_search: String,
    last_workspace: Option<Workspace>,
    last_autosave: std::time::Instant,
    pending_history: HashMap<Uuid, HistoryEntry>,
    confirm_tab_action: Option<TabAction>,
    confirm_rebind: Option<(Uuid, QueryTarget)>,
    confirm_exit: bool,
    allow_exit: bool,
    confirm_request: Option<(Uuid, Request, String)>,
    confirm_sql: Option<PendingSqlExecution>,
    catalog: Catalog,
    catalog_store: Option<CatalogStore>,
    library_open: bool,
    library_search: String,
    catalog_dialog: Option<CatalogDialog>,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        for session in self.sessions.values_mut() {
            session.completion_pause = false;
        }
        self.poll();
        if self.screenshot_path.is_none() {
            let incoming = if self.modal_open() || self.cell_editor.is_some() || self.allow_exit {
                crate::shell_open::poll_when(ctx, false)
            } else {
                crate::shell_open::poll(ctx)
            };
            match incoming {
                Ok(Some(paths)) => self.open_shell_paths(&paths),
                Ok(None) => {}
                Err(error) => self.fail(format!("读取文件打开请求失败：{error}")),
            }
        }
        apply_theme(ctx);
        egui::TopBottomPanel::top("title")
            .exact_height(window_chrome::TITLE_HEIGHT)
            .frame(egui::Frame::new().fill(window_chrome::title_fill(ctx)))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if cfg!(target_os = "windows") {
                    ui.spacing_mut().button_padding.y = 4.0;
                    ui.spacing_mut().interact_size.y = 24.0;
                }
                ui.horizontal(|ui| {
                    ui.set_height(window_chrome::TITLE_HEIGHT);
                    ui.add_space(window_chrome::leading_inset());
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
                            RichText::new(if let Some(name) = self.selected_connected_name() {
                                format!("● {name}")
                            } else {
                                "○ 未连接".into()
                            })
                            .color(
                                if self.selected_connected_name().is_some() {
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
        self.workspace_shortcuts(ctx);
        self.command_bar(ctx);
        if self.view.status && !self.view.focus {
            egui::TopBottomPanel::bottom("status")
                .frame(
                    egui::Frame::new()
                        .fill(chrome(ctx))
                        .inner_margin(egui::Margin::symmetric(18, 8)),
                )
                .show(ctx, |ui| {
                    let (status, error) = self.workspace_status();
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "{} 个连接 · {} 个操作中",
                            self.connection_activity_counts().0,
                            self.connection_activity_counts().1
                        ));
                        let width = (ui.available_width() * 0.35).min(420.0);
                        let color = if error {
                            Color32::from_rgb(240, 120, 120)
                        } else {
                            ui.visuals().weak_text_color()
                        };
                        ui.add_sized(
                            [width, 22.0],
                            egui::Label::new(RichText::new(&status).small().color(color))
                                .truncate(),
                        )
                        .on_hover_text(status);
                        ui.separator();
                        if !self.browser_visible {
                            if let Some(document) = &self.tabs[self.active_tab].definition {
                                ui.label(
                                    RichText::new(
                                        document
                                            .sql
                                            .as_ref()
                                            .map(|sql| {
                                                format!("定义 · {} 字符", sql.chars().count())
                                            })
                                            .unwrap_or_else(|| "尚未读取定义".into()),
                                    )
                                    .small()
                                    .weak(),
                                );
                            } else if !self.tabs[self.active_tab].management_document() {
                                self.executed_sql_footer(ui);
                            }
                        }
                    });
                });
        }
        let modal = self.modal_open();
        let redis_active = !self.browser_visible && self.tabs[self.active_tab].redis.is_some();
        if self.view.navigation && !self.view.focus {
            egui::SidePanel::left("sidebar")
                .default_width(240.0)
                .width_range(200.0..=340.0)
                .frame(
                    egui::Frame::new()
                        .fill(chrome(ctx))
                        .inner_margin(egui::Margin::symmetric(10, 6)),
                )
                .show(ctx, |ui| {
                    ui.add_enabled_ui(!modal, |ui| self.sidebar(ui));
                });
        }
        if self.inspector_visible
            && !redis_active
            && !self.view.focus
            && (self.view.information_force
                || (ctx.content_rect().width() >= 1100.0 && ctx.available_rect().width() >= 720.0))
        {
            egui::SidePanel::right("object-information")
                .default_width(240.0)
                .width_range(200.0..=290.0)
                .frame(
                    egui::Frame::new()
                        .fill(chrome(ctx))
                        .inner_margin(egui::Margin::symmetric(12, 8)),
                )
                .show(ctx, |ui| {
                    ui.add_enabled_ui(!modal, |ui| self.object_inspector(ui));
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(canvas(ctx)).inner_margin(0))
            .show(ctx, |ui| {
                ui.add_enabled_ui(!modal, |ui| self.workspace(ui));
            });
        self.connection_window(ctx);
        self.delete_window(ctx);
        self.schema_dialog_window(ctx);
        self.delete_rows_window(ctx);
        self.export_window(ctx);
        self.csv_import_window(ctx);
        self.cell_editor_window(ctx);
        self.workspace_windows(ctx);
        self.catalog_windows(ctx);
        window_chrome::paint_border(ctx);
        if self.screenshot_path.is_none() {
            if ctx.input(|i| i.viewport().close_requested())
                && !self.allow_exit
                && self.cell_editor.is_some()
            {
                self.finish_inline_cell(false, CellMovement::Stay);
            }
            if ctx.input(|i| i.viewport().close_requested())
                && !self.allow_exit
                && (self.any_busy()
                    || self.sessions.values().any(|s| s.raw_session_used)
                    || self
                        .query_sessions
                        .values()
                        .any(|s| s.connected && s.user_sql)
                    || self
                        .tabs
                        .iter()
                        .any(|tab| tab.document_dirty() || tab.has_table_drafts())
                    || self.csv_import.is_some()
                    || self.cell_editor.is_some()
                    || self.object_design_dirty()
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
    let footer_height = 42.0;
    let footer_top = (available.bottom() - footer_height).max(available.top());
    let content = egui::Rect::from_min_max(
        available.min,
        egui::pos2(available.right(), (footer_top - 6.0).max(available.top())),
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
        .on_hover_text("单击任意行内容选择整行；Shift + 单击连续选择多行；⌘/Ctrl + 单击选择单元格")
        .clicked()
    {
        let rows = tab
            .selected_rows
            .map(|(anchor, end)| (anchor.min(end)..=anchor.max(end)).collect::<Vec<_>>());
        let text = grid_columns_ui::selected_tsv(tab, set, rows.as_deref());
        ui.ctx().copy_text(text);
        if close_menu {
            ui.close();
        }
    }
    if ui.button("复制全部 (TSV)").clicked() {
        let rows: Vec<_> = (0..set.rows.len()).collect();
        ui.ctx()
            .copy_text(grid_columns_ui::selected_tsv(tab, set, Some(&rows)));
        if close_menu {
            ui.close();
        }
    }
}

const GRID_HEADER_HEIGHT: f32 = 52.0;

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

/// Open a centered dialog while retaining the user's subsequent dragged position.
fn dialog_window(title: impl Into<String>, ctx: &egui::Context) -> egui::Window<'static> {
    egui::Window::new(title.into())
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
}
