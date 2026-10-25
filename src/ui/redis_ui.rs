mod dialogs;

mod values;

mod browser;

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
mod tests;
