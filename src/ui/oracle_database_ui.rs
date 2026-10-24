use super::workspace_ui::SqlReplacement;
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use sqlshop::{
    model::QueryOutput,
    oracle_database::{
        OracleData, OracleObject, OracleRequest, OracleResponse, OracleSettings, OracleTransaction,
        OracleWorker,
    },
    workspace::{SavedEngineDocument, SavedTab, read_sql_file, write_sql_file},
};
use std::path::PathBuf;
use uuid::Uuid;

const INITIAL_SQL: &str = "SELECT 1 AS hello FROM dual";

pub(super) struct OracleDocument {
    worker: OracleWorker,
    pub busy: bool,
    pub connected: bool,
    pub transaction: OracleTransaction,
    generation: u64,
    username: String,
    password: String,
    connect_string: String,
    schema: String,
    sql: String,
    saved_sql: String,
    file_path: Option<PathBuf>,
    output: Option<QueryOutput>,
    result_index: usize,
    objects: Vec<OracleObject>,
    objects_truncated: bool,
    catalog_stale: bool,
    filter: String,
    selected: Option<usize>,
    definition: Option<(OracleObject, String, bool)>,
    page: usize,
    pending_sql: Option<String>,
    status: String,
    error: Option<String>,
    context: egui::Context,
    replacement: Option<SqlReplacement>,
}
impl OracleDocument {
    pub(super) fn new(context: &egui::Context) -> Self {
        let wake = context.clone();
        Self {
            worker: OracleWorker::start(move || wake.request_repaint()),
            busy: false,
            connected: false,
            transaction: OracleTransaction::Idle,
            generation: 0,
            username: String::new(),
            password: String::new(),
            connect_string: "localhost:1521/FREEPDB1".into(),
            schema: String::new(),
            sql: INITIAL_SQL.into(),
            saved_sql: INITIAL_SQL.into(),
            file_path: None,
            output: None,
            result_index: 0,
            objects: Vec::new(),
            objects_truncated: false,
            catalog_stale: false,
            filter: String::new(),
            selected: None,
            definition: None,
            page: 0,
            pending_sql: None,
            status: "Oracle · 填写连接后手动连接".into(),
            error: None,
            context: context.clone(),
            replacement: None,
        }
    }
    /// Only connection labels are restored. Authentication and execution always
    /// require a new explicit user action.
    pub(super) fn restore(context: &egui::Context, saved: &SavedTab) -> Self {
        let mut doc = Self::new(context);
        if let Some(SavedEngineDocument::Oracle {
            username,
            connect_string,
            schema,
        }) = &saved.document
        {
            doc.username = username.clone();
            doc.connect_string = connect_string.clone();
            doc.schema = schema.clone();
        }
        doc.sql = saved.sql.clone();
        doc.saved_sql = saved.saved_sql.clone();
        doc.file_path = saved.file_path.clone();
        doc.page = 1;
        doc.status = "Oracle SQL 草稿已恢复 · 未连接、未执行；请重新填写密码后手动连接".into();
        doc
    }
    pub(super) fn saved_tab(&self, id: Uuid, name: &str) -> SavedTab {
        SavedTab {
            id,
            name: name.into(),
            sql: self.sql.clone(),
            saved_sql: self.saved_sql.clone(),
            file_path: self.file_path.clone(),
            target: None,
            saved_query: None,
            document: Some(SavedEngineDocument::Oracle {
                username: self.username.clone(),
                connect_string: self.connect_string.clone(),
                schema: self.schema.clone(),
            }),
        }
    }
    pub(super) fn matches_saved(&self, saved: &SavedTab) -> bool {
        saved.sql == self.sql
            && saved.saved_sql == self.saved_sql
            && saved.file_path == self.file_path
            && saved.target.is_none()
            && saved.saved_query.is_none()
            && matches!(&saved.document, Some(SavedEngineDocument::Oracle { username, connect_string, schema })
                if username == &self.username && connect_string == &self.connect_string && schema == &self.schema)
    }
    fn replace_sql(&mut self, replacement: SqlReplacement) {
        self.page = 1;
        if self.is_dirty() {
            self.replacement = Some(replacement);
        } else {
            self.replacement = None;
            replacement.apply(&mut self.sql, &mut self.saved_sql, &mut self.file_path);
            self.pending_sql = None;
        }
    }
    fn resolve_replacement(&mut self, confirm: bool) {
        if let Some(replacement) = self.replacement.take()
            && confirm
        {
            replacement.apply(&mut self.sql, &mut self.saved_sql, &mut self.file_path);
            self.pending_sql = None;
        }
    }
    fn open_sql_file(&mut self, path: PathBuf) -> anyhow::Result<()> {
        let sql = read_sql_file(&path)?;
        self.replace_sql(SqlReplacement::File { path, sql });
        Ok(())
    }
    fn save_sql_file(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
        write_sql_file(path, &self.sql)?;
        self.saved_sql = self.sql.clone();
        self.file_path = Some(path.to_owned());
        Ok(())
    }
    pub(super) fn is_dirty(&self) -> bool {
        self.sql != self.saved_sql
    }
    pub(super) fn may_have_transaction(&self) -> bool {
        self.connected && self.transaction.may_be_active()
    }
    pub(super) fn poll(&mut self) {
        while let Ok(response) = self.worker.receiver.try_recv() {
            match response {
                OracleResponse::Finished {
                    generation,
                    data,
                    connected,
                    transaction,
                } if generation == self.generation => {
                    self.busy = false;
                    self.connected = connected;
                    self.transaction = transaction;
                    self.error = None;
                    match data {
                        OracleData::Connected { schema } => {
                            self.schema = schema;
                            self.catalog_stale = true;
                            self.status = "Oracle 已连接 · 自动提交关闭；目录请手动刷新".into();
                        }
                        OracleData::Disconnected => {
                            self.catalog_stale = true;
                            self.pending_sql = None;
                            self.status = "Oracle 已断开；未提交事务已随连接关闭".into();
                        }
                        OracleData::Catalog { objects, truncated } => {
                            self.objects = objects;
                            self.objects_truncated = truncated;
                            self.catalog_stale = false;
                            self.selected = None;
                            self.status = format!("已读取 {} 个 Oracle 对象", self.objects.len());
                        }
                        OracleData::Output(output) => {
                            self.status = format!(
                                "执行完成 · {} ms · {} 个结果",
                                output.elapsed_ms,
                                output.sets.len()
                            );
                            self.output = Some(output);
                            self.result_index = 0;
                            self.pending_sql = None;
                            self.page = 1;
                        }
                        OracleData::Definition {
                            object,
                            text,
                            truncated,
                        } => {
                            self.definition = Some((object, text, truncated));
                            self.page = 2;
                            self.status = "已读取 Oracle 对象定义，只读预览".into();
                        }
                        OracleData::ConfirmSql { sql, kind } => {
                            self.pending_sql = Some(sql);
                            self.page = 1;
                            self.status = format!("{kind} · 需要明确确认才执行；尚未写入");
                        }
                        OracleData::Transaction => {
                            self.status = "Oracle 事务操作已完成；当前状态来自服务器".into()
                        }
                    }
                }
                OracleResponse::Failed {
                    generation,
                    message,
                    connected,
                    transaction,
                } if generation == self.generation => {
                    self.busy = false;
                    self.connected = connected;
                    self.transaction = transaction;
                    self.pending_sql = None;
                    self.error = Some(message);
                    self.status = "Oracle 操作失败 · 没有重连或重放 SQL".into();
                    if !connected {
                        self.catalog_stale = true;
                    }
                }
                OracleResponse::StopRequested {
                    generation,
                    message,
                } if generation == self.generation => {
                    // Sending break is not proof that the server canceled the SQL.
                    self.status = message;
                }
                _ => {}
            }
        }
    }
    fn request(&mut self, request: OracleRequest) {
        if self.busy || std::env::var_os("SQLSHOP_SCREENSHOT").is_some() {
            return;
        }
        let generation = self.generation.wrapping_add(1);
        let query = matches!(&request, OracleRequest::Run { .. });
        let catalog = matches!(&request, OracleRequest::Catalog { .. });
        match self.worker.send(generation, request) {
            Ok(()) => {
                self.generation = generation;
                self.busy = true;
                self.error = None;
                self.status = "Oracle 正在执行…".into();
                if query {
                    self.output = None;
                }
                if catalog {
                    self.catalog_stale = true;
                }
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }
    pub(super) fn show(&mut self, ui: &mut egui::Ui) {
        self.poll();
        ui.spacing_mut().item_spacing.y = 5.0;
        ui.spacing_mut().button_padding.y = 4.0;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Oracle");
            if self.busy {
                ui.spinner();
            }
        });
        let mut connect = false;
        if self.connected {
            ui.weak(format!("{} · {}", self.username, self.connect_string));
        } else {
            ui.add_enabled_ui(!self.busy && !self.connected, |ui| {
                egui::Grid::new("oracle-connection")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("服务 / TNS");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.connect_string)
                                .desired_width(ui.available_width())
                                .hint_text("host:1521/service、TNS 别名或完整描述符"),
                        );
                        ui.end_row();
                        ui.label("用户名");
                        ui.text_edit_singleline(&mut self.username);
                        ui.end_row();
                        ui.label("密码");
                        ui.add(egui::TextEdit::singleline(&mut self.password).password(true));
                        ui.end_row();
                    });
                connect = ui
                    .button("连接 Oracle")
                    .on_hover_text("需要本机安装架构匹配的 Oracle Instant Client。")
                    .clicked();
            });
        }
        if connect {
            self.request(OracleRequest::Connect(OracleSettings {
                username: self.username.clone(),
                password: self.password.clone(),
                connect_string: self.connect_string.clone(),
            }));
            if self.busy {
                self.password.clear();
            }
        }
        let mut operation = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(self.transaction.label());
            if ui
                .add_enabled(
                    self.connected && !self.busy,
                    egui::Button::new("提交 COMMIT"),
                )
                .clicked()
            {
                operation = Some(OracleRequest::Commit);
            }
            if ui
                .add_enabled(
                    self.connected && !self.busy,
                    egui::Button::new("回滚 ROLLBACK"),
                )
                .clicked()
            {
                operation = Some(OracleRequest::Rollback);
            }
            if ui
                .add_enabled(
                    self.connected && !self.busy,
                    egui::Button::new("核对事务状态"),
                )
                .clicked()
            {
                operation = Some(OracleRequest::Transaction);
            }
            if ui
                .add_enabled(
                    self.connected && !self.busy,
                    egui::Button::new("断开（未提交事务回滚）"),
                )
                .clicked()
            {
                operation = Some(OracleRequest::Disconnect);
            }
            if ui
                .add_enabled(self.busy && self.connected, egui::Button::new("停止"))
                .clicked()
            {
                let context = self.context.clone();
                self.worker
                    .stop(self.generation, move || context.request_repaint());
                self.status = "正在发送 Oracle 停止请求；等待服务器返回".into();
            }
        });
        if let Some(operation) = operation {
            self.request(operation);
        }
        ui.horizontal_wrapped(|ui| {
            for (index, name) in ["对象", "SQL / 结果", "定义"].into_iter().enumerate() {
                ui.selectable_value(&mut self.page, index, name);
            }
        });
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.label(RichText::new(&self.status).small().weak());
        ui.separator();
        match self.page {
            0 => self.objects(ui),
            1 => self.editor(ui),
            _ => self.definition(ui),
        }
    }
    fn objects(&mut self, ui: &mut egui::Ui) {
        let mut refresh = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("Schema");
            ui.add_enabled(
                !self.busy,
                egui::TextEdit::singleline(&mut self.schema).desired_width(160.0),
            );
            refresh = ui
                .add_enabled(self.connected && !self.busy, egui::Button::new("刷新目录"))
                .clicked();
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("搜索名称或类型")
                    .desired_width(180.0),
            );
        });
        if refresh {
            self.request(OracleRequest::Catalog {
                owner: self.schema.trim().to_owned(),
            });
        }
        if self.catalog_stale {
            ui.weak("目录未刷新或已过期。");
        }
        if self.objects_truncated {
            ui.weak("目录已截断，仅显示前 2000 项。");
        }
        let filter = self.filter.to_lowercase();
        let mut selected = None;
        if let Some(object) = self
            .selected
            .and_then(|index| self.objects.get(index))
            .cloned()
        {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        self.connected && !self.busy && !self.catalog_stale,
                        egui::Button::new("读取定义"),
                    )
                    .clicked()
                {
                    self.request(OracleRequest::Definition(object.clone()));
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("生成读取 SQL"))
                    .clicked()
                {
                    if matches!(object.kind.as_str(), "TABLE" | "VIEW" | "MATERIALIZED VIEW") {
                        let sql = format!(
                            "SELECT * FROM {}.{} WHERE ROWNUM <= 1000",
                            quote(&object.owner),
                            quote(&object.name)
                        );
                        self.replace_sql(SqlReplacement::Generated(sql));
                    } else {
                        self.error = Some("此对象不是可直接读取行的表或视图".into());
                    }
                }
            });
        }
        let visible = self
            .objects
            .iter()
            .enumerate()
            .filter(|(_, object)| {
                filter.is_empty()
                    || object.name.to_lowercase().contains(&filter)
                    || object.kind.to_lowercase().contains(&filter)
            })
            .collect::<Vec<_>>();
        egui::ScrollArea::horizontal()
            .id_salt("oracle-objects")
            .show(ui, |ui| {
                TableBuilder::new(ui)
                    .id_salt("oracle-object-grid")
                    .striped(true)
                    .resizable(true)
                    .column(Column::initial(180.0).range(100.0..=400.0))
                    .columns(Column::initial(110.0).range(60.0..=300.0), 3)
                    .column(Column::initial(170.0).range(120.0..=300.0))
                    .header(25.0, |mut header| {
                        for heading in ["名称", "类型", "Schema", "状态", "DDL 时间"] {
                            header.col(|ui| {
                                ui.strong(heading);
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(26.0, visible.len(), |mut row| {
                            let (index, object) = visible[row.index()];
                            row.set_selected(self.selected == Some(index));
                            row.col(|ui| {
                                if ui
                                    .selectable_label(self.selected == Some(index), &object.name)
                                    .clicked()
                                {
                                    selected = Some(index);
                                }
                            });
                            for value in [&object.kind, &object.owner, &object.status] {
                                row.col(|ui| {
                                    ui.add(egui::Label::new(value).truncate())
                                        .on_hover_text(value);
                                });
                            }
                            row.col(|ui| {
                                ui.label(object.last_ddl.as_deref().unwrap_or("—"));
                            });
                        });
                    });
            });
        if let Some(index) = selected {
            self.selected = Some(index);
        }
    }
    fn editor(&mut self, ui: &mut egui::Ui) {
        let mut open = false;
        let mut save = false;
        let mut run = false;
        ui.horizontal_wrapped(|ui| {
            open = ui
                .add_enabled(!self.busy, egui::Button::new("打开 SQL…"))
                .clicked();
            save = ui
                .add_enabled(!self.busy, egui::Button::new("保存 SQL…"))
                .clicked();
            run = ui
                .add_enabled(
                    self.connected && !self.busy && self.replacement.is_none(),
                    egui::Button::new("执行当前内容"),
                )
                .on_hover_text("执行一条 SQL 或完整 PL/SQL；普通 SQL 不带结尾分号，不拆分脚本。")
                .clicked();
        });
        if open
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("SQL", &["sql"])
                .pick_file()
            && let Err(error) = self.open_sql_file(path)
        {
            self.error = Some(error.to_string());
        }
        if save {
            let path = self.file_path.clone().or_else(|| {
                rfd::FileDialog::new()
                    .add_filter("SQL", &["sql"])
                    .save_file()
            });
            if let Some(path) = path
                && let Err(error) = self.save_sql_file(&path)
            {
                self.error = Some(error.to_string());
            }
        }
        if let Some(action) = self
            .replacement
            .as_ref()
            .and_then(|replacement| replacement.confirm(ui))
        {
            self.resolve_replacement(action);
        }
        let height = (ui.available_height() * 0.35).clamp(70.0, 180.0);
        if ui
            .add_enabled(
                !self.busy,
                egui::TextEdit::multiline(&mut self.sql)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .desired_rows((height / 18.0) as usize),
            )
            .changed()
        {
            self.pending_sql = None;
        }
        if run {
            self.request(OracleRequest::Run {
                sql: self.sql.clone(),
                allow_write: false,
            });
        }
        if let Some(pending) = self.pending_sql.clone() {
            ui.label("将执行以下写入或结构变更；Oracle DDL 会隐式提交已有事务。");
            egui::CollapsingHeader::new("待执行原文")
                .default_open(true)
                .show(ui, |ui| {
                    ui.label(RichText::new(&pending).monospace());
                });
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        !self.busy && self.replacement.is_none() && pending == self.sql,
                        egui::Button::new("确认执行这条语句"),
                    )
                    .clicked()
                {
                    self.request(OracleRequest::Run {
                        sql: pending,
                        allow_write: true,
                    });
                }
                if ui.button("取消执行").clicked() {
                    self.pending_sql = None;
                }
            });
        }
        let Some(output) = &self.output else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            for (index, set) in output.sets.iter().enumerate() {
                ui.selectable_value(
                    &mut self.result_index,
                    index,
                    format!("结果 {} · {} 行", index + 1, set.rows.len()),
                );
            }
        });
        for message in &output.messages {
            ui.label(format!("{}：{}", message.severity, message.text));
        }
        let Some(set) = output.sets.get(self.result_index) else {
            return;
        };
        if let Some(affected) = set.affected {
            ui.label(format!("服务器报告行数：{affected}"));
        }
        if set.truncated {
            ui.weak("结果已截断。")
                .on_hover_text("最多 1000 行、16 MiB；二进制 / LOB 单格最多 8 KiB。");
        }
        if set.columns.is_empty() {
            return;
        }
        egui::ScrollArea::horizontal()
            .id_salt("oracle-results")
            .show(ui, |ui| {
                TableBuilder::new(ui)
                    .id_salt("oracle-result-grid")
                    .striped(true)
                    .resizable(true)
                    .columns(
                        Column::initial(140.0).range(60.0..=400.0),
                        set.columns.len(),
                    )
                    .header(25.0, |mut header| {
                        for (index, column) in set.columns.iter().enumerate() {
                            header.col(|ui| {
                                ui.strong(column).on_hover_text(
                                    set.column_types.get(index).map_or("", String::as_str),
                                );
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(26.0, set.rows.len(), |mut row| {
                            for value in &set.rows[row.index()] {
                                row.col(|ui| match value {
                                    Some(value) => {
                                        ui.add(egui::Label::new(value).truncate())
                                            .on_hover_text(value);
                                    }
                                    None => {
                                        ui.weak("NULL");
                                    }
                                });
                            }
                        });
                    });
            });
    }
    fn definition(&mut self, ui: &mut egui::Ui) {
        let Some((object, text, truncated)) = &self.definition else {
            ui.label("暂无对象定义。");
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!(
                "{}.{} · {}",
                object.owner, object.name, object.kind
            ));
            if ui.button("复制定义").clicked() {
                ui.ctx().copy_text(text.clone());
            }
        });
        if *truncated {
            ui.weak("DDL 已截断（超过 16 MiB），不可用于完整恢复。");
        }
        egui::ScrollArea::both()
            .id_salt("oracle-definition")
            .show(ui, |ui| {
                ui.label(RichText::new(text).monospace());
            });
    }
}
fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oracle_draft_restore_keeps_source_but_omits_password_and_live_state() {
        let context = egui::Context::default();
        let mut doc = OracleDocument::new(&context);
        doc.username = "业务用户".into();
        doc.password = "secret-not-for-workspace".into();
        doc.connect_string = "db.example:1521/service".into();
        doc.schema = "APP".into();
        doc.sql = "UPDATE items SET value = '未保存';".into();
        doc.file_path = Some(PathBuf::from("draft.sql"));
        doc.connected = true;
        doc.transaction = OracleTransaction::Active("1.2.3".into());
        doc.pending_sql = Some(doc.sql.clone());
        let saved = doc.saved_tab(Uuid::new_v4(), "Oracle 草稿");
        let json = serde_json::to_string(&saved).unwrap();
        assert!(!json.contains("secret-not-for-workspace"));
        assert!(!json.contains("password"));
        assert!(!json.contains("1.2.3"));
        let restored = OracleDocument::restore(&context, &saved);
        assert!(restored.matches_saved(&saved));
        assert!(restored.password.is_empty());
        assert!(!restored.connected && !restored.busy);
        assert!(!restored.may_have_transaction());
        assert!(restored.pending_sql.is_none());
        assert!(restored.output.is_none());
        assert!(restored.is_dirty());
        assert!(restored.worker.receiver.try_recv().is_err());
    }

    #[test]
    fn oracle_open_file_and_object_sql_preserve_draft_until_confirmed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("next.sql");
        write_sql_file(&path, "SELECT '新文件' FROM dual").unwrap();
        let mut doc = OracleDocument::new(&egui::Context::default());
        doc.sql = "SELECT '未保存' FROM dual".into();
        doc.file_path = Some(directory.path().join("original.sql"));
        let original = doc.sql.clone();
        let original_path = doc.file_path.clone();
        let baseline = doc.saved_sql.clone();
        doc.open_sql_file(path.clone()).unwrap();
        assert_eq!(doc.sql, original);
        assert_eq!(doc.file_path, original_path);
        doc.resolve_replacement(false);
        assert_eq!(doc.sql, original);
        assert_eq!(doc.saved_sql, baseline);
        doc.replace_sql(SqlReplacement::Generated("SELECT * FROM APP.ITEMS".into()));
        assert_eq!(doc.sql, original);
        doc.resolve_replacement(false);
        doc.open_sql_file(path.clone()).unwrap();
        doc.resolve_replacement(true);
        assert_eq!(doc.sql, "SELECT '新文件' FROM dual");
        assert_eq!(doc.saved_sql, doc.sql);
        assert_eq!(doc.file_path.as_ref(), Some(&path));
        assert!(!doc.is_dirty());
        doc.replace_sql(SqlReplacement::Generated("SELECT * FROM APP.ITEMS".into()));
        assert!(doc.replacement.is_none());
        assert_eq!(doc.sql, "SELECT * FROM APP.ITEMS");
        assert!(doc.is_dirty());
    }

    #[test]
    fn oracle_file_save_retains_dirty_baseline_on_failure_and_restores_clean_save() {
        let directory = tempfile::tempdir().unwrap();
        let mut doc = OracleDocument::new(&egui::Context::default());
        doc.sql = "SELECT '已保存' FROM dual".into();
        let baseline = doc.saved_sql.clone();
        assert!(doc.save_sql_file(directory.path()).is_err());
        assert_eq!(doc.saved_sql, baseline);
        assert!(doc.file_path.is_none());
        let path = directory.path().join("oracle.sql");
        doc.save_sql_file(&path).unwrap();
        assert_eq!(read_sql_file(&path).unwrap(), doc.sql);
        assert!(!doc.is_dirty());
        let saved = doc.saved_tab(Uuid::new_v4(), "Oracle");
        let restored = OracleDocument::restore(&egui::Context::default(), &saved);
        assert!(!restored.is_dirty());
        assert!(restored.matches_saved(&saved));
        assert_eq!(restored.file_path.as_ref(), Some(&path));
    }
}
