use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use sqlshop::{
    model::Credentials,
    redis_database::{
        RedisData, RedisDatabase, RedisKey, RedisKind, RedisMutation, RedisProfile, RedisRequest,
        RedisValue, RedisWorker, command_needs_confirmation, display_bytes, format_hex,
        parse_command, parse_hex,
    },
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use uuid::Uuid;

const KEY_LIMIT: usize = 20_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueFormat {
    Text,
    Json,
    Hex,
}
impl ValueFormat {
    fn label(self) -> &'static str {
        match self {
            Self::Text => "文本",
            Self::Json => "JSON",
            Self::Hex => "Hex",
        }
    }
    fn show(self, bytes: &[u8]) -> String {
        match self {
            Self::Hex => format_hex(bytes),
            _ => String::from_utf8_lossy(bytes).into_owned(),
        }
    }
    fn bytes(self, text: &str) -> anyhow::Result<Vec<u8>> {
        match self {
            Self::Hex => parse_hex(text),
            Self::Json => {
                serde_json::from_str::<serde_json::Value>(text)?;
                Ok(text.as_bytes().to_vec())
            }
            Self::Text => Ok(text.as_bytes().to_vec()),
        }
    }
}

struct MemberDraft {
    index: usize,
    field: Vec<u8>,
    value: Vec<u8>,
    text: String,
    original: String,
    hex: bool,
    score: f64,
    original_score: f64,
}
impl MemberDraft {
    fn new(index: usize, field: Vec<u8>, value: Vec<u8>, score: f64) -> Self {
        let hex = std::str::from_utf8(&value).is_err();
        let text = if hex {
            format_hex(&value)
        } else {
            String::from_utf8(value.clone()).unwrap_or_default()
        };
        Self {
            index,
            field,
            value,
            original: text.clone(),
            text,
            hex,
            score,
            original_score: score,
        }
    }
    fn dirty(&self) -> bool {
        self.text != self.original || self.score != self.original_score
    }
    fn bytes(&self) -> anyhow::Result<Vec<u8>> {
        if self.hex {
            parse_hex(&self.text)
        } else {
            Ok(self.text.as_bytes().to_vec())
        }
    }
}

struct NewKey {
    key: String,
    kind: RedisKind,
    value: String,
    field: String,
    score: f64,
    hex: bool,
}
impl Default for NewKey {
    fn default() -> Self {
        Self {
            key: String::new(),
            kind: RedisKind::String,
            value: String::new(),
            field: String::new(),
            score: 0.0,
            hex: false,
        }
    }
}

enum Confirmation {
    Mutation {
        key: Vec<u8>,
        change: RedisMutation,
        label: String,
    },
    Command(Vec<Vec<u8>>),
    Namespace {
        token: u64,
        prefix: Vec<u8>,
        count: usize,
        database: u32,
    },
}

#[derive(Default)]
struct KeyTree {
    children: BTreeMap<Vec<u8>, KeyTree>,
    key: Option<Vec<u8>>,
    count: usize,
}
impl KeyTree {
    fn insert(&mut self, key: &[u8]) {
        self.count += 1;
        let mut node = self;
        for part in key.splitn(25, |byte| *byte == b':') {
            node = node.children.entry(part.to_vec()).or_default();
            node.count += 1;
        }
        node.key = Some(key.to_vec());
    }
}

pub(super) struct RedisDocument {
    id: Uuid,
    worker: RedisWorker,
    pub busy: bool,
    connected: bool,
    generation: u64,
    profile: RedisProfile,
    credentials: Credentials,
    database: u32,
    database_draft: u32,
    databases: Vec<RedisDatabase>,
    databases_complete: bool,
    database_notice: Option<String>,
    catalog_pending: bool,
    scan_after_catalog: bool,
    server: String,
    pattern: String,
    scan_pattern: String,
    cursor: u64,
    scan_started: bool,
    keys: BTreeSet<Vec<u8>>,
    tree: KeyTree,
    tree_view: bool,
    selected: Option<RedisKey>,
    stale: bool,
    value_format: ValueFormat,
    value_text: String,
    value_original: String,
    member: Option<MemberDraft>,
    add_value: String,
    add_field: String,
    add_score: f64,
    add_hex: bool,
    ttl: i64,
    rename: String,
    new_key: Option<NewKey>,
    creating: bool,
    confirmation: Option<Confirmation>,
    console_open: bool,
    command: String,
    history: VecDeque<String>,
    history_index: Option<usize>,
    console: VecDeque<String>,
    status: String,
    error: Option<String>,
}

impl RedisDocument {
    pub(super) fn new_bound(
        context: &egui::Context,
        profile: RedisProfile,
        credentials: Credentials,
    ) -> Self {
        let wake = context.clone();
        let database = 0;
        Self {
            id: Uuid::new_v4(),
            worker: RedisWorker::start(move || wake.request_repaint()),
            busy: false,
            connected: false,
            generation: 0,
            profile,
            credentials,
            database,
            database_draft: database,
            databases: Vec::new(),
            databases_complete: false,
            database_notice: None,
            catalog_pending: false,
            scan_after_catalog: false,
            server: String::new(),
            pattern: "*".into(),
            scan_pattern: "*".into(),
            cursor: 0,
            scan_started: false,
            keys: BTreeSet::new(),
            tree: KeyTree::default(),
            tree_view: true,
            selected: None,
            stale: true,
            value_format: ValueFormat::Text,
            value_text: String::new(),
            value_original: String::new(),
            member: None,
            add_value: String::new(),
            add_field: String::new(),
            add_score: 0.0,
            add_hex: false,
            ttl: 3600,
            rename: String::new(),
            new_key: None,
            creating: false,
            confirmation: None,
            console_open: false,
            command: String::new(),
            history: VecDeque::new(),
            history_index: None,
            console: VecDeque::new(),
            status: "从左侧连接树打开 Redis · 尚未连接".into(),
            error: None,
        }
    }

    #[cfg(test)]
    pub(super) fn set_test_state(&mut self, connected: bool, dirty: bool) {
        self.connected = connected;
        self.value_original.clear();
        self.value_text = if dirty {
            "unsaved Redis draft".into()
        } else {
            String::new()
        };
    }

    pub(super) fn profile_id(&self) -> Uuid {
        self.profile.id
    }

    pub(super) fn bound_profile(&self) -> &RedisProfile {
        &self.profile
    }

    pub(super) fn is_connected(&self) -> bool {
        self.connected
    }

    pub(super) fn has_pending_confirmation(&self) -> bool {
        self.confirmation.is_some()
    }

    pub(super) fn database_index(&self) -> u32 {
        self.database
    }

    pub(super) fn databases(&self) -> &[RedisDatabase] {
        &self.databases
    }

    pub(super) fn databases_complete(&self) -> bool {
        self.databases_complete
    }

    pub(super) fn database_notice(&self) -> Option<&str> {
        self.database_notice.as_deref()
    }

    pub(super) fn refresh_databases(&mut self) -> bool {
        if !self.connected || self.busy || self.is_dirty() || self.confirmation.is_some() {
            return false;
        }
        self.request(RedisRequest::Databases)
    }

    #[cfg(test)]
    pub(super) fn set_test_databases(
        &mut self,
        databases: Vec<RedisDatabase>,
        complete: bool,
        notice: Option<String>,
    ) {
        self.databases = databases;
        self.databases_complete = complete;
        self.database_notice = notice;
    }

    pub(super) fn connect(&mut self) -> bool {
        if self.busy || self.is_dirty() || self.confirmation.is_some() {
            return false;
        }
        if self.connected {
            return true;
        }
        self.request(RedisRequest::Connect {
            profile: self.profile.clone(),
            credentials: self.credentials.clone(),
        })
    }

    pub(super) fn disconnect(&mut self) -> bool {
        if self.busy || self.is_dirty() || self.confirmation.is_some() {
            return false;
        }
        if !self.connected {
            return true;
        }
        self.request(RedisRequest::Disconnect)
    }

    pub(super) fn select_database(&mut self, database: u32) -> bool {
        if !self.connected || self.busy || self.is_dirty() || self.confirmation.is_some() {
            return false;
        }
        if self.database == database {
            return true;
        }
        self.request(RedisRequest::SelectDb(database))
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.value_dirty()
            || !self.add_value.is_empty()
            || !self.add_field.is_empty()
            || self.new_key.as_ref().is_some_and(|draft| {
                !draft.key.is_empty() || !draft.value.is_empty() || !draft.field.is_empty()
            })
    }

    fn value_dirty(&self) -> bool {
        self.value_text != self.value_original
            || self.member.as_ref().is_some_and(MemberDraft::dirty)
    }

    fn request(&mut self, request: RedisRequest) -> bool {
        if self.busy || std::env::var_os("SQLSHOP_SCREENSHOT").is_some() {
            return false;
        }
        if self.is_dirty()
            && matches!(
                &request,
                RedisRequest::Connect { .. }
                    | RedisRequest::SelectDb(_)
                    | RedisRequest::Disconnect
                    | RedisRequest::ReadKey { .. }
            )
        {
            self.error = Some("有未保存的 Redis 草稿，请先保存或显式撤销".into());
            return false;
        }
        let generation = self.generation.wrapping_add(1);
        let catalog = matches!(&request, RedisRequest::Databases);
        match self.worker.send(generation, request) {
            Ok(()) => {
                self.generation = generation;
                self.catalog_pending = catalog;
                self.busy = true;
                self.error = None;
                self.status = "正在处理… · 写操作不会自动重试".into();
                true
            }
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                false
            }
        }
    }

    pub(super) fn poll(&mut self) {
        while let Ok(response) = self.worker.receiver.try_recv() {
            if response.generation != self.generation {
                continue;
            }
            self.busy = false;
            let catalog = std::mem::take(&mut self.catalog_pending);
            self.connected = response.connected;
            self.database = response.database;
            match response.result {
                Ok(data) => {
                    self.error = None;
                    match data {
                        RedisData::Connected { database, server } => {
                            self.database = database;
                            self.database_draft = database;
                            self.server = server;
                            self.console.clear();
                            self.history.clear();
                            self.command.clear();
                            self.reset_keys();
                            self.databases.clear();
                            self.databases_complete = false;
                            self.database_notice = None;
                            self.scan_after_catalog = true;
                            self.status = "已连接 · 正在读取全部数据库".into();
                            self.refresh_databases();
                        }
                        RedisData::Databases {
                            databases,
                            complete,
                            notice,
                        } => {
                            self.databases = databases;
                            self.databases_complete = complete;
                            self.database_notice = notice;
                            self.status = format!(
                                "已读取 {} 个数据库{}",
                                self.databases.len(),
                                if complete {
                                    "（含空库）"
                                } else {
                                    "（列表不完整）"
                                }
                            );
                            if std::mem::take(&mut self.scan_after_catalog) {
                                self.scan(true);
                            }
                        }
                        RedisData::Disconnected => {
                            self.databases.clear();
                            self.databases_complete = false;
                            self.database_notice = None;
                            self.scan_after_catalog = false;
                            self.reset_keys();
                            self.status = "已断开 · 没有自动重连".into();
                        }
                        RedisData::Database(database) => {
                            self.database = database;
                            self.database_draft = database;
                            self.reset_keys();
                            self.scan(true);
                        }
                        RedisData::Keys { cursor, keys } => {
                            self.cursor = cursor;
                            self.scan_started = true;
                            for key in keys {
                                if self.keys.len() >= KEY_LIMIT {
                                    break;
                                }
                                if self.keys.insert(key.clone()) {
                                    self.tree.insert(&key);
                                }
                            }
                            self.status = format!(
                                "DB {} · 已加载 {} 个键{}",
                                self.database,
                                self.keys.len(),
                                if cursor == 0 {
                                    " · 本轮扫描结束（非快照）"
                                } else {
                                    " · 可继续加载"
                                }
                            );
                        }
                        RedisData::Key(key) => self.install_key(key),
                        RedisData::Changed { key, deleted } => {
                            let created = self.creating;
                            if created {
                                self.new_key = None;
                                self.creating = false;
                            }
                            if !created
                                && let Some(previous) = &self.selected
                                && previous.key != key
                            {
                                self.keys.remove(&previous.key);
                            }
                            self.value_text.clear();
                            self.value_original.clear();
                            self.member = None;
                            self.add_value.clear();
                            self.add_field.clear();
                            self.status = "写入已确认 · 正在刷新；没有自动重放".into();
                            if deleted {
                                self.keys.remove(&key);
                                self.rebuild_tree();
                                self.selected = None;
                                self.status = "删除已确认 · 键列表为增量视图".into();
                            } else {
                                if self.keys.len() < KEY_LIMIT {
                                    self.keys.insert(key.clone());
                                }
                                self.rebuild_tree();
                                self.stale = true;
                                self.request(RedisRequest::ReadKey { key });
                            }
                        }
                        RedisData::NamespacePreview {
                            token,
                            prefix,
                            count,
                        } => {
                            self.confirmation = Some(Confirmation::Namespace {
                                token,
                                prefix,
                                count,
                                database: self.database,
                            });
                            self.status = "命名空间扫描完成 · 尚未删除，请确认快照范围".into();
                        }
                        RedisData::NamespaceDeleted { prefix, deleted } => {
                            self.keys.retain(|key| !key.starts_with(&prefix));
                            self.rebuild_tree();
                            if self
                                .selected
                                .as_ref()
                                .is_some_and(|key| key.key.starts_with(&prefix))
                            {
                                self.selected = None;
                                self.member = None;
                                self.value_text.clear();
                                self.value_original.clear();
                            }
                            self.status = format!(
                                "命名空间删除完成 · 已删除 {deleted} 个键 · 不包括预览后新增的键，请刷新核实"
                            );
                        }
                        RedisData::Output(output) => {
                            self.push_console(output);
                            self.stale = true;
                            self.status = "命令完成 · 键视图可能已改变，请刷新后编辑".into();
                        }
                    }
                }
                Err(message) if catalog => {
                    self.databases_complete = false;
                    self.database_notice = Some(format!("数据库列表读取失败：{message}"));
                    self.status = "未能读取全部数据库，可重试刷新列表".into();
                    if std::mem::take(&mut self.scan_after_catalog) && self.connected {
                        self.scan(true);
                    }
                }
                Err(message) => {
                    self.creating = false;
                    self.stale = true;
                    self.error = Some(message);
                    self.status = "操作失败 · 保留草稿，不自动重连或重放".into();
                }
            }
            if !self.connected {
                self.stale = true;
            }
        }
    }

    fn push_console(&mut self, text: String) {
        self.console.push_back(text);
        while self.console.len() > 30 {
            self.console.pop_front();
        }
    }

    fn reset_keys(&mut self) {
        self.keys.clear();
        self.tree = KeyTree::default();
        self.cursor = 0;
        self.scan_started = false;
        self.selected = None;
        self.member = None;
        self.value_text.clear();
        self.value_original.clear();
        self.confirmation = None;
        self.stale = true;
    }

    fn rebuild_tree(&mut self) {
        self.tree = KeyTree::default();
        for key in &self.keys {
            self.tree.insert(key);
        }
    }

    fn scan(&mut self, reset: bool) {
        if !self.connected || self.busy {
            return;
        }
        if reset {
            self.keys.clear();
            self.tree = KeyTree::default();
            self.cursor = 0;
            self.scan_started = false;
            self.scan_pattern = if self.pattern.is_empty() {
                "*".into()
            } else {
                self.pattern.clone()
            };
        }
        self.request(RedisRequest::Scan {
            cursor: self.cursor,
            pattern: self.scan_pattern.clone(),
        });
    }

    fn install_key(&mut self, key: RedisKey) {
        self.stale = false;
        self.rename = String::from_utf8(key.key.clone()).unwrap_or_default();
        self.ttl = if key.ttl_ms > 0 {
            (key.ttl_ms + 999) / 1000
        } else {
            3600
        };
        self.member = None;
        self.add_field.clear();
        self.add_value.clear();
        if let RedisValue::String(value) = &key.value {
            self.value_format = if std::str::from_utf8(value).is_err() {
                ValueFormat::Hex
            } else {
                ValueFormat::Text
            };
            self.value_text = self.value_format.show(value);
            self.value_original = self.value_text.clone();
        } else {
            self.value_text.clear();
            self.value_original.clear();
        }
        self.status = format!(
            "{} · {} · {}{}",
            display_bytes(&key.key),
            key.kind.label(),
            key.size,
            if key.truncated {
                " · 限量预览"
            } else {
                ""
            }
        );
        self.selected = Some(key);
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui) {
        self.poll();
        ui.push_id(self.id, |ui| {
            ui.spacing_mut().item_spacing.y = 5.0;
            ui.horizontal_wrapped(|ui| {
                ui.heading(format!("Redis · {}", self.profile.name));
                if self.busy {
                    ui.spinner();
                }
                if self.connected {
                    ui.label(
                        RichText::new(format!(
                            "{}:{} / DB {}",
                            self.profile.host, self.profile.port, self.database
                        ))
                        .strong(),
                    );
                    if ui
                        .add_enabled(!self.busy && !self.is_dirty(), egui::Button::new("断开"))
                        .clicked()
                    {
                        self.disconnect();
                    }
                } else if ui
                    .add_enabled(
                        !self.busy && !self.is_dirty(),
                        egui::Button::new("连接 Redis"),
                    )
                    .clicked()
                {
                    self.connect();
                }
            });
            if let Some(error) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if let Some(notice) = &self.database_notice {
                ui.colored_label(ui.visuals().warn_fg_color, notice);
            }
            if self.is_dirty() {
                ui.label(RichText::new("有未保存修改").color(ui.visuals().warn_fg_color));
            }
            ui.separator();
            self.confirmation_ui(ui);
            if self.confirmation.is_some() {
                return;
            }
            if !self.connected && self.is_dirty() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "连接已断开，写入结果待核实；草稿已保留。",
                );
                if ui.button("丢弃断线草稿，允许重新连接").clicked() {
                    self.value_text = self.value_original.clone();
                    self.member = None;
                    self.add_value.clear();
                    self.add_field.clear();
                    self.new_key = None;
                }
                if ui.button("复制断线草稿").clicked() {
                    let mut text = self.value_text.clone();
                    if let Some(member) = &self.member {
                        text.push_str(&format!(
                            "\n成员草稿：{}\nScore：{}",
                            member.text, member.score
                        ));
                    }
                    if let Some(draft) = &self.new_key {
                        text.push_str(&format!(
                            "\n新键：{}\nField：{}\nValue：{}",
                            draft.key, draft.field, draft.value
                        ));
                    }
                    text.push_str(&format!(
                        "\n待添加 Field：{}\nValue：{}",
                        self.add_field, self.add_value
                    ));
                    ui.ctx().copy_text(text);
                }
                if self.selected.is_some() {
                    self.value_ui(ui, (ui.available_height() - 40.0).max(100.0));
                }
            }
            if self.connected {
                ui.horizontal(|ui| {
                    ui.label("数据库");
                    ui.add_enabled(
                        !self.busy && !self.is_dirty(),
                        egui::DragValue::new(&mut self.database_draft)
                            .range(0..=u32::MAX)
                            .prefix("DB "),
                    );
                    if ui
                        .add_enabled(
                            !self.busy
                                && !self.is_dirty()
                                && self.confirmation.is_none()
                                && self.database_draft != self.database,
                            egui::Button::new("切换"),
                        )
                        .clicked()
                    {
                        self.request(RedisRequest::SelectDb(self.database_draft));
                    }
                    if ui
                        .add_enabled(
                            !self.busy && !self.is_dirty(),
                            egui::Button::new("刷新数据库列表"),
                        )
                        .clicked()
                    {
                        self.refresh_databases();
                    }
                    ui.toggle_value(&mut self.console_open, "命令控制台");
                });
                if self.new_key.is_some() {
                    self.new_key_ui(ui);
                }
                if self.console_open {
                    self.console_ui(ui);
                }
                let height = (ui.available_height() - 28.0).max(180.0);
                self.key_value_workspace(ui, height);
            } else {
                ui.add_space(20.0);
                ui.weak("未连接");
            }
            ui.separator();
            ui.small(&self.status);
        });
    }

    fn key_value_workspace(&mut self, ui: &mut egui::Ui, height: f32) -> (egui::Rect, egui::Rect) {
        let available = ui.available_rect_before_wrap();
        let area = egui::Rect::from_min_size(available.min, egui::vec2(available.width(), height));
        let panel_id = egui::Id::new((self.id, "redis-key-panel"));
        let mut splitter = egui::Rect::NOTHING;
        let mut scroll_rect = egui::Rect::NOTHING;
        ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            let width = (area.width() * 0.30).clamp(180.0, 340.0);
            let maximum = (area.width() - 240.0).max(180.0);
            let panel = egui::SidePanel::left(panel_id)
                .resizable(true)
                .default_width(width.min(maximum))
                .width_range(180.0..=maximum)
                .frame(egui::Frame::new().inner_margin(egui::Margin {
                    left: 0,
                    right: 8,
                    top: 0,
                    bottom: 0,
                }))
                .show_inside(ui, |ui| self.key_browser(ui, height));
            scroll_rect = panel.inner;
            splitter = egui::Rect::from_center_size(
                egui::pos2(panel.response.rect.right(), area.center().y),
                egui::vec2(10.0, height),
            );
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(
                    ui.available_rect_before_wrap()
                        .shrink2(egui::vec2(6.0, 0.0)),
                ),
                |ui| {
                    ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                    self.value_ui(ui, height);
                },
            );
        });
        ui.advance_cursor_after_rect(area);
        (splitter, scroll_rect)
    }

    fn preview_namespace(&mut self, prefix: Vec<u8>) -> bool {
        if !self.connected
            || self.busy
            || self.is_dirty()
            || self.confirmation.is_some()
            || prefix.is_empty()
        {
            return false;
        }
        self.request(RedisRequest::PreviewNamespace { prefix })
    }

    fn key_browser(&mut self, ui: &mut egui::Ui, height: f32) -> egui::Rect {
        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
        ui.horizontal(|ui| {
            ui.strong("键空间");
            ui.selectable_value(&mut self.tree_view, true, "树");
            ui.selectable_value(&mut self.tree_view, false, "列表");
        });
        let enter = ui.add(
            egui::TextEdit::singleline(&mut self.pattern)
                .desired_width(f32::INFINITY)
                .hint_text("SCAN MATCH，例如 user:*"),
        );
        let search = enter.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let mut refresh = false;
        ui.horizontal(|ui| {
            refresh = ui
                .add_enabled(!self.busy, egui::Button::new("搜索 / 刷新"))
                .clicked();
            if ui
                .add_enabled(
                    !self.busy && !self.is_dirty(),
                    egui::Button::new("＋ 新建键"),
                )
                .clicked()
            {
                self.new_key = Some(NewKey::default());
            }
        });
        if (search || refresh) && !self.busy {
            self.scan(true);
        }
        ui.small(format!("{} 个已加载", self.keys.len()));
        let mut chosen = None;
        let mut namespace = None;
        let scroll = egui::ScrollArea::both()
            .id_salt("redis-key-tree")
            .auto_shrink([false, false])
            .max_height((height - 150.0).max(80.0))
            .show(ui, |ui| {
                ui.add_enabled_ui(
                    !self.busy && !self.is_dirty() && self.confirmation.is_none(),
                    |ui| {
                        let selected = self.selected.as_ref().map(|key| key.key.as_slice());
                        if self.tree_view {
                            draw_tree(
                                ui,
                                &self.tree,
                                selected,
                                &mut Vec::new(),
                                &mut chosen,
                                &mut namespace,
                                0,
                            );
                        } else {
                            for key in &self.keys {
                                if key_leaf(
                                    ui,
                                    key,
                                    &display_bytes(key),
                                    selected == Some(key.as_slice()),
                                ) {
                                    chosen = Some(key.clone());
                                }
                            }
                        }
                    },
                );
            });
        if let Some(prefix) = namespace {
            self.preview_namespace(prefix);
        }
        if let Some(key) = chosen {
            self.stale = true;
            self.selected = None;
            self.request(RedisRequest::ReadKey { key });
        }
        if self.keys.len() >= KEY_LIMIT {
            ui.weak("已达 20,000 键预览上限，请缩小 MATCH 范围");
        }
        if ui
            .add_enabled(
                !self.busy && self.scan_started && self.cursor != 0 && self.keys.len() < KEY_LIMIT,
                egui::Button::new("继续 SCAN 加载"),
            )
            .clicked()
        {
            self.scan(false);
        }
        scroll.inner_rect
    }

    fn value_ui(&mut self, ui: &mut egui::Ui, height: f32) {
        let Some(key) = self.selected.clone() else {
            ui.add_space(24.0);
            ui.heading("选择一个 Key");
            return;
        };
        egui::ScrollArea::vertical()
            .id_salt("redis-value-scroll")
            .max_height(height)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(key.kind.label());
                    ui.add(
                        egui::Label::new(RichText::new(display_bytes(&key.key)).monospace()).wrap(),
                    );
                    if ui.button("复制键名").clicked() {
                        ui.ctx().copy_text(display_bytes(&key.key));
                    }
                    if ui
                        .add_enabled(
                            self.connected && !self.busy && !self.is_dirty(),
                            egui::Button::new("重新读取"),
                        )
                        .clicked()
                    {
                        self.stale = true;
                        self.request(RedisRequest::ReadKey {
                            key: key.key.clone(),
                        });
                    }
                    if ui
                        .add_enabled(
                            (self.value_dirty()
                                || !self.add_value.is_empty()
                                || !self.add_field.is_empty())
                                && !self.busy,
                            egui::Button::new("撤销本地修改"),
                        )
                        .clicked()
                    {
                        self.value_text = self.value_original.clone();
                        self.member = None;
                        self.add_value.clear();
                        self.add_field.clear();
                    }
                });
                ui.weak(format!(
                    "大小 / 元素数：{} · TTL：{}",
                    key.size,
                    ttl_label(key.ttl_ms)
                ));
                if key.truncated {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "预览已截断，截断的 String 不可保存。",
                    );
                }
                if self.stale {
                    ui.colored_label(ui.visuals().warn_fg_color, "数据已过期，请刷新。");
                }
                let writable = self.connected
                    && !self.busy
                    && !self.stale
                    && self.confirmation.is_none()
                    && self.new_key.is_none();
                ui.add_enabled_ui(writable, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("过期秒数");
                        ui.add(egui::DragValue::new(&mut self.ttl).range(1..=i32::MAX as i64));
                        if ui
                            .add_enabled(!self.is_dirty(), egui::Button::new("设置 TTL…"))
                            .clicked()
                        {
                            self.confirmation = Some(Confirmation::Mutation {
                                key: key.key.clone(),
                                change: RedisMutation::Expire { seconds: self.ttl },
                                label: format!("将过期时间设为 {} 秒", self.ttl),
                            });
                        }
                        if ui
                            .add_enabled(!self.is_dirty(), egui::Button::new("永不过期"))
                            .clicked()
                        {
                            self.mutate(key.key.clone(), RedisMutation::Persist);
                        }
                        if ui
                            .add_enabled(!self.is_dirty(), egui::Button::new("删除键…"))
                            .clicked()
                        {
                            self.confirmation = Some(Confirmation::Mutation {
                                key: key.key.clone(),
                                change: RedisMutation::Delete,
                                label: "删除此键及其全部值（不可撤销）".into(),
                            });
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.rename)
                                .hint_text("新键名（UTF-8）")
                                .desired_width(240.0),
                        );
                        if ui
                            .add_enabled(
                                !self.is_dirty()
                                    && self.rename.as_bytes() != key.key
                                    && !self.rename.is_empty(),
                                egui::Button::new("重命名…"),
                            )
                            .clicked()
                        {
                            self.confirmation = Some(Confirmation::Mutation {
                                key: key.key.clone(),
                                change: RedisMutation::Rename {
                                    new_key: self.rename.as_bytes().to_vec(),
                                },
                                label: format!("重命名为 {}（不覆盖已有键）", self.rename),
                            });
                        }
                    });
                });
                ui.separator();
                match &key.value {
                    RedisValue::String(bytes) => self.string_ui(ui, &key, bytes, writable),
                    RedisValue::Unsupported(message) => {
                        ui.weak(message);
                    }
                    _ => self.collection_ui(ui, &key, writable),
                }
            });
    }

    fn mutate(&mut self, key: Vec<u8>, change: RedisMutation) {
        self.request(RedisRequest::Mutate { key, change });
        if self.busy {
            self.stale = true;
        }
    }

    fn string_ui(&mut self, ui: &mut egui::Ui, key: &RedisKey, bytes: &[u8], writable: bool) {
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!self.value_dirty() && !self.busy, |ui| {
                for format in [ValueFormat::Text, ValueFormat::Json, ValueFormat::Hex] {
                    if ui
                        .add_enabled(
                            format == ValueFormat::Hex || std::str::from_utf8(bytes).is_ok(),
                            egui::Button::selectable(self.value_format == format, format.label()),
                        )
                        .clicked()
                    {
                        self.value_format = format;
                        self.value_text = format.show(bytes);
                        self.value_original = self.value_text.clone();
                    }
                }
            });
            if ui
                .add_enabled(
                    writable && !key.truncated && self.value_format != ValueFormat::Hex,
                    egui::Button::new("格式化 JSON"),
                )
                .clicked()
            {
                match serde_json::from_str::<serde_json::Value>(&self.value_text)
                    .and_then(|value| serde_json::to_string_pretty(&value))
                {
                    Ok(text) => self.value_text = text,
                    Err(error) => self.error = Some(format!("JSON 无效：{error}")),
                }
            }
            if ui.button("复制显示内容").clicked() {
                ui.ctx().copy_text(self.value_text.clone());
            }
        });
        ui.add_enabled(
            writable && !key.truncated,
            egui::TextEdit::multiline(&mut self.value_text)
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY)
                .desired_rows(13),
        );
        if ui
            .add_enabled(
                writable && !key.truncated && self.value_dirty(),
                egui::Button::new("保存值（保留 TTL）"),
            )
            .clicked()
        {
            match self.value_format.bytes(&self.value_text) {
                Ok(value) => self.mutate(
                    key.key.clone(),
                    RedisMutation::SetString {
                        value,
                        expected: bytes.to_vec(),
                    },
                ),
                Err(error) => self.error = Some(format!("内容无效：{error:#}")),
            }
        }
    }

    fn collection_ui(&mut self, ui: &mut egui::Ui, key: &RedisKey, writable: bool) {
        let rows: Vec<(Vec<u8>, Vec<u8>, f64)> = match &key.value {
            RedisValue::Hash(rows) => rows
                .iter()
                .map(|(field, value)| (field.clone(), value.clone(), 0.0))
                .collect(),
            RedisValue::List(rows) => rows
                .iter()
                .enumerate()
                .map(|(index, value)| (index.to_string().into_bytes(), value.clone(), 0.0))
                .collect(),
            RedisValue::Set(rows) => rows
                .iter()
                .map(|value| (Vec::new(), value.clone(), 0.0))
                .collect(),
            RedisValue::ZSet(rows) => rows
                .iter()
                .map(|(value, score)| (Vec::new(), value.clone(), *score))
                .collect(),
            _ => Vec::new(),
        };
        let mut chosen = None;
        TableBuilder::new(ui)
            .id_salt("redis-members")
            .striped(true)
            .resizable(true)
            .column(Column::auto().at_least(80.0))
            .column(Column::remainder().at_least(100.0))
            .max_scroll_height(220.0)
            .header(24.0, |mut header| {
                header.col(|ui| {
                    ui.strong(if key.kind == RedisKind::ZSet {
                        "Score"
                    } else if key.kind == RedisKind::Hash {
                        "Field"
                    } else {
                        "索引"
                    });
                });
                header.col(|ui| {
                    ui.strong("Value / Member · 单击选择编辑");
                });
            })
            .body(|mut body| {
                for (index, (field, value, score)) in rows.iter().enumerate() {
                    body.row(24.0, |mut row| {
                        row.set_selected(
                            self.member
                                .as_ref()
                                .is_some_and(|draft| draft.index == index),
                        );
                        row.col(|ui| {
                            ui.label(if key.kind == RedisKind::ZSet {
                                score.to_string()
                            } else if key.kind == RedisKind::Hash {
                                preview(field)
                            } else {
                                index.to_string()
                            });
                        });
                        row.col(|ui| {
                            if ui
                                .add_enabled(
                                    !self.busy && !self.value_dirty(),
                                    egui::Button::selectable(
                                        self.member
                                            .as_ref()
                                            .is_some_and(|draft| draft.index == index),
                                        preview(value),
                                    ),
                                )
                                .on_hover_text(
                                    display_bytes(value).chars().take(4096).collect::<String>(),
                                )
                                .clicked()
                            {
                                chosen = Some(index);
                            }
                        });
                    });
                }
            });
        if let Some(index) = chosen {
            let (field, value, score) = &rows[index];
            self.member = Some(MemberDraft::new(
                index,
                field.clone(),
                value.clone(),
                *score,
            ));
        }
        let mut mutation = None;
        let mut removal = None;
        let no_add_draft = self.add_value.is_empty() && self.add_field.is_empty();
        if let Some(member) = &mut self.member {
            ui.separator();
            ui.strong(format!("编辑选中项 #{}", member.index));
            ui.add_enabled_ui(writable && no_add_draft, |ui| {
                if key.kind == RedisKind::ZSet {
                    ui.horizontal(|ui| {
                        ui.label("Score");
                        ui.add(egui::DragValue::new(&mut member.score).speed(0.1));
                    });
                }
                let was_hex = member.hex;
                ui.add_enabled_ui(!member.dirty(), |ui| {
                    ui.checkbox(&mut member.hex, "Hex 编辑");
                });
                if was_hex != member.hex {
                    member.text = if member.hex {
                        format_hex(&member.value)
                    } else {
                        String::from_utf8_lossy(&member.value).into_owned()
                    };
                    member.original = member.text.clone();
                    // Invalid UTF-8 is never silently converted back into editable text.
                    if !member.hex && std::str::from_utf8(&member.value).is_err() {
                        member.hex = true;
                        member.text = format_hex(&member.value);
                        member.original = member.text.clone();
                    }
                }
                ui.add_enabled(
                    matches!(key.kind, RedisKind::Hash | RedisKind::List),
                    egui::TextEdit::multiline(&mut member.text)
                        .font(egui::TextStyle::Monospace)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    let editable = matches!(
                        key.kind,
                        RedisKind::Hash | RedisKind::List | RedisKind::ZSet
                    );
                    if ui
                        .add_enabled(editable && member.dirty(), egui::Button::new("保存此项"))
                        .clicked()
                    {
                        match member.bytes() {
                            Ok(value) => {
                                mutation = Some(match key.kind {
                                    RedisKind::Hash => RedisMutation::HashSet {
                                        field: member.field.clone(),
                                        value,
                                    },
                                    RedisKind::List => RedisMutation::ListSet {
                                        index: member.index as i64,
                                        expected: member.value.clone(),
                                        value,
                                    },
                                    _ => RedisMutation::ZSetAdd {
                                        score: member.score,
                                        value: member.value.clone(),
                                    },
                                })
                            }
                            Err(error) => self.error = Some(format!("Hex 无效：{error:#}")),
                        }
                    }
                    if ui
                        .add_enabled(
                            !member.dirty() && !matches!(key.kind, RedisKind::List),
                            egui::Button::new("移除此项…"),
                        )
                        .clicked()
                    {
                        removal = Some(match key.kind {
                            RedisKind::Hash => RedisMutation::HashDelete {
                                field: member.field.clone(),
                            },
                            RedisKind::Set => RedisMutation::SetRemove {
                                value: member.value.clone(),
                            },
                            _ => RedisMutation::ZSetRemove {
                                value: member.value.clone(),
                            },
                        });
                    }
                });
            });
        }
        if let Some(change) = mutation {
            self.mutate(key.key.clone(), change);
        }
        if let Some(change) = removal {
            self.confirmation = Some(Confirmation::Mutation {
                key: key.key.clone(),
                change,
                label: "移除选中的集合项（移除最后一项会删除键）".into(),
            });
        }
        ui.separator();
        egui::CollapsingHeader::new("添加集合项").show(ui, |ui| {
            ui.add_enabled_ui(writable && !self.value_dirty(), |ui| {
                if key.kind == RedisKind::Hash {
                    ui.horizontal(|ui| {
                        ui.label("Field（文本）");
                        ui.text_edit_singleline(&mut self.add_field);
                    });
                }
                if key.kind == RedisKind::ZSet {
                    ui.horizontal(|ui| {
                        ui.label("Score");
                        ui.add(egui::DragValue::new(&mut self.add_score).speed(0.1));
                    });
                }
                ui.checkbox(&mut self.add_hex, "Value 使用 Hex");
                ui.add(
                    egui::TextEdit::multiline(&mut self.add_value)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                if ui
                    .button(if key.kind == RedisKind::List {
                        "追加到列表末尾"
                    } else {
                        "添加 / 更新"
                    })
                    .clicked()
                {
                    let value = if self.add_hex {
                        parse_hex(&self.add_value)
                    } else {
                        Ok(self.add_value.as_bytes().to_vec())
                    };
                    match value {
                        Ok(value) => {
                            let change = match key.kind {
                                RedisKind::Hash => RedisMutation::HashSet {
                                    field: self.add_field.as_bytes().to_vec(),
                                    value,
                                },
                                RedisKind::List => RedisMutation::ListPush { value },
                                RedisKind::Set => RedisMutation::SetAdd { value },
                                _ => RedisMutation::ZSetAdd {
                                    score: self.add_score,
                                    value,
                                },
                            };
                            self.mutate(key.key.clone(), change);
                        }
                        Err(error) => self.error = Some(format!("Hex 无效：{error:#}")),
                    }
                }
            });
        });
    }

    fn new_key_ui(&mut self, ui: &mut egui::Ui) {
        let Some(draft) = &mut self.new_key else {
            return;
        };
        let mut create = false;
        let mut cancel = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.add_enabled_ui(!self.busy, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong("新建键");
                    ui.add(egui::TextEdit::singleline(&mut draft.key).hint_text("Key（UTF-8）"));
                    egui::ComboBox::from_id_salt("redis-new-kind")
                        .selected_text(draft.kind.label())
                        .show_ui(ui, |ui| {
                            for kind in [
                                RedisKind::String,
                                RedisKind::Hash,
                                RedisKind::List,
                                RedisKind::Set,
                                RedisKind::ZSet,
                            ] {
                                let label = kind.label();
                                ui.selectable_value(&mut draft.kind, kind.clone(), label);
                            }
                        });
                    ui.checkbox(&mut draft.hex, "值为 Hex");
                });
                if draft.kind == RedisKind::Hash {
                    ui.horizontal(|ui| {
                        ui.label("Field");
                        ui.text_edit_singleline(&mut draft.field);
                    });
                }
                if draft.kind == RedisKind::ZSet {
                    ui.horizontal(|ui| {
                        ui.label("Score");
                        ui.add(egui::DragValue::new(&mut draft.score).speed(0.1));
                    });
                }
                ui.add(
                    egui::TextEdit::multiline(&mut draft.value)
                        .hint_text("初始值 / 首个成员")
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    create = ui
                        .add_enabled(!draft.key.is_empty(), egui::Button::new("创建（拒绝覆盖）"))
                        .clicked();
                    cancel = ui.button("取消新建").clicked();
                });
            });
        });
        if create {
            let value = if draft.hex {
                parse_hex(&draft.value)
            } else {
                Ok(draft.value.as_bytes().to_vec())
            };
            match value {
                Ok(value) => {
                    let request = RedisRequest::Create {
                        key: draft.key.as_bytes().to_vec(),
                        kind: draft.kind.clone(),
                        value,
                        field: draft.field.as_bytes().to_vec(),
                        score: draft.score,
                    };
                    self.request(request);
                    self.creating = self.busy;
                    // Keep the new-key draft until the server confirms success.
                }
                Err(error) => self.error = Some(format!("初始值无效：{error:#}")),
            }
        }
        if cancel {
            self.new_key = None;
        }
    }

    fn confirmation_ui(&mut self, ui: &mut egui::Ui) {
        let Some(confirmation) = &self.confirmation else {
            return;
        };
        let mut confirm = false;
        let mut cancel = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.strong(format!(
                "确认写入 · {}:{} / DB {}",
                self.profile.host, self.profile.port, self.database
            ));
            match confirmation {
                Confirmation::Namespace { prefix, count, database, .. } => {
                    ui.colored_label(ui.visuals().warn_fg_color, format!("删除 db{database} 中命名空间下预览到的 {count} 个键（不可撤销）"));
                    ui.monospace(display_bytes(prefix));
                    ui.label("删除预览内的全部键；可能部分完成，不可撤销。")
                        .on_hover_text("按字面前缀匹配，包括未加载键；仅删除预览键名，同名键被并发替换仍会删除。分批 UNLINK，不自动重试。");
                }
                Confirmation::Mutation { key, label, .. } => {
                    ui.label(label);
                    ui.monospace(display_bytes(key));
                }
                Confirmation::Command(args) => {
                    ui.label("执行写命令，不可撤销。");
                    ui.monospace(
                        args.iter()
                            .map(|arg| display_bytes(arg))
                            .collect::<Vec<_>>()
                            .join(" "),
                    );
                }
            }
            ui.horizontal(|ui| {
                confirm = ui
                    .add_enabled(!self.busy && !self.is_dirty(), egui::Button::new("确认执行"))
                    .clicked();
                cancel = ui
                    .add_enabled(!self.busy, egui::Button::new("取消"))
                    .clicked();
            });
        });
        if cancel {
            self.confirmation = None;
        }
        if confirm && let Some(confirmation) = self.confirmation.take() {
            match confirmation {
                Confirmation::Mutation { key, change, .. } => self.mutate(key, change),
                Confirmation::Command(args) => self.execute_command(args, true),
                Confirmation::Namespace {
                    token, database, ..
                } => {
                    if database != self.database || !self.connected {
                        self.error = Some(
                            "数据库或连接已改变，命名空间删除预览已失效，请重新扫描确认".into(),
                        );
                    } else {
                        self.stale = true;
                        self.request(RedisRequest::DeleteNamespace { token });
                    }
                }
            }
        }
    }

    fn console_ui(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong(format!("Redis 控制台 · DB {}", self.database));
                if ui.button("清空输出").clicked() {
                    self.console.clear();
                }
            });
            egui::ScrollArea::vertical()
                .id_salt("redis-console-output")
                .max_height(115.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for output in &self.console {
                        ui.monospace(output);
                    }
                });
            let mut run = false;
            ui.add_enabled_ui(
                !self.busy && !self.is_dirty() && self.confirmation.is_none(),
                |ui| {
                    ui.horizontal(|ui| {
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut self.command)
                                .font(egui::TextStyle::Monospace)
                                .desired_width((ui.available_width() - 70.0).max(100.0))
                                .hint_text("例如 GET \"user:1\""),
                        );
                        run = ui.button("执行").clicked()
                            || (response.lost_focus()
                                && ui.input(|input| input.key_pressed(egui::Key::Enter)));
                        if response.has_focus() {
                            if ui.input(|input| input.key_pressed(egui::Key::ArrowUp))
                                && !self.history.is_empty()
                            {
                                let index =
                                    self.history_index.map_or(self.history.len() - 1, |index| {
                                        index.saturating_sub(1)
                                    });
                                self.history_index = Some(index);
                                self.command = self.history[index].clone();
                            }
                            if ui.input(|input| input.key_pressed(egui::Key::ArrowDown))
                                && let Some(index) = self.history_index
                            {
                                self.history_index =
                                    (index + 1 < self.history.len()).then_some(index + 1);
                                self.command = self
                                    .history_index
                                    .map_or_else(String::new, |index| self.history[index].clone());
                            }
                        }
                    });
                },
            );
            if run {
                match parse_command(&self.command) {
                    Ok(args) if args.is_empty() => {}
                    Ok(args) => {
                        let command = String::from_utf8_lossy(&args[0]).to_ascii_uppercase();
                        if matches!(command.as_str(), "AUTH" | "HELLO" | "ACL" | "CONFIG") {
                            self.error =
                                Some("凭据和管理命令不进入控制台或历史，请使用连接设置".into());
                        } else if command_needs_confirmation(&args) {
                            self.confirmation = Some(Confirmation::Command(args));
                        } else {
                            self.execute_command(args, false);
                        }
                    }
                    Err(error) => self.error = Some(format!("命令解析失败：{error:#}")),
                }
            }
        });
    }

    fn execute_command(&mut self, args: Vec<Vec<u8>>, confirmed: bool) {
        self.history.push_back(self.command.clone());
        while self.history.len() > 100 {
            self.history.pop_front();
        }
        self.history_index = None;
        self.push_console(format!("> DB {} · {}", self.database, self.command));
        self.request(RedisRequest::Command { args, confirmed });
    }

    pub(super) fn fixture(context: &egui::Context) -> Self {
        let mut doc = Self::new_bound(context, RedisProfile::default(), Credentials::default());
        doc.connected = true;
        doc.profile.name = "Redis 离线示例".into();
        doc.server = "离线布局示例，未连接数据库".into();
        for key in [
            "app:users:1001",
            "app:users:1002",
            "app:config:theme",
            "app:config:limits",
            "cache:products:featured",
            "cache:products:recent",
            "queue:emails",
            "session:demo",
            "stats:visits",
        ] {
            doc.keys.insert(key.as_bytes().to_vec());
        }
        doc.rebuild_tree();
        doc.install_key(RedisKey { key: b"app:users:1001".to_vec(), kind: RedisKind::String, ttl_ms: 3_600_000, size: 84, truncated: false, value: RedisValue::String(b"{\n  \"id\": 1001,\n  \"name\": \"SQLShop\",\n  \"roles\": [\"developer\"],\n  \"active\": true\n}".to_vec()) });
        doc.databases = (0..32)
            .map(|index| RedisDatabase {
                index,
                keys: Some(if index == 0 { 401 } else { 0 }),
            })
            .collect();
        doc.databases_complete = true;
        if std::env::var_os("SQLSHOP_SCREENSHOT").is_some()
            && std::env::var_os("SQLSHOP_SCREENSHOT_REDIS_WIDE").is_some()
        {
            doc.keys = (0..200)
                .map(|index| format!("key-{index:04}").into_bytes())
                .collect();
            doc.rebuild_tree();
            doc.selected = None;
            doc.value_text.clear();
            doc.value_original.clear();
            context.data_mut(|data| {
                data.insert_persisted(
                    egui::Id::new((doc.id, "redis-key-panel")),
                    egui::containers::panel::PanelState {
                        rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(560.0, 600.0)),
                    },
                )
            });
        }
        doc.scan_started = true;
        doc.status = "离线布局示例 · 未建立连接或执行 Redis 命令".into();
        doc
    }
}

fn ttl_label(milliseconds: i64) -> String {
    match milliseconds {
        -1 => "永不过期".into(),
        -2 => "键已不存在".into(),
        milliseconds => format!("{} ms（读取时快照）", milliseconds),
    }
}

fn preview(bytes: &[u8]) -> String {
    let text = display_bytes(&bytes[..bytes.len().min(512)]);
    let mut preview: String = text.chars().take(100).collect();
    if text.chars().count() > 100 || bytes.len() > 512 {
        preview.push('…');
    }
    preview.replace('\n', " ↵ ").replace('\r', "")
}

fn key_leaf(ui: &mut egui::Ui, key: &[u8], label: &str, selected: bool) -> bool {
    let response = ui.selectable_label(
        selected,
        RichText::new(if label.is_empty() {
            "(空键名)"
        } else {
            label
        })
        .monospace(),
    );
    response.context_menu(|ui| {
        if ui.button("复制键名").clicked() {
            ui.ctx().copy_text(display_bytes(key));
            ui.close();
        }
        if ui.button("复制键名 Hex").clicked() {
            ui.ctx().copy_text(format_hex(key));
            ui.close();
        }
    });
    response.on_hover_text(preview(key)).clicked()
}

fn namespace_prefix(path: &[Vec<u8>]) -> Vec<u8> {
    let mut prefix = path.join(&b':');
    prefix.push(b':');
    prefix
}

fn draw_tree(
    ui: &mut egui::Ui,
    tree: &KeyTree,
    selected: Option<&[u8]>,
    path: &mut Vec<Vec<u8>>,
    chosen: &mut Option<Vec<u8>>,
    namespace: &mut Option<Vec<u8>>,
    depth: usize,
) {
    // Extremely deeply separated keys are flattened instead of recursing indefinitely.
    if depth >= 24 {
        draw_flat_subtree(ui, tree, selected, chosen);
        return;
    }
    for (part, node) in &tree.children {
        path.push(part.clone());
        if node.children.is_empty() {
            if let Some(key) = &node.key
                && key_leaf(
                    ui,
                    key,
                    &display_bytes(part),
                    selected == Some(key.as_slice()),
                )
            {
                *chosen = Some(key.clone());
            }
        } else {
            let header = egui::CollapsingHeader::new(format!(
                "{}  ({})",
                if part.is_empty() {
                    "(空前缀)".into()
                } else {
                    display_bytes(part)
                },
                node.count
            ))
            .id_salt((&*path, "redis-prefix"))
            .default_open(depth == 0)
            .show(ui, |ui| {
                if let Some(key) = &node.key
                    && key_leaf(
                        ui,
                        key,
                        "此命名空间同名键",
                        selected == Some(key.as_slice()),
                    )
                {
                    *chosen = Some(key.clone());
                }
                draw_tree(ui, node, selected, path, chosen, namespace, depth + 1);
            });
            header.header_response.context_menu(|ui| {
                let prefix = namespace_prefix(path);
                if ui.button("复制命名空间前缀").clicked() {
                    ui.ctx().copy_text(display_bytes(&prefix));
                    ui.close();
                }
                if ui.button("删除整个命名空间…").clicked() {
                    *namespace = Some(prefix);
                    ui.close();
                }
            });
        }
        path.pop();
    }
}

fn draw_flat_subtree(
    ui: &mut egui::Ui,
    tree: &KeyTree,
    selected: Option<&[u8]>,
    chosen: &mut Option<Vec<u8>>,
) {
    let mut stack = vec![tree];
    while let Some(node) = stack.pop() {
        if let Some(key) = &node.key
            && key_leaf(ui, key, &preview(key), selected == Some(key.as_slice()))
        {
            *chosen = Some(key.clone());
        }
        stack.extend(node.children.values().rev());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_tree_scrollbar_stays_at_resized_panel_edge_for_short_and_long_keys() {
        for dark in [false, true] {
            for long_keys in [false, true] {
                let context = egui::Context::default();
                context.set_visuals(if dark {
                    egui::Visuals::dark()
                } else {
                    egui::Visuals::light()
                });
                let mut doc = RedisDocument::fixture(&context);
                doc.keys = (0..200)
                    .map(|index| {
                        if long_keys {
                            format!("{index:04}-{}", "long-key-".repeat(40)).into_bytes()
                        } else {
                            format!("key-{index:04}").into_bytes()
                        }
                    })
                    .collect();
                doc.rebuild_tree();
                let mut time = 0.0;
                let mut frame = |events| {
                    time += 0.1;
                    let mut geometry = (egui::Rect::NOTHING, egui::Rect::NOTHING);
                    let _ = context.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(1200.0, 700.0),
                            )),
                            events,
                            time: Some(time),
                            ..Default::default()
                        },
                        |context| {
                            egui::CentralPanel::default().show(context, |ui| {
                                geometry = doc.key_value_workspace(ui, 600.0);
                            });
                        },
                    );
                    geometry
                };
                frame(vec![]);
                let (divider, viewport) = frame(vec![]);
                let gap = divider.center().x - viewport.right();
                assert!(
                    (7.0..=28.0).contains(&gap),
                    "scrollbar must follow panel edge, gap={gap}"
                );
                let start = divider.center();
                let pointer = |pos, pressed| egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                frame(vec![egui::Event::PointerMoved(start), pointer(start, true)]);
                let moved = start + egui::vec2(230.0, 0.0);
                frame(vec![egui::Event::PointerMoved(moved)]);
                frame(vec![egui::Event::PointerMoved(moved)]);
                frame(vec![pointer(moved, false)]);
                let (resized, viewport) = frame(vec![]);
                assert!(resized.center().x > divider.center().x + 180.0);
                let gap = resized.center().x - viewport.right();
                assert!(
                    (7.0..=28.0).contains(&gap),
                    "resized scrollbar drifted into pane, gap={gap}"
                );
                assert!(
                    viewport.width() > 500.0,
                    "scroll viewport must fill widened pane"
                );
            }
        }
    }

    #[test]
    fn dragging_key_value_separator_resizes_and_preserves_width() {
        for dark in [false, true] {
            let context = egui::Context::default();
            context.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            let mut doc = RedisDocument::fixture(&context);
            let mut time = 0.0;
            let mut frame = |events| {
                time += 0.1;
                let mut rect = egui::Rect::NOTHING;
                let _ = context.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1000.0, 700.0),
                        )),
                        events,
                        time: Some(time),
                        ..Default::default()
                    },
                    |context| {
                        egui::CentralPanel::default().show(context, |ui| {
                            rect = doc.key_value_workspace(ui, 600.0).0;
                        });
                    },
                );
                rect
            };
            let initial = frame(Vec::new());
            let start = initial.center();
            let pointer = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            frame(vec![egui::Event::PointerMoved(start), pointer(start, true)]);
            let moved = start + egui::vec2(160.0, 0.0);
            frame(vec![egui::Event::PointerMoved(moved)]);
            let grown = frame(vec![egui::Event::PointerMoved(moved)]);
            assert!(
                grown.center().x > initial.center().x + 100.0,
                "separator must be draggable"
            );
            frame(vec![pointer(moved, false)]);
            let persisted = frame(Vec::new());
            assert!((persisted.center().x - grown.center().x).abs() < 2.0);
            assert!(
                persisted.center().x <= 760.0,
                "value pane must retain usable width"
            );
        }
    }

    #[test]
    fn namespace_folder_context_menu_requests_preview_not_immediate_delete() {
        let context = egui::Context::default();
        let mut tree = KeyTree::default();
        tree.insert(b"app:users:1");
        let mut requested = None;
        let mut time = 0.0;
        let mut frame = |events| {
            time += 0.1;
            context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 700.0),
                    )),
                    events,
                    time: Some(time),
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        draw_tree(
                            ui,
                            &tree,
                            None,
                            &mut Vec::new(),
                            &mut None,
                            &mut requested,
                            0,
                        );
                    });
                },
            )
        };
        fn text_position(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) if text.galley.text().contains(label) => {
                        Some(text.pos + egui::vec2(10.0, 5.0))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing label: {label}"))
        }
        let output = frame(vec![]);
        let position = text_position(&output, "app  (1)");
        let pointer = |pos, button, pressed| egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![
            egui::Event::PointerMoved(position),
            pointer(position, egui::PointerButton::Secondary, true),
        ]);
        frame(vec![pointer(
            position,
            egui::PointerButton::Secondary,
            false,
        )]);
        let output = frame(vec![]);
        let delete = text_position(&output, "删除整个命名空间");
        frame(vec![
            egui::Event::PointerMoved(delete),
            pointer(delete, egui::PointerButton::Primary, true),
        ]);
        frame(vec![pointer(delete, egui::PointerButton::Primary, false)]);
        assert_eq!(requested, Some(b"app:".to_vec()));
    }

    #[test]
    fn namespace_paths_keep_binary_literal_delimiters() {
        assert_eq!(
            namespace_prefix(&[b"app".to_vec(), b"users".to_vec()]),
            b"app:users:"
        );
        assert_eq!(
            namespace_prefix(&[b"a*?[x]\\\\".to_vec(), vec![0xff]]),
            b"a*?[x]\\\\:\xff:"
        );
        assert_eq!(namespace_prefix(&[Vec::new()]), b":");
    }

    #[test]
    fn namespace_delete_requires_a_clean_stable_database() {
        let context = egui::Context::default();
        let mut doc = RedisDocument::fixture(&context);
        doc.value_text.push('x');
        assert!(!doc.preview_namespace(b"app:".to_vec()));
        assert!(!doc.busy);
        doc.value_text = doc.value_original.clone();
        doc.confirmation = Some(Confirmation::Namespace {
            token: 1,
            prefix: b"app:".to_vec(),
            count: 20,
            database: 0,
        });
        assert!(!doc.select_database(1));
        assert!(!doc.disconnect());
        assert!(!doc.preview_namespace(b"cache:".to_vec()));
        assert!(!doc.busy);
    }

    #[test]
    fn namespaces_keep_binary_and_parent_keys_distinct() {
        let mut tree = KeyTree::default();
        for key in [b"a".as_slice(), b"a:b", b"a::b", b"a:\xff", b""] {
            tree.insert(key);
        }
        assert_eq!(tree.count, 5);
        assert_eq!(tree.children[b"a".as_slice()].key, Some(b"a".to_vec()));
        assert_eq!(tree.children[b"a".as_slice()].count, 4);
        assert!(
            tree.children[b"a".as_slice()]
                .children
                .contains_key(b"\xff".as_slice())
        );
        assert_eq!(tree.children[b"".as_slice()].key, Some(Vec::new()));
    }

    #[test]
    fn bound_document_uses_saved_connection_without_connecting_or_configuration_ui() {
        let context = egui::Context::default();
        let profile = RedisProfile {
            name: "业务 Redis".into(),
            host: "cache.example.test".into(),
            database: 3,
            ..Default::default()
        };
        let mut doc = RedisDocument::new_bound(&context, profile.clone(), Credentials::default());
        assert_eq!(doc.profile_id(), profile.id);
        assert_eq!(doc.bound_profile().host, "cache.example.test");
        assert_eq!(doc.database_index(), 0);
        assert!(!doc.is_connected());
        assert!(!doc.busy);
        let output = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| doc.show(ui));
        });
        fn collect(shape: &egui::epaint::Shape, text: &mut String) {
            match shape {
                egui::epaint::Shape::Text(shape) => text.push_str(shape.galley.text()),
                egui::epaint::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, text);
                    }
                }
                _ => {}
            }
        }
        let mut text = String::new();
        for shape in &output.shapes {
            collect(&shape.shape, &mut text);
        }
        assert!(text.contains("业务 Redis"));
        assert!(text.contains("连接 Redis"));
        assert!(!text.contains("保存连接"));
        assert!(!text.contains("ACL 用户名"));
        assert!(!doc.busy);
    }

    #[test]
    fn connection_lifecycle_rejects_busy_dirty_and_pending_confirmation() {
        let context = egui::Context::default();
        let mut doc = RedisDocument::fixture(&context);
        doc.value_text.push_str("draft");
        assert!(!doc.connect());
        assert!(!doc.disconnect());
        assert!(!doc.select_database(1));
        assert_eq!(doc.database_index(), 0);
        doc.value_text = doc.value_original.clone();
        doc.confirmation = Some(Confirmation::Mutation {
            key: b"key".to_vec(),
            change: RedisMutation::Delete,
            label: "删除".into(),
        });
        assert!(!doc.disconnect());
        assert!(!doc.select_database(1));
        doc.confirmation = None;
        doc.busy = true;
        assert!(!doc.connect());
        assert!(!doc.disconnect());
    }

    #[test]
    fn synchronous_validation_failure_keeps_string_draft_editable() {
        let context = egui::Context::default();
        let mut doc = RedisDocument::fixture(&context);
        doc.value_text = "x".repeat(sqlshop::redis_database::MAX_VALUE_BYTES + 1);
        doc.mutate(
            b"app:users:1001".to_vec(),
            RedisMutation::SetString {
                value: doc.value_text.as_bytes().to_vec(),
                expected: vec![],
            },
        );
        assert!(!doc.busy);
        assert!(!doc.stale);
        assert!(doc.is_dirty());
        assert!(doc.error.is_some());
    }

    #[test]
    fn formatting_json_preserves_high_precision_numbers() {
        let json = r#"{"amount":0.123456789012345678901,"id":184467440737095516160}"#;
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        let formatted = serde_json::to_string_pretty(&value).unwrap();
        assert!(formatted.contains("0.123456789012345678901"));
        assert!(formatted.contains("184467440737095516160"));
    }

    #[test]
    fn editor_hex_roundtrip_and_json_validation() {
        let bytes = b"\x00\xff\"\n";
        assert_eq!(
            ValueFormat::Hex
                .bytes(&ValueFormat::Hex.show(bytes))
                .unwrap(),
            bytes
        );
        assert!(ValueFormat::Json.bytes("{invalid}").is_err());
        assert_eq!(ValueFormat::Text.bytes("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn disconnected_draft_blocks_reconnect_and_database_changes() {
        let context = egui::Context::default();
        let mut doc = RedisDocument::fixture(&context);
        doc.connected = false;
        doc.value_text.push_str("\nunsaved");
        assert!(doc.is_dirty());
        doc.request(RedisRequest::Connect {
            profile: doc.profile.clone(),
            credentials: Credentials::default(),
        });
        assert!(!doc.busy);
        assert!(doc.error.as_deref().unwrap().contains("草稿"));
        doc.request(RedisRequest::SelectDb(1));
        assert!(!doc.busy);
        assert_eq!(doc.database, 0);
        assert!(doc.value_text.ends_with("unsaved"));
    }

    #[test]
    fn deeply_nested_keys_have_bounded_tree_depth() {
        let key = "a:".repeat(10_000).into_bytes();
        let mut tree = KeyTree::default();
        tree.insert(&key);
        let mut node = &tree;
        let mut depth = 0;
        while let Some(child) = node.children.values().next() {
            node = child;
            depth += 1;
        }
        assert_eq!(depth, 25);
        assert_eq!(node.key.as_deref(), Some(key.as_slice()));
    }

    #[test]
    fn pending_confirmation_does_not_render_other_database_controls() {
        for dark in [false, true] {
            let context = egui::Context::default();
            context.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            let mut doc = RedisDocument::fixture(&context);
            doc.confirmation = Some(Confirmation::Mutation {
                key: b"app:users:1001".to_vec(),
                change: RedisMutation::Delete,
                label: "删除目标键".into(),
            });
            let output = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1100.0, 720.0),
                    )),
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| doc.show(ui));
                },
            );
            fn text(shape: &egui::epaint::Shape, labels: &mut String) {
                match shape {
                    egui::epaint::Shape::Text(text) => labels.push_str(text.galley.text()),
                    egui::epaint::Shape::Vec(shapes) => {
                        for shape in shapes {
                            text(shape, labels);
                        }
                    }
                    _ => {}
                }
            }
            let mut labels = String::new();
            for shape in &output.shapes {
                text(&shape.shape, &mut labels);
            }
            assert!(labels.contains("确认写入"));
            assert!(!labels.contains("DB 索引由服务器验证"));
            assert!(!labels.contains("保存值（保留 TTL）"));
            assert!(doc.confirmation.is_some());
            assert!(!doc.busy);
        }
    }

    #[test]
    fn member_changes_are_dirty_and_binary_remains_hex() {
        let mut member = MemberDraft::new(0, vec![], vec![0xff], 1.0);
        assert!(member.hex);
        assert!(!member.dirty());
        assert_eq!(member.bytes().unwrap(), vec![0xff]);
        member.score = 2.0;
        assert!(member.dirty());
    }
}
