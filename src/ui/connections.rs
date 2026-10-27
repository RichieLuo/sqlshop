use super::*;

impl App {
    pub(super) fn edit_connection(&mut self, id: Uuid) {
        if self.is_redis_connection(id) {
            self.edit_redis_connection(id);
            return;
        }
        let Some(profile) = self.profiles.iter().find(|p| p.id == id).cloned() else {
            return;
        };
        let credentials = match self.credentials_for(&profile) {
            Ok(credentials) => credentials.unwrap_or_default(),
            Err(error) => {
                self.fail(error.to_string());
                return;
            }
        };
        self.form = Some(ConnectionForm::sql(profile, credentials, true));
    }

    pub(super) fn credentials_for(&self, profile: &Profile) -> anyhow::Result<Option<Credentials>> {
        if profile.engine == Engine::SqlServer
            && profile.sqlserver_auth == sqlshop::model::SqlServerAuth::Integrated
            && !profile.ssh.enabled
        {
            return Ok(Some(Credentials::default()));
        }
        if let Some(credentials) = self.credentials.get(&profile.id) {
            return Ok(Some(credentials.clone()));
        }
        self.store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("配置目录不可用"))?
            .credentials(profile)
    }

    pub(super) fn save_connection(
        &mut self,
        profile: Profile,
        credentials: Credentials,
    ) -> anyhow::Result<()> {
        profile.validate()?;
        anyhow::ensure!(
            !self.redis_profiles.iter().any(|p| p.id == profile.id),
            "不能将已保存的 Redis 连接改成 SQL，请新建连接"
        );
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("配置目录不可用"))?;
        let mut profiles = self.profiles.clone();
        if let Some(existing) = profiles.iter_mut().find(|p| p.id == profile.id) {
            *existing = profile.clone();
        } else {
            profiles.push(profile.clone());
        }
        store.save_credentials(&profile, &credentials)?;
        store.save(&profiles)?;
        self.selected = Some(profile.id);
        self.tree_selection.selected = Some(TreeNode::Connection(profile.id));
        self.credentials.insert(profile.id, credentials);
        self.profiles = profiles;
        self.status = "连接已保存".into();
        self.error = false;
        Ok(())
    }

    pub(super) fn connect_selected(&mut self) {
        if let Some(id) = self.selected.filter(|id| self.is_redis_connection(*id)) {
            self.open_redis_connection(id, true);
            return;
        }
        let Some(profile) = self
            .profiles
            .iter()
            .find(|p| Some(p.id) == self.selected)
            .cloned()
        else {
            return;
        };
        if self
            .sessions
            .get(&profile.id)
            .is_some_and(|s| s.connected.is_some() || s.busy)
        {
            self.status = "此连接已连接或正在操作；选择节点不会重连或断开其他会话".into();
            return;
        }
        let saved_credentials = match self.credentials_for(&profile) {
            Ok(credentials) => credentials,
            Err(error) => {
                self.fail(error.to_string());
                return;
            }
        };
        let missing_credentials = saved_credentials.is_none();
        let credentials = saved_credentials.unwrap_or_default();
        if missing_credentials
            || (!self.credentials.contains_key(&profile.id)
                && (!profile.remember_password
                    || (profile.ssh.enabled && !profile.ssh.remember_secret)))
        {
            self.form = Some(ConnectionForm::sql(profile, credentials, true));
            self.status = "请填写数据库 / SSH 凭据，勾选记住后保存在本机".into();
            return;
        }
        self.request(Request::Connect(profile, credentials), "正在连接数据库…");
    }

    pub(super) fn load_table(&mut self, index: usize) {
        self.load_table_page(index, false);
    }

    pub(super) fn load_last_table_page(&mut self, index: usize) {
        self.load_table_page(index, true);
    }

    pub(super) fn load_table_page(&mut self, index: usize, last_page: bool) {
        if self.tabs[index].design.is_some() {
            self.load_structure(index);
            return;
        }
        if self.tab_busy(index) || self.cell_editor.is_some() {
            return;
        }
        if self.tabs[index].has_table_drafts() {
            self.fail("存在暂存修改；请先提交或丢弃后再刷新、翻页或筛选".into());
            return;
        }
        let Some(target) = self.tabs[index].table_target.clone() else {
            return;
        };
        let Some(session) = self.sessions.get(&target.connection) else {
            return;
        };
        let Some(profile) = session.connected.as_ref() else {
            return;
        };
        if session.active_database.as_ref() != Some(&target.database) {
            self.fail("请先选择此表所属连接的数据库；不会自动切库".into());
            return;
        }
        let table = session
            .tables
            .iter()
            .find(|table| table.schema == target.schema && table.name == target.name)
            .cloned()
            .unwrap_or(Table {
                schema: target.schema,
                name: target.name,
                kind: "BASE TABLE".into(),
            });
        let tab = &mut self.tabs[index];
        if !tab.dirty() {
            tab.sql = profile.engine.preview_sql(&table);
            tab.saved_sql = tab.sql.clone();
        }
        tab.selection.clear();
        tab.query_started = true;
        tab.table_snapshot_stale = true;
        tab.selected_cell = None;
        tab.selected_rows = None;
        tab.source = format!("{} / {}", profile.name, target.database);
        tab.cell_anchor = None;
        tab.grid_scroll_row = None;
        let query_target = QueryTarget {
            schema: None,
            connection: target.connection,
            database: Some(target.database),
        };
        let request = if last_page {
            Request::ReadLastPage {
                tab: tab.id,
                target: query_target,
                table,
                options: tab.browse.clone(),
            }
        } else {
            Request::ReadPage {
                tab: tab.id,
                target: query_target,
                table,
                options: tab.browse.clone(),
            }
        };
        self.request(
            request,
            if last_page {
                "正在定位并读取末页…"
            } else {
                "正在读取表数据…"
            },
        );
    }

    pub(super) fn execute(&mut self) {
        self.execute_scope(SqlExecutionScope::SelectionOrAll);
    }

    pub(super) fn execute_scope(&mut self, scope: SqlExecutionScope) {
        if self.modal_open()
            || self.table_create.is_some()
            || self.tabs[self.active_tab].table_target.is_some()
            || self.tabs[self.active_tab].definition.is_some()
        {
            return;
        }
        let Some(current) = self.execution_target() else {
            self.fail("请先连接服务器并选择执行目标".into());
            return;
        };
        if !self.query_target_ready(self.tabs[self.active_tab].id, &current) {
            self.fail("查询目标未连接或连接配置已变化；请连接服务器或重新选择标签目标。".into());
            return;
        }
        if self.tab_busy(self.active_tab) {
            return;
        }
        if self.confirm_rebind.is_some()
            || self.confirm_tab_action.is_some()
            || self.confirm_request.is_some()
            || self.confirm_exit
        {
            return;
        }
        if !self.tabs[self.active_tab].bind_for_execution(&current) {
            self.fail("当前连接或数据库与此 SQL 标签绑定目标不同。请选择原目标，或点击“改绑到当前”并确认。".into());
            return;
        }
        let session_epoch = self.tabs[self.active_tab].query_epoch;
        let profile = self
            .profiles
            .iter()
            .find(|p| p.id == current.connection)
            .unwrap()
            .clone();
        let connection_name = profile.name.clone();
        let engine = profile.engine;
        let tab = &self.tabs[self.active_tab];
        let sql = match scope {
            SqlExecutionScope::SelectionOrAll if !tab.selection.trim().is_empty() => {
                tab.selection.clone()
            }
            SqlExecutionScope::SelectionOrAll | SqlExecutionScope::All => tab.sql.clone(),
            SqlExecutionScope::CurrentStatement => {
                match current_statement_range(&tab.sql, tab.cursor, engine) {
                    Ok(Some(range)) => tab.sql[range].to_owned(),
                    Ok(None) => {
                        self.fail("光标所在位置没有可执行的 SQL 语句".into());
                        return;
                    }
                    Err(message) => {
                        self.fail(message);
                        return;
                    }
                }
            }
        };
        if sql.trim().is_empty() {
            self.fail("SQL 不能为空".into());
            return;
        }
        let document_char_start = match scope {
            SqlExecutionScope::SelectionOrAll if !tab.selection.trim().is_empty() => {
                tab.selection_char_start
            }
            SqlExecutionScope::CurrentStatement => {
                current_statement_range(&tab.sql, tab.cursor, engine)
                    .ok()
                    .flatten()
                    .map(|range| byte_to_char_index(&tab.sql, range.start))
            }
            _ => Some(0),
        };
        let pending = PendingSqlExecution {
            tab: tab.id,
            target: current.clone(),
            sql,
            session_epoch,
            connection_name,
            profile,
            document_sql: tab.sql.clone(),
            document_char_start,
        };
        self.send_sql(pending);
    }

    pub(super) fn send_sql(&mut self, pending: PendingSqlExecution) {
        self.send_private_query(pending);
    }
}

pub(super) fn open_table_tab(
    tabs: &mut Vec<QueryTab>,
    next_tab: &mut usize,
    target: TableTarget,
    engine: Engine,
    table: &Table,
) -> (usize, bool) {
    if let Some(index) = tabs
        .iter()
        .position(|tab| tab.design.is_none() && tab.table_target.as_ref() == Some(&target))
    {
        tabs[index].table_kind = Some(table.kind.clone());
        return (index, false);
    }
    let mut tab = QueryTab::new(*next_tab);
    tab.name = table.name.clone();
    tab.sql = engine.preview_sql(table);
    tab.target = Some(QueryTarget {
        schema: (engine == Engine::Postgres).then(|| target.schema.clone()),
        connection: target.connection,
        database: Some(target.database.clone()),
    });
    tab.saved_sql = tab.sql.clone();
    tab.table_target = Some(target);
    tab.table_kind = Some(table.kind.clone());
    *next_tab += 1;
    tabs.push(tab);
    (tabs.len() - 1, true)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn database_context_menu(
    ui: &mut egui::Ui,
    connection: Uuid,
    engine: Engine,
    database: &str,
    active: bool,
    busy: bool,
    protected_database: bool,
    schema_dialog: &mut Option<SchemaDialog>,
    table_create: &mut Option<TableCreateForm>,
    to_select: &mut Option<(Uuid, String)>,
    refresh_tables: &mut Option<Uuid>,
) {
    if ui
        .add_enabled(!busy && active, egui::Button::new("新建数据表…"))
        .clicked()
    {
        *table_create = Some(new_table_create_form(
            connection,
            database.to_owned(),
            match engine {
                Engine::MySql => database.to_owned(),
                Engine::Postgres => "public".into(),
                Engine::SqlServer => "dbo".into(),
            },
            engine,
        ));
        ui.close();
    }
    if ui
        .add_enabled(
            !busy && !active && !protected_database,
            egui::Button::new("删除数据库…"),
        )
        .clicked()
    {
        *schema_dialog = Some(SchemaDialog {
            action: SchemaDialogAction::DropDatabase {
                connection,
                database: database.to_owned(),
            },
            name: String::new(),
        });
        ui.close();
    }
    ui.separator();
    if ui
        .add_enabled(!busy && !active, egui::Button::new("打开数据库"))
        .clicked()
    {
        *to_select = Some((connection, database.to_owned()));
        ui.close();
    }
    if ui
        .add_enabled(active && !busy, egui::Button::new("刷新表和视图"))
        .clicked()
    {
        *refresh_tables = Some(connection);
        ui.close();
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn connection_context_menu(
    ui: &mut egui::Ui,
    connection: Uuid,
    connected: bool,
    busy: bool,
    catalog_writable: bool,
    current_group: Option<Uuid>,
    buckets: &[(Option<Uuid>, String)],
    schema_dialog: &mut Option<SchemaDialog>,
    disconnect: &mut Option<Uuid>,
    refresh_databases: &mut Option<Uuid>,
    to_connect: &mut Option<Uuid>,
    move_connection: &mut Option<(Uuid, Option<Uuid>)>,
    to_edit: &mut Option<Uuid>,
    to_delete: &mut Option<Uuid>,
) {
    if connected {
        if ui
            .add_enabled(!busy, egui::Button::new("新建数据库…"))
            .clicked()
        {
            *schema_dialog = Some(SchemaDialog {
                action: SchemaDialogAction::CreateDatabase { connection },
                name: String::new(),
            });
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled(!busy, egui::Button::new("断开连接"))
            .clicked()
        {
            *disconnect = Some(connection);
            ui.close();
        }
        if ui
            .add_enabled(!busy, egui::Button::new("刷新数据库列表"))
            .clicked()
        {
            *refresh_databases = Some(connection);
            ui.close();
        }
    } else if ui
        .add_enabled(!busy, egui::Button::new("连接服务器"))
        .clicked()
    {
        *to_connect = Some(connection);
        ui.close();
    }
    ui.separator();
    ui.add_enabled_ui(!busy && catalog_writable, |ui| {
        ui.menu_button("移动到分组", |ui| {
            for (id, name) in buckets {
                if ui.selectable_label(current_group == *id, name).clicked() {
                    *move_connection = Some((connection, *id));
                    ui.close();
                }
            }
        });
    });
    if ui
        .add_enabled(!busy, egui::Button::new("编辑连接"))
        .clicked()
    {
        *to_edit = Some(connection);
        ui.close();
    }
    if ui
        .add_enabled(!busy, egui::Button::new("删除连接…"))
        .clicked()
    {
        *to_delete = Some(connection);
        ui.close();
    }
}

#[cfg(test)]
pub(super) fn table_buttons(
    ui: &mut egui::Ui,
    tables: &[&Table],
    selection: &mut TreeSelection,
    connection: Uuid,
    database: &str,
    busy: bool,
    schema_dialog: &mut Option<SchemaDialog>,
) -> Option<Table> {
    table_tree_ui::tables(
        ui,
        tables,
        selection,
        connection,
        database,
        busy,
        schema_dialog,
        &mut table_tree_ui::TreeRender::default(),
        &HashMap::new(),
    )
}

pub(super) fn connection_heading(ui: &mut egui::Ui, form: &mut ConnectionForm) {
    ui.horizontal(|ui| {
        ui.label("连接名称");
        let name = match form.kind {
            ConnectionKind::Sql(_) => &mut form.profile.name,
            ConnectionKind::Redis => &mut form.redis_profile.name,
        };
        ui.add(
            dialog_input(name)
                .hint_text("例如：开发数据库")
                .desired_width((ui.available_width() - 20.0).min(440.0)),
        );
    });
    let mut kind = form.kind;
    ui.horizontal(|ui| {
        ui.label("数据库类型");
        egui::ComboBox::from_id_salt("connection-kind")
            .selected_text(kind.label())
            .show_ui(ui, |ui| {
                for choice in ConnectionKind::ALL {
                    ui.add_enabled_ui(form.permits_kind(choice), |ui| {
                        ui.selectable_value(&mut kind, choice, choice.label());
                    });
                }
            });
    });
    form.set_kind(kind);
}

pub(super) fn database_fields(ui: &mut egui::Ui, form: &mut ConnectionForm) {
    if form.kind == ConnectionKind::Redis {
        redis_connections_ui::redis_database_fields(ui, form);
        return;
    }
    ui.set_min_width(ui.available_width());
    ui.strong("数据库");
    ui.add_space(6.0);
    if form.profile.engine == Engine::SqlServer {
        use sqlshop::model::SqlServerAuth;
        ui.horizontal_wrapped(|ui| {
            ui.label("认证");
            for (method, label) in [
                (SqlServerAuth::Password, "SQL Server 密码"),
                (SqlServerAuth::AadToken, "Entra 令牌"),
            ] {
                ui.selectable_value(&mut form.profile.sqlserver_auth, method, label);
            }
            #[cfg(windows)]
            for (method, label) in [
                (SqlServerAuth::Integrated, "当前 Windows 用户"),
                (SqlServerAuth::Windows, "Windows 凭据"),
            ] {
                ui.selectable_value(&mut form.profile.sqlserver_auth, method, label);
            }
        });
        ui.horizontal(|ui| {
            ui.label("命名实例");
            ui.add(dialog_input(&mut form.profile.sqlserver_instance).hint_text("可选"));
        });
        if form.profile.sqlserver_auth == sqlshop::model::SqlServerAuth::AadToken {
            ui.horizontal(|ui| {
                ui.label("初始数据库");
                ui.add(dialog_input(&mut form.profile.database).hint_text("可选"));
            });
        }
    }
    let input_width = (ui.available_width() - 90.0).clamp(140.0, 330.0);
    egui::Grid::new("connection-form")
        .num_columns(2)
        .spacing([14.0, 12.0])
        .show(ui, |ui| {
            ui.label("主机");
            ui.add(dialog_input(&mut form.profile.host).desired_width(input_width));
            ui.end_row();
            ui.label("端口");
            ui.add(egui::DragValue::new(&mut form.profile.port).range(1..=65535));
            ui.end_row();
            ui.label("用户名");
            ui.add(dialog_input(&mut form.profile.username).desired_width(input_width));
            ui.end_row();
            ui.label(
                if form.profile.engine == Engine::SqlServer
                    && form.profile.sqlserver_auth == sqlshop::model::SqlServerAuth::AadToken
                {
                    "访问令牌"
                } else {
                    "密码"
                },
            );
            ui.add(
                dialog_input(&mut form.password)
                    .password(true)
                    .desired_width(input_width),
            );
            ui.end_row();
        });
    ui.add_space(8.0);
    ui.checkbox(
        &mut form.profile.remember_password,
        if form.profile.engine == Engine::SqlServer
            && form.profile.sqlserver_auth == sqlshop::model::SqlServerAuth::AadToken
        {
            "记住访问令牌"
        } else {
            "记住密码"
        },
    );
}

pub(super) fn connection_ssh_fields(
    ui: &mut egui::Ui,
    form: &mut ConnectionForm,
) -> Option<String> {
    let ssh = match form.kind {
        ConnectionKind::Sql(_) => &mut form.profile.ssh,
        ConnectionKind::Redis => &mut form.redis_profile.ssh,
    };
    ssh_fields(ui, ssh, &mut form.ssh_secret)
}

pub(super) fn ssh_fields(
    ui: &mut egui::Ui,
    ssh: &mut sqlshop::model::SshConfig,
    ssh_secret: &mut String,
) -> Option<String> {
    ui.set_min_width(ui.available_width());
    ui.strong("SSH 隧道");
    ui.checkbox(&mut ssh.enabled, "通过 SSH 隧道连接");
    let mut selection_error = None;
    let input_width = (ui.available_width() - 90.0).clamp(140.0, 330.0);
    ui.add_enabled_ui(ssh.enabled, |ui| {
        egui::Grid::new("ssh-form")
            .num_columns(2)
            .spacing([14.0, 10.0])
            .show(ui, |ui| {
                ui.label("主机");
                ui.add(dialog_input(&mut ssh.host).desired_width(input_width));
                ui.end_row();
                ui.label("端口");
                ui.add(egui::DragValue::new(&mut ssh.port).range(1..=65535));
                ui.end_row();
                ui.label("用户名");
                ui.add(dialog_input(&mut ssh.username).desired_width(input_width));
                ui.end_row();
                ui.label("认证方式");
                let previous = ssh.auth;
                egui::ComboBox::from_id_salt("ssh-auth")
                    .selected_text(match previous {
                        SshAuth::Password => "密码",
                        SshAuth::PrivateKey => "私钥",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut ssh.auth, SshAuth::Password, "密码");
                        ui.selectable_value(&mut ssh.auth, SshAuth::PrivateKey, "私钥");
                    });
                if previous != ssh.auth {
                    (*ssh_secret).clear();
                }
                ui.end_row();
                if ssh.auth == SshAuth::PrivateKey {
                    ui.label("私钥文件");
                    ui.horizontal(|ui| {
                        ui.add(
                            dialog_input(&mut ssh.private_key)
                                .desired_width((input_width - 100.0).max(90.0)),
                        )
                        .on_hover_text(&ssh.private_key);
                        if ui.button("选择文件…").clicked() {
                            match choose_private_key(&ssh.private_key) {
                                Ok(Some(path)) => ssh.private_key = path.display().to_string(),
                                Ok(None) => {}
                                Err(error) => selection_error = Some(format!("{error:#}")),
                            }
                        }
                    });
                    ui.end_row();
                }
                ui.label(if ssh.auth == SshAuth::Password {
                    "密码"
                } else {
                    "私钥口令"
                });
                ui.add(
                    dialog_input(&mut (*ssh_secret))
                        .password(true)
                        .desired_width(input_width)
                        .hint_text(if ssh.auth == SshAuth::PrivateKey {
                            "未加密的私钥可留空"
                        } else {
                            ""
                        }),
                );
                ui.end_row();
            });
        ui.add_space(6.0);
        ui.checkbox(&mut ssh.remember_secret, "记住 SSH 凭据");
        egui::CollapsingHeader::new("SSH 主机校验")
            .default_open(false)
            .show(ui, |ui| {
                ui.label("known_hosts 文件");
                ui.add(dialog_input(&mut ssh.known_hosts).desired_width(input_width));
                ui.label("服务器指纹（可选）");
                ui.add(
                    dialog_input(&mut ssh.fingerprint)
                        .hint_text("SHA256:…")
                        .desired_width(input_width),
                );
            });
    });
    selection_error
}

pub(super) fn database_tls_settings(ui: &mut egui::Ui, profile: &mut Profile) {
    ui.checkbox(&mut profile.tls, "启用 TLS 加密");
    if !profile.tls {
        profile.trust_certificate = false;
    }
    ui.add_enabled(
        profile.tls,
        egui::Checkbox::new(&mut profile.trust_certificate, "信任服务器证书（跳过证书校验）"),
    ).on_hover_text("保留数据库 TLS 加密，但跳过数据库证书校验；请先确认服务器可信。此选项不改变 SSH 主机密钥校验。");
    if profile.ssh.enabled
        && profile.tls
        && !profile.trust_certificate
        && profile.engine != Engine::SqlServer
    {
        ui.label(
            RichText::new("严格数据库 TLS 校验要求证书包含 127.0.0.1；SSH 不会自动改变 TLS 设置。")
                .small(),
        );
    }
}

pub(super) fn choose_private_key(current: &str) -> anyhow::Result<Option<PathBuf>> {
    let current = sqlshop::ssh::expand_path(current)?;
    let directory = if current.is_dir() {
        Some(current.clone())
    } else {
        current
            .parent()
            .filter(|path| path.is_dir())
            .map(PathBuf::from)
    }
    .or_else(|| {
        directories::UserDirs::new().map(|user| {
            let ssh = user.home_dir().join(".ssh");
            if ssh.is_dir() {
                ssh
            } else {
                user.home_dir().to_path_buf()
            }
        })
    });
    let mut dialog = rfd::FileDialog::new().set_title("选择 SSH 私钥文件（不要选择 .pub 公钥）");
    if let Some(directory) = directory {
        dialog = dialog.set_directory(directory);
    }
    // Private keys frequently have no extension; do not filter them out.
    dialog
        .pick_file()
        .map(|path| sqlshop::ssh::private_key_path(&path.display().to_string()))
        .transpose()
}
