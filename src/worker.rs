use crate::object_catalog::{CatalogObject, ObjectCatalog};
use crate::{
    db::{Session, StructureChange, TableBrowseOptions, TablePage, TableStructure},
    model::{
        CellUpdate, CreateTableColumn, Credentials, Engine, InsertValue, Profile, QueryOutput,
        Table, TableData,
    },
    workspace::QueryTarget,
};
use std::{sync::mpsc, time::Duration};
use uuid::Uuid;

pub enum Request {
    ReconnectTableManagement {
        tab: Uuid,
        epoch: u64,
        profile: Profile,
        credentials: Credentials,
        target: QueryTarget,
    },
    Test(Profile, Credentials),
    Connect(Profile, Credentials),
    Disconnect,
    Databases,
    SelectDatabase(String),
    Tables,
    ObjectCatalog {
        database: String,
    },
    ObjectDefinition {
        tab: Uuid,
        target: QueryTarget,
        object: CatalogObject,
    },
    Query(String),
    RunQuery {
        tab: Uuid,
        target: QueryTarget,
        sql: String,
    },
    ReadPage {
        tab: Uuid,
        target: QueryTarget,
        table: Table,
        options: TableBrowseOptions,
    },
    ReadLastPage {
        tab: Uuid,
        target: QueryTarget,
        table: Table,
        options: TableBrowseOptions,
    },
    DescribeTable {
        tab: Uuid,
        target: QueryTarget,
        table: Table,
    },
    CompleteColumns {
        target: QueryTarget,
        table: Table,
        generation: u64,
    },
    SaveCell {
        tab: Uuid,
        target: QueryTarget,
        change: CellUpdate,
    },
    ReadTable {
        tab: Uuid,
        database: String,
        table: Table,
    },
    UpdateCell {
        tab: Uuid,
        change: CellUpdate,
    },
    DeleteRows {
        tab: Uuid,
        target: QueryTarget,
        table: Table,
        primary_keys: Vec<String>,
        key_values: Vec<Vec<Option<String>>>,
    },
    InsertDefaultRow {
        tab: Uuid,
        target: QueryTarget,
        table: Table,
    },
    InsertRows {
        tab: Uuid,
        target: QueryTarget,
        table: Table,
        columns: Vec<String>,
        rows: Vec<Vec<InsertValue>>,
    },
    SaveCells {
        tab: Uuid,
        target: QueryTarget,
        changes: Vec<CellUpdate>,
    },
    SaveTableDraft {
        tab: Uuid,
        target: QueryTarget,
        table: Table,
        changes: Vec<CellUpdate>,
        columns: Vec<String>,
        rows: Vec<Vec<InsertValue>>,
    },
    ExportTable {
        tab: Uuid,
        target: QueryTarget,
        table: Table,
        options: TableBrowseOptions,
        path: std::path::PathBuf,
    },
    CreateDatabase(String),
    DropDatabase(String),
    CreateTable {
        database: String,
        schema: String,
        name: String,
        columns: Vec<CreateTableColumn>,
    },
    DropTable {
        database: String,
        table: Table,
    },
    ChangeStructure {
        tab: Uuid,
        target: QueryTarget,
        table: Table,
        change: StructureChange,
        preview_sql: String,
    },
}

pub enum Response {
    TableManagementReconnected {
        tab: Uuid,
        epoch: u64,
        target: QueryTarget,
        profile: Profile,
    },
    QueryResult {
        tab: Uuid,
        output: QueryOutput,
        database: Option<String>,
    },
    TablePage {
        tab: Uuid,
        page: TablePage,
    },
    TableLastPage {
        tab: Uuid,
        page: TablePage,
        offset: u64,
    },
    TableStructure {
        tab: Uuid,
        structure: TableStructure,
    },
    CompleteColumns {
        target: QueryTarget,
        table: Table,
        generation: u64,
        columns: Vec<String>,
    },
    StructureChanged {
        tab: Uuid,
        structure: TableStructure,
    },
    OperationError {
        tab: Uuid,
        message: String,
        disconnected: bool,
    },
    Tested,
    Connected(Uuid),
    Disconnected,
    Databases(Vec<String>),
    DatabaseSelected(String),
    DatabaseContext(Option<String>),
    Tables(Vec<Table>),
    ObjectCatalog {
        database: String,
        catalog: ObjectCatalog,
    },
    ObjectDefinition {
        tab: Uuid,
        target: QueryTarget,
        object: CatalogObject,
        definition: String,
    },
    TableData {
        tab: Uuid,
        data: TableData,
    },
    CellSaved {
        tab: Uuid,
    },
    RowsDeleted {
        tab: Uuid,
        count: usize,
    },
    RowInserted {
        tab: Uuid,
    },
    RowsInserted {
        tab: Uuid,
        count: usize,
    },
    CellsSaved {
        tab: Uuid,
        count: usize,
    },
    TableDraftSaved {
        tab: Uuid,
        updated_cells: usize,
        inserted_rows: usize,
    },
    TableExported {
        tab: Uuid,
        count: u64,
        path: std::path::PathBuf,
    },
    Query {
        output: QueryOutput,
        database: Option<String>,
    },
    Error {
        message: String,
        disconnected: bool,
    },
}

fn target_matches(target: &QueryTarget, connection: Option<Uuid>, database: Option<&str>) -> bool {
    Some(target.connection) == connection && target.database.as_deref() == database
}

fn operation_error(tab: Uuid, message: &str, disconnected: bool) -> Response {
    Response::OperationError {
        tab,
        message: message.to_owned(),
        disconnected,
    }
}

pub struct Worker {
    sender: tokio::sync::mpsc::UnboundedSender<Request>,
    pub receiver: mpsc::Receiver<Response>,
}

impl Worker {
    pub fn start(wake: impl Fn() + Send + 'static) -> Self {
        let (sender, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let (responses, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            // A worker owns one serialized connection. A current-thread runtime
            // keeps simultaneous connections from each allocating a CPU-sized pool.
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = responses.send(Response::Error {
                        message: error.to_string(),
                        disconnected: true,
                    });
                    wake();
                    return;
                }
            };
            runtime.block_on(async move {
                let mut session = None;
                let mut connection_id = None;
                let mut engine = None;
                let mut physical_database = None;
                let mut selected_database = None;
                while let Some(request) = requests.recv().await {
                    let modifies_session = !matches!(&request, Request::Test(..));
                    let read_only_operation = matches!(&request, Request::ObjectCatalog { .. } | Request::ObjectDefinition { .. } | Request::ReadPage { .. } | Request::ReadLastPage { .. } | Request::DescribeTable { .. } | Request::ExportTable { .. } | Request::CompleteColumns { .. });
                    let operation_tab = match &request {
                        Request::ReconnectTableManagement { tab, .. } | Request::ObjectDefinition { tab, .. } | Request::RunQuery { tab, .. } | Request::ReadPage { tab, .. } | Request::ReadLastPage { tab, .. }
                        | Request::DescribeTable { tab, .. } | Request::SaveCell { tab, .. }
                        | Request::DeleteRows { tab, .. }
                        | Request::InsertDefaultRow { tab, .. } | Request::InsertRows { tab, .. }
                        | Request::SaveCells { tab, .. } | Request::SaveTableDraft { tab, .. } | Request::ExportTable { tab, .. }
                        | Request::ChangeStructure { tab, .. } => Some(*tab),
                        _ => None,
                    };
                    let duration = match &request {
                        Request::Connect(profile, _) | Request::Test(profile, _) if profile.ssh.enabled => 30,
                        Request::Connect(..) | Request::Test(..) => 15,
                        Request::SelectDatabase(_) => 30,
                        Request::InsertRows { .. } | Request::SaveCells { .. } | Request::SaveTableDraft { .. } | Request::ExportTable { .. } => 3600,
                        _ => 60,
                    };
                    let result = tokio::time::timeout(Duration::from_secs(duration), async {
                        Ok::<_, anyhow::Error>(match request {
                            Request::ReconnectTableManagement { tab, epoch, profile, credentials, target } => {
                                if session.is_some() { return Ok(operation_error(tab, "原管理连接仍在线，不能覆盖活动会话", false)); }
                                anyhow::ensure!(target.connection == profile.id, "恢复连接与原表目标不符");
                                let database = target.database.as_ref().ok_or_else(|| anyhow::anyhow!("恢复表管理连接需要明确数据库"))?;
                                let mut bootstrap = profile.clone();
                                bootstrap.database = database.clone();
                                let mut fresh = Session::connect(&bootstrap, &credentials).await?;
                                if fresh.current_database().await?.as_ref() != Some(database) { fresh.select_database(database).await?; }
                                anyhow::ensure!(fresh.current_database().await?.as_ref() == Some(database), "恢复连接未进入原数据库");
                                connection_id = Some(profile.id);
                                engine = Some(profile.engine);
                                physical_database = Some(database.clone());
                                selected_database = Some(database.clone());
                                session = Some(fresh);
                                Response::TableManagementReconnected { tab, epoch, target, profile }
                            }
                            Request::Test(profile, password) => {
                                let mut test = Session::connect(&profile, &password).await.map_err(|error| crate::db::connection_error(&profile, error))?;
                                test.ping().await?;
                                Response::Tested
                            }
                            Request::Connect(profile, password) => {
                                session = None;
                                let mut connection = Session::connect(&profile, &password).await.map_err(|error| crate::db::connection_error(&profile, error))?;
                                connection.ping().await?;
                                physical_database = connection.current_database().await?;
                                selected_database = None;
                                session = Some(connection);
                                connection_id = Some(profile.id);
                                engine = Some(profile.engine);
                                Response::Connected(profile.id)
                            }
                            Request::Disconnect => {
                                session = None; connection_id = None; physical_database = None; selected_database = None;
                                Response::Disconnected
                            }
                            Request::Databases => Response::Databases(session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接服务器"))?.databases().await?),
                            Request::SelectDatabase(database) => {
                                let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接服务器"))?;
                                match connection.select_database(&database).await {
                                    Ok(()) => {
                                        physical_database = Some(database.clone()); selected_database = Some(database.clone());
                                        Response::DatabaseSelected(database)
                                    }
                                    Err(error) if connection.selection_error_preserves_session(&error) => Response::Error {
                                        message: format!("选择数据库失败，原会话已保留：{error:#}"), disconnected: false,
                                    },
                                    Err(error) => return Err(error),
                                }
                            }
                            Request::Tables => Response::Tables(session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?.tables().await?),
                            Request::ObjectCatalog { database } => {
                                if selected_database.as_ref() != Some(&database) {
                                    Response::Error { message: "对象目录所属数据库已变化，请重新选择数据库".into(), disconnected: false }
                                } else {
                                    let catalog = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?.object_catalog().await?;
                                    Response::ObjectCatalog { database, catalog }
                                }
                            }
                            Request::ObjectDefinition { tab, target, object } => {
                                if !target_matches(&target, connection_id, selected_database.as_deref()) {
                                    operation_error(tab, "对象定义所属数据库已变化，请重新选择数据库", false)
                                } else {
                                    let definition = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?.object_definition(&object).await?;
                                    Response::ObjectDefinition { tab, target, object, definition }
                                }
                            }
                            Request::Query(sql) => {
                                let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接服务器"))?;
                                match connection.query(&sql).await {
                                    Ok(output) => {
                                        let current = connection.current_database().await?;
                                        if current != physical_database { selected_database = current.clone(); }
                                        physical_database = current;
                                        Response::Query { output, database: selected_database.clone() }
                                    }
                                    Err(error) if crate::db::query_error_preserves_session(&error) => {
                                        match connection.current_database().await {
                                            Ok(current) => {
                                                if current != physical_database { selected_database = current.clone(); }
                                                physical_database = current;
                                            }
                                            Err(probe_error) if engine != Some(Engine::Postgres) => return Err(probe_error),
                                            Err(probe_error) if !crate::db::query_error_preserves_session(&probe_error) => return Err(probe_error),
                                            Err(_) => {}
                                        }
                                        let _ = responses.send(Response::DatabaseContext(selected_database.clone()));
                                        Response::Error { message: format!("{error:#}\n会话已保留；批处理中先前语句可能已提交。"), disconnected: false }
                                    }
                                    Err(error) => return Err(error),
                                }
                            }
                            Request::RunQuery { tab, target, sql } => {
                                if !target_matches(&target, connection_id, selected_database.as_deref()) {
                                    operation_error(tab, "标签所属连接或数据库与当前会话不符，请明确选择或重新绑定执行目标", false)
                                } else {
                                    let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接服务器"))?;
                                    match connection.query(&sql).await {
                                        Ok(output) => {
                                            let current = connection.current_database().await?;
                                            if current != physical_database { selected_database = current.clone(); }
                                            physical_database = current;
                                            Response::QueryResult { tab, output, database: selected_database.clone() }
                                        }
                                        Err(error) if crate::db::query_error_preserves_session(&error) => {
                                            // A failed batch may already have executed USE. PostgreSQL cannot
                                            // change databases here; keep its target when an aborted transaction
                                            // prevents the probe so the user can still issue ROLLBACK.
                                            match connection.current_database().await {
                                                Ok(current) => {
                                                    if current != physical_database { selected_database = current.clone(); }
                                                    physical_database = current;
                                                }
                                                Err(probe_error) if engine != Some(Engine::Postgres) => return Err(probe_error),
                                                Err(probe_error) if !crate::db::query_error_preserves_session(&probe_error) => return Err(probe_error),
                                                Err(_) => {} // PostgreSQL cannot switch databases inside a batch.
                                            }
                                            let _ = responses.send(Response::DatabaseContext(selected_database.clone()));
                                            operation_error(tab, &format!("{error:#}\n会话已保留；未结束的事务可显式执行 ROLLBACK。批处理中先前语句可能已提交，勿直接重试整批写入。"), false)
                                        }
                                        Err(error) => return Err(error),
                                    }
                                }
                            }
                            Request::ReadPage { tab, target, table, options } => {
                                if !target_matches(&target, connection_id, selected_database.as_deref()) {
                                    operation_error(tab, "请先选择此表所属连接和数据库", false)
                                } else {
                                    let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                    Response::TablePage { tab, page: connection.table_page(&table, &options).await? }
                                }
                            }
                            Request::ReadLastPage { tab, target, table, options } => {
                                if !target_matches(&target, connection_id, selected_database.as_deref()) {
                                    operation_error(tab, "请先选择此表所属连接和数据库", false)
                                } else {
                                    let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                    let (page, offset) = connection.table_last_page(&table, &options).await?;
                                    Response::TableLastPage { tab, page, offset }
                                }
                            }
                            Request::DescribeTable { tab, target, table } => {
                                if !target_matches(&target, connection_id, selected_database.as_deref()) {
                                    operation_error(tab, "请先选择此表所属连接和数据库", false)
                                } else {
                                    let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                    Response::TableStructure { tab, structure: connection.table_structure(&table).await? }
                                }
                            }
                            Request::CompleteColumns { target, table, generation } => {
                                if !target_matches(&target, connection_id, selected_database.as_deref()) || target.database.is_none() {
                                    Response::CompleteColumns { target, table, generation, columns: Vec::new() }
                                } else {
                                    let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                    let columns = connection.table_columns(&table).await
                                        .map(|columns| columns.into_iter().map(|column| column.name).collect())
                                        .unwrap_or_default();
                                    Response::CompleteColumns { target, table, generation, columns }
                                }
                            }
                            Request::SaveCell { tab, target, change } => {
                                if !target_matches(&target, connection_id, selected_database.as_deref())
                                    || target.database.as_deref() != Some(change.database.as_str()) {
                                    operation_error(tab, "保存目标与当前连接或数据库不符，修改未执行", false)
                                } else {
                                    let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                    match connection.update_cell(&change).await {
                                        Ok(()) => Response::CellSaved { tab },
                                        Err(error) if error.downcast_ref::<crate::db::EditRejected>().is_some() =>
                                            operation_error(tab, &format!("{error:#}"), false),
                                        Err(error) => return Err(error),
                                    }
                                }
                            }
                            Request::ReadTable { tab, database, table } => {
                                let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                if selected_database.as_deref() != Some(database.as_str()) {
                                    Response::Error { message: "请先选择此表所属数据库".into(), disconnected: false }
                                } else {
                                    Response::TableData { tab, data: connection.table_data(&table).await? }
                                }
                            }
                            Request::UpdateCell { tab, change } => {
                                let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                match connection.update_cell(&change).await {
                                    Ok(()) => Response::CellSaved { tab },
                                    Err(error) if error.downcast_ref::<crate::db::EditRejected>().is_some() =>
                                        Response::Error { message: format!("{error:#}"), disconnected: false },
                                    Err(error) => return Err(error),
                                }
                            }
                            Request::DeleteRows { tab, target, table, primary_keys, key_values } => {
                                if !target_matches(&target, connection_id, selected_database.as_deref()) {
                                    operation_error(tab, "删除目标与当前连接或数据库不符，未执行", false)
                                } else {
                                    let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                    match connection.delete_rows(target.database.as_deref().unwrap_or_default(), &table, &primary_keys, &key_values).await {
                                        Ok(count) => Response::RowsDeleted { tab, count },
                                        Err(error) if error.downcast_ref::<crate::db::EditRejected>().is_some() => operation_error(tab, &format!("{error:#}"), false),
                                        Err(error) => return Err(error),
                                    }
                                }
                            }
                            Request::InsertDefaultRow { tab, target, table } => {
                                if !target_matches(&target, connection_id, selected_database.as_deref()) {
                                    operation_error(tab, "新增行目标与当前连接或数据库不符，未执行", false)
                                } else {
                                    let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                    match connection.insert_default_row(target.database.as_deref().unwrap_or_default(), &table).await {
                                        Ok(()) => Response::RowInserted { tab },
                                        Err(error) if error.downcast_ref::<crate::db::EditRejected>().is_some() => operation_error(tab, &format!("{error:#}"), false),
                                        Err(error) => return Err(error),
                                    }
                                }
                            }
                            Request::InsertRows { tab, target, table, columns, rows } => {
                                if !target_matches(&target, connection_id, selected_database.as_deref()) || target.database.is_none() {
                                    operation_error(tab, "新增目标与当前连接或数据库不符", false)
                                } else {
                                    let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                    Response::RowsInserted { tab, count: connection.insert_rows(target.database.as_deref().unwrap(), &table, &columns, &rows).await? }
                                }
                            }
                            Request::SaveCells { tab, target, changes } => {
                                if !target_matches(&target, connection_id, selected_database.as_deref())
                                    || changes.iter().any(|c| target.database.as_deref() != Some(c.database.as_str())) {
                                    operation_error(tab, "批量保存目标与当前连接或数据库不符", false)
                                } else {
                                    let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                    Response::CellsSaved { tab, count: connection.update_cells(&changes).await? }
                                }
                            }
                            Request::SaveTableDraft { tab, target, table, changes, columns, rows } => {
                                if !target_matches(&target, connection_id, selected_database.as_deref())
                                    || target.database.is_none()
                                    || changes.iter().any(|change| target.database.as_deref() != Some(change.database.as_str()) || change.table != table) {
                                    operation_error(tab, "暂存提交目标与当前连接、数据库或表不符，未执行写入", false)
                                } else {
                                    let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                    let (updated_cells, inserted_rows) = connection.save_table_draft(target.database.as_deref().unwrap(), &table, &changes, &columns, &rows).await?;
                                    Response::TableDraftSaved { tab, updated_cells, inserted_rows }
                                }
                            }
                            Request::ExportTable { tab, target, table, options, path } => {
                                if !target_matches(&target, connection_id, selected_database.as_deref()) || target.database.is_none() {
                                    operation_error(tab, "导出目标与当前连接或数据库不符", false)
                                } else {
                                    let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                    let count = connection.export_table_csv(target.database.as_deref().unwrap(), &table, &options, &path).await?;
                                    Response::TableExported { tab, count, path }
                                }
                            }
                            Request::CreateDatabase(database) => {
                                let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接服务器"))?;
                                match connection.create_database(&database).await {
                                    Ok(()) => Response::Databases(connection.databases().await?),
                                    Err(error) => Response::Error { message: format!("新建数据库失败：{error:#}"), disconnected: false },
                                }
                            }
                            Request::DropDatabase(database) => {
                                if selected_database.as_deref() == Some(database.as_str()) {
                                    Response::Error { message: "不能删除当前正在使用的数据库；请先切换到其他数据库".into(), disconnected: false }
                                } else {
                                    let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接服务器"))?;
                                    match connection.drop_database(&database).await {
                                        Ok(()) => Response::Databases(connection.databases().await?),
                                        Err(error) => Response::Error { message: format!("删除数据库失败：{error:#}"), disconnected: false },
                                    }
                                }
                            }
                            Request::CreateTable { database, schema, name, columns } => {
                                let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                match connection.create_table(&database, &schema, &name, &columns).await {
                                    Ok(()) => Response::Tables(connection.tables().await?),
                                    Err(error) => Response::Error { message: format!("新建数据表失败：{error:#}"), disconnected: false },
                                }
                            }
                            Request::DropTable { database, table } => {
                                let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                match connection.drop_table(&database, &table).await {
                                    Ok(()) => Response::Tables(connection.tables().await?),
                                    Err(error) => Response::Error { message: format!("删除数据表失败：{error:#}"), disconnected: false },
                                }
                            }
                            Request::ChangeStructure { tab, target, table, change, preview_sql } => {
                                if !target_matches(&target, connection_id, selected_database.as_deref()) {
                                    operation_error(tab, "结构修改目标与当前连接或数据库不符，未执行", false)
                                } else {
                                    let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                    match connection.change_structure(target.database.as_deref().unwrap_or_default(), &table, &change, &preview_sql).await {
                                        Ok(()) => Response::StructureChanged { tab, structure: connection.table_structure(&table).await? },
                                        Err(error) => operation_error(tab, &format!("结构修改失败：{error:#}；如连接出现故障请重新连接并确认实际结构，勿直接重试"), false),
                                    }
                                }
                            }
                        })
                    }).await;
                    let response = match result {
                        Ok(Ok(response)) => response,
                        Ok(Err(error)) if error.downcast_ref::<crate::db::EditRejected>().is_some() || error.downcast_ref::<crate::db::ObjectDefinitionRejected>().is_some()
                            || (read_only_operation && crate::db::query_error_preserves_session(&error)) => {
                            match operation_tab {
                                Some(tab) => operation_error(tab, &format!("{error:#}"), false),
                                None => Response::Error { message: format!("{error:#}"), disconnected: false },
                            }
                        }
                        failure => {
                            if modifies_session { session = None; connection_id = None; physical_database = None; selected_database = None; }
                            let message = match failure {
                                Ok(Err(error)) => format!("{error:#}"),
                                Err(_) if modifies_session => format!("操作超过 {duration} 秒；连接已关闭。写入是否提交请在重新连接后确认。"),
                                Err(_) => format!("连接测试超过 {duration} 秒，请检查服务器地址和网络。"),
                                _ => unreachable!(),
                            };
                            match operation_tab {
                                Some(tab) => Response::OperationError { tab, message, disconnected: modifies_session },
                                None => Response::Error { message, disconnected: modifies_session },
                            }
                        }
                    };
                    if responses.send(response).is_err() { break; }
                    wake();
                }
            });
        });
        Self { sender, receiver }
    }

    pub fn send(&self, request: Request) -> anyhow::Result<()> {
        self.sender
            .send(request)
            .map_err(|_| anyhow::anyhow!("数据库工作线程已退出，请重启应用"))
    }
}

/// A query tab owns this worker independently of its server's catalog/table
/// worker. Dropping an idle worker releases its session (and rolls back open
/// transactions). An in-flight operation runs to completion or timeout first;
/// neither dropping the worker nor queued Disconnect requests cancel that SQL.
/// Do not move it between tabs or recreate it after an ordinary SQL error.
pub struct QueryWorker {
    sender: tokio::sync::mpsc::UnboundedSender<QueryRequest>,
    cancellations: tokio::sync::mpsc::UnboundedSender<Uuid>,
    pub receiver: mpsc::Receiver<QueryResponse>,
}

// Intentionally has no Debug implementation: requests contain credentials.
// Keep the connection payload owned, matching the management Request API.
#[allow(clippy::large_enum_variant)]
pub enum QueryRequest {
    Transaction {
        target: QueryTarget,
        command: crate::query_control::TransactionCommand,
    },
    InspectTransaction {
        target: QueryTarget,
    },
    InspectContext {
        target: QueryTarget,
    },
    LoadSchemas {
        profile: Profile,
        credentials: Credentials,
        target: QueryTarget,
    },
    Run {
        profile: Profile,
        credentials: Credentials,
        target: QueryTarget,
        sql: String,
    },
    Disconnect,
}

pub enum QueryResponse {
    ContextChecked {
        tab: Uuid,
        target: QueryTarget,
    },
    ExecutionStarted {
        tab: Uuid,
        execution: Uuid,
        cancellable: bool,
        reason: Option<String>,
    },
    CancelStatus {
        tab: Uuid,
        execution: Uuid,
        message: String,
        failed: bool,
    },
    TransactionState {
        tab: Uuid,
        state: crate::query_control::QueryTransactionState,
        verified: bool,
    },
    ControlFinished {
        tab: Uuid,
        message: String,
        connected: bool,
    },
    SchemasLoaded {
        tab: Uuid,
        target: QueryTarget,
        schemas: Vec<String>,
    },
    SchemaLoadError {
        tab: Uuid,
        target: QueryTarget,
        message: String,
    },
    Finished {
        tab: Uuid,
        output: QueryOutput,
        database: Option<String>,
        schema: Option<String>,
    },
    Error {
        tab: Uuid,
        message: String,
        database: Option<String>,
        schema: Option<String>,
        connected: bool,
        diagnostic: Option<crate::model::QueryDiagnostic>,
        elapsed_ms: Option<u128>,
        partial_output: Option<QueryOutput>,
    },
    Disconnected {
        tab: Uuid,
    },
}

struct QuerySession {
    session: Session,
    target: QueryTarget,
    engine: Engine,
    profile: Profile,
}

fn query_profile_matches(left: &Profile, right: &Profile) -> bool {
    left.engine == right.engine
        && left.sqlserver_auth == right.sqlserver_auth
        && left.sqlserver_instance == right.sqlserver_instance
        && left.host == right.host
        && left.port == right.port
        && left.username == right.username
        && left.database == right.database
        && left.tls == right.tls
        && left.trust_certificate == right.trust_certificate
        && left.ssh.enabled == right.ssh.enabled
        && (!left.ssh.enabled
            || (left.ssh.host == right.ssh.host
                && left.ssh.port == right.ssh.port
                && left.ssh.username == right.ssh.username
                && left.ssh.auth == right.ssh.auth
                && left.ssh.private_key == right.ssh.private_key
                && left.ssh.known_hosts == right.ssh.known_hosts
                && left.ssh.fingerprint == right.ssh.fingerprint))
}

impl QuerySession {
    async fn connect(
        profile: &Profile,
        credentials: &Credentials,
        target: &QueryTarget,
    ) -> anyhow::Result<Self> {
        // A restricted PostgreSQL account may only have CONNECT on the target.
        // Include it in the established bootstrap fallback without changing the
        // saved profile or the management worker's database.
        let mut bootstrap = profile.clone();
        if let Some(database) = &target.database {
            bootstrap.database = database.clone();
        }
        let mut session = Session::connect(&bootstrap, credentials)
            .await
            .map_err(|error| crate::db::connection_error(profile, error))?;
        let mut database = session.current_database().await?;
        if let Some(selected) = &target.database {
            if database.as_deref() != Some(selected.as_str()) {
                session.select_database(selected).await?;
                database = session.current_database().await?;
            }
            anyhow::ensure!(
                database.as_deref() == Some(selected.as_str()),
                "查询会话未能进入指定数据库，SQL 未执行"
            );
        }
        if let Some(schema) = &target.schema {
            if matches!(profile.engine, Engine::Postgres | Engine::SqlServer) {
                session.select_query_schema(schema).await?;
            } else {
                anyhow::ensure!(
                    profile.engine == Engine::MySql && target.database.as_ref() == Some(schema),
                    "此引擎不能改变会话默认 Schema；请使用限定对象名，SQL 未执行"
                );
            }
        }
        let schema = session.current_query_schema().await?;
        let _ = session.prepare_query_context().await;
        // Failure to establish a safe cancellation identity disables Stop only.
        let _ = session.prepare_query_control().await;
        session.confirm_observed_query_database(database.clone());
        Ok(Self {
            session,
            target: QueryTarget {
                connection: target.connection,
                database,
                schema,
            },
            engine: profile.engine,
            profile: profile.clone(),
        })
    }

    async fn refresh_database(&mut self) -> anyhow::Result<()> {
        if self.engine != Engine::Postgres {
            self.target.database = self.session.observed_query_database().await?;
            if self.engine == Engine::MySql {
                self.target.schema = self.target.database.clone();
            }
            // SQL Server's default schema is captured on connection or explicit
            // inspection. A hidden SCHEMA_NAME() would overwrite @@ROWCOUNT.
            return Ok(());
        }
        match self.session.current_query_schema().await {
            Ok(schema) => self.target.schema = schema,
            Err(error)
                if self.engine == Engine::Postgres
                    && crate::db::query_error_preserves_session(&error) => {}
            Err(error) => return Err(error),
        }
        Ok(())
    }
}

impl QueryWorker {
    pub fn start(tab: Uuid, wake: impl Fn() + Send + 'static) -> Self {
        let (sender, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let (cancellations, mut cancellation_requests) = tokio::sync::mpsc::unbounded_channel();
        let (responses, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let wake = std::rc::Rc::new(wake);
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = responses.send(QueryResponse::Error {
                        tab,
                        message: error.to_string(),
                        database: None,
                        schema: None,
                        connected: false,
                        diagnostic: None,
                        elapsed_ms: None,
                        partial_output: None,
                    });
                    wake();
                    return;
                }
            };
            // The driver futures stay on this actor's current-thread runtime.
            // Local tasks let independent schema discovery progress during SQL
            // I/O without adding Send bounds to the existing driver APIs.
            let local = tokio::task::LocalSet::new();
            runtime.block_on(local.run_until(async move {
                let mut live: Option<QuerySession> = None;
                while let Some(request) = requests.recv().await {
                    let response = match request {
                        QueryRequest::InspectContext { target } => {
                            if let Some(current) = live.as_mut().filter(|current| current.target == target) {
                                let result = tokio::time::timeout(Duration::from_secs(30), async {
                                    let database = current.session.current_database().await?;
                                    let schema = current.session.current_query_schema().await?;
                                    current.session.confirm_observed_query_database(database.clone());
                                    current.target.database = database; current.target.schema = schema;
                                    Ok::<_, anyhow::Error>(())
                                }).await;
                                let connected = matches!(&result, Ok(Ok(()))) || matches!(&result, Ok(Err(error)) if crate::db::query_error_preserves_session(error));
                                let message = match result {
                                    Ok(Ok(())) => { let _ = responses.send(QueryResponse::ContextChecked { tab, target: current.target.clone() }); "执行上下文已重新读取；服务器语句诊断值可能已改变".into() },
                                    Ok(Err(error)) => format!("检查上下文失败：{error:#}"),
                                    Err(_) => "检查上下文超时，查询连接已关闭".into(),
                                };
                                if !connected { live = None; }
                                QueryResponse::ControlFinished { tab, message, connected }
                            } else { QueryResponse::ControlFinished { tab, message: "原查询会话已变化，未检查上下文".into(), connected: live.is_some() } }
                        }
                        QueryRequest::Transaction { target, command } => {
                            if let Some(current) = live.as_mut().filter(|current| current.target == target) {
                                let result = tokio::time::timeout(Duration::from_secs(60), current.session.transaction_command(command)).await;
                                let mut connected = matches!(&result, Ok(Ok(_))) || matches!(&result, Ok(Err(e)) if crate::db::query_error_preserves_session(e) || e.downcast_ref::<crate::db::EditRejected>().is_some());
                                let state = if connected {
                                    match tokio::time::timeout(Duration::from_secs(15), current.session.transaction_state()).await {
                                        Ok(Ok(state)) => state,
                                        _ => { connected = false; crate::query_control::QueryTransactionState::Unknown }
                                    }
                                } else { crate::query_control::QueryTransactionState::Unknown };
                                let _ = responses.send(QueryResponse::TransactionState { tab, state, verified: false });
                                let message = match result { Ok(Ok(_)) => format!("事务操作完成：{command:?}{}", if connected { "" } else { "；后续状态读取失败，查询连接已关闭" }), Ok(Err(e)) => format!("{e:#}；事务结果请核实，不自动重试"), Err(_) => "事务操作超时，提交结果未知，请重新连接核实".into() };
                                if !connected { live = None; }
                                QueryResponse::ControlFinished { tab, message, connected }
                            } else {
                                QueryResponse::ControlFinished { tab, message: "原查询会话已变化，事务操作未执行".into(), connected: live.is_some() }
                            }
                        }
                        QueryRequest::InspectTransaction { target } => {
                            if let Some(current) = live.as_mut().filter(|current| current.target == target) {
                                let result = tokio::time::timeout(Duration::from_secs(30), current.session.inspect_transaction_state()).await;
                                let connected = matches!(&result, Ok(Ok(_))) || matches!(&result, Ok(Err(e)) if crate::db::query_error_preserves_session(e));
                                let (state, verified, message) = match result { Ok(Ok(state)) => (state, true, "事务状态已核实".into()), Ok(Err(e)) => (crate::query_control::QueryTransactionState::Unknown, false, format!("核实事务状态失败：{e:#}")), Err(_) => (crate::query_control::QueryTransactionState::Unknown, false, "核实事务状态超时，查询连接已关闭".into()) };
                                let _ = responses.send(QueryResponse::TransactionState { tab, state, verified });
                                if !connected { live = None; }
                                QueryResponse::ControlFinished { tab, message, connected }
                            } else { QueryResponse::ControlFinished { tab, message: "没有可核实的原查询会话".into(), connected: live.is_some() } }
                        }
                        QueryRequest::LoadSchemas { profile, credentials, target } => {
                            let schema_responses = responses.clone();
                            let schema_wake = wake.clone();
                            // This task owns a separate read-only connection. It
                            // must not delay Run/Disconnect on the user's actor.
                            tokio::task::spawn_local(async move {
                                let loaded = tokio::time::timeout(Duration::from_secs(30), async {
                                    anyhow::ensure!(profile.id == target.connection, "Schema 列表目标与连接配置不一致");
                                    let mut discovery_target = target.clone();
                                    // A saved schema may have been removed; listing the
                                    // remaining schemas must still be possible.
                                    discovery_target.schema = None;
                                    let mut discovery = QuerySession::connect(&profile, &credentials, &discovery_target).await?;
                                    discovery.session.query_schemas().await
                                }).await;
                                let response = match loaded {
                                    Ok(Ok(schemas)) => QueryResponse::SchemasLoaded { tab, target, schemas },
                                    error => QueryResponse::SchemaLoadError {
                                        tab, target,
                                        message: match error {
                                            Ok(Err(error)) => format!("读取 Schema 列表失败：{error:#}"),
                                            Err(_) => "读取 Schema 列表超过 30 秒".into(),
                                            _ => unreachable!(),
                                        },
                                    },
                                };
                                if schema_responses.send(response).is_ok() {
                                    schema_wake();
                                }
                            });
                            continue;
                        }
                        QueryRequest::Disconnect => {
                            live = None;
                            QueryResponse::Disconnected { tab }
                        }
                        QueryRequest::Run { profile, credentials, target, sql } => {
                            let invalid = if profile.id != target.connection {
                                Some("查询目标与连接配置不一致，SQL 未执行")
                            } else if live.as_ref().is_some_and(|current| current.target != target) {
                                Some("查询标签仍持有原数据库会话；请先显式断开或重新绑定目标，SQL 未执行")
                            } else if live.as_ref().is_some_and(|current| !query_profile_matches(&current.profile, &profile)) {
                                Some("连接配置已改变；请先显式结束当前查询会话再使用新配置，SQL 未执行")
                            } else if sql.trim().is_empty() {
                                Some("SQL 不能为空")
                            } else {
                                None
                            };
                            if let Some(message) = invalid {
                                QueryResponse::Error {
                                    tab,
                                    message: message.into(),
                                    database: live.as_ref().and_then(|current| current.target.database.clone()),
                                    schema: live.as_ref().and_then(|current| current.target.schema.clone()),
                                    connected: live.is_some(),
                                    diagnostic: None,
                                    elapsed_ms: None,
                                    partial_output: None,
                                }
                            } else {
                                let connected = if live.is_some() {
                                    Ok(())
                                } else {
                                    match tokio::time::timeout(Duration::from_secs(30),
                                        QuerySession::connect(&profile, &credentials, &target)
                                    ).await {
                                        Ok(Ok(current)) => { live = Some(current); Ok(()) }
                                        Ok(Err(error)) => Err(format!("建立查询会话失败，SQL 未执行：{error:#}")),
                                        Err(_) => Err("建立查询会话超过 30 秒，SQL 未执行".into()),
                                    }
                                };
                                match connected {
                                    Err(message) => QueryResponse::Error { tab, message, database: None, schema: None, connected: false, diagnostic: None, elapsed_ms: None, partial_output: None },
                                    Ok(()) => {
                                        let current = live.as_mut().expect("connected query session");
                                        let started = std::time::Instant::now();
                                        let mut failed_query_diagnostic = None;
                                        let mut failed_query_output = None;
                                        let result = tokio::time::timeout(Duration::from_secs(3600), async {
                                            let result = cancellable_query(&mut current.session, &sql, tab, &mut cancellation_requests, &responses, &wake).await.map(|output| {
                                                // Keep ownership outside the timed future so a failed
                                                // probe or timeout cannot discard completed results.
                                                failed_query_output = Some(output);
                                            });
                                            if let Err(error) = &result {
                                                failed_query_diagnostic = Some(crate::db::query_diagnostic(error, &sql));
                                                failed_query_output = crate::db::query_partial_output(error);
                                            }
                                            let state = tokio::time::timeout(Duration::from_secs(15), current.session.transaction_state()).await.map_err(|_| anyhow::anyhow!("事务尾响应超时，查询连接不再复用"))??;
                                            let _ = responses.send(QueryResponse::TransactionState { tab, state, verified: false });
                                            match result {
                                                Ok(()) => {
                                                    tokio::time::timeout(Duration::from_secs(15), current.refresh_database()).await.map_err(|_| anyhow::anyhow!("查询上下文观察超时，连接不再复用"))??;
                                                    Ok::<_, anyhow::Error>(Ok(failed_query_output.take().expect("completed query output")))
                                                }
                                                Err(error) if crate::db::query_error_preserves_session(&error) && error.downcast_ref::<CancelTransportFailure>().is_none() => {
                                                    let diagnostic = crate::db::query_diagnostic(&error, &sql);
                                                    let partial_output = crate::db::query_partial_output(&error);
                                                    tokio::time::timeout(Duration::from_secs(15), current.refresh_database()).await.map_err(|_| anyhow::anyhow!("查询上下文观察超时，连接不再复用"))??;
                                                    let message = if crate::db::is_query_cancelled(&error) { "查询已停止，服务器已确认取消。先前语句可能已经执行或提交；需要时显式回滚并核实数据。".to_owned() } else { format!("{error:#}\n查询会话已保留；未结束的事务可显式执行 ROLLBACK。批处理中先前语句可能已提交，勿直接重试整批写入。") };
                                                    Ok(Err((message, diagnostic, partial_output)))
                                                }
                                                Err(error) => Err(error),
                                            }
                                        }).await;
                                        match result {
                                            Ok(Ok(Ok(output))) => QueryResponse::Finished {
                                                tab, output, database: current.target.database.clone(),
                                                schema: current.target.schema.clone(),
                                            },
                                            Ok(Ok(Err((message, diagnostic, partial_output)))) => QueryResponse::Error {
                                                tab, message, database: current.target.database.clone(), connected: true,
                                                schema: current.target.schema.clone(),
                                                diagnostic: Some(diagnostic), elapsed_ms: Some(started.elapsed().as_millis()),
                                                partial_output,
                                            },
                                            failure => {
                                                let (message, diagnostic) = match failure {
                                                    Ok(Err(error)) => (format!("{error:#}\n查询连接已关闭；先前语句可能已执行或提交，请重新连接并核对实际数据，勿直接重试整批写入。"), failed_query_diagnostic),
                                                    Err(_) => ("查询超过 3600 秒，连接已关闭；写入是否提交请重新连接后核对，勿直接重试。".into(), None),
                                                    _ => unreachable!(),
                                                };
                                                live = None;
                                                QueryResponse::Error { tab, message, database: None, schema: None, connected: false, diagnostic, elapsed_ms: Some(started.elapsed().as_millis()), partial_output: failed_query_output }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    };
                    if responses.send(response).is_err() { break; }
                    wake();
                }
            }));
        });
        Self {
            sender,
            cancellations,
            receiver,
        }
    }

    pub fn cancel(&self, execution: Uuid) -> anyhow::Result<()> {
        self.cancellations
            .send(execution)
            .map_err(|_| anyhow::anyhow!("原查询工作线程已退出"))
    }

    pub fn send(&self, request: QueryRequest) -> anyhow::Result<()> {
        self.sender
            .send(request)
            .map_err(|_| anyhow::anyhow!("查询工作线程已退出，请重新打开查询标签"))
    }
}

#[derive(Debug)]
struct CancelTransportFailure(String);
impl std::fmt::Display for CancelTransportFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CancelTransportFailure {}

// Cancellation travels independently from the serialized command queue. The
// generation gate is closed before any context probe or subsequent SQL begins.
async fn cancellable_query(
    session: &mut Session,
    sql: &str,
    tab: Uuid,
    cancellations: &mut tokio::sync::mpsc::UnboundedReceiver<Uuid>,
    responses: &mpsc::Sender<QueryResponse>,
    wake: &std::rc::Rc<impl Fn()>,
) -> anyhow::Result<QueryOutput> {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    session.prepare_user_query().await?;
    let execution = Uuid::new_v4();
    let (handle, reason) = match session.cancel_handle() {
        Ok(handle) => (Some(handle), None),
        Err(error) => (None, Some(format!("停止不可用：{error:#}"))),
    };
    let _ = responses.send(QueryResponse::ExecutionStarted {
        tab,
        execution,
        cancellable: handle.is_some(),
        reason,
    });
    wake();
    let active = Arc::new(AtomicBool::new(true));
    struct ExecutionGuard {
        active: Arc<AtomicBool>,
        handle: Option<crate::db::QueryCancelHandle>,
        tasks: Vec<tokio::task::AbortHandle>,
    }
    impl Drop for ExecutionGuard {
        fn drop(&mut self) {
            self.active.store(false, Ordering::SeqCst);
            if let Some(handle) = &self.handle {
                handle.disarm();
            }
            for task in &self.tasks {
                task.abort();
            }
        }
    }
    let mut guard = ExecutionGuard {
        active: active.clone(),
        handle: handle.clone(),
        tasks: Vec::new(),
    };
    let mut control: Option<tokio::task::JoinHandle<anyhow::Result<bool>>> = None;
    let mut control_failure = None;
    let mut requested = false;
    let result;
    {
        let query = session.query(sql);
        tokio::pin!(query);
        loop {
            tokio::select! {
                value = &mut query => { result = value; break; }
                Some(generation) = cancellations.recv() => {
                    if generation == execution && !requested && let Some(handle) = handle.clone() {
                        requested = true;
                        let active = active.clone();
                        control = Some(tokio::task::spawn_local(async move {
                            tokio::time::timeout(Duration::from_secs(30), handle.cancel_if_active(active)).await
                                .map_err(|_| anyhow::anyhow!("取消控制通道超时，不能确认取消已送达"))?
                        }));
                        guard.tasks.push(control.as_ref().unwrap().abort_handle());
                    }
                }
                completed = async { control.as_mut().expect("enabled cancellation").await }, if control.is_some() => {
                    control = None;
                    let outcome = completed.map_err(anyhow::Error::from).and_then(|result| result);
                    let (message, failed) = match outcome {
                        Ok(true) => ("已发送停止请求，正在等待服务器确认…".into(), false),
                        Ok(false) => ("查询已经结束，停止请求未发送".into(), false),
                        Err(error) => { let message = format!("停止请求失败：{error:#}；正在等待原查询结束"); control_failure = Some(message.clone()); (message, true) }
                    };
                    let _ = responses.send(QueryResponse::CancelStatus { tab, execution, message, failed });
                    wake();
                }
            }
        }
    }
    active.store(false, Ordering::SeqCst);
    if let Some(handle) = &handle {
        handle.disarm();
    }
    // A sent cancellation must finish its transport before we reuse this socket.
    if let Some(control) = control
        && let Err(error) = control
            .await
            .map_err(anyhow::Error::from)
            .and_then(|result| result)
    {
        control_failure = Some(format!("取消控制通道未完成：{error:#}"));
    }
    if let Some(message) = control_failure {
        let output = match &result {
            Ok(output) => Some(output.clone()),
            Err(error) => crate::db::query_partial_output(error),
        };
        let failure = CancelTransportFailure(format!(
            "{message}；原查询可能已完成，连接不再复用，请核实写入结果"
        ));
        let error = match result {
            Ok(_) => anyhow::Error::new(failure),
            Err(error) => error.context(failure),
        };
        return Err(match output {
            Some(output) => crate::db::retain_query_output(error, output),
            None => error,
        });
    }
    drop(guard);
    result
}
