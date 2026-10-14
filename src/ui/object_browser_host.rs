use super::object_browser_ui::{
    BrowserAction, BrowserData, BrowserItem, BrowserScope, InspectorAction, InspectorData,
    TableMetadata,
};
use super::*;
use sqlshop::object_catalog::CatalogObject;

pub(super) struct DefinitionDocument {
    pub connection: Uuid,
    pub database: String,
    pub object: CatalogObject,
    pub sql: Option<String>,
    pub error: Option<String>,
    pub stale: bool,
}

pub(super) fn same_object(left: &CatalogObject, right: &CatalogObject) -> bool {
    left.kind == right.kind
        && left.schema == right.schema
        && left.name == right.name
        && left.signature == right.signature
        && left.parent == right.parent
}

pub(super) fn example_catalog() -> sqlshop::object_catalog::ObjectCatalog {
    use sqlshop::object_catalog::{ObjectCatalog, ObjectKind};
    let mut objects = Vec::new();
    for (index, (name, comment)) in [
        ("users", "用户档案与登录信息"),
        ("orders", "订单主表"),
        ("order_items", "订单明细"),
        ("products", "商品目录"),
        ("customers", "客户与联系人"),
        ("payments", "支付流水"),
        ("invoices", "发票记录"),
        ("inventory", "库存与仓位"),
        ("merchants", "商户资料"),
        ("shipments", "发货与物流"),
    ]
    .iter()
    .enumerate()
    {
        objects.push(CatalogObject {
            schema: "public".into(),
            name: (*name).into(),
            kind: ObjectKind::Table,
            owner: Some("app_owner".into()),
            comment: Some((*comment).into()),
            estimated_rows: Some((index as u64 + 1) * 2480),
            size_bytes: Some((index as u64 + 1) * 1048576),
            created_at: None,
            updated_at: None,
            engine: None,
            signature: None,
            parent: None,
        });
    }
    for (kind, name, comment, signature, parent) in [
        (
            ObjectKind::View,
            "active_customers",
            "活跃客户视图",
            None,
            None,
        ),
        (
            ObjectKind::View,
            "pending_orders",
            "待处理订单视图",
            None,
            None,
        ),
        (
            ObjectKind::MaterializedView,
            "daily_sales",
            "每日销售汇总",
            None,
            None,
        ),
        (
            ObjectKind::Function,
            "calculate_total",
            "计算含税总额",
            Some("amount numeric, tax_rate numeric"),
            None,
        ),
        (
            ObjectKind::Function,
            "normalize_email",
            "规范化邮箱地址",
            Some("email text"),
            None,
        ),
        (
            ObjectKind::Procedure,
            "archive_orders",
            "归档历史订单",
            Some("before_date date"),
            None,
        ),
        (
            ObjectKind::Trigger,
            "audit_user_changes",
            "记录用户变更",
            None,
            Some("users"),
        ),
        (
            ObjectKind::Sequence,
            "order_number_seq",
            "订单编号序列",
            None,
            None,
        ),
    ] {
        objects.push(CatalogObject {
            schema: "public".into(),
            name: name.into(),
            kind,
            owner: Some("app_owner".into()),
            comment: Some(comment.into()),
            estimated_rows: None,
            size_bytes: None,
            created_at: None,
            updated_at: None,
            engine: None,
            signature: signature.map(str::to_owned),
            parent: parent.map(str::to_owned),
        });
    }
    ObjectCatalog {
        objects,
        warnings: Vec::new(),
        unavailable_kinds: Vec::new(),
        truncated: false,
    }
}

impl App {
    pub(super) fn open_object_definition(
        &mut self,
        connection: Uuid,
        database: String,
        object: CatalogObject,
    ) {
        if !self.finish_object_navigation() {
            return;
        }
        let target = QueryTarget {
            connection,
            database: Some(database.clone()),
            schema: self
                .profiles
                .iter()
                .find(|profile| profile.id == connection)
                .filter(|profile| profile.engine == Engine::Postgres)
                .map(|_| object.schema.clone()),
        };
        if !self.target_ready(&target) {
            self.fail("请先选择对象所属数据库，再读取定义".into());
            return;
        }
        let existing = self.tabs.iter().position(|tab| {
            tab.definition.as_ref().is_some_and(|document| {
                document.connection == connection
                    && document.database == database
                    && same_object(&document.object, &object)
            })
        });
        let index = existing.unwrap_or_else(|| {
            let mut tab = QueryTab::new(self.next_tab);
            self.next_tab += 1;
            tab.name = format!("{} · 定义", object.display_name());
            tab.sql.clear();
            tab.saved_sql.clear();
            tab.focus_editor = false;
            tab.target = Some(target.clone());
            tab.definition = Some(DefinitionDocument {
                connection,
                database: database.clone(),
                object: object.clone(),
                sql: None,
                error: None,
                stale: false,
            });
            self.tabs.push(tab);
            self.tabs.len() - 1
        });
        self.active_tab = index;
        self.browser_visible = false;
        if self.tabs[index]
            .definition
            .as_ref()
            .is_some_and(|document| document.sql.is_none())
        {
            self.request_on(
                connection,
                Request::ObjectDefinition {
                    tab: self.tabs[index].id,
                    target,
                    object,
                },
                "正在读取对象定义…",
            );
        }
    }

    pub(super) fn object_definition_workspace(&mut self, ui: &mut egui::Ui) {
        let tab = &self.tabs[self.active_tab];
        let Some(document) = &tab.definition else {
            return;
        };
        let connection = document.connection;
        let target = QueryTarget {
            connection,
            database: Some(document.database.clone()),
            schema: None,
        };
        let tab_id = tab.id;
        let object = document.object.clone();
        let sql = document.sql.clone();
        let error = document.error.clone();
        let stale = document.stale;
        let ready = self.target_ready(&target) && !self.session_busy(connection);
        let mut refresh = false;
        let mut draft = false;
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!(
                "{} · {}",
                object.display_name(),
                object.kind.label()
            ));
            ui.label(RichText::new("只读定义").small().weak());
            refresh = ui
                .add_enabled(ready, egui::Button::new("刷新定义"))
                .clicked();
            if ui
                .add_enabled(sql.is_some(), egui::Button::new("复制定义"))
                .clicked()
            {
                ui.ctx().copy_text(sql.clone().unwrap_or_default());
            }
            draft = ui
                .add_enabled(sql.is_some(), egui::Button::new("转为查询草稿"))
                .clicked();
        });
        ui.label(
            RichText::new(format!(
                "{} / {} / {}",
                self.profiles
                    .iter()
                    .find(|profile| profile.id == connection)
                    .map(|profile| profile.name.as_str())
                    .unwrap_or("原连接"),
                target.database.as_deref().unwrap_or_default(),
                object.schema
            ))
            .small()
            .weak(),
        );
        ui.separator();
        if stale {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "连接上下文已变化，显示上次读取的定义；请重新选择所属数据库并刷新核对。",
            );
        }
        if let Some(error) = &error {
            ui.add(
                egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color))
                    .selectable(true)
                    .wrap(),
            );
        }
        if self.tab_busy(self.active_tab) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("正在读取定义…");
            });
        }
        if let Some(sql) = &sql {
            egui::ScrollArea::both()
                .id_salt(("object-definition", tab_id))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(sql).monospace())
                            .selectable(true)
                            .extend(),
                    );
                });
        } else if error.is_none() {
            ui.label(RichText::new("尚未取得可见定义").weak());
        }
        if refresh {
            if let Some(document) = self.tabs[self.active_tab].definition.as_mut() {
                document.error = None;
            }
            self.request_on(
                connection,
                Request::ObjectDefinition {
                    tab: tab_id,
                    target: target.clone(),
                    object: object.clone(),
                },
                "正在刷新对象定义…",
            );
        }
        if draft
            && let Some(sql) = sql
            && self.apply_ui_tab_action(TabAction::New)
        {
            let tab = &mut self.tabs[self.active_tab];
            tab.name = format!("{} · 查询", object.name);
            tab.sql = sql;
            tab.saved_sql.clear();
            let mut target = target;
            if self
                .profiles
                .iter()
                .any(|profile| profile.id == connection && profile.engine == Engine::Postgres)
            {
                target.schema = Some(object.schema);
            }
            tab.target = Some(target);
        }
    }

    fn finish_object_navigation(&mut self) -> bool {
        if self.cell_editor.is_some() {
            self.finish_inline_cell(false, CellMovement::Stay);
        }
        self.cell_editor.is_none()
    }

    pub(super) fn show_object_scope(&mut self, scope: BrowserScope, selected: Option<BrowserItem>) {
        if !self.finish_object_navigation() {
            return;
        }
        self.selected = Some(scope.connection);
        self.browser_state.set_scope(Some(&scope));
        self.browser_state.selected = selected;
        self.browser_scope = Some(scope);
        self.browser_visible = true;
    }

    pub(super) fn show_selected_objects(&mut self) {
        if !self.finish_object_navigation() {
            return;
        }
        if self.browser_scope.is_none()
            && let Some(connection) = self.selected
        {
            let database = self
                .sessions
                .get(&connection)
                .and_then(|session| session.active_database.clone());
            self.show_object_scope(
                BrowserScope {
                    connection,
                    database,
                    schema: None,
                },
                None,
            );
        }
        self.browser_visible = true;
    }

    pub(super) fn object_browser_workspace(&mut self, ui: &mut egui::Ui) {
        let scope = self.browser_scope.as_ref();
        let connection = scope.map(|scope| scope.connection);
        let profile = connection.and_then(|connection| {
            self.profiles
                .iter()
                .find(|profile| profile.id == connection)
        });
        let session = connection.and_then(|connection| self.sessions.get(&connection));
        let data = BrowserData {
            scope,
            profile,
            connected: session.is_some_and(|session| session.connected.is_some()),
            busy: session.is_some_and(|session| session.busy),
            active_database: session.and_then(|session| session.active_database.as_deref()),
            databases: session.map_or(&[], |session| session.databases.as_slice()),
            catalog: session.and_then(|session| session.object_catalog.as_ref()),
            catalog_loading: session.is_some_and(|session| session.catalog_loading),
            catalog_error: session.and_then(|session| session.catalog_error.as_deref()),
            tables: session.map_or(&[], |session| session.tables.as_slice()),
        };
        let action = object_browser_ui::object_browser(ui, &data, &mut self.browser_state);
        if let Some(action) = action {
            self.handle_browser_action(action);
        }
    }

    pub(super) fn object_inspector(&mut self, ui: &mut egui::Ui) {
        let tab = &self.tabs[self.active_tab];
        let (scope, selected) = if self.browser_visible {
            (
                self.browser_scope.clone(),
                self.browser_state.selected.clone(),
            )
        } else if let Some(document) = &tab.definition {
            (
                Some(BrowserScope {
                    connection: document.connection,
                    database: Some(document.database.clone()),
                    schema: Some(document.object.schema.clone()),
                }),
                Some(BrowserItem::CatalogObject {
                    connection: document.connection,
                    database: document.database.clone(),
                    object: document.object.clone(),
                }),
            )
        } else if let Some(target) = &tab.table_target {
            let table =
                self.sessions
                    .get(&target.connection)
                    .filter(|session| session.active_database.as_ref() == Some(&target.database))
                    .and_then(|session| {
                        session.tables.iter().find(|table| {
                            table.schema == target.schema && table.name == target.name
                        })
                    })
                    .cloned()
                    .unwrap_or_else(|| Table {
                        schema: target.schema.clone(),
                        name: target.name.clone(),
                        kind: tab.table_kind.clone().unwrap_or_default(),
                    });
            (
                Some(BrowserScope {
                    connection: target.connection,
                    database: Some(target.database.clone()),
                    schema: Some(target.schema.clone()),
                }),
                Some(BrowserItem::Object {
                    connection: target.connection,
                    database: target.database.clone(),
                    table,
                }),
            )
        } else {
            (
                tab.target.as_ref().map(|target| BrowserScope {
                    connection: target.connection,
                    database: target.database.clone(),
                    schema: target.schema.clone(),
                }),
                None,
            )
        };
        let connection = scope.as_ref().map(|scope| scope.connection);
        let session = connection.and_then(|connection| self.sessions.get(&connection));
        let sql_document = !self.browser_visible
            && !tab.management_document()
            && tab.table_target.is_none()
            && tab.definition.is_none();
        let own_query = self.query_sessions.get(&tab.id);
        let profile = connection.and_then(|connection| {
            self.profiles
                .iter()
                .find(|profile| profile.id == connection)
        });
        let browser = BrowserData {
            scope: scope.as_ref(),
            profile: if sql_document {
                own_query.map(|query| &query.profile).or(profile)
            } else {
                profile
            },
            connected: if sql_document {
                own_query.is_some_and(|query| query.connected)
            } else {
                session.is_some_and(|session| session.connected.is_some())
            },
            busy: if sql_document {
                own_query.is_some_and(|query| query.busy)
            } else {
                session.is_some_and(|session| session.busy)
            },
            active_database: if sql_document {
                tab.target
                    .as_ref()
                    .and_then(|target| target.database.as_deref())
            } else {
                session.and_then(|session| session.active_database.as_deref())
            },
            databases: if sql_document {
                &[]
            } else {
                session.map_or(&[], |session| session.databases.as_slice())
            },
            catalog: if sql_document {
                None
            } else {
                session
                    .and_then(|session| session.object_catalog.as_ref())
                    .filter(|_| {
                        scope.as_ref().and_then(|scope| scope.database.as_ref())
                            == session.and_then(|session| session.active_database.as_ref())
                    })
            },
            catalog_loading: session.is_some_and(|session| session.catalog_loading),
            catalog_error: session.and_then(|session| session.catalog_error.as_deref()),
            tables: if sql_document {
                &[]
            } else {
                session.map_or(&[], |session| session.tables.as_slice())
            },
        };
        let metadata = selected.as_ref().and_then(|selected| match selected {
            BrowserItem::Object {
                connection,
                database,
                table,
            } => self
                .tabs
                .iter()
                .find(|tab| {
                    tab.design.is_none()
                        && tab.table_target.as_ref().is_some_and(|target| {
                            target.connection == *connection
                                && target.database == *database
                                && target.schema == table.schema
                                && target.name == table.name
                        })
                })
                .and_then(|tab| {
                    tab.table_data.as_ref().map(|data| TableMetadata {
                        columns: &data.columns,
                        primary_keys: &data.primary_keys,
                        writable: data.writable,
                        stale: tab.table_snapshot_stale,
                    })
                }),
            BrowserItem::Database { .. } | BrowserItem::CatalogObject { .. } => None,
        });
        let structure = selected.as_ref().and_then(|selected| match selected {
            BrowserItem::Object {
                connection,
                database,
                table,
            } => self.tabs.iter().find_map(|document| {
                if !document.table_target.as_ref().is_some_and(|target| {
                    target.connection == *connection
                        && target.database == *database
                        && target.schema == table.schema
                        && target.name == table.name
                }) {
                    return None;
                }
                document.design.as_ref().and_then(|design| {
                    design
                        .structure
                        .as_ref()
                        .map(|structure| (structure, design.stale))
                })
            }),
            BrowserItem::Database { .. } | BrowserItem::CatalogObject { .. } => None,
        });
        let focused_column = if !self.browser_visible {
            tab.selected_cell.and_then(|(_, column)| {
                tab.table_data
                    .as_ref()?
                    .columns
                    .get(column)
                    .map(|column| column.name.as_str())
            })
        } else {
            None
        };
        let action = object_browser_ui::inspector(
            ui,
            &InspectorData {
                definition: selected
                    .as_ref()
                    .and_then(|selected| {
                        self.tabs
                            .iter()
                            .filter_map(|tab| tab.definition.as_ref())
                            .find(|document| match selected {
                                BrowserItem::CatalogObject {
                                    connection,
                                    database,
                                    object,
                                } => {
                                    document.connection == *connection
                                        && document.database == *database
                                        && same_object(&document.object, object)
                                }
                                BrowserItem::Object {
                                    connection,
                                    database,
                                    table,
                                } => {
                                    document.connection == *connection
                                        && document.database == *database
                                        && document.object.schema == table.schema
                                        && document.object.name == table.name
                                        && document.object.kind.is_relation()
                                }
                                BrowserItem::Database { .. } => false,
                            })
                    })
                    .and_then(|document| document.sql.as_deref()),
                browser: &browser,
                selected: selected.as_ref(),
                metadata,
                structure,
                query_preview: sql_document.then_some(object_browser_ui::QueryPreview {
                    name: &tab.name,
                    sql: &tab.sql,
                    executed_sql: tab.last_executed_sql.as_deref(),
                }),
                focused_column,
            },
        );
        match action {
            Some(InspectorAction::Close) => self.inspector_visible = false,
            Some(InspectorAction::Browser(action)) => self.handle_browser_action(*action),
            None => {}
        }
    }

    pub(super) fn handle_browser_action(&mut self, action: BrowserAction) {
        if matches!(
            action,
            BrowserAction::Connect(_)
                | BrowserAction::SelectDatabase { .. }
                | BrowserAction::OpenData { .. }
                | BrowserAction::OpenStructure { .. }
                | BrowserAction::NewQuery { .. }
                | BrowserAction::CreateTable { .. }
        ) && !self.finish_object_navigation()
        {
            return;
        }
        match action {
            BrowserAction::Connect(connection) => {
                self.selected = Some(connection);
                self.connect_selected();
            }
            BrowserAction::SelectDatabase {
                connection,
                database,
            } => {
                self.show_object_scope(
                    BrowserScope {
                        connection,
                        database: Some(database.clone()),
                        schema: None,
                    },
                    None,
                );
                self.tree_selection.selected =
                    Some(TreeNode::Database(connection, database.clone()));
                self.expand_database = Some((connection, database.clone()));
                if !self.sessions.get(&connection).is_some_and(|session| {
                    session.connected.is_some()
                        && session.active_database.as_ref() == Some(&database)
                }) {
                    self.request_on(
                        connection,
                        Request::SelectDatabase(database),
                        "正在选择数据库…",
                    );
                }
            }
            BrowserAction::RefreshDatabases(connection) => {
                self.request_on(connection, Request::Databases, "正在刷新数据库目录…")
            }
            BrowserAction::RefreshObjects {
                connection,
                database,
            } => {
                let target = QueryTarget {
                    schema: None,
                    connection,
                    database: Some(database),
                };
                if self.target_ready(&target) {
                    self.request_on(
                        connection,
                        Request::ObjectCatalog {
                            database: target.database.clone().unwrap(),
                        },
                        "正在刷新对象目录与属性…",
                    );
                }
            }
            BrowserAction::LoadCatalog {
                connection,
                database,
            } => {
                self.request_on(
                    connection,
                    Request::ObjectCatalog { database },
                    "正在读取对象目录与属性…",
                );
            }
            BrowserAction::OpenDefinition {
                connection,
                database,
                object,
            } => {
                self.open_object_definition(connection, database, object);
            }
            BrowserAction::OpenData {
                connection,
                database,
                table,
            } => {
                if let Some((index, created)) = self.open_browser_table(connection, database, table)
                {
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
            BrowserAction::OpenStructure {
                connection,
                database,
                table,
            } => {
                self.open_structure_document(connection, database, table);
            }
            BrowserAction::NewQuery {
                connection,
                database,
                schema,
                table,
            } => {
                if !self.apply_ui_tab_action(TabAction::New) {
                    return;
                }
                let tab = &mut self.tabs[self.active_tab];
                tab.target = Some(QueryTarget {
                    schema: self
                        .profiles
                        .iter()
                        .any(|profile| {
                            profile.id == connection && profile.engine == Engine::Postgres
                        })
                        .then(|| {
                            schema
                                .or_else(|| table.as_ref().map(|table| table.schema.clone()))
                                .or_else(|| {
                                    self.browser_scope
                                        .as_ref()
                                        .and_then(|scope| scope.schema.clone())
                                })
                        })
                        .flatten(),
                    connection,
                    database,
                });
                if let Some(table) = table
                    && let Some(profile) = self
                        .profiles
                        .iter()
                        .find(|profile| profile.id == connection)
                {
                    tab.sql = profile.engine.preview_sql(&table);
                    tab.saved_sql = tab.sql.clone();
                    tab.name = format!("{} · 查询", table.name);
                }
                self.browser_visible = false;
            }
            BrowserAction::CreateTable {
                connection,
                database,
                schema,
            } => {
                let target = QueryTarget {
                    schema: None,
                    connection,
                    database: Some(database.clone()),
                };
                if !self.target_ready(&target) || self.session_busy(connection) {
                    return;
                }
                let Some(profile) = self
                    .profiles
                    .iter()
                    .find(|profile| profile.id == connection)
                else {
                    return;
                };
                let engine = profile.engine;
                let schema = schema.unwrap_or_else(|| match engine {
                    Engine::MySql => database.clone(),
                    Engine::Postgres => "public".into(),
                    Engine::SqlServer => "dbo".into(),
                });
                self.table_create =
                    Some(new_table_create_form(connection, database, schema, engine));
                self.browser_visible = false;
            }
        }
    }

    fn open_browser_table(
        &mut self,
        connection: Uuid,
        database: String,
        table: Table,
    ) -> Option<(usize, bool)> {
        if !self.finish_object_navigation() {
            return None;
        }
        let target = QueryTarget {
            schema: None,
            connection,
            database: Some(database.clone()),
        };
        if !self.target_ready(&target) || self.session_busy(connection) {
            self.fail("请先选择此表所属数据库并等待当前操作结束".into());
            return None;
        }
        let engine = self.sessions.get(&connection)?.connected.as_ref()?.engine;
        self.show_object_scope(
            BrowserScope {
                connection,
                database: Some(database.clone()),
                schema: Some(table.schema.clone()),
            },
            Some(BrowserItem::Object {
                connection,
                database: database.clone(),
                table: table.clone(),
            }),
        );
        let target = TableTarget {
            connection,
            database,
            schema: table.schema.clone(),
            name: table.name.clone(),
        };
        let result = open_table_tab(&mut self.tabs, &mut self.next_tab, target, engine, &table);
        self.active_tab = result.0;
        self.browser_visible = false;
        Some(result)
    }

    pub(super) fn workspace_navigation(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 0.0);
        ui.spacing_mut().button_padding = egui::vec2(7.0, 4.0);
        let action = ui
            .scope(|ui| {
                tab_bar_ui::tab_bar(
                    ui,
                    &self.tabs,
                    &self.profiles,
                    self.active_tab,
                    self.browser_visible,
                    self.cell_editor.is_some(),
                    self.inspector_visible
                        && !self.view.focus
                        && (self.view.information_force
                            || ui.ctx().content_rect().width() >= 1100.0),
                )
            })
            .inner;
        ui.spacing_mut().item_spacing.y = 4.0;
        if matches!(
            action,
            Some(
                tab_bar_ui::BarAction::Objects
                    | tab_bar_ui::BarAction::Document(TabAction::Select(_) | TabAction::New)
            )
        ) && self.cell_editor.is_some()
        {
            self.finish_inline_cell(false, CellMovement::Stay);
            if self.cell_editor.is_some() {
                return;
            }
        }
        match action {
            Some(tab_bar_ui::BarAction::Objects) => self.show_selected_objects(),
            Some(tab_bar_ui::BarAction::Document(action)) => self.change_tab(action),
            Some(tab_bar_ui::BarAction::Information) => {
                let visible = self.inspector_visible
                    && !self.view.focus
                    && (self.view.information_force || ui.ctx().content_rect().width() >= 1100.0);
                self.inspector_visible = !visible;
                self.view.information_force = self.inspector_visible;
                self.view.focus = false;
            }
            Some(tab_bar_ui::BarAction::Reorder { from, to })
                if self.cell_editor.is_none() && from < self.tabs.len() && to < self.tabs.len() =>
            {
                let active = self.tabs[self.active_tab].id;
                let document = self.tabs.remove(from);
                self.tabs.insert(to, document);
                self.active_tab = self
                    .tabs
                    .iter()
                    .position(|tab| tab.id == active)
                    .unwrap_or(0);
            }
            Some(tab_bar_ui::BarAction::Reorder { .. }) | None => {}
        }
    }

    pub(super) fn workspace_status(&self) -> (String, bool) {
        if self.browser_visible {
            let session = self
                .browser_scope
                .as_ref()
                .and_then(|scope| self.sessions.get(&scope.connection));
            return session.map_or_else(
                || (self.status.clone(), self.error),
                |session| (session.status.clone(), session.error),
            );
        }
        let tab = &self.tabs[self.active_tab];
        if let Some(doc) = &tab.advanced {
            return (
                doc.error
                    .clone()
                    .or_else(|| doc.notice.clone())
                    .unwrap_or_else(|| {
                        format!(
                            "{} · {}",
                            doc.kind.label(),
                            if doc.busy {
                                "处理中"
                            } else {
                                "独立文档"
                            }
                        )
                    }),
                doc.error.is_some(),
            );
        }
        if let Some(doc) = &tab.backup {
            return (
                doc.error
                    .clone()
                    .or_else(|| doc.result.as_ref().map(|result| result.summary.clone()))
                    .unwrap_or_else(|| {
                        if doc.pending.is_some() {
                            "备份任务处理中".into()
                        } else {
                            "备份与恢复 · 尚未执行".into()
                        }
                    }),
                doc.error.is_some(),
            );
        }
        if let Some(doc) = &tab.compare {
            return (
                doc.error.clone().unwrap_or_else(|| doc.status.clone()),
                doc.error.is_some(),
            );
        }
        if let Some(document) = &tab.definition {
            return (
                format!(
                    "{} · {} · {}",
                    document.object.kind.label(),
                    document.object.display_name(),
                    if self.tab_busy(self.active_tab) {
                        "正在读取"
                    } else {
                        "只读定义"
                    }
                ),
                document.error.is_some(),
            );
        }
        if tab.table_target.is_some() {
            if let Some(doc) = &tab.design {
                return (doc.status.clone(), doc.error.is_some());
            }
            return tab
                .target
                .as_ref()
                .and_then(|target| self.sessions.get(&target.connection))
                .map_or_else(
                    || (self.status.clone(), self.error),
                    |session| (session.status.clone(), session.error),
                );
        }
        if let Some(error) = &tab.query_error {
            return (error.clone(), !error.starts_with("查询已停止"));
        }
        if self
            .query_sessions
            .get(&tab.id)
            .is_some_and(|session| session.busy)
        {
            return ("正在执行查询…".into(), false);
        }
        if let Some(output) = &tab.output {
            return (
                format!(
                    "查询完成 · {} ms · {} 个结果集",
                    output.elapsed_ms,
                    output.sets.len()
                ),
                false,
            );
        }
        if self
            .query_sessions
            .get(&tab.id)
            .is_some_and(|session| session.connected)
        {
            return ("查询会话已连接".into(), false);
        }
        (self.status.clone(), self.error)
    }
}
