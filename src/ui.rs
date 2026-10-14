use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use sqlshop::{
    db::{
        ColumnFilter, ColumnSort, FilterOperator, StructureChange, TableBrowseOptions,
        TableStructure, structure_change_sql,
    },
    model::{
        CellUpdate, CreateTableColumn, Credentials, Engine, InsertValue, Profile, QueryOutput,
        ResultSet, SshAuth, Table, TableData, export_csv, parse_nullable_csv,
    },
    sql_editor::{
        byte_to_char_index, char_to_byte_index, completion_word, completions,
        current_statement_range, find_matches, highlight_sql, insert_completion, replace_all,
        replace_match,
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
mod icons;
mod local_database_ui;
mod management_ui;
mod object_browser_host;
mod object_browser_ui;
mod oracle_database_ui;
mod query_result_ui;
mod query_session_ui;
mod recovery_ui;
mod structure_ui;
mod tab_bar_ui;
mod table_insert_ui;
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
    grid_width: f32,
    grid_scroll_row: Option<usize>,
    target: Option<QueryTarget>,
    file_path: Option<PathBuf>,
    saved_sql: String,
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
            grid_width: 0.0,
            grid_scroll_row: None,
            target: None,
            file_path: None,
            saved_sql: "".into(),
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

fn sql_needs_confirmation(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut first = true;
    let mut word = "";
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i = sql[i..].find('\n').map_or(bytes.len(), |end| i + end);
            }
            b'#' => i = sql[i..].find('\n').map_or(bytes.len(), |end| i + end),
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let Some(end) = sql[i + 2..].find("*/") else {
                    return true;
                };
                i += end + 4;
            }
            b';' if first => i += 1,
            b';' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b' ' | b'\t' | b'\r' | b'\n' | b';' => i += 1,
                        b'-' if bytes.get(i + 1) == Some(&b'-') => {
                            i = sql[i..].find('\n').map_or(bytes.len(), |end| i + end);
                        }
                        b'#' => i = sql[i..].find('\n').map_or(bytes.len(), |end| i + end),
                        b'/' if bytes.get(i + 1) == Some(&b'*') => {
                            let Some(end) = sql[i + 2..].find("*/") else {
                                return true;
                            };
                            i += end + 4;
                        }
                        _ => return true,
                    }
                }
            }
            b'A'..=b'Z' | b'a'..=b'z' if first => {
                let start = i;
                while bytes.get(i).is_some_and(|b| b.is_ascii_alphabetic()) {
                    i += 1;
                }
                let first_word = &sql[start..i];
                if !first_word.eq_ignore_ascii_case("SELECT")
                    && !first_word.eq_ignore_ascii_case("SHOW")
                    && !first_word.eq_ignore_ascii_case("DESCRIBE")
                {
                    return true;
                }
                if bytes
                    .get(i)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
                {
                    return true;
                }
                word = first_word;
                first = false;
            }
            b'\'' | b'"' | b'`' | b'[' if !first => {
                let close = if bytes[i] == b'[' { b']' } else { bytes[i] };
                i += 1;
                let mut closed = false;
                while i < bytes.len() {
                    if bytes[i] == b'\\' && close != b']' {
                        i = (i + 2).min(bytes.len());
                    } else if bytes[i] == close {
                        i += 1;
                        if bytes.get(i) == Some(&close) {
                            i += 1;
                        } else {
                            closed = true;
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                if !closed {
                    return true;
                }
            }
            b'$' if !first => {
                let start = i;
                i += 1;
                while bytes
                    .get(i)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
                {
                    i += 1;
                }
                if bytes.get(i) == Some(&b'$') {
                    let delimiter = &sql[start..=i];
                    i += 1;
                    let Some(end) = sql[i..].find(delimiter) else {
                        return true;
                    };
                    i += end + delimiter.len();
                }
            }
            b'A'..=b'Z' | b'a'..=b'z' if !first => {
                let start = i;
                while bytes.get(i).is_some_and(|b| b.is_ascii_alphabetic()) {
                    i += 1;
                }
                let token = &sql[start..i];
                if word.eq_ignore_ascii_case("SELECT") && token.eq_ignore_ascii_case("INTO") {
                    return true;
                }
            }
            _ if first => return true,
            _ => i += 1,
        }
    }
    first
}

#[cfg(test)]
mod sql_risk_tests {
    use super::sql_needs_confirmation;

    #[test]
    fn comments_and_literals_do_not_trigger() {
        for sql in [
            "-- DELETE\nSELECT 'DROP; INSERT'",
            "/* UPDATE */ SELECT 'DELETE'",
            "# TRUNCATE\n SELECT \"DROP\"",
            "SELECT $$DELETE; DROP$$",
            "SELECT 'it''s UPDATE'",
            "SELECT 1; /* DROP */",
            "SELECT 1 /* DROP; */",
            "SELECT 'DROP;'",
        ] {
            assert!(!sql_needs_confirmation(sql), "{sql}");
        }
    }

    #[test]
    fn writes_ctes_batches_and_unknown_statements_confirm() {
        for sql in [
            "INSERT INTO t VALUES (1)",
            "UPDATE t SET x = 1",
            "DELETE FROM t",
            "CREATE TABLE t (x int)",
            "DROP TABLE t",
            "ALTER TABLE t ADD x int",
            "TRUNCATE t",
            "REPLACE INTO t VALUES (1)",
            "MERGE INTO t USING s ON 1=1",
            "GRANT SELECT ON t TO u",
            "REVOKE SELECT ON t FROM u",
            "WITH c AS (DELETE FROM t RETURNING *) SELECT * FROM c",
            "SELECT 1; DELETE FROM t",
            "SELECT * INTO backup FROM users",
            "SELECT 'unterminated",
            "/* unclosed",
        ] {
            assert!(sql_needs_confirmation(sql), "{sql}");
        }
    }

    #[test]
    fn simple_reads_do_not_confirm() {
        for sql in [
            "SELECT 1",
            "  SELECT name FROM users WHERE name = 'DELETE'",
            "SHOW TABLES",
            "DESCRIBE users",
            "SELECT 1; -- trailing comment",
        ] {
            assert!(!sql_needs_confirmation(sql), "{sql}");
        }
    }
}

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
    Connection(Uuid),
    Database(Uuid, String),
    Schema(Uuid, String, String),
    Table(Uuid, String, Table),
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
    history_open: bool,
    history_search: String,
    last_workspace: String,
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
            history_open: false,
            history_search: String::new(),
            last_workspace: String::new(),
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
                Ok("catalog"
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
                    | "structure-existing-preview")
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
        !self.tabs[self.active_tab].management_document()
            && self.tabs[self.active_tab].definition.is_none()
            && self.execution_target().is_some_and(|t| {
                self.query_target_ready(self.tabs[self.active_tab].id, &t)
                    && !self.tab_busy(self.active_tab)
            })
    }

    fn completion_columns(&self) -> Vec<String> {
        self.tabs[self.active_tab]
            .target
            .as_ref()
            .and_then(|target| {
                self.sessions
                    .get(&target.connection)
                    .map(|session| session.columns_for(target))
            })
            .unwrap_or_default()
    }

    fn completion_tables(&self) -> Vec<String> {
        self.tabs[self.active_tab]
            .target
            .as_ref()
            .and_then(|target| {
                self.sessions
                    .get(&target.connection)
                    .map(|session| session.tables_for(target))
            })
            .unwrap_or_default()
    }

    fn prefetch_completion_columns(&mut self) {
        if self.screenshot_path.is_some() {
            return;
        }
        let Some(target) = self.tabs[self.active_tab].target.clone() else {
            return;
        };
        let Some(session) = self.sessions.get_mut(&target.connection) else {
            return;
        };
        if session.busy
            || session.connected.as_ref().map(|p| p.id) != Some(target.connection)
            || session.active_database != target.database
            || target.database.is_none()
            || session.completion_pending.is_some()
            || session.completion_pause
        {
            return;
        }
        let Some(table) = session
            .tables
            .iter()
            .find(|table| {
                !session
                    .column_cache
                    .contains_key(&(table.schema.clone(), table.name.clone()))
            })
            .cloned()
        else {
            return;
        };
        let generation = session.completion_generation;
        if session
            .worker
            .send(Request::CompleteColumns {
                target: target.clone(),
                table: table.clone(),
                generation,
            })
            .is_ok()
        {
            session.completion_pending = Some((target, table, generation));
        }
    }

    fn tab_busy(&self, index: usize) -> bool {
        let tab = &self.tabs[index];
        if tab.oracle.as_ref().is_some_and(|doc| doc.busy) {
            return true;
        }
        if tab.advanced.as_ref().is_some_and(|doc| doc.busy)
            || tab
                .compare
                .as_ref()
                .is_some_and(|doc| doc.pending.is_some())
            || tab.backup.as_ref().is_some_and(|doc| doc.pending.is_some())
            || tab
                .local_database
                .as_ref()
                .is_some_and(|doc| doc.pending.is_some())
        {
            return true;
        }
        if self.tabs[index]
            .recovery
            .as_ref()
            .is_some_and(|doc| doc.busy)
        {
            return true;
        }
        if let Some(document) = &self.tabs[index].definition {
            return self
                .sessions
                .get(&document.connection)
                .is_some_and(|session| {
                    session.busy && session.running_tab == Some(self.tabs[index].id)
                });
        }
        if self.tabs[index].table_target.is_none() {
            return self
                .query_sessions
                .get(&self.tabs[index].id)
                .is_some_and(|s| s.busy);
        }
        self.tabs[index]
            .target
            .as_ref()
            .is_some_and(|t| self.session_busy(t.connection))
    }

    fn any_busy(&self) -> bool {
        self.test_busy
            || self.tabs.iter().any(|tab| {
                tab.oracle.as_ref().is_some_and(|doc| doc.busy)
                    || tab.advanced.as_ref().is_some_and(|doc| doc.busy)
                    || tab
                        .compare
                        .as_ref()
                        .is_some_and(|doc| doc.pending.is_some())
                    || tab.backup.as_ref().is_some_and(|doc| doc.pending.is_some())
                    || tab
                        .local_database
                        .as_ref()
                        .is_some_and(|doc| doc.pending.is_some())
            })
            || self.job_scheduler.as_ref().is_some_and(|scheduler| {
                scheduler
                    .store
                    .records()
                    .iter()
                    .any(|record| record.state == sqlshop::jobs::JobState::Running)
            })
            || self
                .tabs
                .iter()
                .any(|tab| tab.recovery.as_ref().is_some_and(|doc| doc.busy))
            || self.sessions.values().any(|s| s.busy)
            || self.query_sessions.values().any(|s| s.busy)
    }

    fn request(&mut self, request: Request, status: &str) {
        if self.screenshot_path.is_some() {
            return;
        }
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
            Request::ObjectDefinition { target, .. }
            | Request::ReconnectTableManagement { target, .. }
            | Request::RunQuery { target, .. }
            | Request::ReadPage { target, .. }
            | Request::ReadLastPage { target, .. }
            | Request::DescribeTable { target, .. }
            | Request::ChangeStructure { target, .. }
            | Request::SaveCell { target, .. }
            | Request::DeleteRows { target, .. }
            | Request::InsertDefaultRow { target, .. }
            | Request::InsertRows { target, .. }
            | Request::SaveTableDraft { target, .. }
            | Request::SaveCells { target, .. }
            | Request::ExportTable { target, .. } => Some(target.connection),
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
        if self.screenshot_path.is_some() {
            return;
        }
        if self.screenshot_path.is_some() {
            return;
        }
        if self.session_busy(id) {
            return;
        }
        if matches!(&request, Request::ChangeStructure { .. })
            && self.tabs.iter().any(|tab| {
                tab.has_table_drafts()
                    && tab
                        .target
                        .as_ref()
                        .is_some_and(|target| target.connection == id)
            })
        {
            self.fail("此连接有暂存修改或新增行；请先提交或撤销草稿，再修改表结构".into());
            return;
        }
        let table_write_tab = match &request {
            Request::SaveCell { tab, .. }
            | Request::SaveCells { tab, .. }
            | Request::DeleteRows { tab, .. }
            | Request::InsertDefaultRow { tab, .. }
            | Request::InsertRows { tab, .. } => Some(*tab),
            Request::SaveTableDraft { tab, .. } => Some(*tab),
            _ => None,
        };
        if table_write_tab.is_some_and(|id| {
            self.tabs
                .iter()
                .find(|tab| tab.id == id)
                .is_none_or(|tab| tab.table_snapshot_stale || self.cell_editor.is_some())
        }) {
            self.fail("表数据已过期或单元格编辑窗尚未关闭；请先刷新或结束编辑再写入".into());
            return;
        }
        if self.tabs.iter().any(|tab| {
            tab.has_table_drafts() && tab.target.as_ref().is_some_and(|t| t.connection == id)
        }) && matches!(
            &request,
            Request::Connect(..) | Request::Disconnect | Request::SelectDatabase(..)
        ) {
            self.fail("此连接有暂存修改；请先提交或丢弃后再切库/断开".into());
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
        if matches!(&request, Request::Connect(..) | Request::Disconnect) {
            if self.query_connection_busy(id) {
                self.fail("此连接的查询正在执行，请等待执行结束后再重连或断开".into());
                return;
            }
            if !confirmed && self.query_connection_used(id) {
                self.confirm_request = Some((id, request, status.into()));
                return;
            }
            self.close_query_sessions(id);
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
        if matches!(
            &request,
            Request::Connect(..) | Request::Disconnect | Request::SelectDatabase(..)
        ) {
            session.clear_columns();
            session.object_catalog = None;
            session.catalog_loading = false;
            session.catalog_error = None;
        }
        let tab = match &request {
            Request::ObjectDefinition { tab, .. }
            | Request::ReconnectTableManagement { tab, .. }
            | Request::RunQuery { tab, .. }
            | Request::ReadPage { tab, .. }
            | Request::ReadLastPage { tab, .. }
            | Request::ReadTable { tab, .. }
            | Request::DescribeTable { tab, .. }
            | Request::ChangeStructure { tab, .. }
            | Request::SaveCell { tab, .. }
            | Request::DeleteRows { tab, .. }
            | Request::InsertDefaultRow { tab, .. }
            | Request::InsertRows { tab, .. }
            | Request::SaveCells { tab, .. }
            | Request::SaveTableDraft { tab, .. }
            | Request::ExportTable { tab, .. } => Some(*tab),
            _ => None,
        };
        if matches!(&request, Request::ObjectCatalog { .. }) {
            session.catalog_loading = true;
            session.catalog_error = None;
        }
        let raw = matches!(&request, Request::RunQuery { .. } | Request::Query(..));
        let recovery_reconnect_request =
            matches!(&request, Request::ReconnectTableManagement { .. });
        let table_read = matches!(
            &request,
            Request::ReadPage { .. } | Request::ReadLastPage { .. } | Request::ReadTable { .. }
        );
        let connection_reset = matches!(&request, Request::Connect(..) | Request::Disconnect);
        match session.worker.send(request) {
            Ok(()) => {
                if connection_reset {
                    session.connected = None;
                    session.active_database = None;
                    for tab in &mut self.tabs {
                        if tab
                            .table_target
                            .as_ref()
                            .is_some_and(|target| target.connection == id)
                        {
                            tab.table_snapshot_stale = true;
                        }
                    }
                }
                if table_read && let Some(tab) = self.tabs.iter_mut().find(|t| Some(t.id) == tab) {
                    tab.table_read_pending = true;
                    tab.table_snapshot_stale = true;
                }
                session.busy = true;
                if let Some(source) =
                    table_write_tab.and_then(|id| self.tabs.iter_mut().find(|tab| tab.id == id))
                {
                    source.table_write_pending = true;
                    if source.recovery_profile.is_none() {
                        source.recovery_profile = session.connected.clone();
                    }
                }
                session.epoch = session.epoch.wrapping_add(1);
                if recovery_reconnect_request
                    && let Some(source) =
                        tab.and_then(|id| self.tabs.iter_mut().find(|tab| tab.id == id))
                {
                    source.recovery_manager_epoch = Some(session.epoch);
                }
                session.running_tab = tab;
                session.raw_session_used |= raw;
                session.error = false;
                session.status = status.into();
                self.error = false;
                self.status = status.into();
            }
            Err(error) => {
                session.catalog_loading = false;
                if let Some(source) = tab
                    .and_then(|id| self.tabs.iter_mut().find(|tab| tab.id == id))
                    .and_then(|tab| tab.definition.as_mut())
                {
                    source.error = Some(error.to_string());
                }
                self.fail(error.to_string());
            }
        }
    }

    fn poll(&mut self) {
        self.poll_management();
        self.poll_query_sessions();
        self.poll_table_recovery();
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
            if let Response::CompleteColumns {
                target,
                table,
                generation,
                columns,
            } = response
            {
                if let Some(session) = self.sessions.get_mut(&connection)
                    && session.completion_generation == generation
                    && session.completion_pending.as_ref()
                        == Some(&(target.clone(), table.clone(), generation))
                {
                    session.completion_pending = None;
                    session.completion_pause = true;
                    if session.connected.as_ref().map(|profile| profile.id)
                        == Some(target.connection)
                        && session.active_database == target.database
                        && target.database.is_some()
                        && session
                            .tables
                            .iter()
                            .any(|known| known.schema == table.schema && known.name == table.name)
                    {
                        session
                            .column_cache
                            .insert((table.schema, table.name), columns);
                    }
                }
            } else {
                self.handle_response(connection, response);
            }
        }
    }

    fn invalidate_table_snapshots(&mut self, connection: Uuid) {
        for tab in &mut self.tabs {
            if let Some(document) = &mut tab.definition
                && document.connection == connection
            {
                document.stale = true;
            }
            if tab
                .table_target
                .as_ref()
                .is_some_and(|target| target.connection == connection)
            {
                tab.table_snapshot_stale = true;
                if tab.has_table_drafts() {
                    tab.recovery
                        .get_or_insert_with(recovery_ui::RecoveryDocument::default)
                        .pending = true;
                }
                if let Some(doc) = &mut tab.design {
                    doc.stale = true;
                    doc.status = "连接上下文已变化，请重新读取实际结构".into();
                }
            }
        }
    }

    fn handle_response(&mut self, connection: Uuid, response: Response) {
        let Some(mut session) = self.sessions.remove(&connection) else {
            return;
        };
        let response = match response {
            Response::TableLastPage {
                tab: id,
                page,
                offset,
            } => {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id)
                    && tab.table_target.as_ref().is_some_and(|target| {
                        target.connection == connection
                            && session.active_database.as_ref() == Some(&target.database)
                    })
                {
                    tab.browse.offset = offset;
                }
                Response::TablePage { tab: id, page }
            }
            response => response,
        };
        if let Response::DatabaseContext(database) = &response {
            if session.active_database != *database {
                self.invalidate_table_snapshots(connection);
                session.active_database = database.clone();
                session.tables.clear();
                session.object_catalog = None;
                session.catalog_loading = false;
                session.clear_columns();
            }
            self.sessions.insert(connection, session);
            return;
        }
        session.busy = false;
        self.error = false;
        let mut follow_up = None;
        let mut recovered_management = None;
        match response {
            Response::DatabaseContext(_)
            | Response::CompleteColumns { .. }
            | Response::TableLastPage { .. } => unreachable!(),
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
                if changed {
                    self.invalidate_table_snapshots(connection);
                }
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
                session.clear_columns();
                if changed {
                    session.tables.clear();
                    session.object_catalog = None;
                    session.catalog_loading = false;
                    if session.active_database.is_some() {
                        follow_up = Some((Request::Tables, "数据库已切换 · 正在读取表目录…"));
                    }
                }
            }
            Response::TablePage { tab: id, mut page } => {
                if let Some(tab) = self.tabs.iter().find(|tab| tab.id == id)
                    && let Some(table) = &tab.table_target
                    && table.connection == connection
                    && session.active_database.as_ref() == Some(&table.database)
                {
                    session.column_cache.insert(
                        (table.schema.clone(), table.name.clone()),
                        page.data
                            .columns
                            .iter()
                            .map(|column| column.name.clone())
                            .collect(),
                    );
                }
                self.status = format!(
                    "已读取表分页 · {} 行",
                    page.data
                        .output
                        .sets
                        .first()
                        .map_or(0, |set| set.rows.len())
                );
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                    if !tab.has_table_drafts() {
                        tab.recovery_profile = session.connected.clone();
                        tab.recovery = None;
                    }
                    tab.table_snapshot_stale = false;
                    tab.table_read_pending = false;
                    if let Some(notice) = tab.table_write_notice.take() {
                        self.status = if notice.contains("新增") {
                            format!("{notice} · 已刷新当前页；新记录可能受筛选、排序或分页影响")
                        } else {
                            format!("{notice} · 已刷新当前页")
                        };
                    }
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
                    tab.grid_scroll_row = None;
                }
            }
            Response::TableStructure { tab, structure } => {
                if let Some(doc) = self
                    .tabs
                    .iter_mut()
                    .find(|source| source.id == tab)
                    .and_then(|source| source.design.as_mut())
                {
                    doc.received(structure, false);
                }
                self.status = "表结构已读取".into();
            }
            Response::StructureChanged { tab, structure } => {
                let identity = self
                    .tabs
                    .iter()
                    .find(|source| source.id == tab)
                    .and_then(|source| source.table_target.clone());
                for source in &mut self.tabs {
                    if source.id == tab {
                        if let Some(doc) = &mut source.design {
                            doc.received(structure.clone(), true);
                        }
                    } else if identity.is_some() && source.table_target == identity {
                        source.table_write_saved("表结构已修改成功，请刷新数据".into());
                    }
                }
                session.clear_columns();
                self.status = "表结构已修改 · 请刷新相关表数据标签".into();
            }
            Response::OperationError {
                tab,
                message,
                disconnected,
            } => {
                if let Some(source) = self.tabs.iter_mut().find(|source| source.id == tab) {
                    if source.table_write_pending
                        && (disconnected || message.contains("提交结果未知"))
                        && source.has_table_drafts()
                    {
                        source.table_snapshot_stale = true;
                        let recovery = source
                            .recovery
                            .get_or_insert_with(recovery_ui::RecoveryDocument::default);
                        recovery.pending = true;
                        recovery.error = Some(format!(
                            "提交结果未知，草稿已保留；请先核实，勿直接重复提交。{message}"
                        ));
                    }
                    if source.recovery_reconnect.is_some() {
                        source.recovery_reconnect = None;
                        source.recovery_disconnect_pending = false;
                        if let Some(recovery) = &mut source.recovery {
                            recovery.busy = false;
                            recovery.error = Some(message.clone());
                        }
                    }
                }
                self.finish_history(tab, false);
                if let Some(document) = self
                    .tabs
                    .iter_mut()
                    .find(|document| document.id == tab)
                    .and_then(|tab| tab.definition.as_mut())
                {
                    document.error = Some(message.clone());
                }
                if let Some(doc) = self
                    .tabs
                    .iter_mut()
                    .find(|source| source.id == tab)
                    .and_then(|source| source.design.as_mut())
                {
                    doc.failed(&message);
                }
                let message = if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab)
                    && tab.table_read_pending
                {
                    tab.table_read_pending = false;
                    tab.table_snapshot_stale = true;
                    match &tab.table_write_notice {
                        Some(notice) => {
                            format!("{notice}；刷新失败，显示的是过期快照，请重新刷新。{message}")
                        }
                        None => format!(
                            "读取表数据失败，保留上次快照；数据已过期，请重新刷新。{message}"
                        ),
                    }
                } else {
                    message
                };
                if disconnected {
                    self.invalidate_table_snapshots(connection);
                    session.connected = None;
                    session.active_database = None;
                    session.databases.clear();
                    session.tables.clear();
                    session.object_catalog = None;
                    session.catalog_loading = false;
                    session.raw_session_used = false;
                    session.clear_columns();
                }
                self.fail(message);
            }
            Response::Tested => self.status = "连接测试成功".into(),
            Response::TableManagementReconnected {
                tab,
                epoch,
                target,
                profile,
            } => {
                self.invalidate_table_snapshots(connection);
                session.connected = Some(profile.clone());
                session.active_database = target.database.clone();
                session.raw_session_used = false;
                session.clear_columns();
                session.tables.clear();
                session.object_catalog = None;
                session.catalog_loading = false;
                recovered_management = Some((tab, epoch, target, profile));
                self.status = "表管理连接已重建；正在处理已选择的本地草稿，没有重放 SQL".into();
            }
            Response::Connected(id) => {
                self.invalidate_table_snapshots(connection);
                session.clear_columns();
                session.raw_session_used = false;
                session.connected = self.profiles.iter().find(|p| p.id == id).cloned();
                session.databases.clear();
                session.active_database = None;
                session.tables.clear();
                session.object_catalog = None;
                session.catalog_loading = false;
                self.expand_connection = Some(id);
                follow_up = Some((Request::Databases, "已连接服务器 · 正在读取数据库列表…"));
            }
            Response::Disconnected => {
                self.invalidate_table_snapshots(connection);
                session.clear_columns();
                session.raw_session_used = false;
                session.connected = None;
                session.databases.clear();
                session.active_database = None;
                session.tables.clear();
                session.object_catalog = None;
                session.catalog_loading = false;
                self.status = "已断开连接".into();
                if let Some(id) = session.running_tab
                    && self
                        .tabs
                        .iter()
                        .find(|source| source.id == id)
                        .is_some_and(|source| {
                            source.recovery_disconnect_pending
                                && source.recovery_reconnect.is_some()
                        })
                {
                    match self.recovery_reconnect_request(id) {
                        Ok(request) => {
                            if let Some(source) =
                                self.tabs.iter_mut().find(|source| source.id == id)
                            {
                                source.recovery_disconnect_pending = false;
                            }
                            follow_up = Some((
                                request,
                                "原管理会话已明确断开 · 正在重连原数据库，草稿保留…",
                            ));
                        }
                        Err(message) => {
                            self.table_recovery_failed(id, &message);
                            self.fail(message);
                        }
                    }
                }
            }
            Response::Databases(databases) => {
                self.status = format!(
                    "已连接服务器 · {} 个数据库 · 点击数据库选择",
                    databases.len()
                );
                // Discovery must not silently alter the worker's database context.
                session.databases = databases;
                if let Some((pending_connection, database)) = self.database_create_pending.as_ref()
                    && *pending_connection == connection
                {
                    let database = database.clone();
                    let exists = session.databases.iter().any(|name| name == &database);
                    self.database_create_pending = None;
                    if exists {
                        self.expand_database = Some((connection, database.clone()));
                        follow_up = Some((
                            Request::SelectDatabase(database),
                            "数据库已创建 · 正在切换到新数据库…",
                        ));
                    }
                }
            }
            Response::DatabaseSelected(database) => {
                self.invalidate_table_snapshots(connection);
                session.clear_columns();
                session.active_database = Some(database.clone());
                session.tables.clear();
                session.object_catalog = None;
                session.catalog_loading = false;
                self.expand_database = Some((connection, database));
                follow_up = Some((
                    Request::ObjectCatalog {
                        database: session.active_database.clone().unwrap(),
                    },
                    "正在读取对象目录与属性…",
                ));
            }
            Response::Tables(tables) => {
                session.clear_columns();
                self.status = format!("已连接 · {} 个表 / 视图", tables.len());
                session.tables = tables;
                if let Some(database) = session.active_database.clone() {
                    follow_up = Some((
                        Request::ObjectCatalog { database },
                        "正在读取对象目录与属性…",
                    ));
                }
                if self.table_create_pending == Some(connection) {
                    self.table_create_pending = None;
                    self.table_create = None;
                    self.status = "数据表已创建 · 表目录已刷新".into();
                }
            }
            Response::ObjectCatalog { database, catalog } => {
                session.catalog_loading = false;
                if session.active_database.as_ref() == Some(&database) {
                    session.catalog_error = None;
                    session.clear_columns();
                    session.tables = catalog
                        .objects
                        .iter()
                        .filter_map(|object| object.as_table())
                        .collect();
                    self.status = format!("已读取 {} 个对象", catalog.objects.len());
                    if let Some(object_browser_ui::BrowserItem::CatalogObject {
                        connection: selected_connection,
                        database: selected_database,
                        object,
                    }) = &mut self.browser_state.selected
                        && *selected_connection == connection
                        && *selected_database == database
                        && let Some(updated) = catalog
                            .objects
                            .iter()
                            .find(|candidate| object_browser_host::same_object(candidate, object))
                    {
                        *object = updated.clone();
                    }
                    session.object_catalog = Some(catalog);
                }
            }
            Response::ObjectDefinition {
                tab,
                target,
                object,
                definition,
            } => {
                if let Some(document) = self
                    .tabs
                    .iter_mut()
                    .find(|document| document.id == tab)
                    .and_then(|tab| tab.definition.as_mut())
                    && document.connection == connection
                    && document.database == target.database.unwrap_or_default()
                    && object_browser_host::same_object(&document.object, &object)
                {
                    document.sql = Some(definition);
                    document.error = None;
                    document.stale = false;
                }
                self.status = "对象定义已读取".into();
            }
            Response::TableData { tab, mut data } => {
                self.status = format!(
                    "已读取表数据 · {} 行",
                    data.output.sets.first().map_or(0, |set| set.rows.len())
                );
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab) {
                    if !tab.has_table_drafts() {
                        tab.recovery_profile = session.connected.clone();
                        tab.recovery = None;
                    }
                    tab.table_snapshot_stale = false;
                    tab.table_read_pending = false;
                    tab.table_write_notice = None;
                    tab.output = Some(std::mem::take(&mut data.output));
                    tab.table_data = Some(data);
                    tab.result_index = 0;
                }
            }
            Response::CellSaved { tab } => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab) {
                    tab.table_write_saved("单元格已保存成功".into());
                }
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
            Response::RowsDeleted { tab, count } => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab) {
                    tab.table_write_saved(format!("已成功删除 {count} 行"));
                }
                self.status = format!("已删除 {count} 行 · 正在刷新表数据…");
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
                        "正在刷新删除后的表数据…",
                    ));
                }
            }
            Response::RowsInserted { tab, count } => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab) {
                    tab.table_write_saved(format!("已成功新增 {count} 行"));
                }
                self.csv_import = None;
                self.status = format!("已新增 {count} 行 · 正在刷新表数据…");
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
                        "正在刷新新增后的表数据…",
                    ));
                }
            }
            Response::CellsSaved { tab, count } => {
                self.status = format!("已批量保存 {count} 格 · 正在刷新表数据…");
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab) {
                    tab.staged_cells.clear();
                    tab.table_write_saved(format!("已成功提交 {count} 格修改"));
                    if let (Some(target), Some(table)) =
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
                            "正在刷新批量修改后的表数据…",
                        ));
                    }
                }
            }
            Response::TableDraftSaved {
                tab: id,
                updated_cells,
                inserted_rows,
            } => {
                let notice = format!("已成功修改 {updated_cells} 格、新增 {inserted_rows} 行");
                self.status = format!("{notice} · 正在刷新，新记录可能受筛选或分页影响");
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.clear_table_drafts();
                    tab.table_write_saved(notice);
                    if let (Some(target), Some(table)) =
                        (tab.target.clone(), tab.table_target.as_ref())
                    {
                        follow_up = Some((
                            Request::ReadPage {
                                tab: id,
                                target,
                                table: Table {
                                    schema: table.schema.clone(),
                                    name: table.name.clone(),
                                    kind: "BASE TABLE".into(),
                                },
                                options: tab.browse.clone(),
                            },
                            "新增和修改已提交 · 正在刷新当前页（新记录可能受筛选或分页影响）…",
                        ));
                    }
                }
            }
            Response::TableExported { count, path, .. } => {
                self.status = format!("已完整导出 {count} 行：{}", path.display());
            }
            Response::RowInserted { tab } => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab) {
                    tab.table_write_saved("已成功新增一行".into());
                }
                self.status = "已新增一行 · 正在刷新表数据…".into();
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
                        "正在刷新新增后的表数据…",
                    ));
                }
            }
            Response::Query { output, database } => {
                let changed = session.active_database != database;
                if changed {
                    self.invalidate_table_snapshots(connection);
                }
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
                session.clear_columns();
                if changed {
                    session.tables.clear();
                    session.object_catalog = None;
                    session.catalog_loading = false;
                }
            }
            Response::Error {
                message,
                disconnected,
            } => {
                if let Some(source) = session
                    .running_tab
                    .and_then(|id| self.tabs.iter_mut().find(|tab| tab.id == id))
                {
                    if source.table_write_pending && disconnected && source.has_table_drafts() {
                        source.table_snapshot_stale = true;
                        let recovery = source
                            .recovery
                            .get_or_insert_with(recovery_ui::RecoveryDocument::default);
                        recovery.pending = true;
                        recovery.error = Some(format!(
                            "写入没有可靠完成回执，提交结果未知；请核实后再处理草稿。{message}"
                        ));
                    }
                    if source.recovery_reconnect.is_some() {
                        source.recovery_reconnect = None;
                        source.recovery_disconnect_pending = false;
                        if let Some(recovery) = &mut source.recovery {
                            recovery.busy = false;
                            recovery.error = Some(message.clone());
                        }
                    }
                }
                if session.catalog_loading {
                    session.catalog_loading = false;
                    session.catalog_error = Some(message.clone());
                }
                if self
                    .database_create_pending
                    .as_ref()
                    .is_some_and(|(pending_connection, _)| *pending_connection == connection)
                {
                    self.database_create_pending = None;
                }
                if self.table_create_pending == Some(connection) {
                    self.table_create_pending = None;
                }
                if disconnected {
                    self.invalidate_table_snapshots(connection);
                    session.connected = None;
                    session.databases.clear();
                    session.active_database = None;
                    session.tables.clear();
                    session.object_catalog = None;
                    session.catalog_loading = false;
                    session.raw_session_used = false;
                    session.clear_columns();
                }
                self.fail(if disconnected {
                    format!("{message}\n连接已关闭；重新连接后可继续操作。")
                } else {
                    message
                });
            }
        }
        if let Some(source) = session
            .running_tab
            .and_then(|id| self.tabs.iter_mut().find(|tab| tab.id == id))
        {
            source.table_write_pending = false;
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
        if let Some((tab, epoch, target, profile)) = recovered_management {
            self.finish_recovery_reconnect(tab, epoch, &target, &profile);
        }
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
        if profile.engine == Engine::SqlServer
            && profile.sqlserver_auth == sqlshop::model::SqlServerAuth::Integrated
            && !profile.ssh.enabled
        {
            return Ok(Some(Credentials::default()));
        }
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
        self.load_table_page(index, false);
    }

    fn load_last_table_page(&mut self, index: usize) {
        self.load_table_page(index, true);
    }

    fn load_table_page(&mut self, index: usize, last_page: bool) {
        if self.tabs[index].design.is_some() {
            self.load_structure(index);
            return;
        }
        if self.tab_busy(index) || self.cell_editor.is_some() {
            return;
        }
        if self.tabs[index].has_table_drafts() {
            self.fail("存在暂存修改；请先提交或丢弃后再刷新、翻页或筛选".into());
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
        tab.table_snapshot_stale = true;
        tab.selected_cell = None;
        tab.selected_rows = None;
        tab.source = format!("{} / {}", profile.name, target.database);
        tab.cell_anchor = None;
        tab.grid_scroll_row = None;
        let query_target = QueryTarget {
            schema: None,
            connection: target.connection,
            database: Some(target.database),
        };
        let request = if last_page {
            Request::ReadLastPage {
                tab: tab.id,
                target: query_target,
                table,
                options: tab.browse.clone(),
            }
        } else {
            Request::ReadPage {
                tab: tab.id,
                target: query_target,
                table,
                options: tab.browse.clone(),
            }
        };
        self.request(
            request,
            if last_page {
                "正在定位并读取末页…"
            } else {
                "正在读取表数据…"
            },
        );
    }

    fn execute(&mut self) {
        self.execute_scope(SqlExecutionScope::SelectionOrAll);
    }

    fn execute_scope(&mut self, scope: SqlExecutionScope) {
        if self.modal_open()
            || self.table_create.is_some()
            || self.tabs[self.active_tab].table_target.is_some()
            || self.tabs[self.active_tab].definition.is_some()
        {
            return;
        }
        let Some(current) = self.execution_target() else {
            self.fail("请先连接服务器并选择执行目标".into());
            return;
        };
        if !self.query_target_ready(self.tabs[self.active_tab].id, &current) {
            self.fail("查询目标未连接或连接配置已变化；请连接服务器或重新选择标签目标。".into());
            return;
        }
        if self.tab_busy(self.active_tab) {
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
        let session_epoch = self.tabs[self.active_tab].query_epoch;
        let profile = self
            .profiles
            .iter()
            .find(|p| p.id == current.connection)
            .unwrap()
            .clone();
        let connection_name = profile.name.clone();
        let engine = profile.engine;
        let tab = &self.tabs[self.active_tab];
        let sql = match scope {
            SqlExecutionScope::SelectionOrAll if !tab.selection.trim().is_empty() => {
                tab.selection.clone()
            }
            SqlExecutionScope::SelectionOrAll | SqlExecutionScope::All => tab.sql.clone(),
            SqlExecutionScope::CurrentStatement => {
                match current_statement_range(&tab.sql, tab.cursor, engine) {
                    Ok(Some(range)) => tab.sql[range].to_owned(),
                    Ok(None) => {
                        self.fail("光标所在位置没有可执行的 SQL 语句".into());
                        return;
                    }
                    Err(message) => {
                        self.fail(message);
                        return;
                    }
                }
            }
        };
        if sql.trim().is_empty() {
            self.fail("SQL 不能为空".into());
            return;
        }
        let document_char_start = match scope {
            SqlExecutionScope::SelectionOrAll if !tab.selection.trim().is_empty() => {
                tab.selection_char_start
            }
            SqlExecutionScope::CurrentStatement => {
                current_statement_range(&tab.sql, tab.cursor, engine)
                    .ok()
                    .flatten()
                    .map(|range| byte_to_char_index(&tab.sql, range.start))
            }
            _ => Some(0),
        };
        let pending = PendingSqlExecution {
            tab: tab.id,
            target: current.clone(),
            sql,
            session_epoch,
            connection_name,
            profile,
            document_sql: tab.sql.clone(),
            document_char_start,
        };
        if sql_needs_confirmation(&pending.sql) {
            self.confirm_sql = Some(pending);
        } else {
            self.send_sql(pending);
        }
    }

    fn send_sql(&mut self, pending: PendingSqlExecution) {
        self.send_private_query(pending);
    }

    fn workspace_shortcuts(&mut self, ctx: &egui::Context) {
        if self.modal_open() || egui::Popup::is_any_open(ctx) {
            return;
        }
        let (navigation, information, previous, next) = ctx.input_mut(|input| {
            let shifted = egui::Modifiers {
                shift: true,
                ..egui::Modifiers::COMMAND
            };
            (
                input.consume_key(egui::Modifiers::COMMAND, egui::Key::Num1),
                input.consume_key(egui::Modifiers::COMMAND, egui::Key::Num2),
                input.consume_key(shifted, egui::Key::OpenBracket)
                    || input.consume_key(
                        egui::Modifiers {
                            ctrl: true,
                            shift: true,
                            ..Default::default()
                        },
                        egui::Key::Tab,
                    ),
                input.consume_key(shifted, egui::Key::CloseBracket)
                    || input.consume_key(egui::Modifiers::CTRL, egui::Key::Tab),
            )
        });
        if navigation {
            self.view.navigation = self.view.focus || !self.view.navigation;
            self.view.focus = false;
        }
        if information {
            let visible = self.inspector_visible
                && !self.view.focus
                && (self.view.information_force || ctx.content_rect().width() >= 1100.0);
            self.inspector_visible = !visible;
            self.view.information_force = self.inspector_visible;
            self.view.focus = false;
        }
        if previous || next {
            if self.cell_editor.is_some() {
                self.finish_inline_cell(false, CellMovement::Stay);
                if self.cell_editor.is_some() {
                    return;
                }
            }
            let position = if self.browser_visible {
                0
            } else {
                self.active_tab + 1
            };
            let count = self.tabs.len() + 1;
            let position = if previous {
                (position + count - 1) % count
            } else {
                (position + 1) % count
            };
            if position == 0 {
                self.show_selected_objects();
            } else {
                self.change_tab(TabAction::Select(position - 1));
            }
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.visuals_mut().indent_has_left_vline = false;
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        ui.add_space(4.0);
        let search_width =
            (ui.available_width() - 56.0 - 2.0 * ui.spacing().item_spacing.x).max(60.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("搜索数据库、数据表…")
                    .desired_width(search_width)
                    .margin(egui::vec2(4.0, 2.0)),
            );
            let (filter_rect, filter_response) =
                ui.allocate_exact_size(egui::vec2(24.0, 20.0), egui::Sense::click());
            let color = if self.view.connected_only {
                accent(ui)
            } else {
                ui.visuals().weak_text_color()
            };
            if self.view.connected_only {
                ui.painter()
                    .rect_filled(filter_rect, 3.0, accent(ui).gamma_multiply(0.14));
            }
            let center = filter_rect.center();
            ui.painter()
                .circle_stroke(center, 5.0, egui::Stroke::new(1.3, color));
            ui.painter().line_segment(
                [
                    center + egui::vec2(0.0, -8.0),
                    center + egui::vec2(0.0, -2.0),
                ],
                egui::Stroke::new(1.3, color),
            );
            if filter_response
                .on_hover_text("仅显示已连接的服务器")
                .clicked()
            {
                self.view.connected_only = !self.view.connected_only;
            }
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(24.0, 20.0), egui::Sense::click());
            let visuals = ui.style().interact(&response);
            ui.painter().rect(
                rect,
                3.0,
                visuals.weak_bg_fill,
                visuals.bg_stroke,
                egui::StrokeKind::Inside,
            );
            let color = visuals.fg_stroke;
            for y in [8.0, 12.0] {
                let center = rect.left_top() + egui::vec2(12.0, y);
                ui.painter()
                    .line_segment([center + egui::vec2(-4.0, 2.0), center], color);
                ui.painter()
                    .line_segment([center, center + egui::vec2(4.0, 2.0)], color);
            }
            if response.on_hover_text("全部折叠").clicked() {
                self.filter.clear();
                self.expand_connection = None;
                self.expand_database = None;
                self.view.tree_generation = self.view.tree_generation.wrapping_add(1);
            }
        });
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        let tree_before = self.tree_selection.selected.clone();
        let mut clicked_node = None;
        let filter = self.filter.to_lowercase();
        let searching = !filter.is_empty();
        let connected_only = self.view.connected_only;
        let mut to_connect = None;
        let mut to_edit = None;
        let mut to_delete = None;
        let mut disconnect = None;
        let mut refresh_databases = None;
        let mut refresh_tables = None;
        let mut to_select = None;
        let mut preview = None;
        let mut schema_dialog = None;
        let mut table_create = None;
        let mut move_connection = None;
        let mut group_dialog = None;
        let mut buckets: Vec<_> = self
            .catalog
            .groups
            .iter()
            .map(|g| (Some(g.id), g.name.clone()))
            .collect();
        buckets.push((None, "未分组".into()));
        let tree_generation = self.view.tree_generation;
        let expand_connection = self.expand_connection.take();
        let expand_database = self.expand_database.take();
        egui::ScrollArea::vertical()
            .id_salt("connection-tree")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut visible_connections = 0usize;
                for (group_id, group_name) in &buckets {
                    let group_matches = searching && group_name.to_lowercase().contains(&filter);
                    let members: Vec<_> = self
                        .profiles
                        .iter()
                        .filter(|p| self.catalog.memberships.get(&p.id).copied() == *group_id)
                        .filter(|profile| {
                            let session = self.sessions.get(&profile.id);
                            (!connected_only || session.is_some_and(|session| session.connected.is_some()))
                                && (!searching || group_matches || profile.name.to_lowercase().contains(&filter)
                                    || session.is_some_and(|session| {
                                        session.databases.iter().any(|database| database.to_lowercase().contains(&filter))
                                            || session.tables.iter().any(|table| format!("{}.{}", table.schema, table.name).to_lowercase().contains(&filter))
                                    }))
                        })
                        .cloned()
                        .collect();
                    visible_connections += members.len();
                    if members.is_empty() && (searching || connected_only) { continue; }
                    let expand_group =
                        expand_connection.is_some_and(|id| members.iter().any(|p| p.id == id));
                    let group_header =
                        egui::CollapsingHeader::new(format!("{} ({})", group_name, members.len()))
                            .id_salt(("connection-group", group_id, tree_generation))
                            .default_open(tree_generation == 0)
                            .open(if expand_group || !filter.is_empty() {
                                Some(true)
                            } else {
                                None
                            })
                            .show(ui, |ui| {
                                for profile in members {
                                    let filter = if group_matches || (searching && profile.name.to_lowercase().contains(&filter)) { String::new() } else { filter.clone() };
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
                                    let header_response =
                                        if connected {
                                            egui::CollapsingHeader::new(title)
                                    .id_salt(("server", profile.id, tree_generation))
                                    .open(if expand_connection == Some(profile.id) || searching {
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
                                            let active = active_database.as_ref() == Some(database);
                                            let protected_database = match profile.engine {
                                                Engine::MySql => ["mysql", "information_schema", "performance_schema", "sys"].iter().any(|name| database.eq_ignore_ascii_case(name)),
                                                Engine::Postgres => ["postgres", "template0", "template1"].iter().any(|name| database.eq_ignore_ascii_case(name)),
                                                Engine::SqlServer => ["master", "model", "msdb", "tempdb"].iter().any(|name| database.eq_ignore_ascii_case(name)),
                                            };
                                            let matches_database =
                                                database.to_lowercase().contains(&filter);
                                            if !filter.is_empty() && !matches_database && !active {
                                                continue;
                                            }
                                            let title = RichText::new(database)
                                                .color(ui.visuals().text_color());
                                            let background = ui.painter().add(egui::Shape::Noop);
                                            let database_header =
                                                egui::CollapsingHeader::new(title)
                                                    .id_salt(("database", profile.id, database, tree_generation))
                                                    .open(
                                                        if expand_database.as_ref()
                                                            == Some(&(profile.id, database.clone())) || searching
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
                                                                &mut schema_dialog,
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
                                                                let schema_background = ui.painter().add(egui::Shape::Noop);
                                                                let schema_header = egui::CollapsingHeader::new(schema)
                                                                    .id_salt((
                                                                        profile.id, database,
                                                                        schema,
                                                                        tree_generation,
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
                                                                                &mut schema_dialog,
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
                                                                self.tree_selection.row(&schema_header.header_response, schema_background,
                                                                    TreeNode::Schema(profile.id, database.clone(), schema.to_string()));
                                                                let schema_tail_response = row_tail_response(
                                                                    ui,
                                                                    &schema_header.header_response,
                                                                    ("schema-row-tail", profile.id, database, schema),
                                                                );
                                                                if schema_tail_response.as_ref().is_some_and(|response| response.clicked() || response.secondary_clicked()) {
                                                                    self.tree_selection.selected = Some(TreeNode::Schema(profile.id, database.clone(), schema.to_string()));
                                                                    clicked_node = self.tree_selection.selected.clone();
                                                                }
                                                                schema_header.header_response.context_menu(|ui| {
                                                                    if ui.add_enabled(!busy && active, egui::Button::new("在此 Schema 新建数据表…")).clicked() {
                                                                        table_create = Some(new_table_create_form(
                                                                            profile.id,
                                                                            database.clone(),
                                                                            schema.to_string(),
                                                                            profile.engine,
                                                                        ));
                                                                        ui.close();
                                                                    }
                                                                });
                                                                if let Some(response) = schema_tail_response {
                                                                    response.context_menu(|ui| {
                                                                        if ui.add_enabled(!busy && active, egui::Button::new("在此 Schema 新建数据表…")).clicked() {
                                                                            table_create = Some(new_table_create_form(
                                                                                profile.id,
                                                                                database.clone(),
                                                                                schema.to_string(),
                                                                                profile.engine,
                                                                            ));
                                                                            ui.close();
                                                                        }
                                                                    });
                                                                }
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
                                                TreeNode::Database(profile.id, database.clone()),
                                            );
                                            let database_tail_response = row_tail_response(
                                                ui,
                                                &database_header.header_response,
                                                ("database-row-tail", profile.id, database),
                                            );
                                            let database_row_tail_clicked =
                                                database_tail_response.as_ref().is_some_and(
                                                    egui::Response::clicked,
                                                );
                                            let database_row_tail_secondary_clicked =
                                                database_tail_response.as_ref().is_some_and(
                                                    egui::Response::secondary_clicked,
                                                );
                                            if database_row_tail_clicked
                                                || database_row_tail_secondary_clicked
                                            {
                                                self.tree_selection.selected = Some(
                                                    TreeNode::Database(
                                                        profile.id,
                                                        database.clone(),
                                                    ),
                                                );
                                                clicked_node = self.tree_selection.selected.clone();
                                                self.expand_database =
                                                    Some((profile.id, database.clone()));
                                            }
                                            if (database_header.header_response.clicked()
                                                || database_row_tail_clicked)
                                                && !busy
                                                && !active
                                            {
                                                to_select = Some((profile.id, database.clone()));
                                            }
                                            database_header.header_response.context_menu(|ui| {
                                                database_context_menu(
                                                    ui,
                                                    profile.id,
                                                    profile.engine,
                                                    database,
                                                    active,
                                                    busy,
                                                    protected_database,
                                                    &mut schema_dialog,
                                                    &mut table_create,
                                                    &mut to_select,
                                                    &mut refresh_tables,
                                                );
                                            });
                                            if let Some(response) = database_tail_response {
                                                response.context_menu(|ui| {
                                                    database_context_menu(
                                                        ui,
                                                        profile.id,
                                                        profile.engine,
                                                        database,
                                                        active,
                                                        busy,
                                                        protected_database,
                                                        &mut schema_dialog,
                                                        &mut table_create,
                                                        &mut to_select,
                                                        &mut refresh_tables,
                                                    );
                                                });
                                            }
                                        }
                                    })
                                    .header_response
                                        } else {
                                            ui.add_sized(
                                                [
                                                    ui.available_width(),
                                                    ui.spacing().interact_size.y,
                                                ],
                                                egui::Button::new(title)
                                                    .frame(false)
                                                    .right_text(""),
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
                                    let connection_tail_response = row_tail_response(
                                        ui,
                                        &header_response,
                                        ("connection-row-tail", profile.id),
                                    );
                                    let connection_row_tail_clicked =
                                        connection_tail_response.as_ref().is_some_and(
                                            egui::Response::clicked,
                                        );
                                    let connection_row_tail_secondary_clicked =
                                        connection_tail_response.as_ref().is_some_and(
                                            egui::Response::secondary_clicked,
                                        );
                                    self.tree_selection.row(
                                        &header_response,
                                        background,
                                        TreeNode::Connection(profile.id),
                                    );
                                    if connection_row_tail_clicked
                                        || connection_row_tail_secondary_clicked
                                    {
                                        self.tree_selection.selected =
                                            Some(TreeNode::Connection(profile.id));
                                        clicked_node = self.tree_selection.selected.clone();
                                        self.selected = Some(profile.id);
                                        if connected {
                                            self.expand_connection = Some(profile.id);
                                        }
                                    }
                                    if header_response.clicked()
                                        || header_response.secondary_clicked()
                                        || connection_row_tail_clicked
                                        || connection_row_tail_secondary_clicked
                                    {
                                        self.selected = Some(profile.id);
                                    }
                                    if header_response.double_clicked() && !busy && !connected {
                                        to_connect = Some(profile.id);
                                    }
                                    header_response.context_menu(|ui| {
                                        connection_context_menu(
                                            ui,
                                            profile.id,
                                            connected,
                                            busy,
                                            self.catalog_store.is_some(),
                                            self.catalog.memberships.get(&profile.id).copied(),
                                            &buckets,
                                            &mut schema_dialog,
                                            &mut disconnect,
                                            &mut refresh_databases,
                                            &mut to_connect,
                                            &mut move_connection,
                                            &mut to_edit,
                                            &mut to_delete,
                                        );
                                    });
                                    if let Some(response) = connection_tail_response {
                                        response.context_menu(|ui| {
                                            connection_context_menu(
                                                ui,
                                                profile.id,
                                                connected,
                                                busy,
                                                self.catalog_store.is_some(),
                                                self.catalog.memberships.get(&profile.id).copied(),
                                                &buckets,
                                                &mut schema_dialog,
                                                &mut disconnect,
                                                &mut refresh_databases,
                                                &mut to_connect,
                                                &mut move_connection,
                                                &mut to_edit,
                                                &mut to_delete,
                                            );
                                        });
                                    }
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
                if visible_connections == 0 && !self.profiles.is_empty() {
                    ui.label(RichText::new(if searching { "没有匹配的连接或已读对象" } else { "当前没有已连接的服务器" }).weak());
                }
                if let Some(position) = ui.input(|input| {
                    (input.pointer.button_released(egui::PointerButton::Primary)
                        || input.pointer.button_released(egui::PointerButton::Secondary))
                        .then(|| input.pointer.interact_pos()).flatten()
                }) && let Some((_, _, node)) = self.tree_selection.rows.iter().find(|(_, rect, _)| rect.contains(position)) {
                        clicked_node = Some(node.clone());
                }
                self.tree_selection.paint(ui);
                if let Some(node) = &self.tree_selection.selected {
                    self.selected = Some(match node {
                        TreeNode::Connection(id)
                        | TreeNode::Database(id, _)
                        | TreeNode::Schema(id, _, _)
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
        if let Some(node) = clicked_node.or_else(|| {
            (self.tree_selection.selected != tree_before)
                .then(|| self.tree_selection.selected.clone())
                .flatten()
        }) {
            let (scope, item) = match node {
                TreeNode::Connection(connection) => (
                    object_browser_ui::BrowserScope {
                        connection,
                        database: None,
                        schema: None,
                    },
                    None,
                ),
                TreeNode::Database(connection, database) => (
                    object_browser_ui::BrowserScope {
                        connection,
                        database: Some(database),
                        schema: None,
                    },
                    None,
                ),
                TreeNode::Schema(connection, database, schema) => (
                    object_browser_ui::BrowserScope {
                        connection,
                        database: Some(database),
                        schema: Some(schema),
                    },
                    None,
                ),
                TreeNode::Table(connection, database, table) => (
                    object_browser_ui::BrowserScope {
                        connection,
                        database: Some(database.clone()),
                        schema: Some(table.schema.clone()),
                    },
                    Some(object_browser_ui::BrowserItem::Object {
                        connection,
                        database,
                        table,
                    }),
                ),
            };
            self.show_object_scope(scope, item);
        }
        if let Some((connection, group)) = move_connection {
            self.move_to_group(connection, group);
        }
        if let Some(dialog) = group_dialog {
            self.catalog_dialog = Some(dialog);
        }
        if let Some(dialog) = schema_dialog {
            self.schema_dialog = Some(dialog);
        }
        if let Some(form) = table_create {
            self.table_create = Some(form);
            self.schema_dialog = None;
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
            if self.cell_editor.is_some() {
                self.finish_inline_cell(false, CellMovement::Stay);
                if self.cell_editor.is_some() {
                    return;
                }
            }
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
            self.browser_visible = false;
            let tab = &self.tabs[index];
            if created
                || ((tab.output.is_none() || tab.table_snapshot_stale)
                    && !tab.has_table_drafts()
                    && self.cell_editor.is_none())
            {
                self.load_table(index);
            }
        }
    }

    fn workspace(&mut self, ui: &mut egui::Ui) {
        self.workspace_navigation(ui);
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                self.workspace_body(ui);
            });
    }

    fn workspace_body(&mut self, ui: &mut egui::Ui) {
        if !self.browser_visible
            && self.tabs[self.active_tab].management_document()
            && self.table_create.is_none()
        {
            self.management_workspace(ui);
            return;
        }
        if !self.browser_visible
            && self.tabs[self.active_tab].definition.is_some()
            && self.table_create.is_none()
        {
            self.object_definition_workspace(ui);
            return;
        }
        if self.browser_visible && self.table_create.is_none() {
            self.object_browser_workspace(ui);
            return;
        }
        if self.tabs[self.active_tab].design.is_some() && self.table_create.is_none() {
            self.structure_workspace(ui);
            return;
        }
        if self.tabs[self.active_tab].table_target.is_none() || self.table_create.is_some() {
            self.workspace_content(ui);
            return;
        }
        if self.table_recovery_workspace(ui) {
            return;
        }
        // Reserve pagination before rendering content, including failed or empty queries.
        let records = self.tabs[self.active_tab]
            .output
            .as_ref()
            .and_then(|output| output.sets.get(self.tabs[self.active_tab].result_index))
            .is_some_and(|set| !set.rows.is_empty());
        let (content, footer) =
            table_workspace_rects_with_records(ui.available_rect_before_wrap(), records);
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
        if self.table_create.is_some() {
            self.table_create_workspace(ui);
            return;
        }
        let table_tab = self.tabs[self.active_tab].table_target.is_some();
        self.workspace_tools(ui);
        ui.add_space(4.0);
        let show_results = table_tab
            || self.tabs[self.active_tab].query_started
            || self.tabs[self.active_tab].output.is_some();
        if !table_tab {
            let columns = self.completion_columns();
            let tables = self.completion_tables();
            let (_, needs_columns) = sql_editor(
                ui,
                &mut self.tabs[self.active_tab],
                &mut self.editor_height,
                show_results,
                &columns,
                &tables,
            );
            if needs_columns && self.tabs[self.active_tab].target.is_some() {
                self.prefetch_completion_columns();
            }
            if self.cell_editor.is_none()
                && self.form.is_none()
                && self.confirm_delete.is_none()
                && self.export_path.is_none()
                && self.confirm_rebind.is_none()
                && self.confirm_tab_action.is_none()
                && self.confirm_request.is_none()
                && !self.confirm_exit
                && !self.history_open
                && ui.is_enabled()
            {
                let current = ui.input_mut(|i| {
                    i.consume_key(
                        egui::Modifiers {
                            shift: true,
                            ..egui::Modifiers::COMMAND
                        },
                        egui::Key::Enter,
                    )
                });
                if current {
                    self.execute_scope(SqlExecutionScope::CurrentStatement);
                } else if ui
                    .input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter))
                {
                    self.execute();
                }
            }
        }
        if !show_results {
            return;
        }
        ui.add_space(4.0);
        let tab = &mut self.tabs[self.active_tab];
        if !table_tab {
            let can_locate_error = tab.last_executed_document.as_ref() == Some(&tab.sql)
                && tab.last_executed_char_start.is_some();
            let action = query_result_ui::show_results(
                ui,
                &mut tab.result_index,
                query_result_ui::QueryResultContext {
                    tab: tab.id,
                    output: tab.output.as_ref(),
                    error: tab.query_error.as_deref(),
                    diagnostic: tab.query_diagnostic.as_ref(),
                    elapsed_ms: tab.query_elapsed_ms,
                    busy,
                    can_locate_error,
                },
            );
            if let Some(offset) = action.locate_error.filter(|_| can_locate_error) {
                let offset = tab.last_executed_char_start.unwrap().saturating_add(offset);
                let start = char_to_byte_index(&tab.sql, offset);
                let end = char_to_byte_index(&tab.sql, offset.saturating_add(1));
                tab.select_range = Some(start..end);
            }
            if !action.show_grid {
                return;
            }
        }
        let Some(output) = &tab.output else {
            if let Some(message) = &tab.query_error {
                ui.colored_label(ui.visuals().error_fg_color, message);
                return;
            }
            ui.label(
                RichText::new(
                    if self
                        .sessions
                        .values()
                        .any(|s| s.running_tab == Some(tab.id))
                        || self.query_sessions.get(&tab.id).is_some_and(|s| s.busy)
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
        if table_tab && output.sets.len() > 1 {
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
                if !set.columns.is_empty() {
                    ui.menu_button("复制", |ui| result_copy_controls(ui, tab, true));
                    if ui.button("导出 CSV").clicked() {
                        self.export_path = Some(default_csv_export_path());
                    }
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
            ui.label("语句已完成，无行数据；详细执行信息见消息页。");
            return;
        }
        let context_matches = tab.table_target.as_ref().is_some_and(|target| {
            self.sessions.get(&target.connection).is_some_and(|s| {
                s.connected.is_some() && s.active_database.as_ref() == Some(&target.database)
            })
        });
        if let Some(data) = &tab.table_data {
            let reason = if tab.table_snapshot_stale {
                Some("表数据已过期；刷新成功前暂不能编辑、新增、删除或导入。")
            } else if !context_matches {
                Some("此表来自其他连接或数据库，选择对应数据库后可编辑。")
            } else if data.primary_keys.is_empty() {
                Some("已有记录只读：此表没有主键，无法准确定位修改；普通表仍可新增记录。")
            } else if !data.columns.iter().any(|column| column.editable) {
                Some("只读：视图、生成列或当前表类型不支持编辑。")
            } else {
                None
            };
            if let Some(reason) = reason {
                ui.label(RichText::new(reason).small().weak());
            }
            if let Some(notice) = &tab.table_write_notice {
                ui.label(RichText::new(format!("{notice} · 等待刷新确认最新数据")).small());
            }
        }
        let (editor, grid_action) = result_grid_inline(
            ui,
            tab,
            context_matches && !busy && !tab.table_snapshot_stale,
            self.cell_editor.as_mut(),
        );
        if let Some(editor) = editor {
            self.cell_editor = Some(editor);
        }
        if let Some(action) = grid_action {
            let tab = &self.tabs[self.active_tab];
            match action {
                GridAction::FinishCell { cancel, movement } => {
                    self.finish_inline_cell(cancel, movement)
                }
                GridAction::PasteValues(text) => {
                    match paste_grid_values(
                        &mut self.tabs[self.active_tab],
                        &text,
                        context_matches && !busy,
                    ) {
                        Ok(count) => {
                            self.status = format!("已暂存 {count} 格粘贴内容；提交后写入数据库");
                            self.error = false;
                        }
                        Err(message) => self.fail(message),
                    }
                }
                GridAction::DraftError(message) => self.fail(message),
                GridAction::Browse(action) => self.apply_grid_browse(action),
                GridAction::ExportRows(rows) => {
                    self.export_rows = Some((tab.id, rows));
                    self.export_path = Some(default_csv_export_path());
                }
                GridAction::DeleteRows(rows) => {
                    if let (Some(target), Some(_table), Some(data)) =
                        (&tab.table_target, &tab.target, &tab.table_data)
                    {
                        let key_values = rows
                            .iter()
                            .filter_map(|index| data.keys.get(*index).cloned())
                            .collect();
                        self.delete_rows = Some((
                            tab.id,
                            target.clone(),
                            data.primary_keys.clone(),
                            key_values,
                        ));
                    }
                }
            }
        }
    }

    fn table_create_workspace(&mut self, ui: &mut egui::Ui) {
        let connection = self.table_create.as_ref().map(|form| form.connection);
        let busy = connection.is_some_and(|id| self.session_busy(id))
            || self.table_create_pending.is_some();
        let Some(form) = self.table_create.as_mut() else {
            return;
        };
        let mut cancel = false;
        let mut create = false;
        let mut remove_column = None;
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.heading("新建数据表");
            ui.label(RichText::new(format!("{}  /  {}", form.database, form.schema)).weak());
        });
        ui.add_space(8.0);
        ui.label("表名称");
        ui.add(
            egui::TextEdit::singleline(&mut form.name)
                .hint_text("例如 users")
                .desired_width(360.0),
        );
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.heading("字段定义");
            ui.label(RichText::new("配置字段类型、约束和默认值").small().weak());
        });
        ui.add_space(6.0);
        let column_area_height = (ui.available_height() - 72.0).max(140.0);
        egui::ScrollArea::both()
            .id_salt("table-create-columns")
            .max_height(column_area_height)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_sized([150.0, 26.0], egui::Label::new("字段名"));
                    ui.add_sized([165.0, 26.0], egui::Label::new("数据类型"));
                    ui.add_sized([100.0, 26.0], egui::Label::new("长度 / 精度"));
                    ui.add_sized([70.0, 26.0], egui::Label::new("可空"));
                    ui.add_sized([70.0, 26.0], egui::Label::new("主键"));
                    ui.add_sized([70.0, 26.0], egui::Label::new("自增"));
                    ui.add_sized([180.0, 26.0], egui::Label::new("默认值"));
                    ui.add_sized([50.0, 26.0], egui::Label::new("操作"));
                });
                ui.separator();
                for (index, column) in form.columns.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.add_sized(
                                [150.0, 26.0],
                                egui::TextEdit::singleline(&mut column.name)
                                    .hint_text("column_name"),
                            );
                        });
                        egui::ComboBox::from_id_salt(("column-type", index))
                            .width(165.0)
                            .selected_text(&column.data_type)
                            .show_ui(ui, |ui| {
                                for data_type in column_type_options(form.engine) {
                                    ui.selectable_value(
                                        &mut column.data_type,
                                        (*data_type).to_owned(),
                                        *data_type,
                                    );
                                }
                            });
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.add_sized(
                                [100.0, 26.0],
                                egui::TextEdit::singleline(&mut column.size).hint_text("可选"),
                            );
                        });
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.add_sized(
                                [70.0, 26.0],
                                egui::Checkbox::new(&mut column.nullable, ""),
                            );
                        });
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.add_sized(
                                [70.0, 26.0],
                                egui::Checkbox::new(&mut column.primary_key, ""),
                            );
                        });
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.add_sized(
                                [70.0, 26.0],
                                egui::Checkbox::new(&mut column.auto_increment, ""),
                            );
                        });
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.add_sized(
                                [180.0, 26.0],
                                egui::TextEdit::singleline(&mut column.default_value)
                                    .hint_text("如 0 或 CURRENT_TIMESTAMP"),
                            );
                        });
                        if ui
                            .add_enabled_ui(!busy, |ui| {
                                ui.add_sized([50.0, 26.0], egui::Button::new("移除"))
                            })
                            .inner
                            .clicked()
                        {
                            remove_column = Some(index);
                        }
                    });
                    ui.add_space(4.0);
                }
                if let Some(index) = remove_column.take() {
                    form.columns.remove(index);
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("＋ 添加字段"))
                    .clicked()
                {
                    form.columns.push(CreateTableColumn {
                        name: String::new(),
                        data_type: match form.engine {
                            Engine::MySql => "VARCHAR",
                            Engine::Postgres => "VARCHAR",
                            Engine::SqlServer => "NVARCHAR",
                        }
                        .into(),
                        nullable: true,
                        ..Default::default()
                    });
                }
            });
        ui.add_space(16.0);
        ui.separator();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            create = ui
                .add_enabled(
                    !busy && !form.name.trim().is_empty() && !form.columns.is_empty(),
                    egui::Button::new(RichText::new("创建数据表").color(Color32::WHITE))
                        .fill(accent(ui)),
                )
                .on_hover_text("保存当前字段定义并在数据库中创建表")
                .clicked();
            cancel = ui.add_enabled(!busy, egui::Button::new("取消")).clicked();
            if busy {
                ui.label(RichText::new("正在创建数据表…").weak());
            }
        });
        if cancel {
            self.table_create = None;
        } else if create {
            let form = self.table_create.take().expect("table form still set");
            let connection = form.connection;
            self.table_create = Some(form);
            let form = self.table_create.as_ref().expect("table form restored");
            let request = Request::CreateTable {
                database: form.database.clone(),
                schema: form.schema.clone(),
                name: form.name.trim().to_owned(),
                columns: form.columns.clone(),
            };
            self.table_create_pending = Some(connection);
            self.request_on(connection, request, "正在新建数据表…");
        }
    }

    fn finish_inline_cell(&mut self, cancel: bool, movement: CellMovement) {
        let Some(editor) = self.cell_editor.take() else {
            return;
        };
        if cancel {
            return;
        }
        let valid = self
            .sessions
            .get(&editor.connection)
            .is_some_and(|session| {
                session.connected.is_some()
                    && !session.busy
                    && session.active_database.as_ref() == Some(&editor.change.database)
            });
        let Some(index) = self.tabs.iter().position(|tab| tab.id == editor.tab) else {
            return;
        };
        if !valid || self.tabs[index].table_snapshot_stale {
            self.cell_editor = Some(editor);
            self.fail("表快照或连接已变化，输入已保留；取消编辑后刷新。".into());
            return;
        }
        let tab = &mut self.tabs[index];
        let location = tab.table_data.as_ref().and_then(|data| {
            let row = data
                .keys
                .iter()
                .position(|keys| keys == &editor.change.key_values)?;
            let column = data
                .columns
                .iter()
                .position(|column| column.name == editor.change.column)?;
            Some((row, column))
        });
        let mut change = editor.change;
        change.value = if editor.is_null {
            None
        } else {
            Some(editor.text)
        };
        tab.stage_cell(change);
        self.status = format!("已暂存 {} 格；提交后写入数据库", tab.staged_cells.len());
        if let Some((row, column)) = location {
            let Some(set) = tab
                .output
                .as_ref()
                .and_then(|output| output.sets.get(tab.result_index))
            else {
                return;
            };
            let columns = set.columns.len();
            let count = set.rows.len().saturating_mul(columns);
            let current = row * columns + column;
            let next = match movement {
                CellMovement::Stay => None,
                CellMovement::Across(step) => {
                    let mut position = current as i64 + i64::from(step);
                    let mut found = None;
                    while position >= 0 && position < count as i64 {
                        let pair = (position as usize / columns, position as usize % columns);
                        if grid_cell_editor(tab, pair.0, pair.1, true).is_some() {
                            found = Some(pair);
                            break;
                        }
                        position += i64::from(step);
                    }
                    found
                }
                CellMovement::Down(step) => {
                    let next_row = row as i64 + i64::from(step);
                    (next_row >= 0 && next_row < set.rows.len() as i64)
                        .then_some((next_row as usize, column))
                }
            };
            tab.selected_cell = next.or(Some((row, column)));
            tab.cell_anchor = tab.selected_cell;
            tab.selected_rows = None;
            if let Some((row, column)) = next {
                self.cell_editor = grid_cell_editor(tab, row, column, true);
            }
        }
    }

    fn cell_editor_window(&mut self, _ctx: &egui::Context) {
        // The grid owns the editor. Keep this lifecycle hook for pending snapshots.
        if self
            .cell_editor
            .as_ref()
            .is_some_and(|editor| !self.tabs.iter().any(|tab| tab.id == editor.tab))
        {
            self.cell_editor = None;
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
        dialog_window("数据库连接", ctx)
            .open(&mut open)
            .resizable(true)
            .default_width((ctx.content_rect().width() - 120.0).clamp(620.0, 860.0))
            .min_width(620.0)
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
        let busy = self.session_busy(id) || self.query_connection_busy(id);
        let mut cancel = false;
        let mut delete = false;
        dialog_window("删除本机连接", ctx).show(ctx, |ui| {
                ui.label("将删除此连接配置及保存的密码，数据库中的数据不受影响。");
                if self.sessions.get(&id).is_some_and(|s| s.connected.is_some()) || self.query_connection_used(id) {
                    ui.label("此连接仍有存活会话。删除会关闭其管理会话和所有 SQL 标签会话，未提交事务可能回滚；其他连接不受影响。");
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
                    self.close_query_sessions(id);
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

    fn delete_rows_window(&mut self, ctx: &egui::Context) {
        let Some((tab_id, target, primary_keys, key_values)) = self.delete_rows.clone() else {
            return;
        };
        let mut cancel = false;
        let mut confirm = false;
        let can_delete = !self.session_busy(target.connection)
            && self.cell_editor.is_none()
            && self
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .is_some_and(|tab| !tab.table_snapshot_stale && !tab.has_table_drafts());
        dialog_window("删除选中行", ctx).show(ctx, |ui| {
            ui.label(format!(
                "将从 {}.{} 删除 {} 行。此操作无法撤销。",
                target.schema,
                target.name,
                key_values.len()
            ));
            ui.horizontal(|ui| {
                cancel = ui.button("取消").clicked();
                confirm = ui
                    .add_enabled(can_delete, egui::Button::new("删除行"))
                    .clicked();
            });
        });
        if cancel {
            self.delete_rows = None;
        }
        if confirm {
            let request = Request::DeleteRows {
                tab: tab_id,
                target: QueryTarget {
                    schema: None,
                    connection: target.connection,
                    database: Some(target.database.clone()),
                },
                table: Table {
                    schema: target.schema.clone(),
                    name: target.name.clone(),
                    kind: "BASE TABLE".into(),
                },
                primary_keys,
                key_values,
            };
            self.delete_rows = None;
            self.request_on(target.connection, request, "正在删除选中行…");
        }
    }

    fn schema_dialog_window(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.schema_dialog.as_mut() else {
            return;
        };
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;
        let title = match dialog.action {
            SchemaDialogAction::CreateDatabase { .. } => "新建数据库",
            SchemaDialogAction::DropDatabase { .. } => "删除数据库",
            SchemaDialogAction::DropTable { .. } => "删除数据表",
        };
        dialog_window(title, ctx).open(&mut open).show(ctx, |ui| {
            match &mut dialog.action {
                SchemaDialogAction::CreateDatabase { .. } => {
                    ui.label("数据库名称");
                    ui.add(egui::TextEdit::singleline(&mut dialog.name).desired_width(360.0));
                }
                SchemaDialogAction::DropDatabase { database, .. } => {
                    ui.label(format!(
                        "确定删除数据库 {database}？其中的所有表和数据都会被删除，且无法撤销。"
                    ));
                }
                SchemaDialogAction::DropTable {
                    database, table, ..
                } => {
                    ui.label(format!(
                        "确定删除数据表 {database}.{}.{}？表内所有数据都会被删除，且无法撤销。",
                        table.schema, table.name
                    ));
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                cancel = ui.button("取消").clicked();
                let label = if matches!(
                    dialog.action,
                    SchemaDialogAction::DropDatabase { .. } | SchemaDialogAction::DropTable { .. }
                ) {
                    "删除"
                } else {
                    "创建"
                };
                confirm = ui.button(label).clicked();
            });
        });
        if cancel || !open {
            self.schema_dialog = None;
            return;
        }
        if !confirm {
            return;
        }
        let dialog = self.schema_dialog.take().expect("dialog still set");
        let (connection, request, status, created_database) = match dialog.action {
            SchemaDialogAction::CreateDatabase { connection } => {
                let database = dialog.name.trim().to_owned();
                (
                    connection,
                    Request::CreateDatabase(database.clone()),
                    "正在新建数据库…",
                    Some(database),
                )
            }
            SchemaDialogAction::DropDatabase {
                connection,
                database,
            } => (
                connection,
                Request::DropDatabase(database),
                "正在删除数据库…",
                None,
            ),
            SchemaDialogAction::DropTable {
                connection,
                database,
                table,
            } => (
                connection,
                Request::DropTable { database, table },
                "正在删除数据表…",
                None,
            ),
        };
        if let Some(database) = created_database {
            self.database_create_pending = Some((connection, database));
        }
        self.request_on(connection, request, status);
    }

    fn open_csv_import(&mut self) {
        let tab = &self.tabs[self.active_tab];
        if tab.table_snapshot_stale {
            self.fail("表数据已过期，请刷新后再导入".into());
            return;
        }
        let (Some(target), Some(table), Some(data)) = (
            tab.target.clone(),
            tab.table_target.as_ref(),
            tab.table_data.as_ref(),
        ) else {
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("CSV", &["csv"])
            .pick_file()
        else {
            return;
        };
        let result = (|| -> anyhow::Result<_> {
            let file = std::fs::File::open(&path)?;
            anyhow::ensure!(
                file.metadata()?.len() <= 64 * 1024 * 1024,
                "CSV 文件超过 64 MiB；请分批导入"
            );
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(
                &mut std::io::Read::take(file, 64 * 1024 * 1024 + 1),
                &mut bytes,
            )?;
            anyhow::ensure!(
                bytes.len() <= 64 * 1024 * 1024,
                "CSV 文件超过 64 MiB；请分批导入"
            );
            parse_nullable_csv(&bytes)
        })();
        match result {
            Ok((headers, rows)) => {
                let mapping = headers
                    .iter()
                    .map(|name| {
                        data.columns
                            .iter()
                            .find(|c| c.editable && c.name == *name)
                            .map(|c| c.name.clone())
                    })
                    .collect();
                self.csv_import = Some(CsvImportForm {
                    tab: tab.id,
                    target,
                    table: Table {
                        schema: table.schema.clone(),
                        name: table.name.clone(),
                        kind: "BASE TABLE".into(),
                    },
                    headers,
                    mapping,
                    rows,
                });
            }
            Err(error) => self.fail(format!("读取 CSV 失败：{error:#}")),
        }
    }

    fn csv_import_window(&mut self, ctx: &egui::Context) {
        if let Some((connection, tab_id)) = self
            .csv_import
            .as_ref()
            .map(|form| (form.target.connection, form.tab))
        {
            let busy = self.session_busy(connection);
            let stale = self
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .is_none_or(|tab| tab.table_snapshot_stale);
            let available = self
                .tabs
                .iter()
                .find(|t| t.id == tab_id)
                .and_then(|t| t.table_data.as_ref())
                .map(|data| {
                    data.columns
                        .iter()
                        .filter(|c| c.editable)
                        .map(|c| c.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let form = self.csv_import.as_mut().unwrap();
            let mut open = true;
            let mut submit = false;
            dialog_window("CSV 导入 · 映射预览", ctx)
                .open(&mut open)
                .default_width(700.0)
                .show(ctx, |ui| {
                ui.label(format!("{} / {}.{} · {} 行", form.target.database.as_deref().unwrap_or(""), form.table.schema, form.table.name, form.rows.len()));
                ui.label("CSV 空字段（未引号）= NULL；双引号包裹的空文本=空字符串；文字 NULL 始终是字符串。未映射列使用默认值。");
                for (index, header) in form.headers.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{header} →"));
                        egui::ComboBox::from_id_salt(("csv-map", index)).selected_text(form.mapping[index].as_deref().unwrap_or("忽略列"))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut form.mapping[index], None, "忽略列");
                                for name in &available { ui.selectable_value(&mut form.mapping[index], Some(name.clone()), name); }
                            });
                    });
                }
                egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                    for row in form.rows.iter().take(5) { ui.label(format!("{:?}", row)); }
                });
                let valid = !form.rows.is_empty() && form.mapping.iter().any(Option::is_some)
                    && form.mapping.iter().flatten().enumerate().all(|(i, name)| form.mapping.iter().flatten().take(i).all(|previous| previous != name));
                if stale { ui.label("表数据已过期，请取消并刷新后重新导入。"); }
                submit = ui.add_enabled(valid && !busy && !stale, egui::Button::new(format!("确认事务导入 {} 行", form.rows.len()))).clicked();
            });
            if !open && !busy {
                self.csv_import = None;
            }
            if submit {
                let form = self.csv_import.as_ref().unwrap();
                let selected = form
                    .mapping
                    .iter()
                    .enumerate()
                    .filter_map(|(i, name)| name.as_ref().map(|name| (i, name.clone())))
                    .collect::<Vec<_>>();
                let columns = selected.iter().map(|(_, name)| name.clone()).collect();
                let rows = form
                    .rows
                    .iter()
                    .map(|row| selected.iter().map(|(i, _)| row[*i].clone()).collect())
                    .collect();
                let request = Request::InsertRows {
                    tab: form.tab,
                    target: form.target.clone(),
                    table: form.table.clone(),
                    columns,
                    rows,
                };
                self.request(request, "正在事务导入 CSV…");
            }
        }
    }

    fn export_window(&mut self, ctx: &egui::Context) {
        let Some(path) = &mut self.export_path else {
            return;
        };
        let mut open = true;
        let mut export = false;
        let selected_count = self.export_rows.as_ref().map(|(_, rows)| rows.len());
        let complete_table =
            selected_count.is_none() && self.tabs[self.active_tab].table_target.is_some();
        dialog_window(
            if selected_count.is_some() {
                "导出选中行"
            } else if complete_table {
                "完整导出表（含当前筛选）"
            } else {
                "导出当前结果集"
            },
            ctx,
        )
        .open(&mut open)
        .default_width(500.0)
        .show(ctx, |ui| {
            ui.label("保存路径");
            ui.add(egui::TextEdit::singleline(path).desired_width(f32::INFINITY));
            ui.label(
                RichText::new(if let Some(count) = selected_count {
                    format!("导出所选的 {count} 行；NULL 为未引号空字段，文本始终加引号。")
                } else if complete_table {
                    "导出此表全部匹配筛选的行（不限当前页）；需要主键；NULL 为未引号空字段，文本始终加引号。".into()
                } else {
                    "仅导出当前展示的查询结果；NULL 为未引号空字段，文本始终加引号。".into()
                })
                .small()
                .weak(),
            );
            export = ui.button("保存 CSV").clicked();
        });
        let destination = PathBuf::from(path.as_str());
        if !open {
            self.export_path = None;
            self.export_rows = None;
        }
        if export && complete_table {
            let tab = &self.tabs[self.active_tab];
            if let (Some(target), Some(table)) = (tab.target.clone(), tab.table_target.as_ref()) {
                let request = Request::ExportTable {
                    tab: tab.id,
                    target,
                    table: Table {
                        schema: table.schema.clone(),
                        name: table.name.clone(),
                        kind: "BASE TABLE".into(),
                    },
                    options: tab.browse.clone(),
                    path: destination,
                };
                self.export_path = None;
                self.request(request, "正在完整导出表数据…");
            }
        } else if export {
            let tab = &self.tabs[self.active_tab];
            let result = tab
                .output
                .as_ref()
                .and_then(|o| o.sets.get(tab.result_index))
                .ok_or_else(|| anyhow::anyhow!("没有可导出的结果集"))
                .and_then(|set| {
                    if let Some((tab_id, rows)) = &self.export_rows {
                        anyhow::ensure!(*tab_id == tab.id, "导出目标标签已改变");
                        let selected = ResultSet {
                            rows: rows
                                .iter()
                                .filter_map(|index| set.rows.get(*index).cloned())
                                .collect(),
                            ..set.clone()
                        };
                        export_csv(&selected, &destination)
                    } else {
                        export_csv(set, &destination)
                    }
                });
            match result {
                Ok(()) => {
                    self.export_path = None;
                    self.export_rows = None;
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
        for session in self.sessions.values_mut() {
            session.completion_pause = false;
        }
        self.poll();
        apply_theme(ctx);
        egui::TopBottomPanel::top("title")
            .exact_height(window_chrome::TITLE_HEIGHT)
            .frame(egui::Frame::new().fill(chrome(ctx)))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
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
                            self.sessions
                                .values()
                                .filter(|s| s.connected.is_some())
                                .count(),
                            self.sessions.values().filter(|s| s.busy).count()
                                + self.query_sessions.values().filter(|s| s.busy).count()
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
                            } else if self.tabs[self.active_tab].management_document() {
                                ui.label(RichText::new("独立工具文档").small().weak());
                            } else {
                                self.executed_sql_footer(ui);
                            }
                        }
                    });
                });
        }
        let modal = self.modal_open();
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

#[cfg(test)]
fn table_workspace_rects(available: egui::Rect) -> (egui::Rect, egui::Rect) {
    table_workspace_rects_with_records(available, false)
}

fn table_workspace_rects_with_records(
    available: egui::Rect,
    records: bool,
) -> (egui::Rect, egui::Rect) {
    let footer_height = if records || table_footer_two_rows(available.width()) {
        74.0
    } else {
        42.0
    };
    let footer_top = (available.bottom() - footer_height).max(available.top());
    let content = egui::Rect::from_min_max(
        available.min,
        egui::pos2(available.right(), (footer_top - 6.0).max(available.top())),
    );
    let footer = egui::Rect::from_min_max(egui::pos2(available.left(), footer_top), available.max);
    (content, footer)
}

fn table_footer_two_rows(width: f32) -> bool {
    width < 820.0
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

#[derive(Clone, Debug)]
enum GridAction {
    ExportRows(Vec<usize>),
    DeleteRows(Vec<usize>),
    Browse(grid_browse_ui::BrowseAction),
    FinishCell {
        cancel: bool,
        movement: CellMovement,
    },
    PasteValues(String),
    DraftError(String),
}

#[derive(Clone, Copy, Debug)]
enum CellMovement {
    Stay,
    Across(i32),
    Down(i32),
}

#[derive(Clone, Copy)]
enum CellDraftAction {
    SetNull,
    SetEmptyString,
    Revert,
}

fn grid_cell_eligible(
    can_edit: bool,
    target: Option<&TableTarget>,
    data: Option<&TableData>,
    set: &ResultSet,
    row: usize,
    column: usize,
) -> bool {
    can_edit
        && target.is_some()
        && data.is_some_and(|data| {
            data.writable
                && !data.primary_keys.is_empty()
                && data.keys.get(row).is_some_and(|keys| {
                    keys.len() == data.primary_keys.len() && keys.iter().all(Option::is_some)
                })
                && data.columns.get(column).is_some_and(|metadata| {
                    metadata.editable
                        && metadata.kind != sqlshop::model::CellKind::Unsupported
                        && set.columns.get(column) == Some(&metadata.name)
                })
                && data
                    .columns
                    .iter()
                    .filter(|column| column.primary_key)
                    .all(|column| column.kind != sqlshop::model::CellKind::Unsupported)
                && data
                    .columns
                    .iter()
                    .filter(|column| column.primary_key)
                    .map(|column| &column.name)
                    .eq(data.primary_keys.iter())
                && set
                    .rows
                    .get(row)
                    .is_some_and(|values| values.get(column).is_some())
        })
}

fn grid_cell_editor(
    tab: &QueryTab,
    row: usize,
    column: usize,
    can_edit: bool,
) -> Option<CellEditor> {
    let set = tab.output.as_ref()?.sets.get(tab.result_index)?;
    let target = tab.table_target.as_ref()?;
    let data = tab.table_data.as_ref()?;
    if !grid_cell_eligible(
        can_edit && !tab.table_snapshot_stale,
        Some(target),
        Some(data),
        set,
        row,
        column,
    ) {
        return None;
    }
    let original = set.rows[row][column].clone();
    let value = tab
        .staged_cells
        .iter()
        .find(|change| change.key_values == data.keys[row] && change.column == set.columns[column])
        .map_or_else(|| original.clone(), |change| change.value.clone());
    Some(CellEditor {
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
            key_values: data.keys[row].clone(),
            column: set.columns[column].clone(),
            original,
            value: value.clone(),
        },
        text: value.clone().unwrap_or_default(),
        is_null: value.is_none(),
        focus: true,
        expanded: value
            .as_deref()
            .is_some_and(|text| text.contains('\n') || text.chars().count() > 140),
    })
}

fn apply_grid_cell_draft(
    tab: &mut QueryTab,
    row: usize,
    column: usize,
    action: CellDraftAction,
    can_edit: bool,
) -> bool {
    let Some(editor) = grid_cell_editor(tab, row, column, can_edit) else {
        return false;
    };
    match action {
        CellDraftAction::Revert => {
            tab.staged_cells.retain(|change| {
                change.key_values != editor.change.key_values
                    || change.column != editor.change.column
            });
        }
        CellDraftAction::SetNull => tab.stage_cell(CellUpdate {
            value: None,
            ..editor.change
        }),
        CellDraftAction::SetEmptyString => {
            if tab.table_data.as_ref().unwrap().columns[column].kind
                != sqlshop::model::CellKind::Text
            {
                return false;
            }
            tab.stage_cell(CellUpdate {
                value: Some(String::new()),
                ..editor.change
            });
        }
    }
    true
}

fn selected_grid_cells(tab: &QueryTab, row: usize, column: usize) -> Vec<(usize, usize)> {
    if !cell_in_selection(tab.cell_anchor, tab.selected_cell, row, column) {
        return vec![(row, column)];
    }
    let (end_row, end_column) = tab.selected_cell.unwrap();
    let (start_row, start_column) = tab.cell_anchor.unwrap_or((end_row, end_column));
    (start_row.min(end_row)..=start_row.max(end_row))
        .flat_map(|row| {
            (start_column.min(end_column)..=start_column.max(end_column))
                .map(move |column| (row, column))
        })
        .collect()
}

fn apply_grid_selection_draft(
    tab: &mut QueryTab,
    row: usize,
    column: usize,
    action: CellDraftAction,
    can_edit: bool,
) -> Result<usize, String> {
    let cells = selected_grid_cells(tab, row, column);
    // Validate the complete block before changing any draft.
    for &(row, column) in &cells {
        if grid_cell_editor(tab, row, column, can_edit).is_none() {
            return Err("选中区域包含只读或无效单元格，未修改草稿。".into());
        }
        if matches!(action, CellDraftAction::SetEmptyString)
            && tab.table_data.as_ref().unwrap().columns[column].kind
                != sqlshop::model::CellKind::Text
        {
            return Err("空字符串操作仅支持文本列，未修改草稿。".into());
        }
    }
    let count = cells.len();
    for (row, column) in cells {
        apply_grid_cell_draft(tab, row, column, action, can_edit);
    }
    Ok(count)
}

fn paste_grid_values(tab: &mut QueryTab, text: &str, can_edit: bool) -> Result<usize, String> {
    let (row, column) = tab.selected_cell.ok_or("请先选择粘贴的起始单元格")?;
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .delimiter(b'\t')
        .flexible(false)
        .from_reader(text.as_bytes());
    let records = reader
        .records()
        .map(|record| record.map(|record| record.iter().map(str::to_owned).collect::<Vec<_>>()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法解析粘贴内容：{error}"))?;
    if records.is_empty() {
        return Ok(0);
    }
    let destinations: Vec<_> = if records.len() == 1 && records[0].len() == 1 {
        selected_grid_cells(tab, row, column)
            .into_iter()
            .map(|(row, column)| (row, column, records[0][0].clone()))
            .collect()
    } else {
        let anchor = tab.cell_anchor.unwrap_or((row, column));
        let start_row = anchor.0.min(row);
        let start_column = anchor.1.min(column);
        records
            .iter()
            .enumerate()
            .flat_map(|(r, values)| {
                values
                    .iter()
                    .enumerate()
                    .map(move |(c, value)| (start_row + r, start_column + c, value.clone()))
            })
            .collect()
    };
    if destinations.len() > 10_000 {
        return Err("一次最多暂存 10000 格，请缩小粘贴范围。".into());
    }
    let mut changes = Vec::with_capacity(destinations.len());
    for (row, column, value) in destinations {
        let mut editor = grid_cell_editor(tab, row, column, can_edit)
            .ok_or("粘贴区域超出当前页或包含只读单元格，未修改草稿。")?;
        // Pasted text stays text, including the literal NULL and empty string.
        editor.change.value = Some(value);
        changes.push(editor.change);
    }
    let count = changes.len();
    for change in changes {
        tab.stage_cell(change);
    }
    Ok(count)
}

fn row_context_menu(
    response: egui::Response,
    selected_rows: &mut Option<(usize, usize)>,
    set: &ResultSet,
    deletable: bool,
    row_index: usize,
) -> Option<GridAction> {
    if response.secondary_clicked() && !row_in_selection(*selected_rows, row_index) {
        *selected_rows = Some((row_index, row_index));
    }
    let mut action = None;
    response.context_menu(|ui| {
        let rows = selected_rows
            .map(|(a, b)| (a.min(b)..=a.max(b)).collect::<Vec<_>>())
            .unwrap_or_else(|| vec![row_index]);
        if ui
            .button(format!("复制选中行 ({} 行, TSV)", rows.len()))
            .clicked()
        {
            let selected = ResultSet {
                rows: rows
                    .iter()
                    .filter_map(|index| set.rows.get(*index).cloned())
                    .collect(),
                ..set.clone()
            };
            ui.ctx().copy_text(selected_tsv(
                &selected,
                Some((0, 0)),
                Some((
                    selected.rows.len().saturating_sub(1),
                    selected.columns.len().saturating_sub(1),
                )),
            ));
            ui.close();
        }
        if ui
            .button(format!("导出选中行 CSV… ({} 行)", rows.len()))
            .clicked()
        {
            action = Some(GridAction::ExportRows(rows.clone()));
            ui.close();
        }
        if ui
            .add_enabled(
                deletable,
                egui::Button::new(format!("删除选中行… ({} 行)", rows.len())),
            )
            .clicked()
        {
            action = Some(GridAction::DeleteRows(rows));
            ui.close();
        }
    });
    action
}

#[cfg(test)]
fn result_grid(
    ui: &mut egui::Ui,
    tab: &mut QueryTab,
    can_edit: bool,
) -> (Option<CellEditor>, Option<GridAction>) {
    result_grid_inline(ui, tab, can_edit, None)
}

fn result_grid_inline(
    ui: &mut egui::Ui,
    tab: &mut QueryTab,
    can_edit: bool,
    mut inline_editor: Option<&mut CellEditor>,
) -> (Option<CellEditor>, Option<GridAction>) {
    let can_edit = can_edit && !tab.table_snapshot_stale;
    let has_drafts = tab.has_table_drafts();
    let Some(set) = tab
        .output
        .as_ref()
        .and_then(|output| output.sets.get(tab.result_index))
    else {
        return (None, None);
    };
    let available_width = ui.available_width();
    let reset_widths = (tab.grid_width - available_width).abs() > 2.0;
    tab.grid_width = available_width;
    let mut edit_cell = None;
    let mut draft_action = None;
    let mut remove_insert = None;
    let mut row_action = None;
    let target = tab.table_target.as_ref();
    let metadata = tab.table_data.as_ref();
    let editing_location = inline_editor
        .as_ref()
        .filter(|editor| editor.tab == tab.id)
        .and_then(|editor| {
            let data = metadata?;
            Some((
                data.keys
                    .iter()
                    .position(|keys| keys == &editor.change.key_values)?,
                set.columns
                    .iter()
                    .position(|name| name == &editor.change.column)?,
            ))
        });
    if let Some(editor) = inline_editor.as_mut().filter(|editor| editor.tab == tab.id) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("编辑 {}", editor.change.column)).small());
            ui.add_enabled(can_edit, egui::Checkbox::new(&mut editor.is_null, "NULL"));
            if ui.selectable_label(editor.expanded, "展开编辑").clicked() {
                editor.expanded = !editor.expanded;
                editor.focus = true;
            }
            if ui
                .add_enabled(can_edit, egui::Button::new("暂存"))
                .clicked()
            {
                row_action = Some(GridAction::FinishCell {
                    cancel: false,
                    movement: CellMovement::Stay,
                });
            }
            if ui.button("取消").clicked() {
                row_action = Some(GridAction::FinishCell {
                    cancel: true,
                    movement: CellMovement::Stay,
                });
            }
            ui.label(
                RichText::new(if editor.expanded {
                    "⌘/Ctrl+Enter 暂存 · Esc 取消"
                } else {
                    "Tab 下一格 · Enter 下一行 · Esc 取消"
                })
                .small()
                .weak(),
            );
        });
        if editor.expanded {
            let was_focus = editor.focus;
            egui::Frame::group(ui.style())
                .inner_margin(6)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(("expanded-cell", tab.id))
                        .max_height(140.0)
                        .show(ui, |ui| {
                            let response = ui.add_enabled(
                                can_edit && !editor.is_null,
                                egui::TextEdit::multiline(&mut editor.text)
                                    .id(egui::Id::new(("inline-cell", tab.id)))
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(4),
                            );
                            if was_focus {
                                if !editor.is_null {
                                    response.request_focus();
                                }
                                editor.focus = false;
                            }
                            let (cancel, save) = ui.input_mut(|input| {
                                (
                                    input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                                    input.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter),
                                )
                            });
                            if row_action.is_none()
                                && (cancel
                                    || (can_edit
                                        && (save
                                            || (response.lost_focus()
                                                && !was_focus
                                                && !egui::Popup::is_any_open(ui.ctx())))))
                            {
                                row_action = Some(GridAction::FinishCell {
                                    cancel,
                                    movement: CellMovement::Stay,
                                });
                            }
                        });
                });
        }
    }
    let can_browse = can_edit
        && inline_editor.is_none()
        && target.is_some()
        && metadata.is_some()
        && !has_drafts;
    let staged_values: HashMap<_, _> = tab
        .staged_cells
        .iter()
        .map(|change| {
            (
                (change.key_values.as_slice(), change.column.as_str()),
                &change.value,
            )
        })
        .collect();
    let grid_response = egui::Frame::new()
        .fill(surface(ui))
        .corner_radius(6)
        .stroke(egui::Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            ui.visuals_mut().selection.bg_fill = accent(ui).gamma_multiply(0.18);
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
                    if let Some(row) = tab.grid_scroll_row.take() {
                        builder = builder.scroll_to_row(row, Some(egui::Align::Center));
                    }
                    if let Some((row, _)) = editing_location.filter(|_| inline_editor.as_ref().is_some_and(|editor| editor.focus)) {
                        builder = builder.scroll_to_row(row, Some(egui::Align::Center));
                    }
                    if let Some(index) = tab.insert_focus.and_then(|id| tab.insert_drafts.iter().position(|draft| draft.id == id)) {
                        builder = builder.scroll_to_row(set.rows.len() + index, Some(egui::Align::BOTTOM));
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
                                    let sort = tab.browse.sorts.iter().enumerate().find(|(_, sort)| sort.column == *name);
                                    let filtered = tab.browse.filters.iter().any(|filter| filter.column == *name);
                                    let marker = sort.map(|(index, sort)| {
                                        format!("{}{}{}", if sort.descending { "↓" } else { "↑" },
                                            if tab.browse.sorts.len() > 1 { (index + 1).to_string() } else { String::new() },
                                            if filtered { " •" } else { "" })
                                    }).or_else(|| filtered.then(|| "•".into()));
                                    let response = grid_header_cell_marked(ui, name, sql_type, marker.as_deref());
                                    header_rects.push(response.rect);
                                    let key = if column.is_some_and(|column| column.primary_key) {
                                        " · 主键"
                                    } else {
                                        ""
                                    };
                                    let hint = if target.is_some() {
                                        "单击：升序 / 降序 / 清除 · Shift+单击追加排序\n右键：排序或筛选 · 拖动分隔线调整列宽\n排序在数据库执行，返回第一页；有暂存修改时禁用"
                                    } else {
                                        "拖动表头分隔线调整列宽"
                                    };
                                    let response = response.on_hover_text(format!("{name} · {sql_type}{key}\n{hint}"));
                                    if target.is_some()
                                        && let Some(action) = grid_browse_ui::header_controls(response, name, &tab.browse, can_browse) {
                                        row_action = Some(GridAction::Browse(action));
                                    }
                                });
                            }
                        })
                        .body(|body| {
                            body.rows(32.0, set.rows.len() + tab.insert_drafts.len(), |mut row| {
                                let row_index = row.index();
                                if row_index >= set.rows.len() {
                                    let draft = &mut tab.insert_drafts[row_index - set.rows.len()];
                                    if table_insert_ui::render_draft_row(&mut row, draft, metadata.map_or(&[], |data| data.columns.as_slice()), can_edit, &mut tab.selected_insert, &mut tab.insert_focus) {
                                        remove_insert = Some(draft.id);
                                    }
                                    if tab.selected_insert == Some(draft.id) {
                                        tab.selected_rows = None;
                                        tab.selected_cell = None;
                                        tab.cell_anchor = None;
                                    }
                                    return;
                                }
                                row.set_selected(row_in_selection(tab.selected_rows, row_index));
                                row.col(|ui| {
                                    let response = grid_cell(
                                        ui,
                                        RichText::new((row_index + 1).to_string()).weak(),
                                        false,
                                        false,
                                    );
                                    if response.secondary_clicked() { tab.selected_insert = None; }
                                    if response.clicked() {
                                        tab.selected_insert = None;
                                        tab.selected_rows = Some(row_selection(
                                            tab.selected_rows,
                                            row_index,
                                            ui.input(|i| i.modifiers.shift),
                                        ));
                                        tab.selected_cell = None;
                                        tab.cell_anchor = None;
                                    }
                                    let response = response.on_hover_text(
                                        "单击任意行内容选中整行 · Shift + 单击连续选择多行 · 右键打开行操作 · ⌘/Ctrl+A 全选 · ⌘/Ctrl+C 复制",
                                    );
                                    let deletable = can_edit
                                        && !has_drafts
                                        && tab.table_target.is_some()
                                        && tab
                                            .table_data
                                            .as_ref()
                                            .is_some_and(TableData::can_delete_rows);
                                    if let Some(action) = row_context_menu(
                                        response,
                                        &mut tab.selected_rows,
                                        set,
                                        deletable,
                                        row_index,
                                    ) {
                                        row_action = Some(action);
                                    }
                                });
                                for (column_index, cell) in set.rows[row_index].iter().enumerate() {
                                    let staged_value = metadata.and_then(|data| data.keys.get(row_index))
                                        .and_then(|keys| staged_values.get(&(keys.as_slice(), set.columns[column_index].as_str())))
                                        .copied();
                                    row.col(|ui| {
                                        if editing_location == Some((row_index, column_index))
                                            && let Some(editor) = inline_editor.as_mut()
                                            && !editor.expanded {
                                            let was_focus = editor.focus;
                                            let response = ui.add_enabled(can_edit && !editor.is_null,
                                                egui::TextEdit::singleline(&mut editor.text)
                                                    .id(egui::Id::new(("inline-cell", tab.id)))
                                                    .desired_width(f32::INFINITY)
                                                    .font(egui::TextStyle::Monospace));
                                            if was_focus {
                                                if !editor.is_null { response.request_focus(); }
                                                editor.focus = false;
                                            }
                                            let (escape, enter, next, shift) = ui.input_mut(|input| {
                                                let shift = input.modifiers.shift;
                                                let escape = input.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
                                                let modifiers = if shift { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE };
                                                let enter = input.consume_key(modifiers, egui::Key::Enter);
                                                let next = input.consume_key(modifiers, egui::Key::Tab);
                                                (escape, enter, next, shift)
                                            });
                                            if row_action.is_none() && (escape || (can_edit && (enter || next || (response.lost_focus() && !was_focus && !egui::Popup::is_any_open(ui.ctx()))))) {
                                                    row_action = Some(GridAction::FinishCell { cancel: escape, movement: if next {
                                                        CellMovement::Across(if shift { -1 } else { 1 })
                                                    } else if enter { CellMovement::Down(if shift { -1 } else { 1 }) }
                                                    else { CellMovement::Stay } });
                                            }
                                            return;
                                        }
                                        let displayed = staged_value.unwrap_or(cell);
                                        let value = displayed.as_deref().unwrap_or("NULL");
                                        let text = if staged_value.is_some() {
                                            RichText::new(format!("● {value}"))
                                                .strong().color(accent(ui))
                                        } else if displayed.is_none() {
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
                                            tab.selected_insert = None;
                                            let (extend, select_cell) = ui.input(|i| {
                                                (
                                                    i.modifiers.shift,
                                                    i.modifiers.command || i.modifiers.ctrl,
                                                )
                                            });
                                            if select_cell {
                                                if !extend || tab.selected_cell.is_none() {
                                                    tab.cell_anchor =
                                                        Some((row_index, column_index));
                                                }
                                                tab.selected_cell = Some((row_index, column_index));
                                                tab.selected_rows = None;
                                            } else {
                                                tab.selected_rows = Some(row_selection(
                                                    tab.selected_rows,
                                                    row_index,
                                                    extend,
                                                ));
                                                tab.selected_cell = Some((row_index, column_index));
                                                tab.cell_anchor = Some((row_index, column_index));
                                            }
                                        }
                                        let editable = inline_editor.is_none() && grid_cell_eligible(can_edit, target, metadata, set, row_index, column_index);
                                        let begin_edit = response.double_clicked() && editable && inline_editor.is_none();
                                        let mut menu_edit = false;
                                        let hint = if staged_value.is_some() {
                                            format!("暂存值：{displayed:?}\n数据库快照：{cell:?}\n复制和导出使用数据库快照；双击可继续编辑")
                                        } else if editable {
                                            format!(
                                                "{value}
双击 / F2 / Enter 编辑 · ⌘/Ctrl+Shift+单击选择区域 · 粘贴仅暂存，提交后写入"
                                            )
                                        } else if target.is_none() {
                                            format!(
                                                "{value}
SQL 查询结果只读；双击左侧表名打开可编辑的表数据"
                                            )
                                        } else {
                                            value.to_owned()
                                        };
                                        if response.secondary_clicked() { tab.selected_insert = None; }
                                        if response.secondary_clicked()
                                            && !cell_in_selection(tab.cell_anchor, tab.selected_cell, row_index, column_index)
                                            && !row_in_selection(tab.selected_rows, row_index)
                                        {
                                            tab.selected_insert = None;
                                            tab.selected_rows = Some((row_index, row_index));
                                            tab.selected_cell = None;
                                            tab.cell_anchor = None;
                                        }
                                        let response = response.on_hover_text(hint);
                                        response.context_menu(|ui| {
                                            if cell_in_selection(tab.cell_anchor, tab.selected_cell, row_index, column_index)
                                                && let Some((end_row, end_column)) = tab.selected_cell {
                                                let (start_row, start_column) = tab.cell_anchor.unwrap_or((end_row, end_column));
                                                let count = (start_row.abs_diff(end_row) + 1) * (start_column.abs_diff(end_column) + 1);
                                                if count > 1 { ui.label(format!("操作选中 {count} 格")); }
                                            }
                                            if target.is_some() {
                                                let filterable = metadata.and_then(|data| data.columns.get(column_index))
                                                    .is_some_and(|column| column.kind != sqlshop::model::CellKind::Unsupported);
                                                let text_column = metadata.and_then(|data| data.columns.get(column_index))
                                                    .is_some_and(|column| column.kind == sqlshop::model::CellKind::Text);
                                                if let Some(action) = grid_browse_ui::cell_filter_controls(ui, &set.columns[column_index], cell, text_column, can_browse && filterable) {
                                                    row_action = Some(GridAction::Browse(action));
                                                }
                                                ui.separator();
                                            }
                                            if ui.add_enabled(editable, egui::Button::new("设为 NULL"))
                                                .on_hover_text("仅暂存此格为 NULL，提交全部修改后写入数据库").clicked() {
                                                draft_action = Some((row_index, column_index, CellDraftAction::SetNull));
                                                ui.close();
                                            }
                                            let text_column = metadata.and_then(|data| data.columns.get(column_index))
                                                .is_some_and(|column| column.kind == sqlshop::model::CellKind::Text);
                                            if ui.add_enabled(editable && text_column, egui::Button::new("设为空字符串"))
                                                .on_hover_text("仅暂存此文本格为空字符串；空字符串与 NULL 不同").clicked() {
                                                draft_action = Some((row_index, column_index, CellDraftAction::SetEmptyString));
                                                ui.close();
                                            }
                                            if ui.add_enabled(editable && staged_value.is_some(), egui::Button::new("撤销此格修改"))
                                                .on_hover_text("恢复此格的数据库快照值，保留其他格的暂存修改").clicked() {
                                                draft_action = Some((row_index, column_index, CellDraftAction::Revert));
                                                ui.close();
                                            }
                                            ui.separator();
                                            let rows = tab
                                                .selected_rows
                                                .map(|(a, b)| {
                                                    (a.min(b)..=a.max(b)).collect::<Vec<_>>()
                                                })
                                                .unwrap_or_else(|| vec![row_index]);
                                            if ui
                                                .button(format!(
                                                    "复制选中行 ({} 行, TSV)",
                                                    rows.len()
                                                ))
                                                .clicked()
                                            {
                                                let selected = ResultSet {
                                                    rows: rows
                                                        .iter()
                                                        .filter_map(|index| {
                                                            set.rows.get(*index).cloned()
                                                        })
                                                        .collect(),
                                                    ..set.clone()
                                                };
                                                ui.ctx().copy_text(selected_tsv(
                                                    &selected,
                                                    Some((0, 0)),
                                                    Some((
                                                        selected.rows.len().saturating_sub(1),
                                                        selected.columns.len().saturating_sub(1),
                                                    )),
                                                ));
                                                ui.close();
                                            }
                                            if ui
                                                .button(format!(
                                                    "导出选中行 CSV… ({} 行)",
                                                    rows.len()
                                                ))
                                                .clicked()
                                            {
                                                row_action =
                                                    Some(GridAction::ExportRows(rows.clone()));
                                                ui.close();
                                            }
                                            let deletable = can_edit
                                        && !has_drafts
                                                && tab.table_target.is_some()
                                                && tab.table_data.as_ref().is_some_and(|data| {
                                                    data.can_delete_rows()
                                                });
                                            if ui
                                                .add_enabled(
                                                    deletable,
                                                    egui::Button::new(format!(
                                                        "删除选中行… ({} 行)",
                                                        rows.len()
                                                    )),
                                                )
                                                .clicked()
                                            {
                                                row_action = Some(GridAction::DeleteRows(rows));
                                                ui.close();
                                            }
                                            if ui.button("复制单元格").clicked() {
                                                ui.ctx().copy_text(cell.as_deref().unwrap_or("NULL").into());
                                                ui.close();
                                            }
                                            if ui
                                                .add_enabled(
                                                    editable && inline_editor.is_none(),
                                                    egui::Button::new("编辑单元格"),
                                                )
                                                .clicked()
                                            {
                                                menu_edit = true;
                                                ui.close();
                                            }
                                        });
                                        if begin_edit || menu_edit {
                                            edit_cell = Some((row_index, column_index));
                                        }
                                    });
                                }
                            });
                        });
                    resize_grid_columns(ui, widths_id, &header_rects, &mut widths);
                    ui.data_mut(|data| data.insert_temp(widths_id, widths));
                });
        });
    let pointer_over_grid = ui
        .ctx()
        .pointer_latest_pos()
        .is_some_and(|position| grid_response.response.rect.contains(position));
    let owner_id = egui::Id::new("grid-keyboard-owner");
    if pointer_over_grid && ui.input(|input| input.pointer.any_pressed()) {
        ui.data_mut(|data| data.insert_temp(owner_id, tab.id));
    }
    let keyboard_shortcuts = !ui.ctx().wants_keyboard_input()
        && (pointer_over_grid || ui.data(|data| data.get_temp::<Uuid>(owner_id)) == Some(tab.id));
    let (select_all, copy) = ui.ctx().input(|input| {
        (
            keyboard_shortcuts && input.modifiers.command && input.key_pressed(egui::Key::A),
            keyboard_shortcuts && input.modifiers.command && input.key_pressed(egui::Key::C),
        )
    });
    if keyboard_shortcuts
        && can_edit
        && inline_editor.is_none()
        && tab.selected_cell.is_some()
        && let Some(text) = ui.input(|input| {
            input.events.iter().find_map(|event| {
                if let egui::Event::Paste(text) = event {
                    Some(text.clone())
                } else {
                    None
                }
            })
        })
    {
        row_action = Some(GridAction::PasteValues(text));
    }
    if select_all && !set.rows.is_empty() {
        tab.selected_insert = None;
        tab.selected_rows = Some((0, set.rows.len() - 1));
        tab.selected_cell = None;
        tab.cell_anchor = None;
    }
    if copy {
        let text = if let Some((anchor, end)) = tab.selected_rows {
            selected_tsv(
                set,
                Some((anchor, 0)),
                Some((end, set.columns.len().saturating_sub(1))),
            )
        } else {
            selected_tsv(set, tab.cell_anchor, tab.selected_cell)
        };
        if !text.is_empty() {
            ui.ctx().copy_text(text);
        }
    }
    if ui.is_enabled()
        && keyboard_shortcuts
        && !egui::Popup::is_any_open(ui.ctx())
        && inline_editor.is_none()
        && let Some((row, column)) = tab.selected_cell
    {
        let (horizontal, vertical, home, end, extend) = ui.input_mut(|input| {
            let modifiers = if input.modifiers.shift {
                egui::Modifiers::SHIFT
            } else {
                egui::Modifiers::NONE
            };
            (
                i32::from(input.consume_key(modifiers, egui::Key::ArrowRight))
                    - i32::from(input.consume_key(modifiers, egui::Key::ArrowLeft)),
                i32::from(input.consume_key(modifiers, egui::Key::ArrowDown))
                    - i32::from(input.consume_key(modifiers, egui::Key::ArrowUp)),
                input.consume_key(modifiers, egui::Key::Home),
                input.consume_key(modifiers, egui::Key::End),
                input.modifiers.shift,
            )
        });
        if horizontal != 0 || vertical != 0 || home || end {
            let next_row = if home {
                0
            } else if end {
                set.rows.len().saturating_sub(1)
            } else {
                row.saturating_add_signed(vertical as isize)
                    .min(set.rows.len().saturating_sub(1))
            };
            let next_column = column
                .saturating_add_signed(horizontal as isize)
                .min(set.columns.len().saturating_sub(1));
            tab.selected_cell = Some((next_row, next_column));
            tab.grid_scroll_row = Some(next_row);
            if !extend {
                tab.cell_anchor = tab.selected_cell;
            }
            tab.selected_rows = None;
        }
    }
    if ui.is_enabled()
        && !ui.ctx().wants_keyboard_input()
        && !egui::Popup::is_any_open(ui.ctx())
        && can_edit
        && inline_editor.is_none()
        && let Some(selected) = tab.selected_cell
        && ui.input_mut(|input| {
            input.consume_key(egui::Modifiers::NONE, egui::Key::F2)
                || input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
        })
    {
        edit_cell = Some(selected);
    }
    if let Some((row, column, action)) = draft_action
        && let Err(message) = apply_grid_selection_draft(tab, row, column, action, can_edit)
    {
        row_action = Some(GridAction::DraftError(message));
    }
    if let Some(id) = remove_insert {
        tab.insert_drafts.retain(|draft| draft.id != id);
        if tab.selected_insert == Some(id) {
            tab.selected_insert = None;
        }
        if tab.insert_focus == Some(id) {
            tab.insert_focus = None;
        }
    }
    let editor = edit_cell.and_then(|(row, column)| grid_cell_editor(tab, row, column, can_edit));
    (editor, row_action)
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
    grid_header_cell_marked(ui, name, sql_type, None)
}

fn grid_header_cell_marked(
    ui: &mut egui::Ui,
    name: &str,
    sql_type: &str,
    marker: Option<&str>,
) -> egui::Response {
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .rect_filled(rect, 0, accent(ui).gamma_multiply(0.07));
    let response = ui.allocate_rect(rect, egui::Sense::click());
    let mut text_rect = rect.shrink2(egui::vec2(8.0, 5.0));
    if let Some(marker) = marker {
        let font = egui::FontId::proportional(12.0);
        let width = ui
            .painter()
            .layout_no_wrap(marker.into(), font.clone(), accent(ui))
            .size()
            .x;
        text_rect.max.x = (text_rect.max.x - width - 6.0).max(text_rect.min.x);
        ui.painter().text(
            rect.right_top() + egui::vec2(-8.0, 7.0),
            egui::Align2::RIGHT_TOP,
            marker,
            font,
            accent(ui),
        );
    }
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(text_rect)
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
    let (_, response) = ui.allocate_exact_size(rect.size(), egui::Sense::click());
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (rect.width() - 16.0).max(1.0),
        egui::TextStyle::Body,
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y * 0.5),
        galley,
        ui.visuals().text_color(),
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

fn sql_editor(
    ui: &mut egui::Ui,
    tab: &mut QueryTab,
    height: &mut Option<f32>,
    show_results: bool,
    columns: &[String],
    tables: &[String],
) -> (egui::Response, bool) {
    let minimum = if ui.available_height() < 320.0 {
        60.0
    } else {
        100.0
    };
    let maximum = (ui.available_height() - 240.0).max(minimum);
    let editor_height = if show_results {
        height
            .unwrap_or(ui.available_height() * 0.45)
            .clamp(minimum, maximum)
    } else {
        (ui.available_height() - 2.0).max(100.0)
    };
    let mut needs_columns = false;
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
                    let editor_id = ui.make_persistent_id(egui::Id::new(tab.id));
                    let completion_key = if ui.is_enabled()
                        && !tab.find_open
                        && ui.ctx().memory(|m| m.has_focus(editor_id))
                    {
                        egui::TextEdit::load_state(ui.ctx(), editor_id)
                            .and_then(|state| state.cursor.char_range())
                            .filter(|range| range.is_empty())
                            .and_then(|range| {
                                completion_word(&tab.sql, range.primary.index)
                                    .map(|word| (tab.sql.clone(), range.primary.index, word))
                            })
                            .filter(|(sql, cursor, _)| {
                                tab.completion_dismissed.as_ref() != Some(&(sql.clone(), *cursor))
                            })
                    } else {
                        None
                    };
                    let accepting = completion_key.as_ref().is_some_and(|(_, cursor, word)| {
                        !completions(
                            &tab.sql[word.start..char_to_byte_index(&tab.sql, *cursor)],
                            columns,
                            tables,
                        )
                        .is_empty()
                    });
                    let completion_events = if accepting {
                        ui.input(|input| {
                            input
                                .events
                                .iter()
                                .filter_map(|event| {
                                    if let egui::Event::Key {
                                        key,
                                        pressed: true,
                                        modifiers,
                                        ..
                                    } = event
                                        && matches!(
                                            key,
                                            egui::Key::Tab
                                                | egui::Key::Enter
                                                | egui::Key::ArrowUp
                                                | egui::Key::ArrowDown
                                                | egui::Key::Escape
                                        )
                                        && !modifiers.command
                                        && !modifiers.ctrl
                                        && !modifiers.alt
                                        && !modifiers.shift
                                    {
                                        return Some(*key);
                                    }
                                    None
                                })
                                .collect::<Vec<_>>()
                        })
                    } else {
                        Vec::new()
                    };
                    if accepting {
                        for key in &completion_events {
                            ui.input_mut(|input| {
                                input.consume_key(egui::Modifiers::NONE, *key);
                            });
                        }
                    }
                    let mut output = egui::TextEdit::multiline(&mut tab.sql)
                        .id(editor_id)
                        .layouter(&mut layouter)
                        .font(egui::TextStyle::Monospace)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(1)
                        .frame(false)
                        .show(ui);
                    if tab.focus_editor && ui.is_enabled() {
                        output.response.request_focus();
                        tab.focus_editor = false;
                    }
                    if let Some(range) = tab.select_range.take() {
                        let selected = egui::text::CCursorRange::two(
                            egui::text::CCursor::new(byte_to_char_index(&tab.sql, range.start)),
                            egui::text::CCursor::new(byte_to_char_index(&tab.sql, range.end)),
                        );
                        output.state.cursor.set_char_range(Some(selected));
                        output.state.clone().store(ui.ctx(), output.response.id);
                        output.response.request_focus();
                    }
                    let cursor_range = output.state.cursor.char_range();
                    if ui.is_enabled()
                        && output.response.has_focus()
                        && !tab.find_open
                        && let Some(range) = cursor_range
                            .filter(|range| range.is_empty())
                            .and_then(|range| completion_word(&tab.sql, range.primary.index))
                    {
                        let cursor = cursor_range.unwrap().primary.index;
                        let prefix = &tab.sql[range.start..char_to_byte_index(&tab.sql, cursor)];
                        needs_columns =
                            tab.completion_dismissed.as_ref() != Some(&(tab.sql.clone(), cursor));
                        let choices = completions(prefix, columns, tables);
                        let key = (tab.sql.clone(), cursor);
                        if tab.completion_dismissed.as_ref() != Some(&key) && !choices.is_empty() {
                            tab.completion_selected =
                                tab.completion_selected.min(choices.len() - 1);
                            if completion_events.contains(&egui::Key::ArrowDown) {
                                tab.completion_selected =
                                    (tab.completion_selected + 1) % choices.len();
                            } else if completion_events.contains(&egui::Key::ArrowUp) {
                                tab.completion_selected =
                                    (tab.completion_selected + choices.len() - 1) % choices.len();
                            }
                            let mut chosen = None;
                            let cursor_rect = output
                                .galley
                                .pos_from_cursor(egui::text::CCursor::new(cursor))
                                .translate(output.galley_pos.to_vec2());
                            let popup = egui::Popup::from_response(&output.response)
                                .anchor(cursor_rect)
                                .id(egui::Id::new(("sql-completion", tab.id)))
                                .open(true)
                                .show(|ui| {
                                    ui.set_min_width(200.0);
                                    for (index, (name, kind)) in choices.iter().enumerate() {
                                        let label = format!("{}  · {}", name, kind.label());
                                        if ui
                                            .selectable_label(
                                                index == tab.completion_selected,
                                                label,
                                            )
                                            .clicked()
                                        {
                                            chosen = Some(index);
                                        }
                                    }
                                });
                            let _ = popup;
                            if chosen.is_none()
                                && (completion_events.contains(&egui::Key::Tab)
                                    || completion_events.contains(&egui::Key::Enter))
                            {
                                chosen = Some(tab.completion_selected);
                            }
                            if completion_events.contains(&egui::Key::Escape) {
                                tab.completion_dismissed = Some(key);
                            } else if let Some(index) = chosen
                                && let Some(cursor) =
                                    insert_completion(&mut tab.sql, range, &choices[index].0)
                            {
                                output.state.cursor.set_char_range(Some(
                                    egui::text::CCursorRange::one(egui::text::CCursor::new(cursor)),
                                ));
                                output.state.clone().store(ui.ctx(), output.response.id);
                                output.response.request_focus();
                                tab.completion_dismissed = Some((tab.sql.clone(), cursor));
                            }
                        }
                    }
                    tab.cursor = output
                        .state
                        .cursor
                        .char_range()
                        .map_or(0, |range| range.primary.index);
                    tab.selection_char_start = output
                        .state
                        .cursor
                        .char_range()
                        .filter(|range| !range.is_empty())
                        .map(|range| range.primary.index.min(range.secondary.index));
                    tab.selection = output
                        .state
                        .cursor
                        .char_range()
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
        return (editor.response, needs_columns);
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
    (response, needs_columns)
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
        .position(|tab| tab.design.is_none() && tab.table_target.as_ref() == Some(&target))
    {
        tabs[index].table_kind = Some(table.kind.clone());
        return (index, false);
    }
    let mut tab = QueryTab::new(*next_tab);
    tab.name = table.name.clone();
    tab.sql = engine.preview_sql(table);
    tab.target = Some(QueryTarget {
        schema: (engine == Engine::Postgres).then(|| target.schema.clone()),
        connection: target.connection,
        database: Some(target.database.clone()),
    });
    tab.saved_sql = tab.sql.clone();
    tab.table_target = Some(target);
    tab.table_kind = Some(table.kind.clone());
    *next_tab += 1;
    tabs.push(tab);
    (tabs.len() - 1, true)
}

fn row_tail_response(
    ui: &mut egui::Ui,
    row: &egui::Response,
    id: impl std::hash::Hash,
) -> Option<egui::Response> {
    let rect = egui::Rect::from_min_max(
        egui::pos2(row.rect.right(), row.rect.top()),
        egui::pos2(ui.max_rect().right(), row.rect.bottom()),
    );
    if rect.width() <= 1.0 || rect.height() <= 1.0 {
        return None;
    }
    Some(
        ui.interact(rect, ui.id().with(id), egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand),
    )
}

#[allow(clippy::too_many_arguments)]
fn database_context_menu(
    ui: &mut egui::Ui,
    connection: Uuid,
    engine: Engine,
    database: &str,
    active: bool,
    busy: bool,
    protected_database: bool,
    schema_dialog: &mut Option<SchemaDialog>,
    table_create: &mut Option<TableCreateForm>,
    to_select: &mut Option<(Uuid, String)>,
    refresh_tables: &mut Option<Uuid>,
) {
    if ui
        .add_enabled(!busy && active, egui::Button::new("新建数据表…"))
        .clicked()
    {
        *table_create = Some(new_table_create_form(
            connection,
            database.to_owned(),
            match engine {
                Engine::MySql => database.to_owned(),
                Engine::Postgres => "public".into(),
                Engine::SqlServer => "dbo".into(),
            },
            engine,
        ));
        ui.close();
    }
    if ui
        .add_enabled(
            !busy && !active && !protected_database,
            egui::Button::new("删除数据库…"),
        )
        .clicked()
    {
        *schema_dialog = Some(SchemaDialog {
            action: SchemaDialogAction::DropDatabase {
                connection,
                database: database.to_owned(),
            },
            name: String::new(),
        });
        ui.close();
    }
    ui.separator();
    if ui
        .add_enabled(!busy && !active, egui::Button::new("选择数据库"))
        .clicked()
    {
        *to_select = Some((connection, database.to_owned()));
        ui.close();
    }
    if ui
        .add_enabled(active && !busy, egui::Button::new("刷新表和视图"))
        .clicked()
    {
        *refresh_tables = Some(connection);
        ui.close();
    }
}

#[allow(clippy::too_many_arguments)]
fn connection_context_menu(
    ui: &mut egui::Ui,
    connection: Uuid,
    connected: bool,
    busy: bool,
    catalog_writable: bool,
    current_group: Option<Uuid>,
    buckets: &[(Option<Uuid>, String)],
    schema_dialog: &mut Option<SchemaDialog>,
    disconnect: &mut Option<Uuid>,
    refresh_databases: &mut Option<Uuid>,
    to_connect: &mut Option<Uuid>,
    move_connection: &mut Option<(Uuid, Option<Uuid>)>,
    to_edit: &mut Option<Uuid>,
    to_delete: &mut Option<Uuid>,
) {
    if connected {
        if ui
            .add_enabled(!busy, egui::Button::new("新建数据库…"))
            .clicked()
        {
            *schema_dialog = Some(SchemaDialog {
                action: SchemaDialogAction::CreateDatabase { connection },
                name: String::new(),
            });
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled(!busy, egui::Button::new("断开连接"))
            .clicked()
        {
            *disconnect = Some(connection);
            ui.close();
        }
        if ui
            .add_enabled(!busy, egui::Button::new("刷新数据库列表"))
            .clicked()
        {
            *refresh_databases = Some(connection);
            ui.close();
        }
    } else if ui
        .add_enabled(!busy, egui::Button::new("连接服务器"))
        .clicked()
    {
        *to_connect = Some(connection);
        ui.close();
    }
    ui.separator();
    ui.add_enabled_ui(!busy && catalog_writable, |ui| {
        ui.menu_button("移动到分组", |ui| {
            for (id, name) in buckets {
                if ui.selectable_label(current_group == *id, name).clicked() {
                    *move_connection = Some((connection, *id));
                    ui.close();
                }
            }
        });
    });
    if ui
        .add_enabled(!busy, egui::Button::new("编辑连接"))
        .clicked()
    {
        *to_edit = Some(connection);
        ui.close();
    }
    if ui
        .add_enabled(!busy, egui::Button::new("删除连接…"))
        .clicked()
    {
        *to_delete = Some(connection);
        ui.close();
    }
}

fn table_buttons(
    ui: &mut egui::Ui,
    tables: &[&Table],
    selection: &mut TreeSelection,
    connection: Uuid,
    database: &str,
    busy: bool,
    schema_dialog: &mut Option<SchemaDialog>,
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
        response.context_menu(|ui| {
            if ui
                .add_enabled(
                    !busy && !table.kind.contains("VIEW"),
                    egui::Button::new("删除数据表…"),
                )
                .clicked()
            {
                *schema_dialog = Some(SchemaDialog {
                    action: SchemaDialogAction::DropTable {
                        connection,
                        database: database.to_owned(),
                        table: (*table).clone(),
                    },
                    name: String::new(),
                });
                ui.close();
            }
        });
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
    if form.profile.engine == Engine::SqlServer {
        use sqlshop::model::SqlServerAuth;
        ui.horizontal_wrapped(|ui| {
            ui.label("认证");
            for (method, label) in [
                (SqlServerAuth::Password, "SQL Server 密码"),
                (SqlServerAuth::AadToken, "Entra 令牌"),
            ] {
                ui.selectable_value(&mut form.profile.sqlserver_auth, method, label);
            }
            #[cfg(windows)]
            for (method, label) in [
                (SqlServerAuth::Integrated, "当前 Windows 用户"),
                (SqlServerAuth::Windows, "Windows 凭据"),
            ] {
                ui.selectable_value(&mut form.profile.sqlserver_auth, method, label);
            }
        });
        ui.horizontal(|ui| {
            ui.label("命名实例");
            ui.add(
                egui::TextEdit::singleline(&mut form.profile.sqlserver_instance)
                    .hint_text("可选；使用 SQL Browser UDP 1434 发现端口"),
            );
        });
        if form.profile.sqlserver_auth == sqlshop::model::SqlServerAuth::AadToken {
            ui.horizontal(|ui| {
                ui.label("初始数据库");
                ui.add(
                    egui::TextEdit::singleline(&mut form.profile.database)
                        .hint_text("可选；令牌仅能访问特定数据库时填写"),
                );
            });
        }
    }
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
            ui.label(
                if form.profile.engine == Engine::SqlServer
                    && form.profile.sqlserver_auth == sqlshop::model::SqlServerAuth::AadToken
                {
                    "访问令牌"
                } else {
                    "密码"
                },
            );
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
        if form.profile.engine == Engine::SqlServer
            && form.profile.sqlserver_auth == sqlshop::model::SqlServerAuth::AadToken
        {
            "记住访问令牌（加密保存；到期后需更新）"
        } else {
            "记住数据库密码（本机保存）"
        },
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

/// Modal-style dialog window. The pivot is the window center and the default
/// position is the content-area center, so dialogs open centered instead of
/// at the top-left egui default; once the user drags a dialog, egui remembers
/// that position per window id as usual.
fn dialog_window(title: impl Into<String>, ctx: &egui::Context) -> egui::Window<'static> {
    egui::Window::new(title.into())
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
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
        Color32::from_rgb(250, 251, 252)
    }
}

fn canvas(ctx: &egui::Context) -> Color32 {
    if ctx.style().visuals.dark_mode {
        Color32::from_rgb(15, 21, 27)
    } else {
        Color32::from_rgb(234, 239, 242)
    }
}

fn surface(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(25, 34, 42)
    } else {
        Color32::from_rgb(255, 255, 255)
    }
}

fn apply_theme(ctx: &egui::Context) {
    ctx.style_mut(|style| {
        let dark = style.visuals.dark_mode;
        let text = if dark {
            Color32::from_rgb(230, 236, 240)
        } else {
            Color32::from_rgb(30, 40, 48)
        };
        style.visuals.weak_text_color = Some(if dark {
            Color32::from_rgb(170, 184, 194)
        } else {
            Color32::from_rgb(71, 84, 94)
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
            Color32::from_rgb(229, 235, 238)
        };
        style.visuals.selection.bg_fill = accent.gamma_multiply(0.38);
        style.visuals.selection.stroke.color = accent;
        style.visuals.hyperlink_color = accent;
        style.visuals.widgets.inactive.bg_fill = control;
        style.visuals.widgets.inactive.weak_bg_fill = control;
        style.visuals.widgets.noninteractive.bg_fill = if dark {
            Color32::from_rgb(25, 34, 42)
        } else {
            Color32::WHITE
        };
        style.visuals.widgets.noninteractive.weak_bg_fill =
            style.visuals.widgets.noninteractive.bg_fill;
        style.visuals.widgets.hovered.bg_fill = if dark {
            Color32::from_rgb(46, 61, 68)
        } else {
            Color32::from_rgb(211, 226, 224)
        };
        style.visuals.widgets.active.bg_fill = accent.gamma_multiply(if dark { 0.5 } else { 0.25 });
        style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(
            1.0,
            if dark {
                Color32::from_rgb(58, 70, 79)
            } else {
                Color32::from_rgb(190, 201, 207)
            },
        );
        style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(
            1.0,
            if dark {
                Color32::from_rgb(58, 70, 79)
            } else {
                Color32::from_rgb(184, 196, 202)
            },
        );
        style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, accent);
        style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, accent);
        style.visuals.window_fill = if dark {
            Color32::from_rgb(25, 34, 42)
        } else {
            Color32::from_rgb(250, 251, 252)
        };
        style.visuals.panel_fill = if dark {
            Color32::from_rgb(15, 21, 27)
        } else {
            Color32::from_rgb(234, 239, 242)
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

    #[test]
    fn table_draft_overlay_preserves_snapshot_null_empty_and_primary_key() {
        for new_note in [None, Some(String::new()), Some("changed".into())] {
            let (mut app, _, mut change) = table_flow_fixture();
            change.value = new_note.clone();
            let tab = &mut app.tabs[0];
            tab.stage_cell(CellUpdate {
                column: "id".into(),
                original: Some("7".into()),
                value: Some("8".into()),
                ..change.clone()
            });
            tab.stage_cell(change);
            let output = app.context.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    result_grid(ui, tab, true);
                });
            });
            let labels = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert!(labels.contains(&"● 8"), "{labels:?}");
            let expected = format!("● {}", new_note.as_deref().unwrap_or("NULL"));
            assert!(labels.contains(&expected.as_str()), "{labels:?}");
            let snapshot = &tab.output.as_ref().unwrap().sets[0];
            assert_eq!(
                snapshot.rows[0],
                [Some("7".into()), Some("original".into())]
            );
            assert_eq!(tab.table_data.as_ref().unwrap().keys[0], [Some("7".into())]);
            assert_eq!(
                selected_tsv(snapshot, Some((0, 0)), Some((0, 1))),
                "7\toriginal"
            );
            assert!(app.discard_staged_cells(0));
            assert!(app.tabs[0].staged_cells.is_empty());
        }
    }

    #[test]
    fn table_drafts_cannot_be_discarded_during_commit_or_cell_editing() {
        let (mut app, target, change) = table_flow_fixture();
        app.tabs[0].stage_cell(change.clone());
        app.sessions.get_mut(&target.connection).unwrap().busy = true;
        assert!(!app.discard_staged_cells(0));
        assert_eq!(app.tabs[0].staged_cells.len(), 1);
        app.handle_response(
            target.connection,
            Response::OperationError {
                tab: app.tabs[0].id,
                message: "conflict rolled back".into(),
                disconnected: false,
            },
        );
        assert_eq!(app.tabs[0].staged_cells.len(), 1);
        app.cell_editor = Some(CellEditor {
            tab: app.tabs[0].id,
            connection: target.connection,
            change: change.clone(),
            text: "editing".into(),
            is_null: false,
            focus: false,
            expanded: false,
        });
        assert!(!app.discard_staged_cells(0));
        app.request_on(
            target.connection,
            Request::SaveCells {
                tab: app.tabs[0].id,
                target: target.clone(),
                changes: vec![change],
            },
            "save",
        );
        assert!(!app.session_busy(target.connection));
        assert_eq!(app.tabs[0].staged_cells.len(), 1);
        app.cell_editor = None;
        assert!(app.discard_staged_cells(0));
    }

    #[test]
    fn table_expanded_filters_cannot_change_applied_conditions_with_drafts() {
        let (mut app, target, change) = table_flow_fixture();
        app.tabs[0].filters_open = true;
        app.tabs[0].browse.filters = vec![sqlshop::db::ColumnFilter {
            column: "id".into(),
            operator: sqlshop::db::FilterOperator::Eq,
            value: Some("7".into()),
        }];
        app.tabs[0].browse.offset = 200;
        let original = app.tabs[0].browse.clone();
        app.tabs[0].stage_cell(change);
        let ctx = app.context.clone();
        let mut frame = |time, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 820.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.table_controls(ui));
                },
            )
        };
        let output = frame(0.0, vec![]);
        let positions = ["应用筛选与排序", "重置"].map(|label| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
                    }
                    _ => None,
                })
                .expect("expanded filter buttons should render")
        });
        for (index, position) in positions.into_iter().enumerate() {
            for pressed in [true, false] {
                let _ = frame(
                    1.0 + index as f64 + f64::from(!pressed) * 0.1,
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
            }
        }
        assert_eq!(app.tabs[0].browse, original);
        assert_eq!(app.tabs[0].staged_cells.len(), 1);
        assert!(!app.session_busy(target.connection));
    }

    #[test]
    fn table_refresh_failure_preserves_snapshot_and_blocks_every_data_write() {
        let (mut app, target, change) = table_flow_fixture();
        let tab_id = app.tabs[0].id;
        app.load_table(0);
        assert!(app.tabs[0].output.is_some());
        assert!(app.tabs[0].table_data.is_some());
        assert!(app.tabs[0].table_snapshot_stale);
        app.handle_response(
            target.connection,
            Response::OperationError {
                tab: tab_id,
                message: "read failed".into(),
                disconnected: false,
            },
        );
        assert!(app.status.contains("保留上次快照"));
        assert!(!app.tabs[0].table_read_pending);
        assert_eq!(
            app.tabs[0].output.as_ref().unwrap().sets[0].rows[0][1].as_deref(),
            Some("original")
        );
        for request in [
            Request::SaveCell {
                tab: tab_id,
                target: target.clone(),
                change: change.clone(),
            },
            Request::SaveCells {
                tab: tab_id,
                target: target.clone(),
                changes: vec![change.clone()],
            },
            Request::DeleteRows {
                tab: tab_id,
                target: target.clone(),
                table: change.table.clone(),
                primary_keys: change.primary_keys.clone(),
                key_values: vec![change.key_values.clone()],
            },
            Request::InsertDefaultRow {
                tab: tab_id,
                target: target.clone(),
                table: change.table.clone(),
            },
            Request::InsertRows {
                tab: tab_id,
                target: target.clone(),
                table: change.table.clone(),
                columns: vec!["note".into()],
                rows: vec![vec![InsertValue::Text("new".into())]],
            },
        ] {
            app.request_on(target.connection, request, "write");
            assert!(!app.session_busy(target.connection));
            assert!(app.status.contains("表数据已过期"));
        }
        app.cell_editor = Some(CellEditor {
            tab: tab_id,
            connection: target.connection,
            change,
            text: "must not stage".into(),
            is_null: false,
            focus: false,
            expanded: false,
        });
        let ctx = app.context.clone();
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                }],
                ..Default::default()
            },
            |ctx| app.cell_editor_window(ctx),
        );
        assert!(app.tabs[0].staged_cells.is_empty());
        assert!(app.cell_editor.is_some());
        app.cell_editor = None;
        app.load_table(0);
        app.handle_response(
            target.connection,
            Response::TablePage {
                tab: tab_id,
                page: table_flow_page(),
            },
        );
        assert!(!app.tabs[0].table_snapshot_stale);
        assert!(!app.tabs[0].table_read_pending);
    }

    #[test]
    fn table_commit_success_then_refresh_failure_keeps_success_evidence() {
        let (mut app, target, change) = table_flow_fixture();
        let tab_id = app.tabs[0].id;
        app.tabs[0].stage_cell(change);
        app.handle_response(
            target.connection,
            Response::CellsSaved {
                tab: tab_id,
                count: 1,
            },
        );
        assert!(app.tabs[0].staged_cells.is_empty());
        assert!(app.tabs[0].table_snapshot_stale);
        assert!(app.tabs[0].table_read_pending);
        app.handle_response(
            target.connection,
            Response::OperationError {
                tab: tab_id,
                message: "refresh unavailable".into(),
                disconnected: false,
            },
        );
        assert!(app.status.contains("已成功提交 1 格修改"));
        assert!(app.status.contains("刷新失败"));
        assert!(app.tabs[0].table_snapshot_stale);
        assert!(app.tabs[0].table_write_notice.is_some());
        app.load_table(0);
        app.handle_response(
            target.connection,
            Response::TablePage {
                tab: tab_id,
                page: table_flow_page(),
            },
        );
        assert!(app.tabs[0].table_write_notice.is_none());
        assert!(!app.tabs[0].table_snapshot_stale);
    }

    #[test]
    fn table_draft_details_start_collapsed_and_top_toolbar_has_no_write_buttons() {
        let (mut app, _, change) = table_flow_fixture();
        app.tabs[0].stage_cell(change);
        let mut height = 0.0;
        let ctx = app.context.clone();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                height = ui.scope(|ui| app.table_controls(ui)).response.rect.height();
            });
        });
        let labels = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(height <= 76.0, "collapsed table toolbar used {height}px");
        assert!(labels.iter().any(|label| label.contains("查看修改")));
        assert!(!labels.contains(&"提交全部"));
        assert!(!labels.contains(&"撤销暂存"));
        assert!(!labels.contains(&"刷新"));
        assert!(
            !labels
                .iter()
                .any(|label| label.contains("original") || label.contains("changed"))
        );
    }

    #[test]
    fn table_footer_controls_remain_visible_at_narrow_workspace_widths() {
        for width in [406.0, 554.0, 934.0] {
            let (mut app, _, change) = table_flow_fixture();
            app.tabs[0].stage_cell(change);
            app.tabs[0].browse.offset = 200;
            app.tabs[0].has_more = true;
            let ctx = app.context.clone();
            install_font(&ctx);
            let available =
                egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(width, 450.0));
            let (content, footer) = table_workspace_rects(available);
            assert_eq!(footer.height(), if width < 820.0 { 74.0 } else { 42.0 });
            assert!(content.bottom() < footer.top());
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| {
                            ui.set_clip_rect(ui.clip_rect().intersect(footer));
                            app.table_pagination(ui);
                        });
                    });
                },
            );
            for label in [
                "+",
                "−",
                "提交全部",
                "撤销暂存",
                "刷新",
                "上一页",
                "下一页",
                "每页",
                "200",
            ] {
                let shape = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.job.text == label => Some(text),
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("missing {label} at width {width}"));
                let rect = egui::Rect::from_min_size(shape.pos, shape.galley.size());
                assert!(
                    footer.contains_rect(rect),
                    "{label} at {rect:?} outside footer {footer:?}, width {width}"
                );
            }
        }
    }

    #[test]
    fn table_footer_submit_revert_and_refresh_share_busy_stale_and_offline_guards() {
        fn click_button(app: &mut App, label: &str, time: f64) {
            let ctx = app.context.clone();
            let mut frame = |time, events| {
                ctx.run(
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
                        egui::CentralPanel::default().show(ctx, |ui| app.table_pagination(ui));
                    },
                )
            };
            let output = frame(time, vec![]);
            let position = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
                    }
                    _ => None,
                })
                .expect("footer button should render");
            for pressed in [true, false] {
                let _ = frame(
                    time + if pressed { 0.02 } else { 0.04 },
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
            }
        }
        let (mut app, target, change) = table_flow_fixture();
        let tab_id = app.tabs[0].id;
        app.tabs[0].stage_cell(change.clone());
        click_button(&mut app, "提交全部", 0.0);
        assert!(app.session_busy(target.connection));
        click_button(&mut app, "撤销暂存", 1.0);
        assert_eq!(app.tabs[0].staged_cells.len(), 1);
        app.handle_response(
            target.connection,
            Response::OperationError {
                tab: tab_id,
                message: "rolled back".into(),
                disconnected: false,
            },
        );
        assert!(app.error);
        app.tabs[0].table_snapshot_stale = true;
        click_button(&mut app, "提交全部", 2.0);
        assert!(!app.session_busy(target.connection));
        app.sessions.get_mut(&target.connection).unwrap().connected = None;
        click_button(&mut app, "撤销暂存", 3.0);
        assert!(app.tabs[0].staged_cells.is_empty());
        assert!(app.status.contains("本地暂存"));
        assert!(!app.error);
        assert!(app.tabs[0].table_snapshot_stale);
        app.sessions.get_mut(&target.connection).unwrap().connected = Some(app.profiles[0].clone());
        click_button(&mut app, "刷新", 4.0);
        assert!(app.session_busy(target.connection));
        assert!(app.tabs[0].table_read_pending);
        assert!(app.tabs[0].output.is_some());
    }

    #[test]
    fn table_snapshot_stays_stale_after_connection_or_database_context_changes() {
        for event in 0..7 {
            let (mut app, target, change) = table_flow_fixture();
            let other = add_test_session(&mut app, "B", "db_b");
            let other_index = app.tabs.len();
            app.tabs.push(QueryTab::new(other_index + 1));
            app.tabs[other_index].table_target = Some(TableTarget {
                connection: other.connection,
                database: "db_b".into(),
                schema: "public".into(),
                name: "other".into(),
            });
            app.tabs[0].stage_cell(change);
            let tab_id = app.tabs[0].id;
            let response = match event {
                0 => Response::OperationError {
                    tab: tab_id,
                    message: "transport lost".into(),
                    disconnected: true,
                },
                1 => Response::Error {
                    message: "transport lost".into(),
                    disconnected: true,
                },
                2 => Response::Connected(target.connection),
                3 => Response::Disconnected,
                4 => Response::DatabaseContext(Some("another_db".into())),
                5 => Response::QueryResult {
                    tab: Uuid::new_v4(),
                    output: QueryOutput::default(),
                    database: Some("another_db".into()),
                },
                _ => Response::Query {
                    output: QueryOutput::default(),
                    database: Some("another_db".into()),
                },
            };
            app.handle_response(target.connection, response);
            assert!(app.tabs[0].table_snapshot_stale, "event {event}");
            assert!(app.tabs[0].output.is_some());
            assert_eq!(app.tabs[0].staged_cells.len(), 1);
            assert!(!app.tabs[other_index].table_snapshot_stale);
            // Returning to the old context must not make the old result editable.
            app.sessions.get_mut(&target.connection).unwrap().connected =
                Some(app.profiles[0].clone());
            app.handle_response(target.connection, Response::DatabaseSelected("db_a".into()));
            assert!(app.target_ready(&target));
            assert!(app.tabs[0].table_snapshot_stale);
            assert_eq!(app.tabs[0].staged_cells.len(), 1);
            assert!(!app.tabs[other_index].table_snapshot_stale);
        }
    }

    #[test]
    fn cell_context_value_actions_stage_null_empty_and_revert_only_original_cell() {
        let (mut app, target, change) = table_flow_fixture();
        app.tabs[0].stage_cell(CellUpdate {
            column: "id".into(),
            original: Some("7".into()),
            value: Some("8".into()),
            ..change.clone()
        });
        assert!(apply_grid_cell_draft(
            &mut app.tabs[0],
            0,
            1,
            CellDraftAction::SetNull,
            true
        ));
        assert_eq!(app.tabs[0].staged_cells[1].value, None);
        assert!(apply_grid_cell_draft(
            &mut app.tabs[0],
            0,
            1,
            CellDraftAction::SetEmptyString,
            true
        ));
        assert_eq!(app.tabs[0].staged_cells[1].value.as_deref(), Some(""));
        assert_eq!(
            app.tabs[0].staged_cells[1].original.as_deref(),
            Some("original")
        );
        assert_eq!(app.tabs[0].staged_cells[1].key_values, [Some("7".into())]);
        assert!(apply_grid_cell_draft(
            &mut app.tabs[0],
            0,
            1,
            CellDraftAction::Revert,
            true
        ));
        assert_eq!(app.tabs[0].staged_cells.len(), 1);
        assert_eq!(app.tabs[0].staged_cells[0].column, "id");
        assert_eq!(app.tabs[0].staged_cells[0].value.as_deref(), Some("8"));
        assert_eq!(
            app.tabs[0].table_data.as_ref().unwrap().keys[0],
            [Some("7".into())]
        );
        assert_eq!(
            app.tabs[0].output.as_ref().unwrap().sets[0].rows[0],
            [Some("7".into()), Some("original".into())]
        );
        assert!(!app.session_busy(target.connection));
    }

    #[test]
    fn cell_context_menu_clicks_apply_draft_actions_without_database_requests() {
        fn click_menu_item(app: &mut App, label: &str, time: f64) {
            let ctx = app.context.clone();
            let mut frame = |time, events| {
                ctx.run(
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
                            result_grid(ui, &mut app.tabs[0], true);
                        });
                    },
                )
            };
            let position = egui::pos2(550.0, 80.0);
            let _ = frame(time, vec![egui::Event::PointerMoved(position)]);
            let pointer_event = |pos, button, pressed| {
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            };
            let _ = frame(
                time + 0.02,
                pointer_event(position, egui::PointerButton::Secondary, true),
            );
            let _ = frame(
                time + 0.04,
                pointer_event(position, egui::PointerButton::Secondary, false),
            );
            let output = frame(time + 0.05, vec![egui::Event::PointerMoved(position)]);
            let item_position = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| {
                    panic!(
                        "context item {label} should render: {:?}",
                        output
                            .shapes
                            .iter()
                            .filter_map(|shape| match &shape.shape {
                                egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                    )
                });
            let _ = frame(
                time + 0.06,
                pointer_event(item_position, egui::PointerButton::Primary, true),
            );
            let _ = frame(
                time + 0.08,
                pointer_event(item_position, egui::PointerButton::Primary, false),
            );
        }
        let (mut app, target, change) = table_flow_fixture();
        app.tabs[0].stage_cell(CellUpdate {
            column: "id".into(),
            original: Some("7".into()),
            value: Some("8".into()),
            ..change
        });
        click_menu_item(&mut app, "设为 NULL", 0.0);
        assert_eq!(app.tabs[0].staged_cells.len(), 2);
        assert_eq!(app.tabs[0].staged_cells[1].value, None);
        click_menu_item(&mut app, "设为空字符串", 1.0);
        assert_eq!(app.tabs[0].staged_cells[1].value.as_deref(), Some(""));
        click_menu_item(&mut app, "撤销此格修改", 2.0);
        assert_eq!(app.tabs[0].staged_cells.len(), 1);
        assert_eq!(app.tabs[0].staged_cells[0].column, "id");
        assert!(!app.session_busy(target.connection));
    }

    #[test]
    fn cell_context_actions_and_editor_share_snapshot_and_editability_guards() {
        for mode in 0..8 {
            let (mut app, _, _) = table_flow_fixture();
            let tab = &mut app.tabs[0];
            let can_edit = mode != 0;
            match mode {
                1 => tab.table_target = None,
                2 => tab.table_snapshot_stale = true,
                3 => tab.table_data.as_mut().unwrap().writable = false,
                4 => tab.table_data.as_mut().unwrap().primary_keys.clear(),
                5 => tab.table_data.as_mut().unwrap().columns[1].editable = false,
                6 => {
                    tab.table_data.as_mut().unwrap().columns[0].kind =
                        sqlshop::model::CellKind::Unsupported
                }
                7 => tab.table_data.as_mut().unwrap().keys[0].clear(),
                _ => {}
            }
            assert!(
                grid_cell_editor(tab, 0, 1, can_edit).is_none(),
                "mode {mode}"
            );
            for action in [
                CellDraftAction::SetNull,
                CellDraftAction::SetEmptyString,
                CellDraftAction::Revert,
            ] {
                assert!(
                    !apply_grid_cell_draft(tab, 0, 1, action, can_edit),
                    "mode {mode}"
                );
            }
            assert!(tab.staged_cells.is_empty());
        }
        let (mut app, _, _) = table_flow_fixture();
        app.tabs[0].table_data.as_mut().unwrap().columns[1].kind =
            sqlshop::model::CellKind::Integer;
        assert!(!apply_grid_cell_draft(
            &mut app.tabs[0],
            0,
            1,
            CellDraftAction::SetEmptyString,
            true
        ));
        assert!(apply_grid_cell_draft(
            &mut app.tabs[0],
            0,
            1,
            CellDraftAction::SetNull,
            true
        ));
    }

    #[test]
    fn selected_cell_f2_and_enter_open_editor_with_staged_value_and_snapshot_key() {
        for key in [egui::Key::F2, egui::Key::Enter] {
            for readonly in [false, true] {
                let (mut app, _, change) = table_flow_fixture();
                app.tabs[0].selected_cell = Some((0, 1));
                app.tabs[0].stage_cell(change);
                let mut editor = None;
                let _ = app.context.run(
                    egui::RawInput {
                        events: vec![egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        }],
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            editor = result_grid(ui, &mut app.tabs[0], !readonly).0;
                        });
                    },
                );
                if readonly {
                    assert!(editor.is_none());
                } else {
                    let editor =
                        editor.expect("selected cell should open through the shared editor helper");
                    assert_eq!(editor.text, "changed");
                    assert_eq!(editor.change.original.as_deref(), Some("original"));
                    assert_eq!(editor.change.key_values, [Some("7".into())]);
                }
            }
        }
    }

    #[test]
    fn reverting_a_staged_cell_in_the_editor_removes_only_that_change() {
        for original in [None, Some("A".to_owned())] {
            let mut app = session_test_app();
            let target = add_test_session(&mut app, "A", "db_a");
            let tab_id = app.tabs[0].id;
            let change = CellUpdate {
                database: "db_a".into(),
                table: Table {
                    schema: "public".into(),
                    name: "items".into(),
                    kind: "BASE TABLE".into(),
                },
                primary_keys: vec!["id".into()],
                key_values: vec![Some("1".into())],
                column: "note".into(),
                original: original.clone(),
                value: Some("B".into()),
            };
            let other = CellUpdate {
                key_values: vec![Some("2".into())],
                ..change.clone()
            };
            app.tabs[0].stage_cell(change.clone());
            app.tabs[0].stage_cell(other);
            app.cell_editor = Some(CellEditor {
                tab: tab_id,
                connection: target.connection,
                change,
                text: original.clone().unwrap_or_default(),
                is_null: original.is_none(),
                focus: false,
                expanded: false,
            });
            let ctx = app.context.clone();
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: egui::Key::Enter,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::COMMAND,
                    }],
                    ..Default::default()
                },
                |ctx| app.cell_editor_window(ctx),
            );
            assert!(app.cell_editor.is_none());
            assert_eq!(app.tabs[0].staged_cells.len(), 1);
            assert_eq!(app.tabs[0].staged_cells[0].key_values, [Some("2".into())]);
        }
    }

    #[test]
    fn result_cell_double_click_uses_snapshot_keys_and_respects_read_only_mode() {
        let ctx = egui::Context::default();
        let mut tab = QueryTab::new(1);
        let connection = Uuid::new_v4();
        tab.table_target = Some(TableTarget {
            connection,
            database: "app_dev".into(),
            schema: "public".into(),
            name: "users".into(),
        });
        tab.output = Some(QueryOutput {
            sets: vec![sqlshop::model::ResultSet {
                columns: vec!["id".into(), "name".into()],
                rows: vec![vec![Some("7".into()), Some("original".into())]],
                ..Default::default()
            }],
            ..Default::default()
        });
        tab.table_data = Some(TableData {
            writable: true,
            output: QueryOutput::default(),
            primary_keys: vec!["id".into()],
            keys: vec![vec![Some("7".into())]],
            columns: ["id", "name"]
                .into_iter()
                .enumerate()
                .map(|(index, name)| sqlshop::model::TableColumn {
                    name: name.into(),
                    declared_type: "text".into(),
                    sql_type: "text".into(),
                    kind: sqlshop::model::CellKind::Text,
                    editable: true,
                    primary_key: index == 0,
                })
                .collect(),
        });
        let mut frame = |time, pressed: Option<bool>, can_edit| {
            let position = egui::pos2(100.0, 80.0);
            let mut events = vec![egui::Event::PointerMoved(position)];
            if let Some(pressed) = pressed {
                events.push(egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let mut editor = None;
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
                        editor = result_grid(ui, &mut tab, can_edit).0;
                    });
                },
            );
            editor
        };
        frame(0.0, None, true);
        frame(0.02, Some(true), true);
        assert!(frame(0.03, Some(false), true).is_none());
        frame(0.12, Some(true), true);
        let editor =
            frame(0.13, Some(false), true).expect("double click should edit the clicked cell");
        assert_eq!(editor.connection, connection);
        assert_eq!(editor.change.column, "id");
        assert_eq!(editor.change.key_values, [Some("7".into())]);
        assert_eq!(editor.change.original.as_deref(), Some("7"));
        frame(1.0, Some(true), false);
        assert!(frame(1.01, Some(false), false).is_none());
        frame(1.1, Some(true), false);
        assert!(frame(1.11, Some(false), false).is_none());
    }

    #[test]
    fn theme_text_colors_remain_readable_in_light_and_dark_modes() {
        let ctx = egui::Context::default();
        for (visuals, text, secondary) in [
            (
                egui::Visuals::light(),
                Color32::from_rgb(30, 40, 48),
                Color32::from_rgb(71, 84, 94),
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
            history_open: false,
            history_search: String::new(),
            last_workspace: String::new(),
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

    #[test]
    fn completion_columns_require_matching_bound_connected_database_and_generation() {
        let mut app = session_test_app();
        let a = add_test_session(&mut app, "A", "db_a");
        let b = add_test_session(&mut app, "B", "db_b");
        app.tabs[0].target = Some(a.clone());
        let table = app.sessions[&a.connection].tables[0].clone();
        let session = app.sessions.get_mut(&a.connection).unwrap();
        session.column_cache.insert(
            (table.schema.clone(), table.name.clone()),
            vec!["私有字段".into()],
        );
        assert_eq!(app.completion_columns(), ["私有字段"]);
        app.tabs[0].target = Some(b.clone());
        assert!(app.completion_columns().is_empty());
        app.tabs[0].target = Some(a.clone());
        app.sessions.get_mut(&a.connection).unwrap().active_database = Some("other".into());
        assert!(app.completion_columns().is_empty());
        let session = app.sessions.get_mut(&a.connection).unwrap();
        session.active_database = a.database.clone();
        let old_generation = session.completion_generation;
        session.clear_columns();
        assert_ne!(session.completion_generation, old_generation);
        assert!(app.completion_columns().is_empty());
        app.sessions.get_mut(&a.connection).unwrap().connected = None;
        assert!(app.completion_columns().is_empty());
    }

    #[test]
    fn completion_tables_require_matching_bound_connected_database_and_schema() {
        let mut app = session_test_app();
        let a = add_test_session(&mut app, "A", "db_a");
        let b = add_test_session(&mut app, "B", "db_b");
        app.tabs[0].target = Some(a.clone());
        assert_eq!(app.completion_tables(), ["db_a_table"]);
        app.tabs[0].target = Some(b.clone());
        assert_eq!(app.completion_tables(), ["db_b_table"]);
        app.tabs[0].target = Some(a.clone());
        app.sessions.get_mut(&a.connection).unwrap().active_database = Some("other".into());
        assert!(app.completion_tables().is_empty());
        let session = app.sessions.get_mut(&a.connection).unwrap();
        session.active_database = a.database.clone();
        session.tables.push(Table {
            schema: "other_schema".into(),
            name: "db_a_table".into(),
            kind: "BASE TABLE".into(),
        });
        // The same table name in two schemas is offered once.
        assert_eq!(app.completion_tables(), ["db_a_table"]);
        app.tabs[0].target.as_mut().unwrap().schema = Some("other_schema".into());
        assert_eq!(app.completion_tables(), ["db_a_table"]);
        app.tabs[0].target.as_mut().unwrap().schema = Some("missing".into());
        assert!(app.completion_tables().is_empty());
        app.sessions.get_mut(&a.connection).unwrap().connected = None;
        assert!(app.completion_tables().is_empty());
    }

    #[test]
    fn dialog_windows_open_centered_on_screen() {
        let ctx = egui::Context::default();
        let mut rect = None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let response = dialog_window("居中测试", ctx)
                .default_width(400.0)
                .show(ctx, |ui| ui.label("内容"));
            rect = response.map(|inner| inner.response.rect);
        });
        let rect = rect.expect("dialog shown");
        assert!(rect.width() > 0.0 && rect.height() > 0.0);
        let content = ctx.content_rect();
        assert!(
            (rect.center().x - content.center().x).abs() <= 1.0,
            "dialog center x {} vs content center {}",
            rect.center().x,
            content.center().x
        );
        assert!(
            (rect.center().y - content.center().y).abs() <= 1.0,
            "dialog center y {} vs content center {}",
            rect.center().y,
            content.center().y
        );
    }

    #[test]
    fn execution_scope_preserves_target_and_confirms_only_the_requested_sql() {
        let mut app = session_test_app();
        let a = add_test_session(&mut app, "A", "db_a");
        let b = add_test_session(&mut app, "B", "db_b");
        app.selected = Some(b.connection);
        app.tabs[0].target = Some(a.clone());
        let first = "UPDATE items SET note = '中文;值' WHERE id = 1;";
        let second = "DELETE FROM items WHERE id = 2;";
        let sql = format!("{first}\n{second}");
        app.tabs[0].sql = sql.clone();
        app.tabs[0].selection = first.into();
        app.tabs[0].cursor = first.chars().count() + 5;
        app.tabs[0].output = Some(QueryOutput::default());

        for (scope, expected) in [
            (SqlExecutionScope::CurrentStatement, second),
            (SqlExecutionScope::All, sql.as_str()),
            (SqlExecutionScope::SelectionOrAll, first),
        ] {
            app.execute_scope(scope);
            let pending = app.confirm_sql.take().expect("writes require confirmation");
            assert_eq!(pending.sql, expected);
            assert_eq!(pending.target, a);
            assert_eq!(pending.connection_name, "A");
            assert!(app.tabs[0].output.is_some());
            assert!(app.pending_history.is_empty());
            assert!(!app.sessions[&a.connection].busy);
            assert!(!app.sessions[&b.connection].busy);
        }
        assert_eq!(app.tabs[0].sql, sql);
        assert_eq!(app.tabs[0].selection, first);
    }

    #[test]
    fn current_statement_shortcut_does_not_fall_back_to_full_document_or_edit_sql() {
        let mut app = session_test_app();
        let a = add_test_session(&mut app, "A", "db_a");
        app.sessions.get_mut(&a.connection).unwrap().tables.clear();
        app.tabs[0].target = Some(a);
        let sql =
            "UPDATE items SET note = '中文;值' WHERE id = 1;\nDELETE FROM items WHERE id = 2;";
        let cursor = sql.find("DELETE").unwrap();
        app.tabs[0].sql = sql.into();
        app.tabs[0].select_range = Some(cursor..cursor);
        let ctx = app.context.clone();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.workspace(ui));
        });
        let modifiers = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::COMMAND
        };
        let _ = ctx.run(
            egui::RawInput {
                modifiers,
                events: vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.workspace(ui));
            },
        );
        assert_eq!(app.tabs[0].sql, sql);
        assert_eq!(
            app.confirm_sql.as_ref().unwrap().sql,
            "DELETE FROM items WHERE id = 2;"
        );
        assert!(app.pending_history.is_empty());
    }

    #[test]
    fn rejected_current_statement_keeps_results_and_does_not_dispatch() {
        let mut app = session_test_app();
        let a = add_test_session(&mut app, "A", "db_a");
        app.tabs[0].target = Some(a.clone());
        app.tabs[0].output = Some(QueryOutput::default());
        for sql in [
            "-- 只有注释",
            "SELECT 'unterminated",
            "DELIMITER $$\nSELECT 1$$",
        ] {
            app.tabs[0].sql = sql.into();
            app.tabs[0].cursor = 0;
            app.execute_scope(SqlExecutionScope::CurrentStatement);
            assert!(app.error, "{sql}");
            assert!(app.confirm_sql.is_none());
            assert!(app.tabs[0].output.is_some());
            assert!(app.pending_history.is_empty());
            assert!(!app.sessions[&a.connection].busy);
        }
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
            schema: None,
            connection: b.connection,
            database: Some("other".into()),
        };
        app.select_query_target(other.clone());
        assert!(app.can_execute());
        assert_eq!(app.sessions[&b.connection].active_database, b.database);
        app.sessions.get_mut(&b.connection).unwrap().busy = true;
        app.select_query_target(a.clone());
        assert_eq!(app.execution_target(), Some(a));
    }

    #[test]
    fn query_dropdown_supports_offline_targets_without_auto_connecting() {
        let mut app = session_test_app();
        let profile = Profile::default();
        let target = QueryTarget {
            schema: None,
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
                writable: false,
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
        app.query_sessions.insert(
            tab_a,
            query_session_ui::QuerySessionState {
                worker: sqlshop::worker::QueryWorker::start(tab_a, || {}),
                profile: app.profiles[0].clone(),
                target: a.clone(),
                busy: true,
                connected: true,
                user_sql: true,
                schemas_target: None,
                schemas: Vec::new(),
                schemas_loading: false,
                schemas_error: None,
                execution: None,
                cancellable: false,
                cancel_requested: false,
                transaction: Default::default(),
                transaction_verified: false,
            },
        );
        app.sessions.get_mut(&a.connection).unwrap().busy = true;
        app.sessions.get_mut(&a.connection).unwrap().running_tab = Some(tab_a);
        app.selected = Some(b.connection);
        assert_eq!(app.execution_target(), Some(a.clone()));
        assert!(!app.can_execute());
        app.browser_visible = true;
        app.browser_scope = Some(object_browser_ui::BrowserScope {
            connection: b.connection,
            database: b.database.clone(),
            schema: None,
        });
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
        app.query_sessions.get_mut(&tab_a).unwrap().busy = false;
        assert!(app.can_execute()); // Still executes A even with sidebar B selected.
        app.sessions.get_mut(&a.connection).unwrap().active_database = Some("other".into());
        assert!(app.can_execute()); // Management browsing cannot move this query session.
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
                        messages: Vec::new(),
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
                        assert_eq!(footer.height(), 42.0);
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
            schema: None,
            connection: Uuid::new_v4(),
            database: Some("dev".into()),
        };
        assert!(tab.bind_for_execution(&target));
        assert!(tab.bind_for_execution(&target));
        for other in [
            QueryTarget {
                schema: None,
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
                schema: None,
                connection: target.connection,
                database: Some(target.database.clone())
            })
        );
        assert!(!tabs[index].bind_for_execution(&QueryTarget {
            schema: None,
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
                        splitter = sql_editor(ui, &mut tab, &mut height, true, &[], &[]).0.rect;
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
            let mut schema_dialog = None;
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
                        table_buttons(
                            ui,
                            &[&table],
                            &mut selection,
                            connection,
                            "app_dev",
                            true,
                            &mut schema_dialog,
                        );
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
            let mut schema_dialog = None;
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
                            &mut schema_dialog,
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
