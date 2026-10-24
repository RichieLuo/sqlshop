use super::workspace_ui::SqlReplacement;
use super::*;
use sqlshop::local_database::{LocalDatabaseWorker, LocalRequest};
use sqlshop::workspace::SavedEngineDocument;

pub(super) struct LocalDocument {
    worker: Option<LocalDatabaseWorker>,
    pub path: PathBuf,
    pub pending: Option<Uuid>,
    pub sql: String,
    pub saved_sql: String,
    file_path: Option<PathBuf>,
    pub output: Option<QueryOutput>,
    pub error: Option<String>,
    pub objects: Vec<(String, String, String)>,
    pub in_transaction: bool,
    confirmed_sql: Option<String>,
    replacement: Option<SqlReplacement>,
}
impl LocalDocument {
    pub fn new(path: PathBuf, ctx: egui::Context) -> Self {
        let mut doc = Self {
            worker: Some(LocalDatabaseWorker::start(path.clone(), move || {
                ctx.request_repaint()
            })),
            path,
            pending: None,
            sql: "SELECT sqlite_version() AS SQLite;".into(),
            saved_sql: "SELECT sqlite_version() AS SQLite;".into(),
            file_path: None,
            output: None,
            error: None,
            objects: Vec::new(),
            in_transaction: false,
            confirmed_sql: None,
            replacement: None,
        };
        if std::env::var_os("SQLSHOP_SCREENSHOT").is_none() {
            doc.run();
        }
        doc
    }
    /// Restoring a draft never opens the database or runs its saved SQL.
    pub(super) fn restore(path: PathBuf, saved: &SavedTab) -> Self {
        Self {
            worker: None,
            path,
            pending: None,
            sql: saved.sql.clone(),
            saved_sql: saved.saved_sql.clone(),
            file_path: saved.file_path.clone(),
            output: None,
            error: None,
            objects: Vec::new(),
            in_transaction: false,
            confirmed_sql: None,
            replacement: None,
        }
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
            document: Some(SavedEngineDocument::Sqlite {
                database_path: self.path.clone(),
            }),
        }
    }
    pub(super) fn matches_saved(&self, saved: &SavedTab) -> bool {
        saved.sql == self.sql
            && saved.saved_sql == self.saved_sql
            && saved.file_path == self.file_path
            && saved.target.is_none()
            && saved.saved_query.is_none()
            && matches!(&saved.document, Some(SavedEngineDocument::Sqlite { database_path }) if database_path == &self.path)
    }
    fn replace_sql(&mut self, replacement: SqlReplacement) {
        if self.sql != self.saved_sql {
            self.replacement = Some(replacement);
        } else {
            self.replacement = None;
            replacement.apply(&mut self.sql, &mut self.saved_sql, &mut self.file_path);
            self.confirmed_sql = None;
        }
    }
    fn resolve_replacement(&mut self, confirm: bool) {
        if let Some(replacement) = self.replacement.take()
            && confirm
        {
            replacement.apply(&mut self.sql, &mut self.saved_sql, &mut self.file_path);
            self.confirmed_sql = None;
        }
    }
    fn save_sql_file(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
        write_sql_file(path, &self.sql)?;
        self.saved_sql = self.sql.clone();
        self.file_path = Some(path.to_owned());
        Ok(())
    }
    pub fn dirty(&self) -> bool {
        self.sql != self.saved_sql || self.in_transaction
    }
    fn run(&mut self) {
        if self.pending.is_some() || self.replacement.is_some() {
            return;
        }
        let Some(worker) = &self.worker else {
            self.error = Some("草稿已恢复；请先手动连接 SQLite 文件，再执行 SQL".into());
            return;
        };
        let generation = Uuid::new_v4();
        let confirmed = self.confirmed_sql.as_ref() == Some(&self.sql);
        match worker.send(LocalRequest {
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
        while let Some(response) = self.worker.as_ref().and_then(LocalDatabaseWorker::try_recv) {
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
    if doc.worker.is_none() {
        ui.weak("SQL 草稿已恢复 · 文件尚未连接，未执行 SQL");
        if ui.button("连接 SQLite 文件").clicked() {
            let context = ui.ctx().clone();
            doc.worker = Some(LocalDatabaseWorker::start(doc.path.clone(), move || {
                context.request_repaint()
            }));
        }
    }
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                doc.worker.is_some() && doc.pending.is_none() && doc.replacement.is_none(),
                egui::Button::new("▶ 运行一条语句"),
            )
            .on_hover_text("一次执行一条语句，最多显示 1000 行。")
            .clicked()
        {
            doc.run();
        }
        if ui
            .add_enabled(doc.pending.is_some(), egui::Button::new("■ 停止"))
            .clicked()
            && let Some(generation) = doc.pending
            && let Some(worker) = &doc.worker
            && let Err(error) = worker.interrupt(generation)
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
            match doc.save_sql_file(&path) {
                Ok(()) => {}
                Err(error) => doc.error = Some(error.to_string()),
            }
        }
    });
    if let Some(action) = doc
        .replacement
        .as_ref()
        .and_then(|replacement| replacement.confirm(ui))
    {
        doc.resolve_replacement(action);
    }
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
                        doc.replace_sql(SqlReplacement::Generated(sql));
                    }
                },
            );
            if let Some(output) = &doc.output {
                super::management_ui::output(ui, output);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn restored(path: PathBuf) -> LocalDocument {
        let saved = SavedTab {
            id: Uuid::new_v4(),
            name: "SQLite 草稿".into(),
            sql: "DROP TABLE keep_me; -- 未执行的草稿".into(),
            saved_sql: "SELECT 1;".into(),
            file_path: None,
            target: None,
            saved_query: None,
            document: Some(SavedEngineDocument::Sqlite {
                database_path: path.clone(),
            }),
        };
        LocalDocument::restore(path, &saved)
    }

    #[test]
    fn sqlite_restored_draft_never_opens_or_executes_until_requested() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("资料.sqlite");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute("CREATE TABLE keep_me(id INTEGER)", [])
            .unwrap();
        let mut doc = restored(path);
        doc.poll();
        doc.run();
        assert!(doc.worker.is_none());
        assert!(doc.pending.is_none());
        assert!(doc.output.is_none());
        assert!(!doc.in_transaction);
        assert!(doc.dirty());
        assert!(doc.error.as_deref().unwrap().contains("手动连接"));
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM keep_me", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        let missing = directory.path().join("missing.sqlite");
        let mut unopened = restored(missing.clone());
        unopened.poll();
        assert!(
            !missing.exists(),
            "draft recovery must not create a database file"
        );
    }

    #[test]
    fn sqlite_object_sql_requires_explicit_replacement_of_dirty_draft() {
        let mut doc = restored(PathBuf::from("database.sqlite"));
        let original = doc.sql.clone();
        let baseline = doc.saved_sql.clone();
        doc.confirmed_sql = Some(original.clone());
        doc.replace_sql(SqlReplacement::Generated(
            "SELECT * FROM items LIMIT 200;".into(),
        ));
        assert_eq!(doc.sql, original);
        assert!(doc.replacement.is_some());
        doc.resolve_replacement(false);
        assert_eq!(doc.sql, original);
        assert_eq!(doc.saved_sql, baseline);
        doc.replace_sql(SqlReplacement::Generated(
            "SELECT * FROM items LIMIT 200;".into(),
        ));
        doc.resolve_replacement(true);
        assert_eq!(doc.sql, "SELECT * FROM items LIMIT 200;");
        assert_eq!(doc.saved_sql, baseline);
        assert!(doc.confirmed_sql.is_none());
        assert!(doc.dirty());
    }

    #[test]
    fn sqlite_file_save_updates_baseline_only_after_success() {
        let directory = tempfile::tempdir().unwrap();
        let mut doc = restored(directory.path().join("database.sqlite"));
        let baseline = doc.saved_sql.clone();
        assert!(doc.save_sql_file(directory.path()).is_err());
        assert_eq!(doc.saved_sql, baseline);
        assert!(doc.file_path.is_none());
        let path = directory.path().join("草稿.sql");
        doc.save_sql_file(&path).unwrap();
        assert_eq!(read_sql_file(&path).unwrap(), doc.sql);
        assert!(!doc.dirty());
        let saved = doc.saved_tab(Uuid::new_v4(), "SQLite");
        let restored = LocalDocument::restore(doc.path.clone(), &saved);
        assert!(restored.matches_saved(&saved));
        assert!(!restored.dirty());
        assert_eq!(restored.file_path.as_ref(), Some(&path));
    }
}
