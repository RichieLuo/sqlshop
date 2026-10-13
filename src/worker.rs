use crate::{
    db::{Session, TableBrowseOptions, TablePage, TableStructure},
    model::{CellUpdate, Credentials, Engine, Profile, QueryOutput, Table, TableData},
    workspace::QueryTarget,
};
use std::{sync::mpsc, time::Duration};
use uuid::Uuid;

pub enum Request {
    Test(Profile, Credentials),
    Connect(Profile, Credentials),
    Disconnect,
    Databases,
    SelectDatabase(String),
    Tables,
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
    DescribeTable {
        tab: Uuid,
        target: QueryTarget,
        table: Table,
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
}

pub enum Response {
    QueryResult {
        tab: Uuid,
        output: QueryOutput,
        database: Option<String>,
    },
    TablePage {
        tab: Uuid,
        page: TablePage,
    },
    TableStructure {
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
    TableData {
        tab: Uuid,
        data: TableData,
    },
    CellSaved {
        tab: Uuid,
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
                    let read_only_operation = matches!(&request, Request::ReadPage { .. } | Request::DescribeTable { .. });
                    let operation_tab = match &request {
                        Request::RunQuery { tab, .. } | Request::ReadPage { tab, .. }
                        | Request::DescribeTable { tab, .. } | Request::SaveCell { tab, .. } => Some(*tab),
                        _ => None,
                    };
                    let duration = match &request {
                        Request::Connect(profile, _) | Request::Test(profile, _) if profile.ssh.enabled => 30,
                        Request::Connect(..) | Request::Test(..) => 15,
                        Request::SelectDatabase(_) => 30,
                        _ => 60,
                    };
                    let result = tokio::time::timeout(Duration::from_secs(duration), async {
                        Ok::<_, anyhow::Error>(match request {
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
                            Request::DescribeTable { tab, target, table } => {
                                if !target_matches(&target, connection_id, selected_database.as_deref()) {
                                    operation_error(tab, "请先选择此表所属连接和数据库", false)
                                } else {
                                    let connection = session.as_mut().ok_or_else(|| anyhow::anyhow!("请先连接数据库"))?;
                                    Response::TableStructure { tab, structure: connection.table_structure(&table).await? }
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
                        })
                    }).await;
                    let response = match result {
                        Ok(Ok(response)) => response,
                        Ok(Err(error)) if error.downcast_ref::<crate::db::EditRejected>().is_some()
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
