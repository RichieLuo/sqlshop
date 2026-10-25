mod actions;

mod definitions;

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
        self.select_object_scope(scope, selected);
        self.browser_visible = true;
    }

    // Selection updates the browser model without changing the visible document
    // or ending/staging an inline cell edit. Activation owns navigation.
    pub(super) fn select_object_scope(
        &mut self,
        scope: BrowserScope,
        selected: Option<BrowserItem>,
    ) {
        self.selected = Some(scope.connection);
        self.browser_state.set_scope(Some(&scope));
        self.browser_state.selected = selected;
        self.browser_scope = Some(scope);
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
        let structure = selected
            .as_ref()
            .and_then(|selected| match selected {
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
            })
            .or_else(|| {
                let BrowserItem::Object {
                    connection,
                    database,
                    table,
                } = selected.as_ref()?
                else {
                    return None;
                };
                let entry = self
                    .sessions
                    .get(connection)?
                    .tree_metadata
                    .entries
                    .get(&table_tree_ui::key(database, table))?;
                entry
                    .data
                    .as_ref()?
                    .structure
                    .as_ref()
                    .ok()
                    .map(|structure| (structure, false))
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
