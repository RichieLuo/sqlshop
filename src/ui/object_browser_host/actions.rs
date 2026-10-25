use super::*;

impl App {
    pub(in crate::ui) fn handle_browser_action(&mut self, action: BrowserAction) {
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

    pub(super) fn open_browser_table(
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
}
