use crate::{
    db::Session,
    model::{CellUpdate, Credentials, Profile, QueryOutput, Table, TableData},
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
    Tested,
    Connected(Uuid),
    Disconnected,
    Databases(Vec<String>),
    DatabaseSelected(String),
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

pub struct Worker {
    sender: tokio::sync::mpsc::UnboundedSender<Request>,
    pub receiver: mpsc::Receiver<Response>,
}

impl Worker {
    pub fn start(wake: impl Fn() + Send + 'static) -> Self {
        let (sender, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let (responses, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let runtime = match tokio::runtime::Runtime::new() {
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
                let mut physical_database = None;
                let mut selected_database = None;
                while let Some(request) = requests.recv().await {
                    let modifies_session = !matches!(&request, Request::Test(..));
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
                                Response::Connected(profile.id)
                            }
                            Request::Disconnect => {
                                session = None; physical_database = None; selected_database = None;
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
                                let output = connection.query(&sql).await?;
                                let current = connection.current_database().await?;
                                if current != physical_database { selected_database = current.clone(); }
                                physical_database = current;
                                Response::Query { output, database: selected_database.clone() }
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
                        failure => {
                            if modifies_session { session = None; physical_database = None; selected_database = None; }
                            let message = match failure {
                                Ok(Err(error)) => format!("{error:#}"),
                                Err(_) if modifies_session => format!("操作超过 {duration} 秒；连接已关闭。写入是否提交请在重新连接后确认。"),
                                Err(_) => format!("连接测试超过 {duration} 秒，请检查服务器地址和网络。"),
                                _ => unreachable!(),
                            };
                            Response::Error { message, disconnected: modifies_session }
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
