use super::*;
use sqlshop::local_database::{LocalDatabaseWorker, LocalRequest};

pub(super) struct LocalDocument {
    pub worker: LocalDatabaseWorker,
    pub path: PathBuf,
    pub pending: Option<Uuid>,
    pub sql: String,
    pub saved_sql: String,
    pub output: Option<QueryOutput>,
    pub error: Option<String>,
    pub objects: Vec<(String, String, String)>,
    pub in_transaction: bool,
    confirmed_sql: Option<String>,
}
impl LocalDocument {
    pub fn new(path: PathBuf, ctx: egui::Context) -> Self {
        let mut doc = Self {
            worker: LocalDatabaseWorker::start(path.clone(), move || ctx.request_repaint()),
            path,
            pending: None,
            sql: "SELECT sqlite_version() AS SQLite;".into(),
            saved_sql: "SELECT sqlite_version() AS SQLite;".into(),
            output: None,
            error: None,
            objects: Vec::new(),
            in_transaction: false,
            confirmed_sql: None,
        };
        if std::env::var_os("SQLSHOP_SCREENSHOT").is_none() {
            doc.run();
        }
        doc
    }
    pub fn dirty(&self) -> bool {
        self.sql != self.saved_sql || self.in_transaction
    }
    fn run(&mut self) {
        if self.pending.is_some() {
            return;
        }
        let generation = Uuid::new_v4();
        let confirmed = self.confirmed_sql.as_ref() == Some(&self.sql);
        match self.worker.send(LocalRequest {
            generation,
            sql: self.sql.clone(),
            confirmed,
        }) {
            Ok(()) => {
                self.pending = Some(generation);
                self.error = None;
                self.confirmed_sql = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }
    pub fn poll(&mut self) {
        while let Some(response) = self.worker.try_recv() {
            if self.pending != Some(response.generation) {
                continue;
            }
            self.pending = None;
            self.output = response.output;
            self.error = response.error;
            self.objects = response.objects;
            self.in_transaction = response.in_transaction;
        }
    }
}

pub(super) fn show(ui: &mut egui::Ui, doc: &mut LocalDocument) {
    doc.poll();
    ui.heading("SQLite 文件数据库");
    ui.label(doc.path.display().to_string());
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(doc.pending.is_none(), egui::Button::new("▶ 运行一条语句"))
            .clicked()
        {
            doc.run();
        }
        if ui
            .add_enabled(doc.pending.is_some(), egui::Button::new("■ 停止"))
            .clicked()
            && let Some(generation) = doc.pending
            && let Err(error) = doc.worker.interrupt(generation)
        {
            doc.error = Some(error.to_string());
        }
        ui.small(if doc.in_transaction {
            "事务：进行中；关闭文档会回滚"
        } else {
            "事务：空闲"
        });
        if ui
            .add_enabled(doc.pending.is_none(), egui::Button::new("保存 SQL…"))
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("SQL", &["sql"])
                .save_file()
        {
            match write_sql_file(&path, &doc.sql) {
                Ok(()) => doc.saved_sql = doc.sql.clone(),
                Err(error) => doc.error = Some(error.to_string()),
            }
        }
    });
    ui.add_enabled_ui(doc.pending.is_none(), |ui| {
        if ui
            .add(
                egui::TextEdit::multiline(&mut doc.sql)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(7),
            )
            .changed()
        {
            doc.confirmed_sql = None;
        }
        let mut confirmed = doc.confirmed_sql.as_ref() == Some(&doc.sql);
        if ui
            .checkbox(&mut confirmed, "确认执行当前语句中的写入或事务操作")
            .changed()
        {
            doc.confirmed_sql = confirmed.then(|| doc.sql.clone());
        }
    });
    ui.small("每个文档保留独立 SQLite 连接。一次执行一条语句，最多显示 1000 行；不加载扩展，不解密 SQLCipher。");
    if doc.pending.is_some() {
        ui.spinner();
    }
    if let Some(error) = &doc.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt("sqlite-body")
        .show(ui, |ui| {
            ui.collapsing(
                format!("对象目录（{} 项）", doc.objects.len()),
                |ui| {
                    let mut selected = None;
                    for (name, kind, definition) in doc.objects.iter().take(1000) {
                        ui.horizontal(|ui| {
                            ui.label(kind);
                            if ui.button(name).clicked() {
                                selected = Some(if kind == "table" || kind == "view" {
                                    format!(
                                        "SELECT * FROM \"{}\" LIMIT 200;",
                                        name.replace('"', "\"\"")
                                    )
                                } else {
                                    definition.clone()
                                });
                            }
                            ui.collapsing("定义", |ui| {
                                ui.monospace(definition);
                            });
                        });
                    }
                    if doc.objects.len() > 1000 {
                        ui.label("目录超过 1000 项，当前显示已截断");
                    }
                    if let Some(sql) = selected
                        && doc.pending.is_none()
                    {
                        doc.sql = sql;
                        doc.confirmed_sql = None;
                    }
                },
            );
            if let Some(output) = &doc.output {
                super::management_ui::output(ui, output);
            }
        });
}
