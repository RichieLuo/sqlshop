use super::*;
use sqlshop::worker::{QueryRequest, QueryResponse, QueryWorker};

pub(super) struct QuerySessionState {
    pub worker: QueryWorker,
    pub profile: Profile,
    pub target: QueryTarget,
    pub busy: bool,
    pub connected: bool,
    pub user_sql: bool,
    pub schemas_target: Option<QueryTarget>,
    pub schemas: Vec<String>,
    pub schemas_loading: bool,
    pub schemas_error: Option<String>,
    pub execution: Option<Uuid>,
    pub cancellable: bool,
    pub cancel_requested: bool,
    pub transaction: sqlshop::query_control::QueryTransactionState,
    pub transaction_verified: bool,
}

fn same_profile(left: &Profile, right: &Profile) -> bool {
    left.id == right.id
        && left.engine == right.engine
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

impl App {
    pub(super) fn request_query_schemas(&mut self, target: &QueryTarget, refresh: bool) {
        if self.screenshot_path.is_some() || self.tab_busy(self.active_tab) {
            return;
        }
        let id = self.tabs[self.active_tab].id;
        let mut discovery_target = target.clone();
        discovery_target.schema = None;
        if self.query_sessions.get(&id).is_some_and(|session| {
            session.schemas_loading
                || (!refresh && session.schemas_target.as_ref() == Some(&discovery_target))
        }) {
            return;
        }
        let Some(profile) = self
            .profiles
            .iter()
            .find(|profile| profile.id == target.connection)
            .cloned()
        else {
            return;
        };
        if profile.engine != Engine::Postgres || target.database.is_none() {
            return;
        }
        let credentials = match self.credentials_for(&profile) {
            Ok(Some(credentials)) => credentials,
            _ => return,
        };
        let ctx = self.context.clone();
        let state = self
            .query_sessions
            .entry(id)
            .or_insert_with(|| QuerySessionState {
                worker: QueryWorker::start(id, move || ctx.request_repaint()),
                profile: profile.clone(),
                target: target.clone(),
                busy: false,
                connected: false,
                user_sql: false,
                schemas_target: None,
                schemas: Vec::new(),
                schemas_loading: false,
                schemas_error: None,
                execution: None,
                cancellable: false,
                cancel_requested: false,
                transaction: Default::default(),
                transaction_verified: false,
            });
        state.schemas_target = Some(discovery_target.clone());
        state.schemas.clear();
        state.schemas_error = None;
        state.schemas_loading = true;
        if let Err(error) = state.worker.send(QueryRequest::LoadSchemas {
            profile,
            credentials,
            target: discovery_target,
        }) {
            state.schemas_loading = false;
            state.schemas_error = Some(error.to_string());
        }
    }

    pub(super) fn query_target_ready(&self, tab: Uuid, target: &QueryTarget) -> bool {
        let Some(profile) = self.profiles.iter().find(|p| p.id == target.connection) else {
            return false;
        };
        if let Some(session) = self.query_sessions.get(&tab)
            && session.connected
        {
            return session.target == *target && same_profile(&session.profile, profile);
        }
        self.sessions
            .get(&target.connection)
            .is_some_and(|s| s.connected.is_some())
    }

    pub(super) fn query_connection_busy(&self, connection: Uuid) -> bool {
        self.query_sessions
            .values()
            .any(|s| s.target.connection == connection && s.busy)
    }

    pub(super) fn query_connection_used(&self, connection: Uuid) -> bool {
        self.query_sessions
            .values()
            .any(|s| s.target.connection == connection && s.connected && s.user_sql)
    }

    pub(super) fn close_query_sessions(&mut self, connection: Uuid) {
        let ids: Vec<_> = self
            .query_sessions
            .iter()
            .filter(|(_, s)| s.target.connection == connection)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some(session) = self.query_sessions.remove(&id) {
                let _ = session.worker.send(QueryRequest::Disconnect);
            }
        }
        for tab in &mut self.tabs {
            if tab.table_target.is_none()
                && tab
                    .target
                    .as_ref()
                    .is_some_and(|t| t.connection == connection)
            {
                tab.query_epoch = tab.query_epoch.wrapping_add(1);
            }
        }
    }

    pub(super) fn prune_query_sessions(&mut self) {
        let closed: Vec<_> = self
            .query_sessions
            .keys()
            .filter(|id| !self.tabs.iter().any(|t| t.id == **id))
            .copied()
            .collect();
        for id in closed {
            if let Some(session) = self.query_sessions.remove(&id) {
                let _ = session.worker.send(QueryRequest::Disconnect);
            }
        }
    }

    pub(super) fn apply_query_target(&mut self, id: Uuid, target: QueryTarget) {
        if self.query_sessions.get(&id).is_some_and(|s| s.busy) {
            return;
        }
        if let Some(session) = self.query_sessions.remove(&id) {
            let _ = session.worker.send(QueryRequest::Disconnect);
        }
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            tab.target = Some(target);
            tab.query_epoch = tab.query_epoch.wrapping_add(1);
            tab.output = None;
            tab.last_executed_sql = None;
            tab.last_executed_document = None;
            tab.last_executed_char_start = None;
            tab.query_started = false;
            tab.query_error = None;
            tab.query_diagnostic = None;
            tab.query_elapsed_ms = None;
            tab.result_index = 0;
            tab.selection.clear();
            super::query_result_ui::reset_result_view(&self.context, tab.id);
        }
    }

    pub(super) fn send_private_query(&mut self, pending: PendingSqlExecution) {
        if self.screenshot_path.is_some() {
            return;
        }
        let Some(index) = self.tabs.iter().position(|t| t.id == pending.tab) else {
            self.fail("原 SQL 标签已关闭，未执行".into());
            return;
        };
        let profile = self
            .profiles
            .iter()
            .find(|p| p.id == pending.target.connection);
        if self.tabs[index].target.as_ref() != Some(&pending.target)
            || self.tabs[index].query_epoch != pending.session_epoch
            || self.tab_busy(index)
            || !self.query_target_ready(pending.tab, &pending.target)
            || profile.is_none_or(|p| !same_profile(p, &pending.profile))
        {
            self.fail("SQL 执行目标或会话已变化，未执行；请重新点击运行".into());
            return;
        }
        let credentials = if self
            .query_sessions
            .get(&pending.tab)
            .is_some_and(|session| session.connected)
        {
            // A live actor already owns its authenticated connection. Re-reading
            // saved credentials must not prevent ROLLBACK on that connection.
            Credentials::default()
        } else {
            match self.credentials_for(&pending.profile) {
                Ok(Some(value)) => value,
                Ok(None) => {
                    self.fail("此连接缺少凭据，请在连接设置中补充".into());
                    return;
                }
                Err(error) => {
                    self.fail(format!("读取连接凭据失败：{error:#}"));
                    return;
                }
            }
        };
        let ctx = self.context.clone();
        let session = self
            .query_sessions
            .entry(pending.tab)
            .or_insert_with(|| QuerySessionState {
                worker: QueryWorker::start(pending.tab, move || ctx.request_repaint()),
                profile: pending.profile.clone(),
                target: pending.target.clone(),
                busy: false,
                connected: false,
                user_sql: false,
                schemas_target: None,
                schemas: Vec::new(),
                schemas_loading: false,
                schemas_error: None,
                execution: None,
                cancellable: false,
                cancel_requested: false,
                transaction: Default::default(),
                transaction_verified: false,
            });
        if !session.connected {
            session.profile = pending.profile.clone();
            session.target = pending.target.clone();
        }
        if let Err(error) = session.worker.send(QueryRequest::Run {
            profile: pending.profile,
            credentials,
            target: pending.target.clone(),
            sql: pending.sql.clone(),
        }) {
            self.query_sessions.remove(&pending.tab);
            self.tabs[index].query_epoch = self.tabs[index].query_epoch.wrapping_add(1);
            self.tabs[index].query_error = Some(error.to_string());
            self.tabs[index].query_diagnostic = None;
            self.tabs[index].query_elapsed_ms = None;
            self.fail(error.to_string());
            return;
        }
        session.busy = true;
        session.user_sql = true;
        let tab = &mut self.tabs[index];
        tab.last_executed_document = Some(pending.document_sql);
        tab.last_executed_char_start = pending.document_char_start;
        super::query_result_ui::reset_result_view(&self.context, tab.id);
        tab.output = None;
        tab.table_data = None;
        tab.query_started = true;
        tab.query_error = None;
        tab.query_diagnostic = None;
        tab.query_elapsed_ms = None;
        tab.source = query_source(&pending.connection_name, &pending.target);
        self.pending_history.insert(
            tab.id,
            HistoryEntry {
                sql: pending.sql,
                target: pending.target,
                source: tab.source.clone(),
                timestamp: chrono::Utc::now().timestamp(),
                success: false,
            },
        );
        self.status = "正在执行 SQL…".into();
        self.error = false;
    }

    pub(super) fn poll_query_sessions(&mut self) {
        let responses: Vec<_> = self
            .query_sessions
            .values()
            .flat_map(|s| s.worker.receiver.try_iter())
            .collect();
        for response in responses {
            self.handle_query_response(response);
        }
    }

    pub(super) fn handle_query_response(&mut self, response: QueryResponse) {
        match &response {
            QueryResponse::ContextChecked { tab, target } => {
                if let Some(session) = self.query_sessions.get_mut(tab) {
                    session.target = target.clone();
                    if let Some(document) =
                        self.tabs.iter_mut().find(|document| document.id == *tab)
                    {
                        document.target = Some(target.clone());
                        document.source = query_source(&session.profile.name, target);
                    }
                }
                return;
            }
            QueryResponse::ExecutionStarted {
                tab,
                execution,
                cancellable,
                reason,
            } => {
                if let Some(session) = self.query_sessions.get_mut(tab)
                    && session.busy
                {
                    session.execution = Some(*execution);
                    session.cancellable = *cancellable;
                    session.cancel_requested = false;
                    session.transaction_verified = false;
                    if let Some(reason) = reason {
                        self.status = reason.clone();
                    }
                }
                return;
            }
            QueryResponse::CancelStatus {
                tab,
                execution,
                message,
                failed,
            } => {
                if let Some(session) = self.query_sessions.get_mut(tab)
                    && session.execution == Some(*execution)
                {
                    self.status = message.clone();
                    self.error = *failed;
                }
                return;
            }
            QueryResponse::TransactionState {
                tab,
                state,
                verified,
            } => {
                if let Some(session) = self.query_sessions.get_mut(tab) {
                    session.transaction = *state;
                    session.transaction_verified = *verified;
                }
                return;
            }
            QueryResponse::ControlFinished {
                tab,
                message,
                connected,
            } => {
                if let Some(session) = self.query_sessions.get_mut(tab) {
                    session.busy = false;
                    session.connected = *connected;
                    session.execution = None;
                    session.cancellable = false;
                    session.cancel_requested = false;
                }
                self.status = message.clone();
                self.error = !connected;
                if !connected {
                    self.query_sessions.remove(tab);
                }
                return;
            }
            _ => {}
        }
        if let QueryResponse::SchemasLoaded {
            tab,
            target,
            schemas,
        } = &response
        {
            if let Some(session) = self.query_sessions.get_mut(tab)
                && session.schemas_target.as_ref() == Some(target)
            {
                session.schemas = schemas.clone();
                session.schemas_loading = false;
                session.schemas_error = None;
            }
            return;
        }
        if let QueryResponse::SchemaLoadError {
            tab,
            target,
            message,
        } = &response
        {
            if let Some(session) = self.query_sessions.get_mut(tab)
                && session.schemas_target.as_ref() == Some(target)
            {
                session.schemas_loading = false;
                session.schemas_error = Some(message.clone());
            }
            return;
        }
        let (id, database, schema, connected) = match &response {
            QueryResponse::Finished {
                tab,
                database,
                schema,
                ..
            } => (*tab, database.clone(), schema.clone(), true),
            QueryResponse::Error {
                tab,
                database,
                schema,
                connected,
                ..
            } => (*tab, database.clone(), schema.clone(), *connected),
            QueryResponse::Disconnected { tab } => (*tab, None, None, false),
            QueryResponse::SchemasLoaded { .. }
            | QueryResponse::SchemaLoadError { .. }
            | QueryResponse::ExecutionStarted { .. }
            | QueryResponse::CancelStatus { .. }
            | QueryResponse::TransactionState { .. }
            | QueryResponse::ControlFinished { .. }
            | QueryResponse::ContextChecked { .. } => {
                unreachable!()
            }
        };
        let Some(session) = self.query_sessions.get_mut(&id) else {
            return;
        };
        if !session.busy && !matches!(response, QueryResponse::Disconnected { .. }) {
            return;
        }
        session.busy = false;
        session.execution = None;
        session.cancellable = false;
        session.cancel_requested = false;
        session.connected = connected;
        if connected {
            session.target.database = database.clone();
            session.target.schema = schema;
        }
        let source = query_source(&session.profile.name, &session.target);
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        if connected {
            tab.target = Some(session.target.clone());
            tab.source = source;
        }
        match response {
            QueryResponse::Finished { output, .. } => {
                tab.query_elapsed_ms = Some(output.elapsed_ms);
                tab.output = Some(output);
                tab.result_index = 0;
                tab.query_error = None;
                tab.query_diagnostic = None;
                if let Some(history) = self.pending_history.get(&id) {
                    tab.last_executed_sql = Some(history.sql.clone());
                }
                self.finish_history(id, true);
                self.status = "SQL 执行完成".into();
                self.error = false;
            }
            QueryResponse::Error {
                message,
                diagnostic,
                elapsed_ms,
                partial_output,
                ..
            } => {
                tab.output = partial_output;
                tab.result_index = 0;
                tab.query_error = Some(message.clone());
                tab.query_diagnostic = diagnostic;
                tab.query_elapsed_ms = elapsed_ms;
                super::query_result_ui::show_error_view(&self.context, tab.id);
                if let Some(history) = self.pending_history.get(&id) {
                    tab.last_executed_sql = Some(history.sql.clone());
                }
                self.finish_history(id, false);
                if !connected {
                    self.query_sessions.remove(&id);
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
                        tab.query_epoch = tab.query_epoch.wrapping_add(1);
                    }
                }
                if message.starts_with("查询已停止") {
                    self.status = message;
                    self.error = false;
                } else {
                    self.fail(message);
                }
            }
            QueryResponse::Disconnected { .. } => {
                tab.query_epoch = tab.query_epoch.wrapping_add(1);
            }
            QueryResponse::SchemasLoaded { .. }
            | QueryResponse::SchemaLoadError { .. }
            | QueryResponse::ExecutionStarted { .. }
            | QueryResponse::CancelStatus { .. }
            | QueryResponse::TransactionState { .. }
            | QueryResponse::ControlFinished { .. }
            | QueryResponse::ContextChecked { .. } => {
                unreachable!()
            }
        }
    }

    pub(super) fn query_control_toolbar(&mut self, ui: &mut egui::Ui) {
        use sqlshop::query_control::{
            QueryTransactionState as State, TransactionCommand as Command,
        };
        let id = self.tabs[self.active_tab].id;
        let Some(session) = self.query_sessions.get_mut(&id) else {
            return;
        };
        if session.busy {
            if ui
                .add_enabled(
                    session.cancellable && !session.cancel_requested,
                    egui::Button::new("■ 停止"),
                )
                .on_hover_text("取消当前执行；服务器确认后才会结束运行状态")
                .clicked()
                && let Some(execution) = session.execution
            {
                match session.worker.cancel(execution) {
                    Ok(()) => {
                        session.cancel_requested = true;
                        self.status = "正在发送停止请求…".into();
                    }
                    Err(error) => {
                        self.status = error.to_string();
                        self.error = true;
                    }
                }
            }
            return;
        }
        if !session.connected {
            return;
        }
        let label = match session.transaction {
            State::Idle => "事务：空闲",
            State::Active => "事务：进行中",
            State::Failed => "事务：需回滚",
            State::Unknown => "事务：待核实",
        };
        ui.small(label);
        let mut request = None;
        ui.menu_button("事务", |ui| {
            if ui
                .button("检查执行上下文")
                .on_hover_text(
                    "主动读取数据库 / Schema；会改变 MySQL ROW_COUNT() 与 SQL Server @@ROWCOUNT",
                )
                .clicked()
            {
                request = Some(QueryRequest::InspectContext {
                    target: session.target.clone(),
                });
                ui.close();
            }
            if session.profile.engine == Engine::SqlServer && !session.transaction_verified {
                ui.small("SQL Server 可提交状态需要主动核实");
            }
            if ui
                .button("核实状态")
                .on_hover_text("SQL Server 使用 XACT_STATE()，会更新 @@ROWCOUNT")
                .clicked()
            {
                request = Some(QueryRequest::InspectTransaction {
                    target: session.target.clone(),
                });
                ui.close();
            }
            for (label, command, allowed) in [
                (
                    "开始事务",
                    Command::Begin,
                    session.transaction == State::Idle,
                ),
                (
                    "提交",
                    Command::Commit,
                    session.transaction == State::Active
                        && (session.profile.engine != Engine::SqlServer
                            || session.transaction_verified),
                ),
                (
                    "回滚",
                    Command::Rollback,
                    session.transaction != State::Idle,
                ),
            ] {
                if ui.add_enabled(allowed, egui::Button::new(label)).clicked() {
                    request = Some(QueryRequest::Transaction {
                        target: session.target.clone(),
                        command,
                    });
                    ui.close();
                }
            }
        });
        if let Some(request) = request {
            match session.worker.send(request) {
                Ok(()) => {
                    session.busy = true;
                    self.status = "正在操作原查询会话事务…".into();
                }
                Err(error) => {
                    self.status = error.to_string();
                    self.error = true;
                }
            }
        }
    }
}

fn query_source(connection_name: &str, target: &QueryTarget) -> String {
    let mut source = format!(
        "{} / {}",
        connection_name,
        target.database.as_deref().unwrap_or("服务器")
    );
    if let Some(schema) = &target.schema {
        source.push_str(" / ");
        source.push_str(schema);
    }
    source
}
