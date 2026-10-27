use super::*;

impl App {
    pub(super) fn poll(&mut self) {
        self.poll_tree_details();
        self.poll_redis_connection_test();
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
        self.poll_database_opens();
    }

    pub(super) fn invalidate_table_snapshots(&mut self, connection: Uuid) {
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

    pub(super) fn handle_response(&mut self, connection: Uuid, response: Response) {
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
                    session.database_tree = Default::default();
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
                session.database_tree = Default::default();
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
                session.database_tree = Default::default();
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
                session.database_tree = Default::default();
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
                    "已连接服务器 · {} 个数据库 · 展开箭头或双击打开数据库",
                    databases.len()
                );
                // Discovery must not silently alter the worker's database context.
                session.databases = databases;
                session
                    .database_tree
                    .tables
                    .retain(|database, _| session.databases.contains(database));
                session
                    .database_tree
                    .pending
                    .retain(|database| session.databases.contains(database));
                session
                    .database_tree
                    .errors
                    .retain(|database, _| session.databases.contains(database));
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
            Response::TableRenamed {
                database,
                table,
                new_name,
            } => {
                let engine = session
                    .connected
                    .as_ref()
                    .map(|profile| profile.engine)
                    .unwrap_or_default();
                self.apply_table_rename(connection, &database, &table, &new_name, engine);
                session.clear_columns();
                if let Some(tables) = session.database_tree.tables.get_mut(&database) {
                    for known in tables {
                        if *known == table {
                            known.name = new_name.clone();
                        }
                    }
                }
                if session.active_database.as_deref() == Some(database.as_str()) {
                    for known in &mut session.tables {
                        if *known == table {
                            known.name = new_name.clone();
                        }
                    }
                    session.object_catalog = None;
                    follow_up = Some((
                        Request::ObjectCatalog { database },
                        "数据表已重命名 · 正在刷新对象目录…",
                    ));
                }
                self.status = format!("数据表已重命名：{} → {new_name}", table.name);
            }
            Response::Tables(tables) => {
                session.clear_columns();
                self.status = format!("已连接 · {} 个表 / 视图", tables.len());
                session.tables = tables;
                if let Some(database) = session.active_database.clone() {
                    session
                        .database_tree
                        .tables
                        .insert(database.clone(), session.tables.clone());
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
                session.database_tree.errors.remove(&database);
                if session.database_tree.opening.as_ref() == Some(&database) {
                    session.database_tree.opening = None;
                }
                if session.active_database.as_ref() == Some(&database) {
                    session.catalog_error = None;
                    session.clear_columns();
                    session.tables = catalog
                        .objects
                        .iter()
                        .filter_map(|object| object.as_table())
                        .collect();
                    session
                        .database_tree
                        .tables
                        .insert(database.clone(), session.tables.clone());
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
                if let Some(database) = session.database_tree.opening.take() {
                    session
                        .database_tree
                        .errors
                        .insert(database, message.clone());
                }
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
                    session.database_tree = Default::default();
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
}
