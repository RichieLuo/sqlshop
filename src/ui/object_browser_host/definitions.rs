use super::*;

impl App {
    pub(in crate::ui) fn open_object_definition(
        &mut self,
        connection: Uuid,
        database: String,
        object: CatalogObject,
    ) {
        if !self.finish_object_navigation() {
            return;
        }
        let target = QueryTarget {
            connection,
            database: Some(database.clone()),
            schema: self
                .profiles
                .iter()
                .find(|profile| profile.id == connection)
                .filter(|profile| profile.engine == Engine::Postgres)
                .map(|_| object.schema.clone()),
        };
        if !self.target_ready(&target) {
            self.fail("请先选择对象所属数据库，再读取定义".into());
            return;
        }
        let existing = self.tabs.iter().position(|tab| {
            tab.definition.as_ref().is_some_and(|document| {
                document.connection == connection
                    && document.database == database
                    && same_object(&document.object, &object)
            })
        });
        let index = existing.unwrap_or_else(|| {
            let mut tab = QueryTab::new(self.next_tab);
            self.next_tab += 1;
            tab.name = format!("{} · 定义", object.display_name());
            tab.sql.clear();
            tab.saved_sql.clear();
            tab.focus_editor = false;
            tab.target = Some(target.clone());
            tab.definition = Some(DefinitionDocument {
                connection,
                database: database.clone(),
                object: object.clone(),
                sql: None,
                error: None,
                stale: false,
            });
            self.tabs.push(tab);
            self.tabs.len() - 1
        });
        self.active_tab = index;
        self.browser_visible = false;
        if self.tabs[index]
            .definition
            .as_ref()
            .is_some_and(|document| document.sql.is_none())
        {
            self.request_on(
                connection,
                Request::ObjectDefinition {
                    tab: self.tabs[index].id,
                    target,
                    object,
                },
                "正在读取对象定义…",
            );
        }
    }

    pub(in crate::ui) fn object_definition_workspace(&mut self, ui: &mut egui::Ui) {
        let tab = &self.tabs[self.active_tab];
        let Some(document) = &tab.definition else {
            return;
        };
        let connection = document.connection;
        let target = QueryTarget {
            connection,
            database: Some(document.database.clone()),
            schema: None,
        };
        let tab_id = tab.id;
        let object = document.object.clone();
        let sql = document.sql.clone();
        let error = document.error.clone();
        let stale = document.stale;
        let ready = self.target_ready(&target) && !self.session_busy(connection);
        let mut refresh = false;
        let mut draft = false;
        let mut design = false;
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!(
                "{} · {}",
                object.display_name(),
                object.kind.label()
            ));
            if let Some(profile) = self.profiles.iter().find(|p| p.id == connection) {
                design = ui
                    .add_enabled(
                        ready
                            && sql.is_some()
                            && sqlshop::object_design::supported(profile.engine, object.kind, true),
                        egui::Button::new("设计 / 保存定义…"),
                    )
                    .clicked();
            }
            refresh = ui
                .add_enabled(ready, egui::Button::new("刷新定义"))
                .clicked();
            if ui
                .add_enabled(sql.is_some(), egui::Button::new("复制定义"))
                .clicked()
            {
                ui.ctx().copy_text(sql.clone().unwrap_or_default());
            }
            draft = ui
                .add_enabled(sql.is_some(), egui::Button::new("转为查询草稿"))
                .clicked();
        });
        ui.label(
            RichText::new(format!(
                "{} / {} / {}",
                self.profiles
                    .iter()
                    .find(|profile| profile.id == connection)
                    .map(|profile| profile.name.as_str())
                    .unwrap_or("原连接"),
                target.database.as_deref().unwrap_or_default(),
                object.schema
            ))
            .small()
            .weak(),
        );
        ui.separator();
        if stale {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "连接上下文已变化，显示上次读取的定义；请重新选择所属数据库并刷新核对。",
            );
        }
        if let Some(error) = &error {
            ui.add(
                egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color))
                    .selectable(true)
                    .wrap(),
            );
        }
        if self.tab_busy(self.active_tab) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("正在读取定义…");
            });
        }
        if let Some(sql) = &sql {
            egui::ScrollArea::both()
                .id_salt(("object-definition", tab_id))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(sql).monospace())
                            .selectable(true)
                            .extend(),
                    );
                });
        } else if error.is_none() {
            ui.label(RichText::new("尚未取得可见定义").weak());
        }
        if design && let Some(profile) = self.profiles.iter().find(|p| p.id == connection).cloned()
        {
            self.open_object_designer(
                sqlshop::advanced::AdvancedSource::new(profile, target.clone()),
                Some(object.clone()),
                sql.clone(),
                false,
            );
        }
        if refresh {
            if let Some(document) = self.tabs[self.active_tab].definition.as_mut() {
                document.error = None;
            }
            self.request_on(
                connection,
                Request::ObjectDefinition {
                    tab: tab_id,
                    target: target.clone(),
                    object: object.clone(),
                },
                "正在刷新对象定义…",
            );
        }
        if draft
            && let Some(sql) = sql
            && self.apply_ui_tab_action(TabAction::New)
        {
            let tab = &mut self.tabs[self.active_tab];
            tab.name = format!("{} · 查询", object.name);
            tab.sql = sql;
            tab.saved_sql.clear();
            let mut target = target;
            if self
                .profiles
                .iter()
                .any(|profile| profile.id == connection && profile.engine == Engine::Postgres)
            {
                target.schema = Some(object.schema);
            }
            tab.target = Some(target);
        }
    }
}
