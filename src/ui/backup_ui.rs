use eframe::egui::{self, RichText};
use sqlshop::{
    backup::{
        self, BackupFileMove, BackupFormat, BackupLocation, BackupOperation, BackupResult,
        DraftBackupConfig,
    },
    backup_worker::{BackupResponse, BackupResponseKind},
    model::{Engine, Profile},
};
use std::path::PathBuf;
use uuid::Uuid;

pub struct BackupDocument {
    pub id: Uuid,
    pub profile: Profile,
    pub config: DraftBackupConfig,
    pub pending: Option<Uuid>,
    pub tool_status: Option<String>,
    pub error: Option<String>,
    pub result: Option<BackupResult>,
    preview: Option<String>,
    preview_fingerprint: Option<String>,
    confirmed: bool,
}
impl BackupDocument {
    pub fn new(id: Uuid, profile: Profile, database: impl Into<String>) -> Self {
        Self {
            id,
            config: DraftBackupConfig::for_database(&profile, database),
            profile,
            pending: None,
            tool_status: None,
            error: None,
            result: None,
            preview: None,
            preview_fingerprint: None,
            confirmed: false,
        }
    }
    fn fingerprint(&self) -> String {
        // Profile serializes connection settings only; no credential material.
        serde_json::to_string(&(&self.profile, &self.config)).unwrap_or_default()
    }
    pub fn apply_response(&mut self, response: BackupResponse) -> bool {
        if response.document != self.id
            || response.profile != self.profile.id
            || response.database != self.config.database
            || self.pending != Some(response.generation)
            || response.source_fingerprint != self.fingerprint()
        {
            return false;
        }
        self.pending = None;
        match response.kind {
            BackupResponseKind::ToolDetected(Ok(text)) => {
                self.tool_status = Some(text);
                self.error = None;
            }
            BackupResponseKind::ToolDetected(Err(error))
            | BackupResponseKind::Finished(Err(error)) => {
                self.error = Some(error);
            }
            BackupResponseKind::Finished(Ok(result)) => {
                self.result = Some(result);
                self.error = None;
            }
        }
        true
    }
    pub fn dispatch_failed(&mut self, generation: Uuid, error: String) {
        if self.pending == Some(generation) {
            self.pending = None;
            self.error = Some(error);
        }
    }
}

pub enum BackupAction {
    Detect {
        generation: Uuid,
        config: DraftBackupConfig,
    },
    Execute {
        generation: Uuid,
        config: DraftBackupConfig,
    },
}

pub fn show(ui: &mut egui::Ui, document: &mut BackupDocument) -> Option<BackupAction> {
    let mut action = None;
    ui.push_id(document.id, |ui| {
        ui.heading("原生备份与恢复");
        ui.label(format!("{} · {} · {}:{}", document.profile.name, document.profile.engine.label(), document.profile.host, document.profile.port));
        ui.separator();
        let before = document.fingerprint();
        let busy = document.pending.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            egui::Grid::new("backup_config").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                ui.label("操作");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut document.config.operation, BackupOperation::Create, "创建备份");
                    ui.selectable_value(&mut document.config.operation, BackupOperation::Restore, "恢复备份");
                });
                ui.end_row();
                ui.label("目标数据库");
                ui.add(egui::TextEdit::singleline(&mut document.config.database).desired_width(280.0));
                ui.end_row();
                ui.label("格式");
                match document.profile.engine {
                    Engine::MySql => { document.config.format = BackupFormat::Sql; ui.label("MySQL SQL 脚本"); },
                    Engine::Postgres => {
                        egui::ComboBox::from_id_salt("backup_format")
                            .selected_text(if document.config.format == BackupFormat::PostgresCustom { "PostgreSQL 自定义归档" } else { "SQL 脚本" })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut document.config.format, BackupFormat::PostgresCustom, "PostgreSQL 自定义归档");
                                ui.selectable_value(&mut document.config.format, BackupFormat::Sql, "SQL 脚本");
                            });
                    },
                    Engine::SqlServer => { document.config.format = BackupFormat::SqlServerNative; ui.label("SQL Server 原生媒体文件"); },
                }
                ui.end_row();
                ui.label(if document.profile.engine == Engine::SqlServer { "服务器文件路径" } else { "本机文件路径" });
                let mut path = match &document.config.location {
                    BackupLocation::LocalFile(path) => path.to_string_lossy().into_owned(),
                    BackupLocation::ServerFile(path) => path.clone(),
                };
                if ui.add(egui::TextEdit::singleline(&mut path).desired_width(420.0).hint_text(if document.profile.engine == Engine::SqlServer { "数据库服务器可读写的路径" } else { "本机完整文件路径" })).changed() {
                    document.config.location = if document.profile.engine == Engine::SqlServer { BackupLocation::ServerFile(path) } else { BackupLocation::LocalFile(PathBuf::from(path)) };
                }
                ui.end_row();
                if document.profile.engine != Engine::SqlServer {
                    ui.label("原生工具路径");
                    let mut tool = document.config.tool_path.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                    if ui.add(egui::TextEdit::singleline(&mut tool).desired_width(420.0).hint_text("留空使用 PATH；可填工具完整路径")).changed() {
                        document.config.tool_path = if tool.trim().is_empty() { None } else { Some(PathBuf::from(tool)) };
                    }
                    ui.end_row();
                } else if document.config.operation == BackupOperation::Restore {
                    ui.label("备份集序号");
                    ui.add(egui::DragValue::new(&mut document.config.backup_set).range(1..=i32::MAX as u32));
                    ui.end_row();
                }
            });
            if document.profile.engine == Engine::SqlServer && document.config.operation == BackupOperation::Restore {
                ui.add_space(10.0);
                ui.label("恢复文件映射（可选 MOVE）").on_hover_text("未映射的文件沿用原位置；请核对备份集序号和逻辑文件名。");
                let mut remove = None;
                for (index, mapping) in document.config.file_moves.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut mapping.logical_name).desired_width(170.0).hint_text("备份中的逻辑文件名"));
                        ui.add(egui::TextEdit::singleline(&mut mapping.server_path).desired_width(380.0).hint_text("服务器目标文件路径"));
                        if ui.small_button("移除").clicked() { remove = Some(index); }
                    });
                }
                if let Some(index) = remove { document.config.file_moves.remove(index); }
                if ui.button("＋ 添加文件映射").clicked() { document.config.file_moves.push(BackupFileMove { logical_name: String::new(), server_path: String::new() }); }
            } else if document.config.operation == BackupOperation::Create {
                document.config.file_moves.clear();
            }
        });
        if before != document.fingerprint() {
            document.preview = None; document.preview_fingerprint = None; document.confirmed = false;
            document.result = None; document.error = None; document.tool_status = None;
        }
        if document.preview_fingerprint.as_ref().is_some_and(|fingerprint| *fingerprint != document.fingerprint()) {
            document.preview = None; document.preview_fingerprint = None; document.confirmed = false;
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new("检测原生工具")).clicked() {
                let generation = Uuid::new_v4();
                document.pending = Some(generation); document.error = None;
                action = Some(BackupAction::Detect { generation, config: document.config.clone() });
            }
            if ui.add_enabled(!busy, egui::Button::new("预览操作")).clicked() {
                match backup::preview(&document.profile, &document.config) {
                    Ok(preview) => { document.preview = Some(preview); document.preview_fingerprint = Some(document.fingerprint()); document.confirmed = false; document.error = None; },
                    Err(error) => { document.error = Some(format!("{error:#}")); document.preview = None; document.preview_fingerprint = None; },
                }
            }
            if busy { ui.spinner(); ui.label("执行中…"); }
        });
        if let Some(status) = &document.tool_status { ui.label(status); }
        if let Some(preview) = &document.preview {
            ui.add_space(12.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label(RichText::new("操作预览").strong());
                egui::ScrollArea::vertical().id_salt("backup_preview").max_height(220.0).show(ui, |ui| { ui.monospace(preview); });
            });
            ui.add_enabled_ui(!busy, |ui| {
                ui.checkbox(&mut document.confirmed, if document.config.operation == BackupOperation::Restore {
                    "已核对恢复文件、目标空数据库并关闭该库写会话"
                } else { "已核对备份数据库、文件位置和操作" });
            });
            if ui.add_enabled(!busy && document.confirmed, egui::Button::new(if document.config.operation == BackupOperation::Restore { "执行恢复" } else { "开始备份" })).clicked() {
                let generation = Uuid::new_v4();
                document.pending = Some(generation); document.confirmed = false; document.error = None; document.result = None;
                action = Some(BackupAction::Execute { generation, config: document.config.clone() });
            }
        }
        if let Some(error) = &document.error { ui.add_space(10.0); ui.colored_label(ui.visuals().error_fg_color, error); }
        if let Some(result) = &document.result {
            ui.add_space(10.0);
            ui.label(RichText::new(&result.summary).strong());
            match &result.artifact {
                BackupLocation::LocalFile(path) => { ui.monospace(path.display().to_string()); },
                BackupLocation::ServerFile(path) => { ui.label(format!("服务器文件：{path}")); },
            }
            if let Some(bytes) = result.local_bytes { ui.label(format!("已验证本机文件：{bytes} 字节")); }
        }
    });
    action
}
