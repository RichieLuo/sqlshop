//! Redis participates in the connection catalog, but never in SQL engines or sessions.
use super::*;
use sqlshop::redis_database::{RedisData, RedisRequest};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConnectionKind {
    Sql(Engine),
    Redis,
}

impl ConnectionKind {
    pub(super) const ALL: [Self; 4] = [
        Self::Sql(Engine::MySql),
        Self::Sql(Engine::Postgres),
        Self::Sql(Engine::SqlServer),
        Self::Redis,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Sql(engine) => engine.label(),
            Self::Redis => "Redis",
        }
    }

    fn same_family(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::Sql(_), Self::Sql(_)) | (Self::Redis, Self::Redis)
        )
    }
}

impl ConnectionForm {
    pub(super) fn sql(profile: Profile, credentials: Credentials, saved: bool) -> Self {
        let kind = ConnectionKind::Sql(profile.engine);
        Self {
            kind,
            saved_kind: saved.then_some(kind),
            redis_profile: RedisProfile {
                id: profile.id,
                ..Default::default()
            },
            profile,
            password: credentials.database_password,
            ssh_secret: credentials.ssh_secret,
        }
    }

    pub(super) fn redis(profile: RedisProfile, credentials: Credentials, saved: bool) -> Self {
        Self {
            kind: ConnectionKind::Redis,
            saved_kind: saved.then_some(ConnectionKind::Redis),
            profile: Profile {
                id: profile.id,
                ..Default::default()
            },
            redis_profile: profile,
            password: credentials.database_password,
            ssh_secret: credentials.ssh_secret,
        }
    }

    pub(super) fn id(&self) -> Uuid {
        match self.kind {
            ConnectionKind::Sql(_) => self.profile.id,
            ConnectionKind::Redis => self.redis_profile.id,
        }
    }

    pub(super) fn credentials(&self) -> Credentials {
        Credentials {
            database_password: self.password.clone(),
            ssh_secret: self.ssh_secret.clone(),
        }
    }

    pub(super) fn permits_kind(&self, kind: ConnectionKind) -> bool {
        self.saved_kind.is_none_or(|saved| saved.same_family(kind))
    }

    pub(super) fn set_kind(&mut self, kind: ConnectionKind) {
        if self.kind == kind || !self.permits_kind(kind) {
            return;
        }
        if !self.kind.same_family(kind) {
            self.password.clear();
        }
        match (self.kind, kind) {
            (ConnectionKind::Sql(previous), ConnectionKind::Sql(engine)) => {
                let default_user = |engine| match engine {
                    Engine::MySql => "root",
                    Engine::Postgres => "postgres",
                    Engine::SqlServer => "sa",
                };
                if self.profile.port == previous.port() {
                    self.profile.port = engine.port();
                }
                if self.profile.username == default_user(previous) {
                    self.profile.username = default_user(engine).into();
                }
                if self.profile.name == format!("本地 {}", previous.label()) {
                    self.profile.name = format!("本地 {}", engine.label());
                }
                self.profile.engine = engine;
            }
            (ConnectionKind::Sql(engine), ConnectionKind::Redis) => {
                self.redis_profile.name = if self.profile.name == format!("本地 {}", engine.label())
                {
                    "本地 Redis".into()
                } else {
                    self.profile.name.clone()
                };
                self.redis_profile.host = self.profile.host.clone();
                self.redis_profile.ssh = self.profile.ssh.clone();
                self.redis_profile.remember_password = self.profile.remember_password;
            }
            (ConnectionKind::Redis, ConnectionKind::Sql(engine)) => {
                self.profile.name = if self.redis_profile.name == "本地 Redis" {
                    format!("本地 {}", engine.label())
                } else {
                    self.redis_profile.name.clone()
                };
                self.profile.host = self.redis_profile.host.clone();
                self.profile.ssh = self.redis_profile.ssh.clone();
                self.profile.remember_password = self.redis_profile.remember_password;
                let previous = self.profile.engine;
                self.kind = ConnectionKind::Sql(previous);
                self.set_kind(ConnectionKind::Sql(engine));
            }
            _ => {}
        }
        self.kind = kind;
    }
}

#[derive(Clone)]
pub(super) enum TreeConnection {
    Sql(Profile),
    Redis(RedisProfile),
}

impl TreeConnection {
    pub(super) fn id(&self) -> Uuid {
        match self {
            Self::Sql(p) => p.id,
            Self::Redis(p) => p.id,
        }
    }
}

#[derive(Default)]
pub(super) struct RedisTreeEvents {
    pub connect: Option<Uuid>,
    pub disconnect: Option<Uuid>,
    pub edit: Option<Uuid>,
    pub delete: Option<Uuid>,
    pub select: Option<(Uuid, String)>,
    pub move_group: Option<(Uuid, Option<Uuid>)>,
}

/// The catalog deliberately keeps IDs for both families (also when Redis loading failed).
impl App {
    pub(super) fn restore_redis_connections(&mut self) {
        let Some(store) = &self.store else { return };
        match store.load_redis() {
            Ok(profiles) => {
                self.redis_profiles = profiles;
                self.redis_config_error = None;
                if self.selected.is_none() {
                    self.selected = self.redis_profiles.first().map(|p| p.id);
                }
            }
            Err(error) => {
                let message =
                    format!("Redis 配置读取失败，已停用 Redis 保存以保护原文件：{error:#}");
                self.redis_config_error = Some(message.clone());
                self.fail(message);
            }
        }
    }

    pub(super) fn is_redis_connection(&self, id: Uuid) -> bool {
        self.redis_profiles.iter().any(|p| p.id == id)
    }

    pub(super) fn redis_document(&self, id: Uuid) -> Option<&redis_ui::RedisDocument> {
        self.tabs
            .iter()
            .filter_map(|tab| tab.redis.as_ref())
            .find(|doc| doc.profile_id() == id)
    }

    fn redis_document_mut(&mut self, id: Uuid) -> Option<&mut redis_ui::RedisDocument> {
        self.tabs
            .iter_mut()
            .filter_map(|tab| tab.redis.as_mut())
            .find(|doc| doc.profile_id() == id)
    }

    pub(super) fn redis_ready(&self, id: Uuid) -> bool {
        self.redis_document(id).is_some_and(|doc| {
            doc.is_connected() && !doc.busy && !doc.is_dirty() && !doc.has_pending_confirmation()
        })
    }

    pub(super) fn selected_connected_name(&self) -> Option<&str> {
        let id = self.selected?;
        if let Some(doc) = self.redis_document(id).filter(|doc| doc.is_connected()) {
            return Some(&doc.bound_profile().name);
        }
        self.selected_session()
            .and_then(|session| session.connected.as_ref())
            .map(|profile| profile.name.as_str())
    }

    pub(super) fn connection_activity_counts(&self) -> (usize, usize) {
        let redis = self.tabs.iter().filter_map(|tab| tab.redis.as_ref());
        let connected = self
            .sessions
            .values()
            .filter(|s| s.connected.is_some())
            .count()
            + redis.clone().filter(|doc| doc.is_connected()).count();
        let busy = self.sessions.values().filter(|s| s.busy).count()
            + self.query_sessions.values().filter(|s| s.busy).count()
            + redis.filter(|doc| doc.busy).count()
            + usize::from(self.redis_test_worker.is_some());
        (connected, busy)
    }

    fn redis_guard(&self, id: Uuid, disconnected: bool) -> anyhow::Result<()> {
        if let Some(doc) = self.redis_document(id) {
            check_redis_operation(doc.busy, doc.is_dirty(), doc.is_connected(), disconnected)?;
            anyhow::ensure!(
                !doc.has_pending_confirmation(),
                "Redis 有待确认操作，请先确认或取消"
            );
        }
        Ok(())
    }

    pub(super) fn redis_credentials_for(
        &self,
        profile: &RedisProfile,
    ) -> anyhow::Result<Option<Credentials>> {
        if let Some(credentials) = self.credentials.get(&profile.id) {
            return Ok(Some(credentials.clone()));
        }
        self.store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("配置目录不可用"))?
            .redis_credentials(profile)
    }

    pub(super) fn edit_redis_connection(&mut self, id: Uuid) {
        let result = (|| -> anyhow::Result<ConnectionForm> {
            self.redis_guard(id, true)?;
            let profile = self
                .redis_profiles
                .iter()
                .find(|p| p.id == id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Redis 连接不存在"))?;
            let credentials = self.redis_credentials_for(&profile)?.unwrap_or_default();
            Ok(ConnectionForm::redis(profile, credentials, true))
        })();
        match result {
            Ok(form) => self.form = Some(form),
            Err(error) => self.fail(format!("{error:#}")),
        }
    }

    pub(super) fn save_redis_connection(
        &mut self,
        mut profile: RedisProfile,
        credentials: Credentials,
    ) -> anyhow::Result<()> {
        // The legacy field is retained on disk for compatibility, not as a navigation target.
        profile.database = 0;
        profile.validate()?;
        anyhow::ensure!(
            self.redis_config_error.is_none(),
            "Redis 配置读取失败，保存已禁用；请修复原文件后重启"
        );
        anyhow::ensure!(
            !self.profiles.iter().any(|p| p.id == profile.id),
            "不能将已保存的 SQL 连接改成 Redis，请新建连接"
        );
        self.redis_guard(profile.id, true)?;
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("配置目录不可用"))?;
        let mut profiles = self.redis_profiles.clone();
        if let Some(existing) = profiles.iter_mut().find(|p| p.id == profile.id) {
            *existing = profile.clone();
        } else {
            profiles.push(profile.clone());
        }
        // Reading encrypted secrets must succeed before either configuration can be overwritten.
        store.save_redis_credentials(&profile, &credentials)?;
        store.save_redis(&profiles)?;
        let context = self.context.clone();
        if let Some(tab) = self.tabs.iter_mut().find(|tab| {
            tab.redis
                .as_ref()
                .is_some_and(|doc| doc.profile_id() == profile.id)
        }) {
            tab.redis = Some(redis_ui::RedisDocument::new_bound(
                &context,
                profile.clone(),
                credentials.clone(),
            ));
            tab.name = format!("Redis · {}", profile.name);
        }
        self.credentials.insert(profile.id, credentials);
        self.redis_profiles = profiles;
        self.selected = Some(profile.id);
        self.tree_selection.selected = Some(TreeNode::Connection(profile.id));
        self.status = "Redis 连接已保存到主连接树".into();
        self.error = false;
        Ok(())
    }

    pub(super) fn open_redis_connection(&mut self, id: Uuid, connect: bool) -> bool {
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.redis.as_ref().is_some_and(|doc| doc.profile_id() == id))
        {
            self.active_tab = index;
            self.browser_visible = false;
        } else {
            let Some(profile) = self.redis_profiles.iter().find(|p| p.id == id).cloned() else {
                return false;
            };
            let credentials = match self.redis_credentials_for(&profile) {
                Ok(credentials) => credentials.unwrap_or_default(),
                Err(error) => {
                    self.fail(format!("{error:#}"));
                    return false;
                }
            };
            if !self.credentials.contains_key(&id)
                && (!profile.remember_password
                    || (profile.ssh.enabled && !profile.ssh.remember_secret))
            {
                self.form = Some(ConnectionForm::redis(profile, credentials, true));
                self.status = "请填写本次 Redis / SSH 凭据；无认证 Redis 的密码可以留空".into();
                self.error = false;
                return false;
            }
            if self
                .management_tab(format!("Redis · {}", profile.name), None)
                .is_none()
            {
                return false;
            }
            self.tabs[self.active_tab].redis = Some(redis_ui::RedisDocument::new_bound(
                &self.context,
                profile,
                credentials,
            ));
        }
        self.selected = Some(id);
        if !connect {
            return true;
        }
        if let Err(error) = self.redis_guard(id, false) {
            self.fail(format!("{error:#}"));
            return false;
        }
        let sent = self.redis_document_mut(id).is_some_and(|doc| doc.connect());
        if !sent {
            self.fail("Redis 连接请求未发送，请检查草稿和连接状态".into());
        }
        sent
    }

    pub(super) fn disconnect_connection(&mut self, id: Uuid) {
        if !self.is_redis_connection(id) {
            self.request_on(id, Request::Disconnect, "正在断开连接…");
            return;
        }
        if let Err(error) = self.redis_guard(id, false) {
            self.fail(format!("{error:#}"));
            return;
        }
        if let Some(doc) = self.redis_document_mut(id)
            && !doc.disconnect()
        {
            self.fail("Redis 断开请求未发送".into());
        }
    }

    pub(super) fn select_redis_database(&mut self, id: Uuid, database: u32) -> bool {
        if !self.open_redis_connection(id, false) {
            return false;
        }
        if let Err(error) = self.redis_guard(id, false) {
            self.fail(format!("{error:#}"));
            return false;
        }
        let Some(doc) = self.redis_document_mut(id) else {
            return false;
        };
        // New connections always start in DB0; legacy profile.database never selects a DB.
        let accepted = if doc.is_connected() {
            doc.select_database(database)
        } else if database == 0 {
            doc.connect()
        } else {
            false
        };
        if !accepted {
            self.fail("请先连接 Redis，再选择数据库；有草稿或忙时不能切库".into());
        }
        accepted
    }

    pub(super) fn refresh_redis_databases(&mut self, id: Uuid) -> bool {
        if let Err(error) = self.redis_guard(id, false) {
            self.fail(format!("{error:#}"));
            return false;
        }
        let accepted = self
            .redis_document_mut(id)
            .is_some_and(|doc| doc.refresh_databases());
        if accepted {
            self.status = "正在刷新 Redis 数据库列表…".into();
            self.error = false;
        } else {
            self.fail("请先连接 Redis，再刷新数据库列表".into());
        }
        accepted
    }

    pub(super) fn test_redis_connection(
        &mut self,
        profile: RedisProfile,
        credentials: Credentials,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.test_busy && self.redis_test_worker.is_none(),
            "连接测试正在进行"
        );
        profile.validate()?;
        let wake = self.context.clone();
        let worker = RedisWorker::start(move || wake.request_repaint());
        worker.send(
            1,
            RedisRequest::Connect {
                profile,
                credentials,
            },
        )?;
        self.redis_test_worker = Some(worker);
        self.status = "正在测试 Redis 连接（独立临时连接）…".into();
        self.error = false;
        Ok(())
    }

    pub(super) fn poll_redis_connection_test(&mut self) {
        let response = self
            .redis_test_worker
            .as_ref()
            .and_then(|worker| worker.receiver.try_recv().ok());
        if let Some(response) = response {
            // Dropping both channels closes the temporary worker after this one response.
            self.redis_test_worker = None;
            match response.result {
                Ok(RedisData::Connected { .. }) => {
                    self.status = "Redis 连接测试成功（未打开标签、未扫描、未保存）".into();
                    self.error = false;
                }
                Ok(_) => self.fail("Redis 测试返回了意外结果".into()),
                Err(error) => self.fail(error),
            }
        }
    }

    pub(super) fn delete_redis_connection(&mut self, id: Uuid) -> anyhow::Result<()> {
        self.redis_guard(id, false)?;
        anyhow::ensure!(
            self.redis_config_error.is_none(),
            "Redis 配置读取失败，删除已禁用"
        );
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("配置目录不可用"))?;
        let profiles: Vec<_> = self
            .redis_profiles
            .iter()
            .filter(|p| p.id != id)
            .cloned()
            .collect();
        store.forget_credentials(id)?;
        store.save_redis(&profiles)?;
        let active_id = self.tabs[self.active_tab].id;
        self.tabs
            .retain(|tab| tab.redis.as_ref().is_none_or(|doc| doc.profile_id() != id));
        if self.tabs.is_empty() {
            self.tabs.push(QueryTab::new(self.next_tab));
            self.next_tab += 1;
        }
        self.active_tab = self
            .tabs
            .iter()
            .position(|tab| tab.id == active_id)
            .unwrap_or_else(|| self.active_tab.min(self.tabs.len() - 1));
        self.redis_profiles = profiles;
        self.credentials.remove(&id);
        self.selected = self
            .profiles
            .first()
            .map(|p| p.id)
            .or_else(|| self.redis_profiles.first().map(|p| p.id));
        self.tree_selection.selected = self.selected.map(TreeNode::Connection);
        self.confirm_delete = None;
        self.status = "Redis 连接已删除；其他连接和 SQL 会话未改变".into();
        self.error = false;
        Ok(())
    }

    pub(super) fn connection_tree_members(
        &self,
        group: Option<Uuid>,
        filter: &str,
        group_matches: bool,
        connected_only: bool,
    ) -> Vec<TreeConnection> {
        let sql = self
            .profiles
            .iter()
            .filter(|p| self.catalog.memberships.get(&p.id).copied() == group)
            .filter(|profile| {
                let session = self.sessions.get(&profile.id);
                (!connected_only || session.is_some_and(|s| s.connected.is_some()))
                    && (filter.is_empty()
                        || group_matches
                        || profile.name.to_lowercase().contains(filter)
                        || session.is_some_and(|s| {
                            s.databases
                                .iter()
                                .any(|db| db.to_lowercase().contains(filter))
                                || s.tables.iter().any(|table| {
                                    format!("{}.{}", table.schema, table.name)
                                        .to_lowercase()
                                        .contains(filter)
                                })
                        }))
            })
            .cloned()
            .map(TreeConnection::Sql);
        let redis = self
            .redis_profiles
            .iter()
            .filter(|p| self.catalog.memberships.get(&p.id).copied() == group)
            .filter(|profile| {
                let document = self.redis_document(profile.id);
                (!connected_only || document.is_some_and(|d| d.is_connected()))
                    && (filter.is_empty()
                        || group_matches
                        || format!("Redis {} {}:{}", profile.name, profile.host, profile.port)
                            .to_lowercase()
                            .contains(filter)
                        || document.is_some_and(|d| {
                            d.is_connected()
                                && d.databases()
                                    .iter()
                                    .any(|db| format!("db{}", db.index).contains(filter))
                        }))
            })
            .cloned()
            .map(TreeConnection::Redis);
        sql.chain(redis).collect()
    }

    pub(super) fn redis_connection_tree(
        &mut self,
        ui: &mut egui::Ui,
        profile: &RedisProfile,
        force_open: bool,
        buckets: &[(Option<Uuid>, String)],
    ) -> RedisTreeEvents {
        let mut events = RedisTreeEvents::default();
        let mut refresh_databases = false;
        let document = self.redis_document(profile.id);
        let connected = document.is_some_and(|doc| doc.is_connected());
        let busy = document.is_some_and(|doc| doc.busy);
        let dirty = document.is_some_and(|doc| doc.is_dirty() || doc.has_pending_confirmation());
        let current = document
            .filter(|_| connected)
            .map(|doc| doc.database_index());
        let databases = document
            .filter(|_| connected)
            .map(|doc| doc.databases().to_vec())
            .unwrap_or_default();
        let complete = document.is_some_and(|doc| doc.databases_complete());
        let notice = document
            .and_then(|doc| doc.database_notice())
            .map(str::to_owned);
        let mut title = egui::text::LayoutJob::default();
        title.append(
            &profile.name,
            28.0,
            egui::TextFormat::simple(
                egui::TextStyle::Button.resolve(ui.style()),
                ui.visuals().text_color(),
            ),
        );
        let state_id =
            ui.make_persistent_id(("redis-server", profile.id, self.view.tree_generation));
        let header = tree_navigation_ui::branch(
            ui,
            &mut self.tree_selection,
            TreeNode::Connection(profile.id),
            state_id,
            title.into(),
            false,
            force_open.then_some(true),
            false,
        );
        let response = header.response.clone();
        if header.activated {
            events.connect = Some(profile.id);
        }
        header.body(|ui| {
            if !connected {
                return;
            }
            if databases.is_empty() {
                ui.label(
                    RichText::new(if busy {
                        "正在读取数据库列表…"
                    } else {
                        "尚未读取到数据库，请右键刷新数据库列表。"
                    })
                    .small()
                    .weak(),
                );
            }
            for database in &databases {
                let index = database.index;
                let active = current == Some(index);
                let count = database
                    .keys
                    .map(|keys| format!(" · {keys} 个键"))
                    .unwrap_or_default();
                let leaf = tree_navigation_ui::leaf(
                    ui,
                    &mut self.tree_selection,
                    TreeNode::Database(profile.id, index.to_string()),
                    format!("db{index}{}{count}", if active { " · 当前" } else { "" }).into(),
                );
                if leaf.activated {
                    events.select = Some((profile.id, index.to_string()));
                }
                leaf.response.context_menu(|ui| {
                    if ui
                        .add_enabled(!busy && !dirty, egui::Button::new("打开 / 选择数据库"))
                        .clicked()
                    {
                        events.select = Some((profile.id, index.to_string()));
                        ui.close();
                    }
                });
            }
            if !complete && (notice.is_some() || !busy) {
                let detail = notice
                    .as_deref()
                    .unwrap_or("仅显示服务器已确认的数据库，请检查账号权限后刷新。");
                ui.label(
                    RichText::new(format!("数据库列表不完整，无法列出全部空 DB：{detail}"))
                        .small()
                        .color(ui.visuals().warn_fg_color),
                );
            } else if let Some(notice) = &notice {
                ui.label(RichText::new(notice).small().weak());
            }
        });
        icons::paint_redis_connection(
            ui,
            egui::Rect::from_center_size(
                egui::pos2(response.rect.left() + 10.0, response.rect.center().y),
                egui::vec2(20.0, 20.0),
            ),
            connected,
        );
        response.clone().on_hover_text(format!(
            "Redis · {}:{}{}{} · {}",
            profile.host,
            profile.port,
            if profile.tls { " · TLS" } else { "" },
            if profile.ssh.enabled { " · SSH" } else { "" },
            if busy {
                "正在操作…"
            } else if connected {
                "已连接"
            } else {
                "尚未连接"
            }
        ));
        response.context_menu(|ui| {
            if ui
                .add_enabled(!busy && !dirty, egui::Button::new("打开 / 连接 Redis"))
                .clicked()
            {
                events.connect = Some(profile.id);
                ui.close();
            }
            if ui
                .add_enabled(
                    connected && !busy && !dirty,
                    egui::Button::new("刷新数据库列表"),
                )
                .clicked()
            {
                refresh_databases = true;
                ui.close();
            }
            if ui
                .add_enabled(connected && !busy && !dirty, egui::Button::new("断开连接"))
                .clicked()
            {
                events.disconnect = Some(profile.id);
                ui.close();
            }
            ui.add_enabled_ui(self.catalog_store.is_some(), |ui| {
                ui.menu_button("移动到分组", |ui| {
                    let current_group = self.catalog.memberships.get(&profile.id).copied();
                    for (id, name) in buckets {
                        if ui.selectable_label(current_group == *id, name).clicked() {
                            events.move_group = Some((profile.id, *id));
                            ui.close();
                        }
                    }
                });
            });
            if ui
                .add_enabled(
                    !connected && !busy && !dirty,
                    egui::Button::new("编辑连接…"),
                )
                .clicked()
            {
                events.edit = Some(profile.id);
                ui.close();
            }
            if ui
                .add_enabled(!busy && !dirty, egui::Button::new("删除连接…"))
                .clicked()
            {
                events.delete = Some(profile.id);
                ui.close();
            }
        });
        // Handle this Redis-only action here without extending the shared SQL tree events.
        if refresh_databases {
            self.refresh_redis_databases(profile.id);
        }
        events
    }
}

fn check_redis_operation(
    busy: bool,
    dirty: bool,
    connected: bool,
    disconnected: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(!busy, "Redis 正在操作，请等待完成");
    anyhow::ensure!(!dirty, "有未保存的 Redis 草稿，请先保存或显式撤销");
    anyhow::ensure!(!disconnected || !connected, "请先断开 Redis 连接再编辑配置");
    Ok(())
}

pub(super) fn redis_database_fields(ui: &mut egui::Ui, form: &mut ConnectionForm) {
    let profile = &mut form.redis_profile;
    ui.set_min_width(ui.available_width());
    ui.strong("Redis");
    let width = (ui.available_width() - 90.0).clamp(140.0, 330.0);
    egui::Grid::new("redis-connection-fields")
        .num_columns(2)
        .spacing([14.0, 12.0])
        .show(ui, |ui| {
            ui.label("主机");
            ui.add(egui::TextEdit::singleline(&mut profile.host).desired_width(width));
            ui.end_row();
            ui.label("端口");
            ui.add(egui::DragValue::new(&mut profile.port).range(1..=65535));
            ui.end_row();
            ui.label("用户名");
            ui.add(
                egui::TextEdit::singleline(&mut profile.username)
                    .hint_text("可留空；ACL 用户名")
                    .desired_width(width),
            );
            ui.end_row();
            ui.label("密码");
            ui.add(
                egui::TextEdit::singleline(&mut form.password)
                    .password(true)
                    .hint_text("无认证 Redis 可留空")
                    .desired_width(width),
            );
            ui.end_row();
        });
    ui.checkbox(&mut profile.remember_password, "记住密码");
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlshop::redis_database::RedisDatabase;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        time::{Duration, Instant},
    };

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("sqlshop-redis-ui-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn store(&self) -> Store {
            Store {
                directory: self.0.clone(),
            }
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn app_with_redis() -> (App, RedisProfile) {
        let mut app = super::super::tests::session_test_app();
        let profile = RedisProfile::default();
        app.redis_profiles.push(profile.clone());
        app.credentials.insert(profile.id, Credentials::default());
        (app, profile)
    }

    fn labels(output: &egui::FullOutput) -> Vec<String> {
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.job.text.clone()),
                _ => None,
            })
            .collect()
    }

    fn render_tree(app: &mut App, events: Vec<egui::Event>, time: f64) -> egui::FullOutput {
        let context = app.context.clone();
        context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 1200.0),
                )),
                events,
                time: Some(time),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.sidebar(ui));
            },
        )
    }

    #[test]
    fn unified_new_connection_has_four_types_and_anonymous_redis_defaults() {
        let mut app = super::super::tests::session_test_app();
        app.new_connection();
        let form = app.form.as_mut().unwrap();
        assert_eq!(form.kind, ConnectionKind::Sql(Engine::MySql));
        assert_eq!(ConnectionKind::ALL.len(), 4);
        assert!(ConnectionKind::ALL.contains(&ConnectionKind::Redis));
        form.password = "sql-only-secret".into();
        form.set_kind(ConnectionKind::Redis);
        assert_eq!(form.redis_profile.port, 6379);
        assert_eq!(form.redis_profile.database, 0);
        assert!(form.redis_profile.username.is_empty() && form.password.is_empty());
        assert!(!form.redis_profile.tls);
        assert!(form.redis_profile.validate().is_ok());
        let context = app.context.clone();
        let output = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                connection_heading(ui, form);
                database_fields(ui, form);
            });
        });
        let rendered = labels(&output);
        let text = rendered.join("\n");
        assert!(text.contains("数据库类型") && text.contains("Redis"));
        assert!(
            !rendered
                .iter()
                .any(|label| matches!(label.as_str(), "数据库" | "默认数据库" | "DB 索引"))
        );
        assert!(rendered.iter().any(|label| label == "记住密码"));
        assert!(!text.contains("连接时不选 DB"));
    }

    #[test]
    fn legacy_redis_form_has_no_database_field() {
        let context = egui::Context::default();
        let mut form = ConnectionForm::redis(
            RedisProfile {
                database: 27,
                ..Default::default()
            },
            Credentials::default(),
            true,
        );
        let output = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                redis_database_fields(ui, &mut form);
            });
        });
        let rendered = labels(&output);
        assert!(
            !rendered
                .iter()
                .any(|label| matches!(label.as_str(), "数据库" | "默认数据库" | "DB 索引" | "27"))
        );
        assert_eq!(
            form.redis_profile.database, 27,
            "rendering the form does not mutate legacy configuration"
        );
    }

    #[test]
    fn saved_profiles_cannot_cross_sql_redis_type_boundary() {
        let mut sql = ConnectionForm::sql(Profile::default(), Credentials::default(), true);
        sql.set_kind(ConnectionKind::Redis);
        assert_eq!(sql.kind, ConnectionKind::Sql(Engine::MySql));
        sql.set_kind(ConnectionKind::Sql(Engine::Postgres));
        assert_eq!(sql.profile.port, 5432);
        assert_eq!(sql.profile.username, "postgres");
        let mut redis =
            ConnectionForm::redis(RedisProfile::default(), Credentials::default(), true);
        redis.set_kind(ConnectionKind::Sql(Engine::MySql));
        assert_eq!(redis.kind, ConnectionKind::Redis);
    }

    #[test]
    fn redis_save_load_delete_preserve_sql_files_and_credentials() {
        let directory = TestDirectory::new();
        let mut app = super::super::tests::session_test_app();
        app.store = Some(directory.store());
        let sql = Profile::default();
        app.save_connection(
            sql.clone(),
            Credentials {
                database_password: "sql-secret".into(),
                ssh_secret: String::new(),
            },
        )
        .unwrap();
        let original = std::fs::read(directory.0.join("connections.json")).unwrap();
        assert!(app.expand_connection.is_none());
        let profile = RedisProfile::default();
        app.save_redis_connection(profile.clone(), Credentials::default())
            .unwrap();
        assert!(
            app.expand_connection.is_none(),
            "saving must not expand a disconnected connection"
        );
        assert_eq!(
            app.tree_selection.selected,
            Some(TreeNode::Connection(profile.id))
        );
        assert_eq!(
            app.store
                .as_ref()
                .unwrap()
                .redis_credentials(&profile)
                .unwrap()
                .unwrap()
                .database_password,
            ""
        );
        let mut loaded = super::super::tests::session_test_app();
        loaded.store = Some(directory.store());
        loaded.restore_redis_connections();
        assert_eq!(loaded.selected, Some(profile.id));
        assert!(!loaded.redis_profiles[0].tls);
        assert!(loaded.open_redis_connection(profile.id, false));
        assert!(!loaded.redis_document(profile.id).unwrap().is_connected());
        assert!(app.open_redis_connection(profile.id, false));
        app.delete_redis_connection(profile.id).unwrap();
        assert!(app.redis_document(profile.id).is_none());
        assert!(app.store.as_ref().unwrap().load_redis().unwrap().is_empty());
        assert_eq!(app.selected, Some(sql.id));
        assert_eq!(
            std::fs::read(directory.0.join("connections.json")).unwrap(),
            original
        );
        assert_eq!(
            app.store
                .as_ref()
                .unwrap()
                .credentials(&sql)
                .unwrap()
                .unwrap()
                .database_password,
            "sql-secret"
        );
    }

    #[test]
    fn broken_redis_config_only_disables_redis_and_preserves_catalog_membership() {
        let directory = TestDirectory::new();
        let mut app = super::super::tests::session_test_app();
        app.store = Some(directory.store());
        let redis = RedisProfile::default();
        let mut catalog = Catalog::default();
        let group = catalog.save_group(None, "混合连接").unwrap();
        catalog.move_connection(redis.id, Some(group)).unwrap();
        CatalogStore::new(directory.0.clone())
            .save(&catalog)
            .unwrap();
        std::fs::write(directory.0.join("redis-connections.json"), b"{broken").unwrap();
        app.restore_redis_connections();
        app.restore_catalog();
        assert!(app.redis_config_error.is_some());
        assert_eq!(app.catalog.memberships.get(&redis.id), Some(&group));
        app.save_connection(Profile::default(), Credentials::default())
            .unwrap();
        assert!(
            app.save_redis_connection(redis, Credentials::default())
                .is_err()
        );
        assert_eq!(
            std::fs::read(directory.0.join("redis-connections.json")).unwrap(),
            b"{broken"
        );
    }

    #[test]
    fn mixed_tree_filters_groups_search_and_connected_only() {
        let (mut app, redis) = app_with_redis();
        let sql = Profile::default();
        app.profiles.push(sql.clone());
        let group = app.catalog.save_group(None, "生产").unwrap();
        app.catalog.move_connection(redis.id, Some(group)).unwrap();
        app.catalog.move_connection(sql.id, Some(group)).unwrap();
        assert_eq!(
            app.connection_tree_members(Some(group), "", false, false)
                .len(),
            2
        );
        assert_eq!(
            app.connection_tree_members(Some(group), "redis", false, false)
                .len(),
            1
        );
        assert_eq!(
            app.connection_tree_members(Some(group), "生产", true, false)
                .len(),
            2
        );
        assert!(
            app.connection_tree_members(Some(group), "", false, true)
                .is_empty()
        );
        assert!(app.open_redis_connection(redis.id, false));
        app.redis_document_mut(redis.id)
            .unwrap()
            .set_test_state(true, false);
        let connected = app.connection_tree_members(Some(group), "", false, true);
        assert_eq!(connected.len(), 1);
        assert_eq!(connected[0].id(), redis.id);
    }

    #[test]
    fn disconnected_redis_tree_has_no_invented_databases_or_sql_navigation() {
        let (mut app, mut profile) = app_with_redis();
        profile.database = 27;
        app.redis_profiles[0] = profile.clone();
        app.browser_visible = false;
        app.expand_connection = Some(profile.id);
        app.context.style_mut(|style| style.animation_time = 0.0);
        let mut output = render_tree(&mut app, vec![], 0.0);
        for time in [0.1, 0.2] {
            output = render_tree(&mut app, vec![], time);
        }
        let rendered = labels(&output);
        let text = rendered.join("\n");
        assert!(rendered.contains(&profile.name), "{text}");
        assert!(
            !text.contains("本地 Redis · Redis"),
            "Redis uses its icon, not a suffix"
        );
        for redundant in [
            "尚未连接",
            "打开并连接",
            "连接后读取全部真实数据库",
            "localhost:6379",
        ] {
            assert!(!text.contains(redundant), "{text}");
        }
        assert!(
            !rendered.iter().any(|label| label.starts_with("db")),
            "{text}"
        );
        assert!(!text.contains("还没有数据库连接"));
        let position = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == profile.name => {
                    Some(text.pos + egui::vec2(35.0, 6.0))
                }
                _ => None,
            })
            .unwrap();
        let pointer = |pressed| egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        render_tree(
            &mut app,
            vec![egui::Event::PointerMoved(position), pointer(true)],
            0.3,
        );
        render_tree(&mut app, vec![pointer(false)], 0.4);
        assert_eq!(app.selected, Some(profile.id));
        assert!(app.browser_scope.is_none() && !app.browser_visible);
        assert!(app.redis_document(profile.id).is_none());
        assert!(app.sessions.is_empty() && app.query_sessions.is_empty());
        assert!(
            app.connection_tree_members(None, "db27", false, false)
                .is_empty()
        );
        assert!(
            app.connection_tree_members(None, "db0", false, false)
                .is_empty()
        );
    }

    #[test]
    fn sql_connection_tree_omits_inline_status_buttons_and_endpoint_details() {
        for engine in Engine::ALL {
            let mut app = super::super::tests::session_test_app();
            let profile = Profile {
                engine,
                name: format!("精简 {} 连接", engine.label()),
                host: "tree-only.example.test".into(),
                port: engine.port(),
                ..Default::default()
            };
            app.profiles.push(profile.clone());
            app.selected = Some(profile.id);
            app.expand_connection = Some(profile.id);
            app.context.style_mut(|style| style.animation_time = 0.0);
            render_tree(&mut app, vec![], 0.0);
            let output = render_tree(&mut app, vec![], 0.1);
            let rendered = labels(&output);
            assert!(rendered.contains(&profile.name));
            for redundant in ["尚未连接", "正在连接…", "连接"] {
                assert!(!rendered.iter().any(|label| label == redundant));
            }
            assert!(!rendered.iter().any(|label| label.contains(&profile.host)));
            assert!(
                app.sessions.is_empty(),
                "rendering an expanded node must not connect"
            );

            let mut session = ConnectionSession::new(&app.context);
            session.connected = Some(profile.clone());
            session.databases = vec!["example_database".into()];
            app.sessions.insert(profile.id, session);
            let output = render_tree(&mut app, vec![], 0.2);
            let rendered = labels(&output);
            assert!(rendered.iter().any(|label| label == "example_database"));
            assert!(!rendered.iter().any(|label| label.contains(&profile.host)));
        }
    }

    #[test]
    fn legacy_database_is_not_a_navigation_or_initial_database_target() {
        let (mut app, mut profile) = app_with_redis();
        profile.database = 27;
        app.redis_profiles[0] = profile.clone();
        assert!(app.open_redis_connection(profile.id, false));
        let doc = app.redis_document(profile.id).unwrap();
        assert_eq!(doc.database_index(), 0);
        assert!(doc.databases().is_empty());
        assert!(!app.select_redis_database(profile.id, 27));
        assert!(
            !app.redis_document(profile.id).unwrap().busy,
            "legacy DB must not trigger CONNECT or SELECT"
        );
        assert!(
            app.connection_tree_members(None, "db27", false, false)
                .is_empty()
        );
        let directory = TestDirectory::new();
        app.store = Some(directory.store());
        app.save_redis_connection(profile, Credentials::default())
            .unwrap();
        assert_eq!(app.redis_profiles[0].database, 0);
        assert_eq!(
            app.store.as_ref().unwrap().load_redis().unwrap()[0].database,
            0
        );
    }

    #[test]
    fn connected_tree_renders_all_32_real_databases_including_empty_and_only_actual_current() {
        let (mut app, mut profile) = app_with_redis();
        profile.database = 27;
        app.redis_profiles[0] = profile.clone();
        assert!(app.open_redis_connection(profile.id, false));
        let doc = app.redis_document_mut(profile.id).unwrap();
        doc.set_test_state(true, false);
        doc.set_test_databases(
            (0..32)
                .map(|index| RedisDatabase {
                    index,
                    keys: Some(if index == 7 { 11 } else { 0 }),
                })
                .collect(),
            true,
            None,
        );
        app.expand_connection = Some(profile.id);
        app.context.style_mut(|style| style.animation_time = 0.0);
        let mut output = render_tree(&mut app, vec![], 0.0);
        for time in [0.1, 0.2] {
            output = render_tree(&mut app, vec![], time);
        }
        let rendered = labels(&output);
        let text = rendered.join("\n");
        for index in 0..32 {
            let expected = format!(
                "db{index}{} · {} 个键",
                if index == 0 { " · 当前" } else { "" },
                if index == 7 { 11 } else { 0 }
            );
            assert!(rendered.contains(&expected), "missing {expected}: {text}");
        }
        assert_eq!(
            rendered
                .iter()
                .filter(|label| label.starts_with("db") && label.contains(" · 当前"))
                .count(),
            1
        );
        assert!(!text.contains("db27 · 当前") && !text.contains("配置"));
        for redundant in ["已连接", "尚未连接", "打开并连接", "localhost:6379"] {
            assert!(!rendered.iter().any(|label| label == redundant), "{text}");
        }
        assert_eq!(
            app.connection_tree_members(None, "db31", false, false)
                .len(),
            1
        );
        assert_eq!(
            app.connection_tree_members(None, "db31", false, true).len(),
            1
        );
        assert!(
            app.connection_tree_members(None, "db32", false, false)
                .is_empty()
        );
    }

    #[test]
    fn partial_database_snapshot_explains_missing_empty_databases_without_filling_gaps() {
        let (mut app, mut profile) = app_with_redis();
        profile.database = 15;
        app.redis_profiles[0] = profile.clone();
        assert!(app.open_redis_connection(profile.id, false));
        let doc = app.redis_document_mut(profile.id).unwrap();
        doc.set_test_state(true, false);
        doc.set_test_databases(
            vec![
                RedisDatabase {
                    index: 0,
                    keys: None,
                },
                RedisDatabase {
                    index: 5,
                    keys: Some(3),
                },
            ],
            false,
            Some("CONFIG GET databases 返回 NOPERM；INFO keyspace 仅确认非空库及当前库。".into()),
        );
        app.expand_connection = Some(profile.id);
        app.context.style_mut(|style| style.animation_time = 0.0);
        let mut output = render_tree(&mut app, vec![], 0.0);
        for time in [0.1, 0.2] {
            output = render_tree(&mut app, vec![], time);
        }
        let rendered = labels(&output);
        let text = rendered.join("\n");
        let databases: Vec<_> = rendered
            .iter()
            .filter(|label| label.starts_with("db"))
            .map(String::as_str)
            .collect();
        assert_eq!(databases, ["db0 · 当前", "db5 · 3 个键"]);
        assert!(
            text.contains("数据库列表不完整")
                && text.contains("无法列出全部空 DB")
                && text.contains("NOPERM"),
            "{text}"
        );
        assert!(
            !text.contains("db0 · 当前 · 0 个键"),
            "an unknown count must not be shown as empty"
        );
        assert!(
            app.connection_tree_members(None, "db15", false, false)
                .is_empty()
        );
        assert_eq!(
            app.connection_tree_members(None, "db5", false, false).len(),
            1
        );
    }

    #[test]
    fn database_single_click_only_selects_and_double_click_requests_select_without_sql() {
        let (mut app, profile) = app_with_redis();
        assert!(app.open_redis_connection(profile.id, false));
        let doc = app.redis_document_mut(profile.id).unwrap();
        doc.set_test_state(true, false);
        doc.set_test_databases(
            vec![
                RedisDatabase {
                    index: 0,
                    keys: Some(0),
                },
                RedisDatabase {
                    index: 7,
                    keys: Some(0),
                },
            ],
            true,
            None,
        );
        app.expand_connection = Some(profile.id);
        app.context.style_mut(|style| style.animation_time = 0.0);
        let mut output = render_tree(&mut app, vec![], 0.0);
        for time in [0.1, 0.2] {
            output = render_tree(&mut app, vec![], time);
        }
        let position = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == "db7 · 0 个键" => {
                    Some(text.pos + egui::vec2(20.0, 6.0))
                }
                _ => None,
            })
            .unwrap();
        let pointer = |pressed| egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        render_tree(
            &mut app,
            vec![egui::Event::PointerMoved(position), pointer(true)],
            0.3,
        );
        render_tree(&mut app, vec![pointer(false)], 0.35);
        assert_eq!(
            app.tree_selection.selected,
            Some(TreeNode::Database(profile.id, "7".into()))
        );
        assert!(!app.redis_document(profile.id).unwrap().busy);
        assert_eq!(app.redis_document(profile.id).unwrap().database_index(), 0);
        assert!(app.browser_scope.is_none() && !app.browser_visible);
        render_tree(&mut app, vec![pointer(true)], 0.4);
        render_tree(&mut app, vec![pointer(false)], 0.45);
        let doc = app.redis_document(profile.id).unwrap();
        assert!(
            doc.busy,
            "double click must send SELECT to the mocked disconnected worker, not connect to a real server"
        );
        assert_eq!(
            doc.database_index(),
            0,
            "current DB changes only after a successful SELECT response"
        );
        assert!(app.browser_scope.is_none() && !app.browser_visible);
        assert!(app.sessions.is_empty() && app.query_sessions.is_empty());
    }

    #[test]
    fn refresh_database_list_uses_redis_document_and_respects_busy_dirty_guards() {
        let (mut app, profile) = app_with_redis();
        assert!(!app.refresh_redis_databases(profile.id));
        assert!(app.redis_document(profile.id).is_none());
        assert!(app.open_redis_connection(profile.id, false));
        assert!(!app.refresh_redis_databases(profile.id));
        let doc = app.redis_document_mut(profile.id).unwrap();
        doc.set_test_state(true, true);
        assert!(!app.refresh_redis_databases(profile.id));
        let doc = app.redis_document_mut(profile.id).unwrap();
        doc.set_test_state(true, false);
        doc.busy = true;
        assert!(!app.refresh_redis_databases(profile.id));
        app.redis_document_mut(profile.id).unwrap().busy = false;
        assert!(app.refresh_redis_databases(profile.id));
        assert!(app.redis_document(profile.id).unwrap().busy);
        assert!(app.sessions.is_empty() && app.query_sessions.is_empty());
    }

    #[test]
    fn redis_open_reuses_bound_tab_and_edit_updates_title() {
        let directory = TestDirectory::new();
        let (mut app, mut profile) = app_with_redis();
        app.store = Some(directory.store());
        assert!(app.open_redis_connection(profile.id, false));
        let tab = app.tabs[app.active_tab].id;
        let count = app.tabs.len();
        assert!(app.open_redis_connection(profile.id, false));
        assert_eq!(app.tabs.len(), count);
        assert_eq!(app.tabs[app.active_tab].id, tab);
        assert!(app.tabs[app.active_tab].target.is_none());
        profile.name = "改名后的缓存".into();
        app.save_redis_connection(profile.clone(), Credentials::default())
            .unwrap();
        assert_eq!(app.tabs[app.active_tab].name, "Redis · 改名后的缓存");
        assert_eq!(
            app.redis_document(profile.id).unwrap().bound_profile().name,
            profile.name
        );
        assert!(app.sessions.is_empty() && app.query_sessions.is_empty());
    }

    #[test]
    fn dirty_busy_and_live_profile_guards_preserve_documents_and_configuration() {
        let directory = TestDirectory::new();
        let (mut app, profile) = app_with_redis();
        app.store = Some(directory.store());
        app.save_redis_connection(profile.clone(), Credentials::default())
            .unwrap();
        app.open_redis_connection(profile.id, false);
        let original = std::fs::read(directory.0.join("redis-connections.json")).unwrap();
        for (busy, dirty, connected) in [
            (true, false, true),
            (false, true, true),
            (false, true, false),
        ] {
            let doc = app.redis_document_mut(profile.id).unwrap();
            doc.set_test_state(connected, dirty);
            doc.busy = busy;
            assert!(!app.open_redis_connection(profile.id, true));
            assert!(!app.select_redis_database(profile.id, 1));
            app.disconnect_connection(profile.id);
            assert!(app.delete_redis_connection(profile.id).is_err());
            assert!(
                app.save_redis_connection(profile.clone(), Credentials::default())
                    .is_err()
            );
            app.edit_connection(profile.id);
            assert!(app.form.is_none());
            assert!(app.redis_document(profile.id).is_some());
            assert_eq!(
                std::fs::read(directory.0.join("redis-connections.json")).unwrap(),
                original
            );
        }
        let doc = app.redis_document_mut(profile.id).unwrap();
        doc.busy = false;
        doc.set_test_state(true, false);
        assert!(
            app.save_redis_connection(profile.clone(), Credentials::default())
                .is_err()
        );
        app.redis_document_mut(profile.id)
            .unwrap()
            .set_test_state(false, false);
        app.edit_connection(profile.id);
        assert_eq!(app.form.as_ref().unwrap().kind, ConnectionKind::Redis);
    }

    #[test]
    fn nonremembered_credentials_prompt_but_anonymous_defaults_open() {
        let (mut app, mut profile) = app_with_redis();
        profile.remember_password = false;
        app.redis_profiles[0] = profile.clone();
        app.credentials.clear();
        let directory = TestDirectory::new();
        app.store = Some(directory.store());
        assert!(!app.open_redis_connection(profile.id, false));
        assert_eq!(app.form.as_ref().unwrap().kind, ConnectionKind::Redis);
        assert!(app.redis_document(profile.id).is_none());
        app.form = None;
        app.redis_profiles[0].remember_password = true;
        assert!(app.open_redis_connection(profile.id, false));
        assert!(app.form.is_none());
    }

    #[test]
    fn corrupt_secrets_never_replace_existing_redis_configuration() {
        let directory = TestDirectory::new();
        let (mut app, profile) = app_with_redis();
        app.store = Some(directory.store());
        app.save_redis_connection(profile.clone(), Credentials::default())
            .unwrap();
        let original = std::fs::read(directory.0.join("redis-connections.json")).unwrap();
        std::fs::write(directory.0.join("credentials.enc"), b"corrupt secrets").unwrap();
        app.credentials.clear();
        app.edit_connection(profile.id);
        assert!(app.form.is_none() && app.error);
        assert!(!app.open_redis_connection(profile.id, false));
        assert!(
            app.save_redis_connection(profile, Credentials::default())
                .is_err()
        );
        assert_eq!(
            std::fs::read(directory.0.join("redis-connections.json")).unwrap(),
            original
        );
        assert_eq!(
            std::fs::read(directory.0.join("credentials.enc")).unwrap(),
            b"corrupt secrets"
        );
    }

    #[test]
    fn redis_title_counts_do_not_enable_sql_sessions() {
        let (mut app, profile) = app_with_redis();
        app.open_redis_connection(profile.id, false);
        app.redis_document_mut(profile.id)
            .unwrap()
            .set_test_state(true, false);
        assert_eq!(app.selected_connected_name(), Some(profile.name.as_str()));
        assert_eq!(app.connection_activity_counts(), (1, 0));
        assert!(app.redis_ready(profile.id));
        assert!(app.selected_session().is_none());
        app.redis_document_mut(profile.id).unwrap().busy = true;
        assert_eq!(app.connection_activity_counts(), (1, 1));
        assert!(!app.redis_ready(profile.id));
    }

    #[test]
    fn temporary_test_worker_only_pings_then_drops_without_scan_tab_or_save() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let profile = RedisProfile {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            ..Default::default()
        };
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            for _ in 0..3 {
                reader.read_line(&mut request).unwrap();
            }
            assert_eq!(request, "*1\r\n$4\r\nPING\r\n");
            stream.write_all(b"+PONG\r\n").unwrap();
            let mut extra = [0u8; 1];
            assert_eq!(
                reader.read(&mut extra).unwrap(),
                0,
                "temporary test must close without SCAN"
            );
        });
        let (mut app, existing) = app_with_redis();
        app.open_redis_connection(existing.id, false);
        app.redis_document_mut(existing.id)
            .unwrap()
            .set_test_state(true, false);
        let tab_count = app.tabs.len();
        let directory = TestDirectory::new();
        app.store = Some(directory.store());
        app.test_redis_connection(profile, Credentials::default())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(6);
        while app.redis_test_worker.is_some() && Instant::now() < deadline {
            app.poll_redis_connection_test();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            app.redis_test_worker.is_none(),
            "test worker did not complete"
        );
        assert!(!app.error, "{}", app.status);
        assert_eq!(app.tabs.len(), tab_count);
        assert_eq!(app.redis_profiles.len(), 1);
        assert!(app.redis_document(existing.id).unwrap().is_connected());
        assert!(!directory.0.join("redis-connections.json").exists());
        server.join().unwrap();
    }
}
