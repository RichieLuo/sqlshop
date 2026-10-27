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
        let active_id = self.tabs.get(self.active_tab).map(|tab| tab.id);
        self.tabs
            .retain(|tab| tab.redis.as_ref().is_none_or(|doc| doc.profile_id() != id));
        self.browser_visible |= self.tabs.is_empty();
        if self.tabs.is_empty() {
            self.next_tab = 1;
        }
        self.active_tab = self
            .tabs
            .iter()
            .position(|tab| Some(tab.id) == active_id)
            .unwrap_or_else(|| self.active_tab.min(self.tabs.len().saturating_sub(1)));
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
                                || s.tables
                                    .iter()
                                    .chain(s.database_tree.tables.values().flatten())
                                    .any(|table| {
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
            ui.add(dialog_input(&mut profile.host).desired_width(width));
            ui.end_row();
            ui.label("端口");
            ui.add(egui::DragValue::new(&mut profile.port).range(1..=65535));
            ui.end_row();
            ui.label("用户名");
            ui.add(
                dialog_input(&mut profile.username)
                    .hint_text("可留空；ACL 用户名")
                    .desired_width(width),
            );
            ui.end_row();
            ui.label("密码");
            ui.add(
                dialog_input(&mut form.password)
                    .password(true)
                    .hint_text("无认证 Redis 可留空")
                    .desired_width(width),
            );
            ui.end_row();
        });
    ui.checkbox(&mut profile.remember_password, "记住密码");
}

#[cfg(test)]
mod tests;
