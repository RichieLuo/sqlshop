//! Object browser and inspector presentation. Actions are dispatched by the host;
//! drawing this module never connects, selects a database, or executes SQL.
mod views;
use views::*;
mod saved_queries;
pub(super) use saved_queries::query_in_scope;

mod inspector;
pub(super) use inspector::inspector;

use super::{accent, chrome, icons, surface};
use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use sqlshop::catalog::SavedQuery;
use sqlshop::model::{Engine, Profile, Table, TableColumn};
use sqlshop::object_catalog::{CatalogObject, ObjectCatalog, ObjectKind};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BrowserScope {
    pub connection: Uuid,
    pub database: Option<String>,
    pub schema: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum BrowserCategory {
    #[default]
    All,
    Tables,
    Views,
    Functions,
    Procedures,
    Triggers,
    Sequences,
    Queries,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum BrowserItem {
    Database {
        connection: Uuid,
        name: String,
    },
    Object {
        connection: Uuid,
        database: String,
        table: Table,
    },
    CatalogObject {
        connection: Uuid,
        database: String,
        object: CatalogObject,
    },
}

impl BrowserItem {
    pub(super) fn connection(&self) -> Uuid {
        match self {
            Self::Database { connection, .. }
            | Self::Object { connection, .. }
            | Self::CatalogObject { connection, .. } => *connection,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum BrowserView {
    #[default]
    Details,
    Icons,
}

#[derive(Default)]
pub(super) struct BrowserState {
    pub search: String,
    pub category: BrowserCategory,
    pub selected: Option<BrowserItem>,
    pub selected_query: Option<Uuid>,
    pub all_queries: bool,
    last_scope: Option<BrowserScope>,
    view: BrowserView,
    sort_column: usize,
    sort_descending: bool,
    hidden_columns: std::collections::HashSet<usize>,
}

pub(super) struct BrowserData<'a> {
    pub scope: Option<&'a BrowserScope>,
    pub profile: Option<&'a Profile>,
    pub connected: bool,
    pub busy: bool,
    pub active_database: Option<&'a str>,
    pub databases: &'a [String],
    pub tables: &'a [Table],
    pub catalog: Option<&'a ObjectCatalog>,
    pub catalog_loading: bool,
    pub catalog_error: Option<&'a str>,
    pub queries: &'a [SavedQuery],
    pub profiles: &'a [Profile],
    pub queries_writable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum BrowserAction {
    OpenSavedQuery(Uuid),
    RenameSavedQuery(Uuid),
    DeleteSavedQuery(Uuid),
    NewSavedQuery,
    Connect(Uuid),
    SelectDatabase {
        connection: Uuid,
        database: String,
    },
    RefreshDatabases(Uuid),
    RefreshObjects {
        connection: Uuid,
        database: String,
    },
    OpenData {
        connection: Uuid,
        database: String,
        table: Table,
    },
    OpenStructure {
        connection: Uuid,
        database: String,
        table: Table,
    },
    NewQuery {
        connection: Uuid,
        database: Option<String>,
        schema: Option<String>,
        table: Option<Table>,
    },
    LoadCatalog {
        connection: Uuid,
        database: String,
    },
    OpenDefinition {
        connection: Uuid,
        database: String,
        object: CatalogObject,
    },
    CreateTable {
        connection: Uuid,
        database: String,
        schema: Option<String>,
    },
    RenameTable {
        connection: Uuid,
        database: String,
        table: Table,
    },
}

pub(super) struct TableMetadata<'a> {
    pub columns: &'a [TableColumn],
    pub primary_keys: &'a [String],
    pub writable: bool,
    pub stale: bool,
}

pub(super) struct InspectorData<'a> {
    pub browser: &'a BrowserData<'a>,
    pub selected: Option<&'a BrowserItem>,
    pub metadata: Option<TableMetadata<'a>>,
    pub structure: Option<(&'a sqlshop::db::TableStructure, bool)>,
    pub query_preview: Option<QueryPreview<'a>>,
    pub focused_column: Option<&'a str>,
    pub definition: Option<&'a str>,
}

pub(super) struct QueryPreview<'a> {
    pub saved: bool,
    pub name: &'a str,
    pub sql: &'a str,
    pub executed_sql: Option<&'a str>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum InspectorPage {
    #[default]
    General,
    Fields,
    Ddl,
    Query,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum InspectorAction {
    Close,
    Browser(Box<BrowserAction>),
}

impl BrowserData<'_> {
    fn scope_ready(&self) -> bool {
        self.connected
            && !self.busy
            && self.scope.is_some_and(|scope| {
                scope.database.as_deref() == self.active_database && scope.database.is_some()
            })
    }

    fn item_ready(&self, item: &BrowserItem) -> bool {
        self.connected
            && !self.busy
            && self.scope.is_some_and(|scope| match item {
                BrowserItem::Database { connection, .. } => *connection == scope.connection,
                BrowserItem::Object {
                    connection,
                    database,
                    ..
                }
                | BrowserItem::CatalogObject {
                    connection,
                    database,
                    ..
                } => {
                    *connection == scope.connection
                        && self.active_database == Some(database.as_str())
                }
            })
    }
}

fn is_view(table: &Table) -> bool {
    table.kind.to_ascii_uppercase().contains("VIEW")
}

fn object_kind(table: &Table) -> &'static str {
    if table.kind.to_ascii_uppercase().contains("MATERIALIZED") {
        "物化视图"
    } else if is_view(table) {
        "视图"
    } else if table.kind.to_ascii_uppercase().contains("TABLE") {
        "表"
    } else {
        "对象"
    }
}

impl BrowserState {
    pub(super) fn set_icon_view(&mut self) {
        self.view = BrowserView::Icons;
    }
    pub(super) fn set_scope(&mut self, scope: Option<&BrowserScope>) {
        if self.last_scope.as_ref() != scope {
            self.last_scope = scope.cloned();
            self.selected = None;
            self.selected_query = None;
            self.all_queries = false;
            self.search.clear();
            self.category = BrowserCategory::All;
        }
    }
}

#[derive(Clone, Copy)]
enum BrowserRow<'a> {
    Database(&'a str),
    Object(&'a Table),
    CatalogTable(&'a Table, &'a CatalogObject),
    Catalog(&'a CatalogObject),
}

impl<'a> BrowserRow<'a> {
    fn name(self) -> &'a str {
        match self {
            Self::Database(name) => name,
            Self::Object(table) | Self::CatalogTable(table, _) => &table.name,
            Self::Catalog(object) => &object.name,
        }
    }

    fn schema(self) -> &'a str {
        match self {
            Self::Database(_) => "",
            Self::Object(table) | Self::CatalogTable(table, _) => &table.schema,
            Self::Catalog(object) => &object.schema,
        }
    }

    fn kind(self) -> Option<ObjectKind> {
        match self {
            Self::Database(_) => None,
            Self::Object(table) => Some(if is_view(table) {
                ObjectKind::View
            } else {
                ObjectKind::Table
            }),
            Self::Catalog(object) | Self::CatalogTable(_, object) => Some(object.kind),
        }
    }

    fn catalog(self, _data: &BrowserData<'a>) -> Option<&'a CatalogObject> {
        match self {
            Self::Catalog(object) | Self::CatalogTable(_, object) => Some(object),
            Self::Object(_) | Self::Database(_) => None,
        }
    }

    fn item(self, scope: &BrowserScope) -> BrowserItem {
        match self {
            Self::Database(name) => BrowserItem::Database {
                connection: scope.connection,
                name: name.into(),
            },
            Self::Object(table) | Self::CatalogTable(table, _) => BrowserItem::Object {
                connection: scope.connection,
                database: scope.database.clone().unwrap_or_default(),
                table: table.clone(),
            },
            Self::Catalog(object)
                if matches!(
                    object.kind,
                    ObjectKind::Table | ObjectKind::View | ObjectKind::MaterializedView
                ) =>
            {
                BrowserItem::Object {
                    connection: scope.connection,
                    database: scope.database.clone().unwrap_or_default(),
                    table: Table {
                        schema: object.schema.clone(),
                        name: object.name.clone(),
                        kind: match object.kind {
                            ObjectKind::Table => "BASE TABLE",
                            ObjectKind::MaterializedView => "MATERIALIZED VIEW",
                            _ => "VIEW",
                        }
                        .into(),
                    },
                }
            }
            Self::Catalog(object) => BrowserItem::CatalogObject {
                connection: scope.connection,
                database: scope.database.clone().unwrap_or_default(),
                object: object.clone(),
            },
        }
    }
}

fn category_matches(category: BrowserCategory, kind: ObjectKind) -> bool {
    match category {
        BrowserCategory::All => true,
        BrowserCategory::Tables => kind == ObjectKind::Table,
        BrowserCategory::Views => matches!(kind, ObjectKind::View | ObjectKind::MaterializedView),
        BrowserCategory::Functions => kind == ObjectKind::Function,
        BrowserCategory::Procedures => kind == ObjectKind::Procedure,
        BrowserCategory::Triggers => kind == ObjectKind::Trigger,
        BrowserCategory::Sequences => kind == ObjectKind::Sequence,
        BrowserCategory::Queries => false,
    }
}

fn catalog_rows<'a>(data: &BrowserData<'a>) -> Vec<BrowserRow<'a>> {
    let mut table_metadata = std::collections::HashMap::new();
    if let Some(catalog) = data.catalog {
        for object in &catalog.objects {
            if matches!(
                object.kind,
                ObjectKind::Table | ObjectKind::View | ObjectKind::MaterializedView
            ) {
                table_metadata.insert((object.schema.as_str(), object.name.as_str()), object);
            }
        }
    }
    let table_identities: std::collections::HashSet<_> = data
        .tables
        .iter()
        .map(|table| (table.schema.as_str(), table.name.as_str()))
        .collect();
    let mut rows: Vec<_> = data
        .tables
        .iter()
        .map(|table| {
            table_metadata
                .get(&(table.schema.as_str(), table.name.as_str()))
                .map_or(BrowserRow::Object(table), |object| {
                    BrowserRow::CatalogTable(table, object)
                })
        })
        .collect();
    if let Some(catalog) = data.catalog {
        for object in &catalog.objects {
            let represented = matches!(
                object.kind,
                ObjectKind::Table | ObjectKind::View | ObjectKind::MaterializedView
            ) && table_identities
                .contains(&(object.schema.as_str(), object.name.as_str()));
            if !represented {
                rows.push(BrowserRow::Catalog(object));
            }
        }
    }
    rows
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn row_value(entry: BrowserRow<'_>, column: usize, data: &BrowserData<'_>) -> String {
    let metadata = entry.catalog(data);
    match column {
        0 => entry.name().into(),
        1 => {
            if entry.schema().is_empty() {
                "—".into()
            } else {
                entry.schema().into()
            }
        }
        2 => match entry {
            BrowserRow::Database(name) => if data.active_database == Some(name) {
                "当前数据库"
            } else {
                "数据库"
            }
            .into(),
            _ => metadata
                .map(|object| object.kind)
                .or(entry.kind())
                .map_or("对象", |kind| kind.label())
                .into(),
        },
        3 => metadata
            .and_then(|object| object.estimated_rows)
            .map_or("—".into(), |rows| rows.to_string()),
        4 => metadata
            .and_then(|object| object.size_bytes)
            .map_or("—".into(), format_bytes),
        5 => metadata
            .and_then(|object| object.engine.as_deref().or(object.owner.as_deref()))
            .unwrap_or("—")
            .into(),
        6 => metadata
            .and_then(|object| object.comment.as_deref())
            .filter(|value| !value.is_empty())
            .unwrap_or("—")
            .into(),
        7 => metadata
            .and_then(|object| object.created_at.as_deref())
            .unwrap_or("—")
            .into(),
        8 => metadata
            .and_then(|object| object.updated_at.as_deref())
            .unwrap_or("—")
            .into(),
        _ => metadata
            .and_then(|object| object.signature.as_deref().or(object.parent.as_deref()))
            .unwrap_or("—")
            .into(),
    }
}

fn visible_rows<'a>(data: &'a BrowserData<'a>, state: &BrowserState) -> Vec<BrowserRow<'a>> {
    let Some(scope) = data.scope else {
        return vec![];
    };
    let search = state.search.trim().to_lowercase();
    let mut rows: Vec<_> = if scope.database.is_none() {
        data.databases
            .iter()
            .filter(|name| name.to_lowercase().contains(&search))
            .map(|name| BrowserRow::Database(name.as_str()))
            .collect()
    } else if data.connected && scope.database.as_deref() == data.active_database {
        catalog_rows(data)
            .into_iter()
            .filter(|entry| {
                scope
                    .schema
                    .as_deref()
                    .is_none_or(|schema| schema == entry.schema())
                    && entry
                        .kind()
                        .is_some_and(|kind| category_matches(state.category, kind))
                    && (search.is_empty()
                        || [0, 1, 2, 5, 6, 9].into_iter().any(|column| {
                            row_value(*entry, column, data)
                                .to_lowercase()
                                .contains(&search)
                        }))
            })
            .collect()
    } else {
        vec![]
    };
    rows.sort_by(|left, right| {
        let ordering = match state.sort_column {
            3 => left
                .catalog(data)
                .and_then(|object| object.estimated_rows)
                .cmp(&right.catalog(data).and_then(|object| object.estimated_rows)),
            4 => left
                .catalog(data)
                .and_then(|object| object.size_bytes)
                .cmp(&right.catalog(data).and_then(|object| object.size_bytes)),
            column => row_value(*left, column, data).cmp(&row_value(*right, column, data)),
        }
        .then_with(|| left.name().cmp(right.name()))
        .then_with(|| left.schema().cmp(right.schema()));
        if state.sort_descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
    rows
}

fn open_action(item: &BrowserItem) -> BrowserAction {
    match item {
        BrowserItem::Database { connection, name } => BrowserAction::SelectDatabase {
            connection: *connection,
            database: name.clone(),
        },
        BrowserItem::Object {
            connection,
            database,
            table,
        } => BrowserAction::OpenData {
            connection: *connection,
            database: database.clone(),
            table: table.clone(),
        },
        BrowserItem::CatalogObject {
            connection,
            database,
            object,
        } => BrowserAction::OpenDefinition {
            connection: *connection,
            database: database.clone(),
            object: object.clone(),
        },
    }
}

fn structure_action(item: &BrowserItem) -> Option<BrowserAction> {
    match item {
        BrowserItem::Object {
            connection,
            database,
            table,
        } => Some(BrowserAction::OpenStructure {
            connection: *connection,
            database: database.clone(),
            table: table.clone(),
        }),
        BrowserItem::Database { .. } | BrowserItem::CatalogObject { .. } => None,
    }
}

fn query_action(data: &BrowserData<'_>, item: Option<&BrowserItem>) -> Option<BrowserAction> {
    let scope = data.scope?;
    let (database, table) = match item.filter(|item| item.connection() == scope.connection) {
        Some(BrowserItem::Database { name, .. }) => (Some(name.clone()), None),
        Some(BrowserItem::Object {
            database, table, ..
        }) => (Some(database.clone()), Some(table.clone())),
        Some(BrowserItem::CatalogObject { database, .. }) => (Some(database.clone()), None),
        None => (scope.database.clone(), None),
    };
    let schema = match item.filter(|item| item.connection() == scope.connection) {
        Some(BrowserItem::Object { table, .. }) => Some(table.schema.clone()),
        Some(BrowserItem::CatalogObject { object, .. }) => Some(object.schema.clone()),
        Some(BrowserItem::Database { .. }) => None,
        None => scope.schema.clone(),
    };
    Some(BrowserAction::NewQuery {
        connection: scope.connection,
        database,
        schema,
        table,
    })
}

pub(super) fn object_browser(
    ui: &mut egui::Ui,
    data: &BrowserData<'_>,
    state: &mut BrowserState,
) -> Option<BrowserAction> {
    compact_style(ui);
    state.set_scope(data.scope);
    let mut action = None;
    let Some(scope) = data.scope else {
        if state.category == BrowserCategory::Queries {
            ui.heading("查询");
            ui.add(egui::TextEdit::singleline(&mut state.search).hint_text("搜索查询名称 / SQL"));
            return saved_queries::workspace(ui, data, state);
        }
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);
            ui.heading("数据库对象");
            ui.label(RichText::new("选择连接、数据库或 Schema").weak());
        });
        return None;
    };
    let databases = scope.database.is_none();
    ui.horizontal(|ui| {
        if let Some(profile) = data.profile {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
            icons::paint_connection(ui, rect, profile.engine, data.connected);
        }
        let location = [
            data.profile.map(|profile| profile.name.as_str()),
            scope.database.as_deref(),
            scope.schema.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" / ");
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width().max(0.0), 25.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| ui.add(egui::Label::new(RichText::new(location).strong()).truncate()),
        );
    });
    ui.separator();
    category_controls(ui, data, state);
    if state.category == BrowserCategory::Queries {
        state.selected = None;
        return saved_queries::workspace(ui, data, state);
    }
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                if databases {
                    data.connected && !data.busy
                } else {
                    data.scope_ready() && !data.catalog_loading
                },
                egui::Button::new("刷新"),
            )
            .on_hover_text(if databases {
                "刷新数据库目录"
            } else {
                "刷新当前数据库的完整对象目录与属性"
            })
            .clicked()
        {
            if databases {
                action = Some(BrowserAction::RefreshDatabases(scope.connection));
            } else if data.scope_ready() {
                action = Some(BrowserAction::RefreshObjects {
                    connection: scope.connection,
                    database: scope.database.clone().unwrap(),
                });
            }
        }
        let selected_ready = state
            .selected
            .as_ref()
            .is_some_and(|item| data.item_ready(item));
        if ui
            .add_enabled(
                selected_ready,
                egui::Button::new(if databases {
                    "打开数据库"
                } else if matches!(state.selected, Some(BrowserItem::CatalogObject { .. })) {
                    "查看定义"
                } else {
                    "打开数据"
                }),
            )
            .clicked()
            && let Some(item) = &state.selected
        {
            action = Some(open_action(item));
        }
        if !databases
            && ui
                .add_enabled(
                    selected_ready
                        && state
                            .selected
                            .as_ref()
                            .is_some_and(|item| structure_action(item).is_some()),
                    egui::Button::new("结构"),
                )
                .clicked()
            && let Some(item) = &state.selected
        {
            action = structure_action(item);
        }
        if ui
            .button("新建查询")
            .on_hover_text("创建绑定到此位置的 SQL 草稿；不会执行 SQL 或切换数据库")
            .clicked()
        {
            action = query_action(data, state.selected.as_ref());
        }
        if !databases
            && ui
                .add_enabled(data.scope_ready(), egui::Button::new("新建表"))
                .clicked()
        {
            action = Some(BrowserAction::CreateTable {
                connection: scope.connection,
                database: scope.database.clone().unwrap(),
                schema: scope.schema.clone(),
            });
        }
    });
    if data.busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(
                RichText::new("连接正在操作，目录操作暂不可用")
                    .small()
                    .weak(),
            );
        });
    }
    if !data.connected {
        ui.add_space(22.0);
        ui.label(RichText::new("连接尚未打开").strong());
        if ui
            .add_enabled(!data.busy, egui::Button::new("连接服务器"))
            .clicked()
        {
            action = Some(BrowserAction::Connect(scope.connection));
        }
        return action;
    }
    if !databases && scope.database.as_deref() != data.active_database {
        ui.add_space(22.0);
        ui.label(RichText::new("此数据库尚未选为当前数据库").strong());
        if ui
            .add_enabled(!data.busy, egui::Button::new("选择数据库"))
            .on_hover_text("读取对象目录；已有文档保持各自的执行目标")
            .clicked()
        {
            action = Some(BrowserAction::SelectDatabase {
                connection: scope.connection,
                database: scope.database.clone().unwrap(),
            });
        }
        return action;
    }
    if !databases {
        if data.catalog_loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("正在读取对象和属性…").small().weak());
            });
        }
        if let Some(error) = data.catalog_error {
            ui.colored_label(ui.visuals().warn_fg_color, format!("目录读取失败：{error}"));
            if ui
                .add_enabled(
                    data.scope_ready() && !data.catalog_loading,
                    egui::Button::new("重新读取目录"),
                )
                .clicked()
            {
                action = Some(BrowserAction::LoadCatalog {
                    connection: scope.connection,
                    database: scope.database.clone().unwrap(),
                });
            }
        } else if data.catalog.is_none() && !data.catalog_loading {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("尚未读取扩展对象与属性").small().weak());
                if ui
                    .add_enabled(data.scope_ready(), egui::Button::new("读取完整目录"))
                    .clicked()
                {
                    action = Some(BrowserAction::LoadCatalog {
                        connection: scope.connection,
                        database: scope.database.clone().unwrap(),
                    });
                }
            });
        }
        if let Some(catalog) = data.catalog {
            for warning in &catalog.warnings {
                ui.colored_label(ui.visuals().warn_fg_color, RichText::new(warning).small());
            }
            if catalog.truncated {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "对象目录达到读取上限，仅显示已读取对象。",
                );
            }
        }
    }
    let rows = visible_rows(data, state);
    if state
        .selected
        .as_ref()
        .is_some_and(|item| !rows.iter().any(|row| row.item(scope) == *item))
    {
        state.selected = None;
    }
    ui.horizontal(|ui| {
        let label = if state.search.trim().is_empty() {
            format!("已读取 {} 个对象", rows.len())
        } else {
            format!("{} 个匹配对象", rows.len())
        };
        ui.label(RichText::new(label).small().weak());
    });
    if rows.is_empty() {
        ui.add_space(28.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(if !state.search.trim().is_empty() {
                    "没有匹配的对象"
                } else if data.catalog_loading {
                    "正在读取目录"
                } else if databases {
                    "没有可见的数据库"
                } else if data.catalog.is_none()
                    && !matches!(
                        state.category,
                        BrowserCategory::All | BrowserCategory::Tables | BrowserCategory::Views
                    )
                {
                    "读取完整目录后查看此类对象"
                } else {
                    "没有可见的此类对象"
                })
                .strong(),
            );
            if !state.search.trim().is_empty() && ui.button("清除搜索").clicked() {
                state.search.clear();
            }
        });
        return action;
    }
    ui.add_space(3.0);
    if !databases && state.view == BrowserView::Icons {
        render_icon_view(ui, data, state, &rows, scope, &mut action);
    } else {
        render_detail_view(ui, data, state, &rows, scope, &mut action);
    }

    action
}

#[cfg(test)]
mod tests;
