//! Object browser and inspector presentation. Actions are dispatched by the host;
//! drawing this module never connects, selects a database, or executes SQL.
use super::{accent, chrome, icons, surface};
use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum BrowserAction {
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

fn inspector_browser_action(action: BrowserAction) -> InspectorAction {
    InspectorAction::Browser(Box::new(action))
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

fn compact_style(ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
    ui.spacing_mut().button_padding = egui::vec2(7.0, 4.0);
    ui.spacing_mut().interact_size.y = 25.0;
    ui.visuals_mut().widgets.inactive.corner_radius = egui::CornerRadius::same(3);
    ui.visuals_mut().widgets.hovered.corner_radius = egui::CornerRadius::same(3);
    ui.visuals_mut().widgets.active.corner_radius = egui::CornerRadius::same(3);
    let color = accent(ui);
    ui.visuals_mut().selection.bg_fill =
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 32);
}

fn category_controls(ui: &mut egui::Ui, data: &BrowserData<'_>, state: &mut BrowserState) {
    let Some(scope) = data.scope else {
        return;
    };
    let databases = scope.database.is_none();
    let rows: Vec<_> = catalog_rows(data)
        .into_iter()
        .filter(|entry| {
            scope
                .schema
                .as_deref()
                .is_none_or(|schema| schema == entry.schema())
        })
        .collect();
    let start = ui.available_rect_before_wrap().min;
    let width = ui.available_width();
    ui.painter().line_segment(
        [
            start + egui::vec2(0.0, 32.0),
            start + egui::vec2(width, 32.0),
        ],
        egui::Stroke::new(0.5, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );
    egui::ScrollArea::horizontal()
        .id_salt("object-category-scroll")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if databases {
                    ui.add_sized(
                        [110.0, 32.0],
                        egui::Label::new(format!("数据库  {}", data.databases.len())),
                    );
                } else {
                    for (category, name) in [
                        (BrowserCategory::All, "全部对象"),
                        (BrowserCategory::Tables, "表"),
                        (BrowserCategory::Views, "视图"),
                        (BrowserCategory::Functions, "函数"),
                        (BrowserCategory::Procedures, "过程"),
                        (BrowserCategory::Triggers, "触发器"),
                        (BrowserCategory::Sequences, "序列"),
                    ] {
                        if category == BrowserCategory::Sequences
                            && data
                                .profile
                                .is_some_and(|profile| profile.engine == Engine::MySql)
                        {
                            continue;
                        }
                        let count = rows
                            .iter()
                            .filter(|entry| {
                                entry
                                    .kind()
                                    .is_some_and(|kind| category_matches(category, kind))
                            })
                            .count();
                        let unavailable = data.catalog.is_some_and(|catalog| {
                            catalog
                                .unavailable_kinds
                                .iter()
                                .any(|kind| category_matches(category, *kind))
                        });
                        let count_label = if unavailable {
                            "—".into()
                        } else {
                            count.to_string()
                        };
                        let label = format!("{name}  {count_label}");
                        let text_width = ui
                            .painter()
                            .layout_no_wrap(
                                label.clone(),
                                egui::FontId::proportional(13.0),
                                ui.visuals().text_color(),
                            )
                            .size()
                            .x;
                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(text_width + 34.0, 32.0),
                            egui::Sense::click(),
                        );
                        let selected = state.category == category;
                        if selected || response.hovered() {
                            ui.painter().rect_filled(
                                rect.shrink2(egui::vec2(1.0, 1.0)),
                                2,
                                if selected {
                                    surface(ui)
                                } else {
                                    ui.visuals().widgets.hovered.bg_fill
                                },
                            );
                        }
                        let color = if selected {
                            accent(ui)
                        } else {
                            ui.visuals().text_color()
                        };
                        let icon = match category {
                            BrowserCategory::Tables => super::tab_bar_ui::TabIcon::Table,
                            BrowserCategory::Views => super::tab_bar_ui::TabIcon::View,
                            BrowserCategory::All => super::tab_bar_ui::TabIcon::Objects,
                            _ => super::tab_bar_ui::TabIcon::Query,
                        };
                        super::tab_bar_ui::paint_icon(
                            ui,
                            egui::Rect::from_min_size(
                                rect.min + egui::vec2(5.0, 8.0),
                                egui::vec2(16.0, 16.0),
                            ),
                            icon,
                            color,
                        );
                        ui.painter().text(
                            rect.min + egui::vec2(25.0, 16.0),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::FontId::proportional(13.0),
                            color,
                        );
                        if selected {
                            ui.painter().line_segment(
                                [
                                    rect.left_bottom() + egui::vec2(4.0, 0.0),
                                    rect.right_bottom() - egui::vec2(4.0, 0.0),
                                ],
                                egui::Stroke::new(2.0, color),
                            );
                        }
                        if response.clicked() {
                            state.category = category;
                        }
                    }
                }
            });
        });
    ui.horizontal(|ui| {
        let width = (ui.available_width() - if databases { 0.0 } else { 145.0 }).max(80.0);
        ui.add(
            egui::TextEdit::singleline(&mut state.search)
                .hint_text(if databases {
                    "搜索数据库"
                } else {
                    "搜索名称、Schema、注释或所有者"
                })
                .desired_width(width),
        );
        if !databases {
            ui.selectable_value(&mut state.view, BrowserView::Details, "列表");
            ui.selectable_value(&mut state.view, BrowserView::Icons, "图标");
            ui.menu_button("列", |ui| {
                ui.label(RichText::new("显示列").small().strong());
                let mut name_visible = true;
                ui.add_enabled(false, egui::Checkbox::new(&mut name_visible, "名称"));
                for (column, label) in object_column_labels(data).into_iter().enumerate().skip(1) {
                    let mut visible = !state.hidden_columns.contains(&column);
                    if ui.checkbox(&mut visible, label).changed() {
                        if visible {
                            state.hidden_columns.remove(&column);
                        } else {
                            state.hidden_columns.insert(column);
                            if state.sort_column == column {
                                state.sort_column = 0;
                            }
                        }
                    }
                }
                ui.separator();
                if ui.button("恢复默认列").clicked() {
                    state.hidden_columns.clear();
                    ui.close();
                }
            });
        }
    });
    ui.add_space(2.0);
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
    if data.connected && (databases || scope.database.as_deref() == data.active_database) {
        category_controls(ui, data, state);
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

fn object_context_menu(
    ui: &mut egui::Ui,
    data: &BrowserData<'_>,
    item: &BrowserItem,
    action: &mut Option<BrowserAction>,
) {
    let caption = match item {
        BrowserItem::Database { .. } => "打开数据库",
        BrowserItem::Object { .. } => "打开数据",
        BrowserItem::CatalogObject { .. } => "查看定义",
    };
    if ui
        .add_enabled(data.item_ready(item), egui::Button::new(caption))
        .clicked()
    {
        *action = Some(open_action(item));
        ui.close();
    }
    if let Some(next) = structure_action(item)
        && ui
            .add_enabled(data.item_ready(item), egui::Button::new("打开结构"))
            .clicked()
    {
        *action = Some(next);
        ui.close();
    }
    if ui.button("新建查询").clicked() {
        *action = query_action(data, Some(item));
        ui.close();
    }
    ui.separator();
    if ui.button("复制名称").clicked() {
        let name = match item {
            BrowserItem::Database { name, .. } => name,
            BrowserItem::Object { table, .. } => &table.name,
            BrowserItem::CatalogObject { object, .. } => &object.name,
        };
        ui.ctx().copy_text(name.clone());
        ui.close();
    }
    if !matches!(item, BrowserItem::Database { .. }) && ui.button("复制限定名称").clicked() {
        let (schema, name) = match item {
            BrowserItem::Object { table, .. } => (&table.schema, &table.name),
            BrowserItem::CatalogObject { object, .. } => (&object.schema, &object.name),
            _ => unreachable!(),
        };
        let engine = data.profile.map_or(Engine::MySql, |profile| profile.engine);
        let qualified = if schema.is_empty() {
            engine.quote(name)
        } else {
            format!("{}.{}", engine.quote(schema), engine.quote(name))
        };
        ui.ctx().copy_text(qualified);
        ui.close();
    }
}

fn entry_icon(ui: &egui::Ui, rect: egui::Rect, entry: BrowserRow<'_>) {
    if matches!(
        entry.kind(),
        Some(
            ObjectKind::Function
                | ObjectKind::Procedure
                | ObjectKind::Trigger
                | ObjectKind::Sequence
        )
    ) {
        let symbol = match entry.kind() {
            Some(ObjectKind::Function) => "ƒ",
            Some(ObjectKind::Procedure) => "P",
            Some(ObjectKind::Trigger) => "T",
            _ => "#",
        };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            symbol,
            egui::FontId::monospace((rect.height() - 2.0).min(24.0)),
            accent(ui),
        );
    } else {
        paint_object_icon(
            ui,
            rect,
            matches!(entry, BrowserRow::Database(_)),
            matches!(
                entry.kind(),
                Some(ObjectKind::View | ObjectKind::MaterializedView)
            ),
        );
    }
}

fn object_column_labels(data: &BrowserData<'_>) -> [&'static str; 10] {
    [
        "名称",
        "Schema",
        "类型",
        "估计行数",
        "目录大小",
        if data
            .profile
            .is_some_and(|profile| profile.engine == Engine::MySql)
        {
            "存储引擎"
        } else {
            "所有者"
        },
        "注释",
        "创建时间",
        "修改时间",
        "签名 / 所属对象",
    ]
}

fn catalog_size_hint(data: &BrowserData<'_>) -> &'static str {
    match data.profile.map(|profile| profile.engine) {
        Some(Engine::Postgres) => {
            "服务器目录中的关系大小，仅包含关系本身，不包含索引或 TOAST；可能无法提供。"
        }
        Some(Engine::MySql) => "服务器目录中的数据与索引大小，为存储引擎提供的近似统计。",
        Some(Engine::SqlServer) => {
            "服务器目录中的行存储分配空间，包含索引，可能与实际磁盘文件大小不同。"
        }
        None => "服务器目录提供的大小统计，计算范围随数据库引擎而异。",
    }
}

fn render_detail_view(
    ui: &mut egui::Ui,
    data: &BrowserData<'_>,
    state: &mut BrowserState,
    rows: &[BrowserRow<'_>],
    scope: &BrowserScope,
    action: &mut Option<BrowserAction>,
) {
    let databases = scope.database.is_none();
    let labels = if databases {
        vec!["名称", "Schema", "状态"]
    } else {
        object_column_labels(data).to_vec()
    };
    let columns: Vec<_> = (0..labels.len())
        .filter(|column| databases || !state.hidden_columns.contains(column))
        .collect();
    let widths: &[f32] = if databases {
        &[220.0, 125.0, 120.0]
    } else {
        &[
            220.0, 125.0, 100.0, 100.0, 105.0, 155.0, 240.0, 170.0, 170.0, 250.0,
        ]
    };
    let viewport_width = ui.available_width();
    let content_width = columns.iter().map(|column| widths[*column]).sum::<f32>()
        + ui.spacing().item_spacing.x * columns.len() as f32;
    let height = (ui.available_height() - 18.0).max(80.0);
    let column_mask = columns
        .iter()
        .fold(0_u16, |mask, column| mask | (1 << column));
    egui::Frame::new()
        .fill(chrome(ui.ctx()))
        .inner_margin(0)
        .show(ui, |ui| {
            egui::ScrollArea::horizontal()
                .id_salt(("object-detail-scroll", scope.connection, &scope.database))
                .scroll_source(egui::scroll_area::ScrollSource {
                    drag: false,
                    ..Default::default()
                })
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(content_width.max(viewport_width));
                    let mut table = TableBuilder::new(ui)
                        .drag_to_scroll(false)
                        .id_salt((
                            "object-browser",
                            scope.connection,
                            &scope.database,
                            &scope.schema,
                            column_mask,
                        ))
                        .striped(true)
                        .resizable(true)
                        .sense(egui::Sense::click())
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
                    for column in &columns {
                        table = table.column(Column::initial(widths[*column]).at_least(55.0));
                    }
                    table
                        .min_scrolled_height(80.0)
                        .max_scroll_height(height)
                        .header(25.0, |mut header| {
                            for &index in &columns {
                                let label = labels[index];
                                header.col(|ui| {
                                    let marker = if state.sort_column == index {
                                        if state.sort_descending {
                                            " ↓"
                                        } else {
                                            " ↑"
                                        }
                                    } else {
                                        ""
                                    };
                                    let header_response = ui.add(
                                        egui::Label::new(
                                            RichText::new(format!("{label}{marker}"))
                                                .small()
                                                .strong(),
                                        )
                                        .sense(egui::Sense::click()),
                                    );
                                    let header_response = if !databases && index == 3 {
                                        header_response
                                            .on_hover_text("服务器统计估计；— 表示未提供")
                                    } else if !databases && index == 4 {
                                        header_response.on_hover_text(catalog_size_hint(data))
                                    } else {
                                        header_response
                                    };
                                    if header_response.clicked() {
                                        if state.sort_column == index {
                                            state.sort_descending = !state.sort_descending;
                                        } else {
                                            state.sort_column = index;
                                            state.sort_descending = false;
                                        }
                                    }
                                });
                            }
                        })
                        .body(|body| {
                            body.rows(25.0, rows.len(), |mut row| {
                                let entry = rows[row.index()];
                                let item = entry.item(scope);
                                row.set_selected(state.selected.as_ref() == Some(&item));
                                let mut cell_response: Option<egui::Response> = None;
                                for &column in &columns {
                                    row.col(|ui| {
                                        if column == 0 {
                                            let (rect, _) = ui.allocate_exact_size(
                                                egui::vec2(17.0, 17.0),
                                                egui::Sense::hover(),
                                            );
                                            entry_icon(ui, rect, entry);
                                        }
                                        let value = row_value(entry, column, data);
                                        let response = ui
                                            .add(
                                                egui::Label::new(&value)
                                                    .truncate()
                                                    .sense(egui::Sense::click()),
                                            )
                                            .on_hover_text(&value);
                                        cell_response = Some(match cell_response.take() {
                                            Some(previous) => previous.union(response),
                                            None => response,
                                        });
                                    });
                                }
                                let response = cell_response.unwrap().union(row.response());
                                if response.clicked() || response.secondary_clicked() {
                                    state.selected = Some(item.clone());
                                }
                                if response.double_clicked() && data.item_ready(&item) {
                                    *action = Some(open_action(&item));
                                }
                                response.context_menu(|ui| {
                                    object_context_menu(ui, data, &item, action)
                                });
                            });
                        });
                });
        });
}

fn render_icon_view(
    ui: &mut egui::Ui,
    data: &BrowserData<'_>,
    state: &mut BrowserState,
    rows: &[BrowserRow<'_>],
    scope: &BrowserScope,
    action: &mut Option<BrowserAction>,
) {
    let columns = (ui.available_width() / 145.0).floor().max(1.0) as usize;
    let row_count = rows.len().div_ceil(columns);
    egui::ScrollArea::vertical()
        .id_salt(("object-icons", scope.connection, &scope.database))
        .auto_shrink([false, false])
        .show_rows(ui, 96.0, row_count, |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    for entry in rows.iter().skip(row * columns).take(columns) {
                        let item = entry.item(scope);
                        let (rect, response) =
                            ui.allocate_exact_size(egui::vec2(138.0, 88.0), egui::Sense::click());
                        if state.selected.as_ref() == Some(&item) || response.hovered() {
                            ui.painter().rect_filled(
                                rect.shrink(1.0),
                                3,
                                if state.selected.as_ref() == Some(&item) {
                                    ui.visuals().selection.bg_fill
                                } else {
                                    ui.visuals().widgets.hovered.bg_fill
                                },
                            );
                        }
                        entry_icon(
                            ui,
                            egui::Rect::from_center_size(
                                rect.center_top() + egui::vec2(0.0, 25.0),
                                egui::vec2(28.0, 28.0),
                            ),
                            *entry,
                        );
                        let name = ui.painter().layout(
                            entry.name().into(),
                            egui::FontId::proportional(13.0),
                            ui.visuals().text_color(),
                            126.0,
                        );
                        let name_rect = egui::Rect::from_min_size(
                            egui::pos2(rect.center().x - name.size().x / 2.0, rect.top() + 44.0),
                            name.size(),
                        );
                        ui.painter()
                            .with_clip_rect(egui::Rect::from_min_max(
                                rect.min,
                                egui::pos2(rect.right(), rect.top() + 68.0),
                            ))
                            .galley(name_rect.min, name, ui.visuals().text_color());
                        ui.painter().text(
                            rect.center_bottom() - egui::vec2(0.0, 9.0),
                            egui::Align2::CENTER_CENTER,
                            entry.kind().map_or("数据库", |kind| kind.label()),
                            egui::FontId::proportional(11.0),
                            ui.visuals().weak_text_color(),
                        );
                        let response = response.on_hover_text(format!(
                            "{} / {}",
                            entry.schema(),
                            entry.name()
                        ));
                        if response.clicked() || response.secondary_clicked() {
                            state.selected = Some(item.clone());
                        }
                        if response.double_clicked() && data.item_ready(&item) {
                            *action = Some(open_action(&item));
                        }
                        response.context_menu(|ui| object_context_menu(ui, data, &item, action));
                    }
                });
            }
        });
}

fn paint_object_icon(ui: &egui::Ui, rect: egui::Rect, database: bool, view: bool) {
    let color = if view {
        accent(ui)
    } else {
        ui.visuals().weak_text_color()
    };
    let stroke = egui::Stroke::new(1.0, color);
    let bounds = rect.shrink(2.0);
    ui.painter()
        .rect_stroke(bounds, 1.0, stroke, egui::StrokeKind::Inside);
    for part in [0.33, 0.66] {
        let y = bounds.top() + bounds.height() * part;
        ui.painter().line_segment(
            [egui::pos2(bounds.left(), y), egui::pos2(bounds.right(), y)],
            stroke,
        );
        if !database {
            let x = bounds.left() + bounds.width() * part;
            ui.painter().line_segment(
                [egui::pos2(x, bounds.top()), egui::pos2(x, bounds.bottom())],
                stroke,
            );
        }
    }
}

fn property(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).small().weak());
    ui.add(egui::Label::new(value).wrap()).on_hover_text(value);
    ui.add_space(5.0);
}

pub(super) fn inspector(ui: &mut egui::Ui, data: &InspectorData<'_>) -> Option<InspectorAction> {
    compact_style(ui);
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("对象信息").strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("×").on_hover_text("收起信息面板").clicked() {
                action = Some(InspectorAction::Close);
            }
        });
    });
    let object = matches!(data.selected, Some(BrowserItem::Object { .. }));
    let definition_object = matches!(
        data.selected,
        Some(BrowserItem::Object { .. } | BrowserItem::CatalogObject { .. })
    );
    let page_id = egui::Id::new("object-inspector-page");
    let mut page = ui
        .ctx()
        .data_mut(|state| state.get_temp::<InspectorPage>(page_id).unwrap_or_default());
    if (!object && page == InspectorPage::Fields)
        || (!definition_object && page == InspectorPage::Ddl)
        || (data.query_preview.is_none() && page == InspectorPage::Query)
    {
        page = InspectorPage::General;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        for (value, icon, caption, enabled) in [
            (InspectorPage::General, "常规", "常规", true),
            (InspectorPage::Fields, "字段", "字段", object),
            (InspectorPage::Ddl, "DDL", "定义 / DDL", definition_object),
            (
                InspectorPage::Query,
                "SQL",
                "查询预览",
                data.query_preview.is_some(),
            ),
        ] {
            if ui
                .add_enabled(
                    enabled,
                    egui::Button::selectable(page == value, icon).min_size(egui::vec2(36.0, 26.0)),
                )
                .on_hover_text(caption)
                .clicked()
            {
                page = value;
            }
        }
    });
    ui.ctx().data_mut(|state| state.insert_temp(page_id, page));
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt(("object-inspector", page as u8))
        .auto_shrink([false, false])
        .show(ui, |ui| match page {
            InspectorPage::General => inspector_general(ui, data, &mut action),
            InspectorPage::Fields => inspector_fields(ui, data),
            InspectorPage::Ddl => {
                ui.label(RichText::new("定义 / DDL").strong())
                    .on_hover_text(
                        if data.definition.is_none()
                            && data.structure.is_some()
                            && data
                                .browser
                                .profile
                                .is_some_and(|profile| profile.engine != Engine::MySql)
                        {
                            "由结构元数据生成的有限 DDL"
                        } else {
                            "对象定义"
                        },
                    );
                if let Some(definition) = data.definition {
                    inspector_sql(ui, definition, "复制定义");
                } else if let Some((structure, stale)) = data.structure {
                    inspector_cache_notice(ui, stale);
                    if structure.ddl.is_empty() {
                        ui.label(RichText::new("当前缓存没有 DDL。").weak());
                    } else {
                        inspector_sql(ui, &structure.ddl, "复制 DDL");
                    }
                } else if let Some(item @ BrowserItem::CatalogObject { .. }) = data.selected {
                    ui.label(RichText::new("尚未读取此对象的定义。").weak());
                    if ui
                        .add_enabled(data.browser.item_ready(item), egui::Button::new("读取定义"))
                        .clicked()
                    {
                        action = Some(inspector_browser_action(open_action(item)));
                    }
                } else {
                    ui.label(RichText::new("尚未读取 DDL").weak());
                }
            }
            InspectorPage::Query => {
                if let Some(preview) = &data.query_preview {
                    property(ui, "查询", preview.name);
                    ui.label(RichText::new("当前文稿").strong())
                        .on_hover_text("编辑器中的 SQL，包含未执行的改动");
                    if preview.sql.is_empty() {
                        ui.label(RichText::new("文稿为空。").weak());
                    } else {
                        inspector_sql(ui, preview.sql, "复制当前文稿");
                    }
                    ui.separator();
                    ui.label(RichText::new("最近执行").strong())
                        .on_hover_text("最近发送给服务器的 SQL，可能是选区或当前语句");
                    if let Some(sql) = preview.executed_sql {
                        inspector_sql(ui, sql, "复制最近执行 SQL");
                    } else {
                        ui.label(RichText::new("此文档尚无执行记录。").weak());
                    }
                }
            }
        });
    action
}

fn inspector_cache_notice(ui: &mut egui::Ui, stale: bool) {
    if stale {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "缓存已过期，请在源文档刷新后核对。",
        );
    }
}

fn inspector_sql(ui: &mut egui::Ui, sql: &str, copy_caption: &str) {
    if ui.button(copy_caption).clicked() {
        ui.ctx().copy_text(sql.to_owned());
    }
    ui.add(
        egui::Label::new(RichText::new(sql).monospace())
            .selectable(true)
            .wrap(),
    );
}

fn inspector_general(
    ui: &mut egui::Ui,
    data: &InspectorData<'_>,
    action: &mut Option<InspectorAction>,
) {
    let Some(profile) = data.browser.profile else {
        ui.label(RichText::new("选择连接或数据库对象查看信息").weak());
        return;
    };
    property(ui, "连接", &profile.name);
    property(ui, "数据库引擎", profile.engine.label());
    property(ui, "服务器", &format!("{}:{}", profile.host, profile.port));
    property(
        ui,
        "状态",
        if data.browser.busy {
            "正在操作"
        } else if data.browser.connected {
            "已连接"
        } else {
            "未连接"
        },
    );
    let database = match data.selected {
        Some(BrowserItem::Database { name, .. }) => Some(name.as_str()),
        Some(
            BrowserItem::Object { database, .. } | BrowserItem::CatalogObject { database, .. },
        ) => Some(database.as_str()),
        None => data
            .browser
            .scope
            .and_then(|scope| scope.database.as_deref()),
    };
    if let Some(database) = database {
        property(ui, "数据库", database);
    }
    if let Some(BrowserItem::Object { table, .. }) = data.selected {
        ui.separator();
        property(ui, "名称", &table.name);
        property(ui, "Schema", &table.schema);
        property(ui, "类型", object_kind(table));
        if let Some(object) = data.browser.catalog.and_then(|catalog| {
            catalog.objects.iter().find(|object| {
                object.schema == table.schema
                    && object.name == table.name
                    && matches!(
                        object.kind,
                        ObjectKind::Table | ObjectKind::View | ObjectKind::MaterializedView
                    )
            })
        }) {
            inspector_catalog_properties(ui, object, data.browser);
        }
        ui.horizontal_wrapped(|ui| {
            let ready = data.browser.item_ready(data.selected.unwrap());
            if ui
                .add_enabled(ready, egui::Button::new("打开数据"))
                .clicked()
            {
                *action = Some(inspector_browser_action(open_action(
                    data.selected.unwrap(),
                )));
            }
            if ui.add_enabled(ready, egui::Button::new("结构")).clicked()
                && let Some(next) = structure_action(data.selected.unwrap())
            {
                *action = Some(inspector_browser_action(next));
            }
        });
        if let Some((structure, stale)) = data.structure {
            property(ui, "字段数", &structure.columns.rows.len().to_string());
            property(ui, "索引明细", &structure.indexes.rows.len().to_string());
            property(
                ui,
                "外键明细",
                &structure.foreign_keys.rows.len().to_string(),
            );
            property(ui, "结构状态", if stale { "需要刷新" } else { "已读取" });
        }
        if let Some(metadata) = &data.metadata {
            property(ui, "已加载字段", &metadata.columns.len().to_string());
            property(
                ui,
                "主键",
                &if metadata.primary_keys.is_empty() {
                    "无主键".into()
                } else {
                    metadata.primary_keys.join(", ")
                },
            );
            property(
                ui,
                "表数据编辑",
                if metadata.stale {
                    "快照已过期，请刷新后编辑"
                } else if !metadata.writable {
                    "此对象不支持事务编辑"
                } else if metadata.primary_keys.is_empty() {
                    "无主键，表数据只读"
                } else {
                    "可暂存修改后提交"
                },
            );
        } else if data.structure.is_none() {
            ui.label(RichText::new("尚未读取结构").small().weak());
        }
    } else if let Some(item @ BrowserItem::CatalogObject { object, .. }) = data.selected {
        ui.separator();
        property(ui, "名称", &object.name);
        property(ui, "Schema", &object.schema);
        property(ui, "类型", object.kind.label());
        inspector_catalog_properties(ui, object, data.browser);
        if ui
            .add_enabled(data.browser.item_ready(item), egui::Button::new("查看定义"))
            .clicked()
        {
            *action = Some(inspector_browser_action(open_action(item)));
        }
    } else if let Some(scope) = data.browser.scope {
        if let Some(schema) = &scope.schema {
            property(ui, "Schema", schema);
        }
        if scope.database.as_deref() == data.browser.active_database && scope.database.is_some() {
            property(
                ui,
                "目录中的对象",
                &catalog_rows(data.browser)
                    .into_iter()
                    .filter(|entry| {
                        scope
                            .schema
                            .as_deref()
                            .is_none_or(|schema| schema == entry.schema())
                    })
                    .count()
                    .to_string(),
            );
        }
    }
}

fn inspector_catalog_properties(
    ui: &mut egui::Ui,
    object: &CatalogObject,
    browser: &BrowserData<'_>,
) {
    if let Some(owner) = &object.owner {
        property(ui, "所有者", owner);
    }
    if let Some(engine) = &object.engine {
        property(ui, "存储引擎", engine);
    }
    if let Some(rows) = object.estimated_rows {
        ui.scope(|ui| property(ui, "估计行数", &rows.to_string()))
            .response
            .on_hover_text("来自服务器统计信息，可能与实际记录数不同");
    }
    if let Some(size) = object.size_bytes {
        ui.scope(|ui| property(ui, "目录大小", &format_bytes(size)))
            .response
            .on_hover_text(catalog_size_hint(browser));
    }
    if let Some(created) = &object.created_at {
        property(ui, "创建时间", created);
    }
    if let Some(updated) = &object.updated_at {
        property(ui, "修改时间", updated);
    }
    if let Some(signature) = &object.signature {
        property(ui, "参数 / 签名", signature);
    }
    if let Some(parent) = &object.parent {
        property(ui, "所属对象", parent);
    }
    if let Some(comment) = &object.comment {
        property(
            ui,
            "注释",
            if comment.is_empty() {
                "（无注释）"
            } else {
                comment
            },
        );
    }
}

fn inspector_fields(ui: &mut egui::Ui, data: &InspectorData<'_>) {
    ui.label(RichText::new("字段").strong());
    if let Some((structure, stale)) = data.structure {
        inspector_cache_notice(ui, stale);
        let name_index = structure
            .columns
            .columns
            .iter()
            .position(|name| name == "name");
        let type_index = structure
            .columns
            .columns
            .iter()
            .position(|name| name == "type");
        if structure.columns.rows.is_empty() {
            ui.label(RichText::new("当前结构缓存没有字段。").weak());
        }
        for (index, row) in structure.columns.rows.iter().enumerate() {
            let name = name_index
                .and_then(|index| row.get(index).and_then(Option::as_deref))
                .unwrap_or("未命名字段");
            let data_type = type_index
                .and_then(|index| row.get(index).and_then(Option::as_deref))
                .unwrap_or("未知类型");
            egui::CollapsingHeader::new(format!("{name} · {data_type}"))
                .id_salt(("inspector-field", index, name))
                .default_open(data.focused_column == Some(name))
                .show(ui, |ui| {
                    for (column, value) in structure.columns.columns.iter().zip(row) {
                        let value = match value {
                            None => "NULL",
                            Some(value) if value.is_empty() => "（空字符串）",
                            Some(value) => value,
                        };
                        property(ui, field_property_label(column), value);
                    }
                });
        }
    } else if let Some(metadata) = &data.metadata {
        inspector_cache_notice(ui, metadata.stale);
        if metadata.columns.is_empty() {
            ui.label(RichText::new("当前数据缓存没有字段。").weak());
        }
        for (index, column) in metadata.columns.iter().enumerate() {
            egui::CollapsingHeader::new(format!("{} · {}", column.name, column.declared_type))
                .id_salt(("inspector-field", index, &column.name))
                .default_open(data.focused_column == Some(column.name.as_str()))
                .show(ui, |ui| {
                    property(ui, "名称", &column.name);
                    property(ui, "类型", &column.declared_type);
                    property(ui, "主键", if column.primary_key { "是" } else { "否" });
                    property(ui, "可编辑", if column.editable { "是" } else { "否" });
                });
        }
    } else {
        ui.label(RichText::new("尚未读取字段信息").weak());
    }
}

fn field_property_label(name: &str) -> &str {
    match name {
        "ordinal" => "序号",
        "name" => "名称",
        "type" => "类型",
        "nullable" => "允许 NULL",
        "default" => "默认值",
        "default_or_generation" => "默认值 / 生成表达式",
        "comment" => "注释",
        "extra" => "额外属性",
        "key" => "键",
        "character_set" => "字符集",
        "collation" => "排序规则",
        "identity" => "标识列",
        "generated" | "generated_always" => "生成属性",
        "generation_expression" => "生成表达式",
        "computed" => "计算表达式",
        "default_name" => "默认约束",
        "identity_seed" => "标识初值",
        "identity_increment" => "标识增量",
        "persisted" => "持久化",
        "rowguid" => "行 GUID",
        "sparse" => "稀疏列",
        "hidden" => "隐藏列",
        "privileges" => "权限",
        "storage" => "存储",
        "statistics_target" => "统计目标",
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables() -> Vec<Table> {
        vec![
            Table {
                schema: "public".into(),
                name: "users".into(),
                kind: "BASE TABLE".into(),
            },
            Table {
                schema: "audit".into(),
                name: "users".into(),
                kind: "BASE TABLE".into(),
            },
            Table {
                schema: "public".into(),
                name: "active_users".into(),
                kind: "VIEW".into(),
            },
        ]
    }

    fn data<'a>(
        scope: &'a BrowserScope,
        profile: &'a Profile,
        tables: &'a [Table],
    ) -> BrowserData<'a> {
        BrowserData {
            scope: Some(scope),
            profile: Some(profile),
            connected: true,
            busy: false,
            active_database: Some("app_dev"),
            databases: &[],
            tables,
            catalog: None,
            catalog_loading: false,
            catalog_error: None,
        }
    }

    fn input(time: f64, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        }
    }

    fn text_position(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing label {label}"))
    }

    fn click(pos: egui::Pos2, button: egui::PointerButton, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn filtering_and_scope_identity_keep_same_named_objects_separate() {
        let profile = Profile::default();
        let tables = tables();
        let scope = BrowserScope {
            connection: profile.id,
            database: Some("app_dev".into()),
            schema: None,
        };
        let data = data(&scope, &profile, &tables);
        let mut state = BrowserState::default();
        state.set_scope(Some(&scope));
        assert_eq!(visible_rows(&data, &state).len(), 3);
        state.category = BrowserCategory::Views;
        assert_eq!(visible_rows(&data, &state).len(), 1);
        state.category = BrowserCategory::Tables;
        state.search = "AUDIT".into();
        let rows = visible_rows(&data, &state);
        assert_eq!(rows.len(), 1);
        assert!(matches!(rows[0], BrowserRow::Object(table) if table.schema == "audit"));
        state.selected = Some(rows[0].item(&scope));
        let another = BrowserScope {
            connection: Uuid::new_v4(),
            ..scope.clone()
        };
        state.set_scope(Some(&another));
        assert!(state.selected.is_none());
        assert!(state.search.is_empty());
        assert_eq!(state.category, BrowserCategory::All);
        let schema_scope = BrowserScope {
            schema: Some("public".into()),
            ..scope.clone()
        };
        let schema_data = BrowserData {
            scope: Some(&schema_scope),
            ..data
        };
        assert_eq!(visible_rows(&schema_data, &state).len(), 2);
    }

    #[test]
    fn inactive_database_never_displays_another_databases_cached_objects() {
        let profile = Profile::default();
        let tables = tables();
        let scope = BrowserScope {
            connection: profile.id,
            database: Some("other_db".into()),
            schema: None,
        };
        let data = data(&scope, &profile, &tables);
        let mut state = BrowserState::default();
        assert!(visible_rows(&data, &state).is_empty());
        let ctx = egui::Context::default();
        let mut action = None;
        let output = ctx.run(input(0.0, vec![]), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                action = object_browser(ui, &data, &mut state);
            });
        });
        assert!(action.is_none());
        let position = text_position(&output, "选择数据库");
        for pressed in [true, false] {
            let _ = ctx.run(
                input(
                    if pressed { 0.02 } else { 0.04 },
                    click(position, egui::PointerButton::Primary, pressed),
                ),
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        action = object_browser(ui, &data, &mut state);
                    });
                },
            );
        }
        assert_eq!(
            action,
            Some(BrowserAction::SelectDatabase {
                connection: profile.id,
                database: "other_db".into()
            })
        );
        assert_eq!(data.active_database, Some("app_dev"));
    }

    #[test]
    fn object_single_click_selects_double_click_opens_with_complete_target() {
        for busy in [false, true] {
            let profile = Profile::default();
            let tables = tables();
            let scope = BrowserScope {
                connection: profile.id,
                database: Some("app_dev".into()),
                schema: Some("public".into()),
            };
            let data = BrowserData {
                busy,
                ..data(&scope, &profile, &tables)
            };
            let ctx = egui::Context::default();
            let mut state = BrowserState::default();
            let mut frame = |time, events| {
                let mut action = None;
                let output = ctx.run(input(time, events), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        action = object_browser(ui, &data, &mut state);
                    });
                });
                (output, action)
            };
            frame(0.0, vec![]);
            let position = text_position(&frame(0.01, vec![]).0, "users");
            frame(0.02, click(position, egui::PointerButton::Primary, true));
            assert!(
                frame(0.04, click(position, egui::PointerButton::Primary, false))
                    .1
                    .is_none()
            );
            frame(0.12, click(position, egui::PointerButton::Primary, true));
            let action = frame(0.14, click(position, egui::PointerButton::Primary, false)).1;
            if busy {
                assert!(action.is_none());
            } else {
                assert_eq!(
                    action,
                    Some(BrowserAction::OpenData {
                        connection: profile.id,
                        database: "app_dev".into(),
                        table: tables[0].clone()
                    })
                );
            }
            assert_eq!(
                state.selected,
                Some(BrowserItem::Object {
                    connection: profile.id,
                    database: "app_dev".into(),
                    table: tables[0].clone()
                })
            );
        }
    }

    #[test]
    fn inspector_reports_existing_metadata_and_returns_close_action() {
        let profile = Profile::default();
        let tables = tables();
        let scope = BrowserScope {
            connection: profile.id,
            database: Some("app_dev".into()),
            schema: None,
        };
        let browser = data(&scope, &profile, &tables);
        let selected = BrowserItem::Object {
            connection: profile.id,
            database: "app_dev".into(),
            table: tables[0].clone(),
        };
        let metadata = TableMetadata {
            columns: &[],
            primary_keys: &[],
            writable: false,
            stale: true,
        };
        let data = InspectorData {
            structure: None,
            query_preview: None,
            focused_column: None,
            definition: None,
            browser: &browser,
            selected: Some(&selected),
            metadata: Some(metadata),
        };
        let ctx = egui::Context::default();
        let mut action = None;
        let output = ctx.run(input(0.0, vec![]), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                action = inspector(ui, &data);
            });
        });
        assert!(action.is_none());
        text_position(&output, "快照已过期，请刷新后编辑");
        text_position(&output, "无主键");
        let position = text_position(&output, "×");
        for pressed in [true, false] {
            let _ = ctx.run(
                input(
                    if pressed { 0.02 } else { 0.04 },
                    click(position, egui::PointerButton::Primary, pressed),
                ),
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        action = inspector(ui, &data);
                    });
                },
            );
        }
        assert_eq!(action, Some(InspectorAction::Close));
    }
}
