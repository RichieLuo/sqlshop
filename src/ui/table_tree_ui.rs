use super::*;
use sqlshop::table_metadata_worker::{TableMetadataRequest, TableMetadataWorker, TableTreeDetails};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum TablePart {
    Fields,
    Indexes,
    ForeignKeys,
    Triggers,
}

impl TablePart {
    fn label(self) -> &'static str {
        match self {
            Self::Fields => "字段",
            Self::Indexes => "索引",
            Self::ForeignKeys => "外键",
            Self::Triggers => "触发器",
        }
    }
    fn section(self) -> Option<structure_ui::DesignSection> {
        match self {
            Self::Fields => Some(structure_ui::DesignSection::Fields),
            Self::Indexes => Some(structure_ui::DesignSection::Indexes),
            Self::ForeignKeys => Some(structure_ui::DesignSection::ForeignKeys),
            Self::Triggers => None,
        }
    }
}

pub(super) type TreeKey = (String, String, String, String);
pub(super) fn key(database: &str, table: &Table) -> TreeKey {
    (
        database.into(),
        table.schema.clone(),
        table.name.clone(),
        table.kind.clone(),
    )
}

#[derive(Clone)]
pub(super) struct TreeEntry {
    pub generation: Uuid,
    pub loading: bool,
    pub data: Option<Arc<TableTreeDetails>>,
    pub error: Option<String>,
}

pub(super) struct TreeMetadata {
    pub worker: TableMetadataWorker,
    pub entries: HashMap<TreeKey, TreeEntry>,
}

impl TreeMetadata {
    pub fn new(ctx: &egui::Context) -> Self {
        let ctx = ctx.clone();
        Self {
            worker: TableMetadataWorker::start(move || ctx.request_repaint()),
            entries: HashMap::new(),
        }
    }
}

#[derive(Default)]
pub(super) struct TreeRender {
    pub entries: HashMap<TreeKey, TreeEntry>,
    pub requests: Vec<(Uuid, String, Table, bool)>,
    pub action: Option<(Uuid, String, Table, TablePart, Option<String>)>,
    pub trigger: Option<object_browser_ui::BrowserAction>,
    pub generation: u64,
    pub fixture: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn tables(
    ui: &mut egui::Ui,
    tables: &[&Table],
    selection: &mut TreeSelection,
    connection: Uuid,
    database: &str,
    busy: bool,
    schema_dialog: &mut Option<SchemaDialog>,
    render: &mut TreeRender,
) -> Option<Table> {
    let mut preview = None;
    for &table in tables {
        let background = ui.painter().add(egui::Shape::Noop);
        let state = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            ui.make_persistent_id((
                "table-children",
                connection,
                database,
                &table.schema,
                &table.name,
                &table.kind,
                render.generation,
            )),
            render.fixture && table.name == "users",
        );
        let header = state.show_header(ui, |ui| {
            let response = ui
                .add(
                    egui::Button::new((
                        RichText::new(&table.name).color(ui.visuals().text_color()),
                        egui::Atom::grow(),
                    ))
                    .frame(false)
                    .min_size(egui::vec2(
                        ui.available_width(),
                        ui.spacing().interact_size.y,
                    )),
                )
                .on_hover_text("单击选中 · 双击打开数据 · 左侧箭头展开字段、索引、外键和触发器");
            selection.row(
                &response,
                background,
                TreeNode::Table(connection, database.into(), table.clone()),
            );
            response.context_menu(|ui| {
                if ui.button("刷新表节点").clicked() {
                    render
                        .requests
                        .push((connection, database.into(), table.clone(), true));
                    ui.close();
                }
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
                            database: database.into(),
                            table: table.clone(),
                        },
                        name: String::new(),
                    });
                    ui.close();
                }
            });
            if response.double_clicked() && !busy {
                preview = Some(table.clone());
            }
        });
        header.body(|ui| {
            let entry = render.entries.get(&key(database, table)).cloned();
            if entry.is_none() {
                render.requests.push((connection, database.into(), table.clone(), false));
            }
            let data = entry.as_ref().and_then(|entry| entry.data.as_deref());
            for part in [TablePart::Fields, TablePart::Indexes, TablePart::ForeignKeys, TablePart::Triggers] {
                let result = data.map(|data| match part {
                    TablePart::Fields => data.structure.as_ref().map(|s| &s.columns),
                    TablePart::Indexes => data.structure.as_ref().map(|s| &s.indexes),
                    TablePart::ForeignKeys => data.structure.as_ref().map(|s| &s.foreign_keys),
                    TablePart::Triggers => data.triggers.as_ref(),
                });
                let set = result.as_ref().and_then(|result| result.as_ref().ok()).copied();
                let name_index = set.and_then(|set| set.columns.iter().position(|name| name == "name")).unwrap_or(0);
                let mut names = Vec::new();
                if let Some(set) = set {
                    for row in &set.rows {
                        if let Some(name) = row.get(name_index).and_then(Option::as_deref)
                            && !names.contains(&name) { names.push(name); }
                    }
                }
                let caption = set.map_or_else(|| part.label().to_owned(), |set| format!("{} ({}){}", part.label(), names.len(), if set.truncated { "+" } else { "" }));
                let part_background = ui.painter().add(egui::Shape::Noop);
                let state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(),
                    ui.make_persistent_id(("table-part", connection, database, &table.schema, &table.name, part, render.generation)), render.fixture && table.name == "users");
                state.show_header(ui, |ui| {
                    let response = ui.add(egui::Button::new((caption, egui::Atom::grow())).frame(false)
                        .min_size(egui::vec2(ui.available_width(), ui.spacing().interact_size.y)));
                    selection.row(&response, part_background, TreeNode::TablePart(connection, database.into(), table.clone(), part, None));
                    if response.double_clicked() && part.section().is_some() && !busy {
                        render.action = Some((connection, database.into(), table.clone(), part, None));
                    }
                }).body(|ui| {
                    if let Some(error) = entry.as_ref().and_then(|e| e.error.as_deref()) {
                        ui.colored_label(ui.visuals().warn_fg_color, "读取失败").on_hover_text(error);
                        if ui.small_button("重试").clicked() { render.requests.push((connection, database.into(), table.clone(), true)); }
                    } else if entry.as_ref().is_none_or(|e| e.loading) {
                        ui.label(RichText::new("正在读取…").small().weak());
                    } else if let Some(Err(error)) = result.as_ref() {
                        ui.colored_label(ui.visuals().warn_fg_color, "无法读取目录").on_hover_text(*error);
                        if ui.small_button("重试").clicked() { render.requests.push((connection, database.into(), table.clone(), true)); }
                    } else if let Some(set) = set {
                        if names.is_empty() { ui.label(RichText::new("未返回可见对象").small().weak()); }
                        for name in &names {
                            let rows: Vec<_> = set.rows.iter().filter(|row| row.get(name_index).and_then(Option::as_deref) == Some(*name)).collect();
                            let detail = rows.iter().map(|row| set.columns.iter().zip(row.iter()).filter_map(|(column, value)| value.as_ref().map(|value| format!("{column}: {value}"))).collect::<Vec<_>>().join("\n")).collect::<Vec<_>>().join("\n\n");
                            let label = if part == TablePart::Fields {
                                let type_index = set.columns.iter().position(|name| name == "type");
                                let ty = rows.first().and_then(|row| type_index.and_then(|i| row.get(i))).and_then(Option::as_deref).unwrap_or("");
                                format!("{name}  {ty}")
                            } else { (*name).to_owned() };
                            let background = ui.painter().add(egui::Shape::Noop);
                            let response = ui.add(egui::Button::new((RichText::new(label).color(ui.visuals().text_color()), egui::Atom::grow())).frame(false)
                                .min_size(egui::vec2(ui.available_width(), ui.spacing().interact_size.y))).on_hover_text(detail);
                            selection.row(&response, background, TreeNode::TablePart(connection, database.into(), table.clone(), part, Some((*name).into())));
                            if response.double_clicked() && !busy {
                                if part == TablePart::Triggers {
                                    let schema = rows.first().and_then(|row| row.first()).and_then(Option::as_deref).unwrap_or(&table.schema);
                                    render.trigger = Some(object_browser_ui::BrowserAction::OpenDefinition { connection, database: database.into(), object: sqlshop::object_catalog::CatalogObject {
                                        schema: schema.into(), name: (*name).into(), kind: sqlshop::object_catalog::ObjectKind::Trigger,
                                        parent: Some(table.name.clone()), owner: None, comment: None, estimated_rows: None, size_bytes: None,
                                        created_at: None, updated_at: None, engine: None, signature: None,
                                    } });
                                } else { render.action = Some((connection, database.into(), table.clone(), part, Some((*name).into()))); }
                            }
                        }
                        if set.truncated { ui.label(RichText::new("目录已截断，刷新或使用SQL查看完整目录").small().weak()); }
                    }
                });
            }
        });
    }
    preview
}

impl App {
    pub(super) fn table_tree_fixture(&mut self) {
        if self.screenshot_path.is_none() {
            return;
        }
        let mode = std::env::var("SQLSHOP_SCREENSHOT_VIEW").unwrap_or_default();
        if mode == "grid-selection" {
            let tab = &mut self.tabs[self.active_tab];
            tab.cell_anchor = Some((0, 1));
            tab.selected_cell = Some((2, 3));
            tab.selected_rows = None;
        }
        if mode != "tree-details" {
            return;
        }
        let Some(profile) = self.profiles.first() else {
            return;
        };
        let connection = profile.id;
        let Some(session) = self.sessions.get_mut(&connection) else {
            return;
        };
        let Some(table) = session
            .tables
            .iter()
            .find(|table| table.name == "users")
            .cloned()
        else {
            return;
        };
        let set = |columns: &[&str], rows: &[&[&str]]| ResultSet {
            columns: columns.iter().map(|column| (*column).into()).collect(),
            rows: rows
                .iter()
                .map(|row| row.iter().map(|value| Some((*value).into())).collect())
                .collect(),
            ..Default::default()
        };
        let details = TableTreeDetails {
            structure: Ok(TableStructure {
                columns: set(&["ordinal", "name", "type", "nullable"], &[&["1", "id", "integer", "false"], &["2", "name", "varchar(100)", "true"], &["3", "department_id", "integer", "true"]]),
                indexes: set(&["name", "column", "primary", "unique"], &[&["users_pkey", "id", "true", "true"], &["idx_users_name", "name", "false", "false"], &["idx_users_name", "department_id", "false", "false"]]),
                foreign_keys: set(&["name", "column", "referenced_schema", "referenced_table", "referenced_column"], &[&["fk_users_department", "department_id", "public", "departments", "id"]]),
                ddl: "-- Offline layout example; no database connection.\nCREATE TABLE public.users (id integer PRIMARY KEY, name varchar(100), department_id integer REFERENCES public.departments(id));".into(),
            }),
            triggers: Ok(set(&["schema", "name", "state", "event", "definition"], &[&["public", "users_before_save", "enabled", "BEFORE UPDATE", "-- Offline layout example; not a database definition"]])),
        };
        session.tree_metadata.entries.insert(
            key("app_dev", &table),
            TreeEntry {
                generation: Uuid::new_v4(),
                loading: false,
                data: Some(Arc::new(details)),
                error: None,
            },
        );
        self.browser_visible = true;
        self.browser_scope = Some(object_browser_ui::BrowserScope {
            connection,
            database: Some("app_dev".into()),
            schema: Some(table.schema.clone()),
        });
        self.browser_state.set_scope(self.browser_scope.as_ref());
        self.browser_state.selected = Some(object_browser_ui::BrowserItem::Object {
            connection,
            database: "app_dev".into(),
            table: table.clone(),
        });
        self.tree_selection.selected = Some(TreeNode::Table(connection, "app_dev".into(), table));
        self.status = "离线结构树示例 · 未连接数据库".into();
    }

    pub(super) fn poll_tree_details(&mut self) {
        for (connection, session) in &mut self.sessions {
            let same_profile = self
                .profiles
                .iter()
                .find(|p| p.id == *connection)
                .zip(session.connected.as_ref())
                .is_some_and(|(a, b)| serde_json::to_value(a).ok() == serde_json::to_value(b).ok());
            if !same_profile {
                session.tree_metadata.entries.clear();
            }
            for response in session.tree_metadata.worker.receiver.try_iter() {
                if !same_profile
                    || session.active_database != response.target.database
                    || session.connected.as_ref().is_none_or(|profile| {
                        serde_json::to_value(profile).ok()
                            != serde_json::to_value(&response.profile).ok()
                    })
                {
                    continue;
                }
                let Some(database) = response.target.database.as_deref() else {
                    continue;
                };
                let Some(entry) = session
                    .tree_metadata
                    .entries
                    .get_mut(&key(database, &response.table))
                else {
                    continue;
                };
                if entry.generation != response.generation {
                    continue;
                }
                entry.loading = false;
                match response.result {
                    Ok(data) => {
                        entry.data = Some(Arc::new(data));
                        entry.error = None;
                    }
                    Err(error) => entry.error = Some(error),
                }
            }
        }
    }

    pub(super) fn request_tree_details(&mut self, requests: Vec<(Uuid, String, Table, bool)>) {
        if self.screenshot_path.is_some() {
            return;
        }
        for (connection, database, table, refresh) in requests {
            let Some(session) = self.sessions.get(&connection) else {
                continue;
            };
            let Some(profile) = session.connected.clone() else {
                continue;
            };
            if session.active_database.as_deref() != Some(&database) {
                continue;
            }
            let key = key(&database, &table);
            if session
                .tree_metadata
                .entries
                .get(&key)
                .is_some_and(|entry| entry.loading || !refresh)
            {
                continue;
            }
            if self
                .profiles
                .iter()
                .find(|p| p.id == connection)
                .is_none_or(|current| {
                    serde_json::to_value(current).ok() != serde_json::to_value(&profile).ok()
                })
            {
                continue;
            }
            let generation = Uuid::new_v4();
            let credentials = self.credentials_for(&profile);
            let Some(session) = self.sessions.get_mut(&connection) else {
                continue;
            };
            let result = match credentials {
                Ok(Some(credentials)) => {
                    session.tree_metadata.worker.request(TableMetadataRequest {
                        generation,
                        profile,
                        credentials,
                        target: QueryTarget {
                            connection,
                            database: Some(database),
                            schema: Some(table.schema.clone()),
                        },
                        table,
                    })
                }
                Ok(None) => Err(anyhow::anyhow!("缺少读取目录所需的凭据，请重新连接")),
                Err(error) => Err(error),
            };
            match result {
                Ok(false) => self
                    .context
                    .request_repaint_after(std::time::Duration::from_millis(100)),
                result => {
                    session.tree_metadata.entries.insert(
                        key,
                        TreeEntry {
                            generation,
                            loading: result.is_ok(),
                            data: None,
                            error: result.err().map(|error| error.to_string()),
                        },
                    );
                }
            }
        }
    }

    pub(super) fn open_tree_part(
        &mut self,
        action: (Uuid, String, Table, TablePart, Option<String>),
    ) {
        let (connection, database, table, part, name) = action;
        let Some(section) = part.section() else {
            return;
        };
        self.open_structure_document(connection, database.clone(), table.clone());
        if let Some(tab) = self.tabs.get_mut(self.active_tab)
            && tab.table_target.as_ref().is_some_and(|target| {
                target.connection == connection
                    && target.database == database
                    && target.schema == table.schema
                    && target.name == table.name
            })
            && let Some(doc) = &mut tab.design
        {
            doc.select_tree_item(section, name);
        }
    }
}
