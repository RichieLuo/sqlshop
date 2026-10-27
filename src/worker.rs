mod query;
use crate::object_catalog::{CatalogObject, ObjectCatalog};
use crate::{
    db::{Session, StructureChange, TableBrowseOptions, TablePage, TableStructure},
    model::{
        CellUpdate, CreateTableColumn, Credentials, Engine, InsertValue, Profile, QueryOutput,
        Table, TableData,
    },
    workspace::QueryTarget,
};
pub use query::{QueryRequest, QueryResponse, QueryWorker};
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
    RenameTable {
        database: String,
        table: Table,
        new_name: String,
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
    TableRenamed {
        database: String,
        table: Table,
        new_name: String,
    },
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

#[derive(Clone, Debug)]
pub struct TableReadExecution {
    pub tab: Uuid,
    pub execution: Uuid,
    pub cancellable: bool,
    pub requested: bool,
    pub message: String,
}

pub struct Worker {
    sender: tokio::sync::mpsc::UnboundedSender<Request>,
    pub receiver: mpsc::Receiver<Response>,
    read_state: std::sync::Arc<std::sync::Mutex<Option<TableReadExecution>>>,
    read_cancellations: tokio::sync::mpsc::UnboundedSender<(Uuid, Uuid)>,
}

impl Worker {
    pub fn start(wake: impl Fn() + Send + 'static) -> Self {
        let (sender, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let (responses, receiver) = mpsc::channel();
        let read_state = std::sync::Arc::new(std::sync::Mutex::new(None));
        let worker_read_state = read_state.clone();
        let (read_cancellations, mut read_cancel_requests) = tokio::sync::mpsc::unbounded_channel();
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
                                    let (page, _) = cancellable_table_read(connection, tab, &table, &options, false, &worker_read_state, &mut read_cancel_requests, &wake).await?;
                                    Response::TablePage { tab, page }
                                }
                            }
                            Request::ReadLastPage { tab, target, table, options } => {
                                if !target_matches(&target, connection_id, selected_database.as_deref()) {
                                    operation_error(tab, "请先选择此表所属连接和数据库", false)
                                } else {
                                    let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                    let (page, offset) = cancellable_table_read(connection, tab, &table, &options, true, &worker_read_state, &mut read_cancel_requests, &wake).await?;
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
                            Request::RenameTable { database, table, new_name } => {
                                let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                match connection.rename_table(&database, &table, &new_name).await {
                                    Ok(()) => Response::TableRenamed { database, table, new_name },
                                    Err(error) if error.downcast_ref::<crate::db::EditRejected>().is_some() || crate::db::query_error_preserves_session(&error) => Response::Error { message: format!("重命名数据表失败：{error:#}"), disconnected: false },
                                    Err(error) => return Err(error),
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
                            || (read_only_operation && error.downcast_ref::<CancelTransportFailure>().is_none() && crate::db::query_error_preserves_session(&error)) => {
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
                    if let Ok(mut state) = worker_read_state.lock() { *state = None; }
                    if responses.send(response).is_err() { break; }
                    wake();
                }
            });
        });
        Self {
            sender,
            receiver,
            read_state,
            read_cancellations,
        }
    }

    pub fn table_read_execution(&self, tab: Uuid) -> Option<TableReadExecution> {
        self.read_state
            .lock()
            .ok()?
            .as_ref()
            .filter(|state| state.tab == tab)
            .cloned()
    }

    pub fn cancel_table_read(&self, tab: Uuid, execution: Uuid) -> anyhow::Result<()> {
        let mut state = self
            .read_state
            .lock()
            .map_err(|_| anyhow::anyhow!("表读取控制状态不可用"))?;
        let state = state
            .as_mut()
            .filter(|s| s.tab == tab && s.execution == execution && s.cancellable && !s.requested)
            .ok_or_else(|| anyhow::anyhow!("此表读取已结束或不能安全停止"))?;
        self.read_cancellations
            .send((tab, execution))
            .map_err(|_| anyhow::anyhow!("表读取线程已退出"))?;
        state.requested = true;
        state.message = "正在发送停止请求…".into();
        Ok(())
    }

    pub fn send(&self, request: Request) -> anyhow::Result<()> {
        self.sender
            .send(request)
            .map_err(|_| anyhow::anyhow!("数据库工作线程已退出，请重启应用"))
    }
}

// One cancellation generation covers metadata, COUNT and the requested page.
// The control future is polled locally, so outer timeout/drop cannot leave an
// independently running cancellation task targeting a future operation.
#[allow(clippy::too_many_arguments)]
async fn cancellable_table_read(
    session: &mut Session,
    tab: Uuid,
    table: &Table,
    options: &TableBrowseOptions,
    last: bool,
    state: &std::sync::Arc<std::sync::Mutex<Option<TableReadExecution>>>,
    requests: &mut tokio::sync::mpsc::UnboundedReceiver<(Uuid, Uuid)>,
    wake: &impl Fn(),
) -> anyhow::Result<(TablePage, u64)> {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let execution = Uuid::new_v4();
    if let Ok(mut state) = state.lock() {
        *state = Some(TableReadExecution {
            tab,
            execution,
            cancellable: false,
            requested: false,
            message: "正在准备表读取控制通道…".into(),
        });
    }
    wake();
    let _ = session.prepare_query_control().await;
    let handle = session.cancel_handle().ok();
    let active = Arc::new(AtomicBool::new(true));
    if let Ok(mut state) = state.lock() {
        *state = Some(TableReadExecution {
            tab,
            execution,
            cancellable: handle.is_some(),
            requested: false,
            message: if handle.is_some() {
                "正在读取表数据…"
            } else {
                "读取中；无法核实安全取消标识"
            }
            .into(),
        });
    }
    wake();
    struct Guard {
        active: Arc<AtomicBool>,
        handle: Option<crate::db::QueryCancelHandle>,
        state: Arc<std::sync::Mutex<Option<TableReadExecution>>>,
        execution: Uuid,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            self.active.store(false, Ordering::SeqCst);
            if let Some(h) = &self.handle {
                h.disarm();
            }
            if let Ok(mut s) = self.state.lock()
                && s.as_ref().is_some_and(|s| s.execution == self.execution)
            {
                *s = None;
            }
        }
    }
    let guard = Guard {
        active: active.clone(),
        handle: handle.clone(),
        state: state.clone(),
        execution,
    };
    type Control = std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<bool>>>>;
    let mut control: Option<Control> = None;
    let mut requested = false;
    let interrupted = Arc::new(AtomicBool::new(false));
    session.set_table_read_interrupt(Some(interrupted.clone()));
    let mut control_failed = None;
    let result;
    {
        let read = async {
            if last {
                session.table_last_page(table, options).await
            } else {
                session
                    .table_page(table, options)
                    .await
                    .map(|page| (page, options.offset))
            }
        };
        tokio::pin!(read);
        loop {
            tokio::select! {
                value = &mut read => { result=value; break; }
                Some((source,generation))=requests.recv() => {
                    if source==tab && generation==execution && !requested && let Some(handle)=handle.clone() {
                        requested=true;interrupted.store(true, Ordering::SeqCst);let active=active.clone();
                        control=Some(Box::pin(async move { tokio::time::timeout(Duration::from_secs(30),handle.cancel_if_active(active)).await.map_err(|_|anyhow::anyhow!("停止控制通道超时"))? }));
                    }
                }
                outcome=async { control.as_mut().expect("guarded control").await }, if control.is_some() => {
                    control=None;
                    let message=match outcome { Ok(true)=>"停止请求已发送，等待服务器确认…".into(),Ok(false)=>"本次读取已结束，未发送停止请求".into(),Err(error)=>{let message=format!("停止失败：{error:#}；等待本次读取结束");control_failed=Some(message.clone());message} };
                    if let Ok(mut state)=state.lock() && let Some(state)=state.as_mut() && state.execution==execution {state.message=message;}
                    wake();
                }
            }
        }
    }
    active.store(false, Ordering::SeqCst);
    if let Some(handle) = &handle {
        handle.disarm();
    }
    if let Some(control) = control
        && let Err(error) = control.await
    {
        control_failed = Some(format!("停止控制通道失败：{error:#}"));
    }
    drop(guard);
    session.set_table_read_interrupt(None);
    wake();
    if let Some(message) = control_failed {
        return Err(CancelTransportFailure(format!(
            "{message}；原读取可能已完成，管理连接不再复用；请重新连接并刷新"
        ))
        .into());
    }
    match result {
        Err(error) if crate::db::is_query_cancelled(&error) => {
            Err(error.context("表读取已停止，服务器已确认；保留上次数据，重新刷新后可继续编辑"))
        }
        other => other,
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
