use super::*;

#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(super) enum DesignSection {
    #[default]
    Fields,
    Indexes,
    ForeignKeys,
    Relations,
    Ddl,
}

#[derive(Default)]
pub(super) struct DesignDraft {
    action: usize,
    name: String,
    data_type: String,
    size: String,
    selected_column: String,
    selected_index: String,
    nullable: bool,
    original_column: Vec<Option<String>>,
    baseline: Option<(String, String, bool)>,
}

#[derive(Clone)]
struct PendingChange {
    target: QueryTarget,
    table: Table,
    change: StructureChange,
    sql: String,
}

pub(super) struct DesignDocument {
    pub structure: Option<TableStructure>,
    pub stale: bool,
    pub error: Option<String>,
    pub status: String,
    pub section: DesignSection,
    draft: DesignDraft,
    editor_open: bool,
    selected_field: Option<String>,
    selected_index: Option<String>,
    selected_foreign_key: Option<String>,
    find: String,
    pending: Option<PendingChange>,
}

impl Default for DesignDocument {
    fn default() -> Self {
        Self {
            structure: None,
            stale: true,
            error: None,
            status: "尚未读取表结构".into(),
            section: DesignSection::Fields,
            draft: DesignDraft::default(),
            editor_open: false,
            selected_field: None,
            selected_index: None,
            selected_foreign_key: None,
            find: String::new(),
            pending: None,
        }
    }
}

impl DesignDocument {
    pub(super) fn dirty(&self) -> bool {
        if self.draft.action == 4 {
            return self.pending.is_some()
                || (self.editor_open
                    && self
                        .draft
                        .baseline
                        .as_ref()
                        .is_some_and(|(data_type, size, nullable)| {
                            data_type != &self.draft.data_type
                                || size != &self.draft.size
                                || nullable != &self.draft.nullable
                        }));
        }
        self.pending.is_some()
            || (self.editor_open
                && (!self.draft.name.is_empty()
                    || !self.draft.data_type.is_empty()
                    || !self.draft.size.is_empty()
                    || !self.draft.selected_column.is_empty()
                    || !self.draft.selected_index.is_empty()))
    }

    pub(super) fn received(&mut self, structure: TableStructure, changed: bool) {
        if !metadata_contains(&structure.columns, self.selected_field.as_deref()) {
            self.selected_field = None;
        }
        if !metadata_contains(&structure.indexes, self.selected_index.as_deref()) {
            self.selected_index = None;
        }
        if !metadata_contains(
            &structure.foreign_keys,
            self.selected_foreign_key.as_deref(),
        ) {
            self.selected_foreign_key = None;
        }
        self.structure = Some(structure);
        self.stale = false;
        self.error = None;
        self.pending = None;
        self.status = if changed {
            "表结构修改成功，已重新读取；相关数据页需刷新"
        } else {
            "表结构已读取"
        }
        .into();
        if changed {
            self.reset_draft();
        }
    }

    pub(super) fn failed(&mut self, message: &str) {
        self.stale = true;
        self.pending = None;
        self.error = Some(message.into());
        self.status = "结构操作失败；重新读取实际结构后再修改，当前输入保留".into();
    }

    fn reset_draft(&mut self) {
        self.draft = DesignDraft::default();
        self.editor_open = false;
        self.pending = None;
    }

    fn edit_selected_field(&mut self, engine: Engine) {
        let original = self.structure.as_ref().and_then(|structure| {
            let index = structure
                .columns
                .columns
                .iter()
                .position(|name| name == "name")?;
            structure
                .columns
                .rows
                .iter()
                .find(|row| {
                    row.get(index).and_then(Option::as_deref) == self.selected_field.as_deref()
                })
                .cloned()
        });
        let Some(original_column) = original else {
            return;
        };
        match sqlshop::db::column_edit_info(engine, &original_column) {
            Ok(info) => {
                self.draft = DesignDraft {
                    action: 4,
                    name: self.selected_field.clone().unwrap_or_default(),
                    baseline: Some((info.data_type.clone(), info.size.clone(), info.nullable)),
                    data_type: info.data_type,
                    size: info.size,
                    nullable: info.nullable,
                    original_column,
                    ..Default::default()
                };
                self.editor_open = true;
                self.pending = None;
                self.error = None;
            }
            Err(error) => {
                self.error = Some(format!("该字段暂不能编辑：{error}"));
            }
        }
    }

    pub(super) fn example(table: &Table, target: QueryTarget, mode: &str) -> Self {
        let rows = [
            [
                "1",
                "id",
                "bigint",
                "NO",
                "",
                "auto_increment",
                "PRI",
                "主键",
            ],
            ["2", "name", "varchar(100)", "NO", "", "", "", "姓名"],
            ["3", "email", "varchar(256)", "YES", "", "", "", "邮箱"],
            ["4", "note", "text", "YES", "", "", "", "备注"],
            ["5", "amount", "decimal(20,4)", "NO", "0.00", "", "", "金额"],
            [
                "6",
                "created_at",
                "datetime",
                "NO",
                "CURRENT_TIMESTAMP",
                "",
                "",
                "创建时间",
            ],
        ]
        .into_iter()
        .map(|row| {
            let character = row[2].starts_with("varchar") || row[2] == "text";
            let values: Vec<Option<String>> = row
                .into_iter()
                .enumerate()
                .map(|(index, value)| {
                    if index == 4 && value.is_empty() {
                        None
                    } else {
                        Some(value.into())
                    }
                })
                .collect();
            vec![
                values[0].clone(),
                values[1].clone(),
                values[2].clone(),
                values[3].clone(),
                values[4].clone(),
                values[5].clone(),
                character.then(|| "utf8mb4".into()),
                character.then(|| "utf8mb4_unicode_ci".into()),
                None,
                values[7].clone(),
                values[6].clone(),
                Some("select,insert,update,references".into()),
            ]
        })
        .collect();
        let structure = TableStructure {
            columns: ResultSet {
                columns: [
                    "ordinal",
                    "name",
                    "type",
                    "nullable",
                    "default",
                    "extra",
                    "character_set",
                    "collation",
                    "generation_expression",
                    "comment",
                    "key",
                    "privileges",
                ]
                .map(str::to_owned)
                .to_vec(),
                rows,
                ..Default::default()
            },
            indexes: ResultSet {
                columns: [
                    "name",
                    "non_unique",
                    "ordinal",
                    "column",
                    "direction",
                    "type",
                ]
                .map(str::to_owned)
                .to_vec(),
                rows: vec![
                    ["PRIMARY", "0", "1", "id", "A", "BTREE"]
                        .map(|value| Some(value.into()))
                        .to_vec(),
                    ["idx_users_email", "1", "1", "email", "A", "BTREE"]
                        .map(|value| Some(value.into()))
                        .to_vec(),
                ],
                ..Default::default()
            },
            foreign_keys: ResultSet {
                columns: [
                    "name",
                    "column",
                    "referenced_table",
                    "referenced_column",
                    "on_delete",
                ]
                .map(str::to_owned)
                .to_vec(),
                ..Default::default()
            },
            ddl: format!(
                "CREATE TABLE `{}`.`{}` (\n  `id` bigint NOT NULL AUTO_INCREMENT,\n  `name` varchar(100) NOT NULL,\n  `email` varchar(256),\n  `note` text,\n  `amount` decimal(20,4) NOT NULL DEFAULT 0.00,\n  `created_at` datetime NOT NULL DEFAULT CURRENT_TIMESTAMP,\n  PRIMARY KEY (`id`),\n  KEY `idx_users_email` (`email`)\n) ENGINE=InnoDB;",
                table.schema, table.name
            ),
        };
        let mut doc = Self::default();
        doc.received(structure, false);
        if matches!(
            mode,
            "structure-existing-edit" | "structure-existing-preview"
        ) {
            doc.structure.as_mut().unwrap().columns.rows[1][9] = None;
            doc.selected_field = Some("name".into());
            doc.edit_selected_field(Engine::MySql);
            doc.draft.size = "200".into();
            if mode == "structure-existing-preview" {
                let change = StructureChange::AlterColumn {
                    name: doc.draft.name.clone(),
                    data_type: doc.draft.data_type.clone(),
                    size: doc.draft.size.clone(),
                    nullable: doc.draft.nullable,
                    original_column: doc.draft.original_column.clone(),
                };
                let sql = structure_change_sql(Engine::MySql, table, &change).unwrap();
                doc.pending = Some(PendingChange {
                    target: target.clone(),
                    table: table.clone(),
                    change,
                    sql,
                });
            }
        }
        if matches!(mode, "structure-edit" | "structure-preview") {
            doc.editor_open = true;
            doc.draft = DesignDraft {
                name: "phone".into(),
                data_type: "VARCHAR".into(),
                size: "32".into(),
                ..Default::default()
            };
            if mode == "structure-preview" {
                let change = StructureChange::AddColumn {
                    name: "phone".into(),
                    data_type: "VARCHAR".into(),
                    size: "32".into(),
                };
                let sql = structure_change_sql(Engine::MySql, table, &change).unwrap();
                doc.pending = Some(PendingChange {
                    target,
                    table: table.clone(),
                    change,
                    sql,
                });
            }
        }
        doc
    }
}

fn metadata_contains(set: &ResultSet, selected: Option<&str>) -> bool {
    selected.is_some_and(|selected| {
        set.columns
            .iter()
            .position(|name| name == "name")
            .is_some_and(|index| {
                set.rows
                    .iter()
                    .any(|row| row.get(index).and_then(Option::as_deref) == Some(selected))
            })
    })
}

impl QueryTab {
    pub(super) fn document_dirty(&self) -> bool {
        self.dirty() || self.design.as_ref().is_some_and(DesignDocument::dirty)
    }
}

impl App {
    pub(super) fn open_structure_document(
        &mut self,
        connection: Uuid,
        database: String,
        table: Table,
    ) {
        if self.cell_editor.is_some() {
            self.finish_inline_cell(false, CellMovement::Stay);
            if self.cell_editor.is_some() {
                return;
            }
        }
        let target = QueryTarget {
            schema: self
                .profiles
                .iter()
                .any(|profile| profile.id == connection && profile.engine == Engine::Postgres)
                .then(|| table.schema.clone()),
            connection,
            database: Some(database.clone()),
        };
        if !self.target_ready(&target) {
            self.fail("请先连接并选择此对象所属数据库，再打开结构设计".into());
            return;
        }
        let identity = TableTarget {
            connection,
            database,
            schema: table.schema.clone(),
            name: table.name.clone(),
        };
        let index = self
            .tabs
            .iter()
            .position(|tab| tab.design.is_some() && tab.table_target.as_ref() == Some(&identity))
            .unwrap_or_else(|| {
                let mut tab = QueryTab::new(self.next_tab);
                self.next_tab += 1;
                tab.name = format!("{} · 设计", table.name);
                tab.sql.clear();
                tab.saved_sql.clear();
                tab.target = Some(target);
                tab.table_target = Some(identity);
                tab.table_kind = Some(table.kind.clone());
                tab.design = Some(DesignDocument::default());
                self.tabs.push(tab);
                self.tabs.len() - 1
            });
        self.active_tab = index;
        self.browser_visible = false;
        if self.tabs[index]
            .design
            .as_ref()
            .is_some_and(|doc| doc.structure.is_none() || doc.stale)
        {
            self.load_structure(index);
        }
    }

    pub(super) fn load_structure(&mut self, index: usize) {
        let tab = &self.tabs[index];
        if tab.design.is_none() || self.tab_busy(index) {
            return;
        }
        let (Some(target), Some(table)) = (tab.target.clone(), tab.table_target.as_ref()) else {
            return;
        };
        if !self.target_ready(&target) {
            self.fail("请先选择设计文档绑定的连接和数据库".into());
            return;
        }
        let request = Request::DescribeTable {
            tab: tab.id,
            target: target.clone(),
            table: Table {
                schema: table.schema.clone(),
                name: table.name.clone(),
                kind: tab
                    .table_kind
                    .clone()
                    .unwrap_or_else(|| "BASE TABLE".into()),
            },
        };
        self.request_on(target.connection, request, "正在读取表结构…");
        if self.session_busy(target.connection) {
            let doc = self.tabs[index].design.as_mut().unwrap();
            doc.stale = true;
            doc.error = None;
            doc.pending = None;
            doc.status = "正在读取表结构…".into();
        }
    }

    pub(super) fn structure_workspace(&mut self, ui: &mut egui::Ui) {
        let index = self.active_tab;
        let (Some(target), Some(identity)) = (
            self.tabs[index].target.clone(),
            self.tabs[index].table_target.clone(),
        ) else {
            return;
        };
        let profile = self
            .profiles
            .iter()
            .find(|profile| profile.id == target.connection);
        let engine = profile.map(|profile| profile.engine);
        let connection_name = profile
            .map_or("不可用连接", |profile| profile.name.as_str())
            .to_owned();
        let busy = self.tab_busy(index);
        let ready = self.target_ready(&target);
        let protected = self.tabs.iter().any(|tab| {
            tab.has_table_drafts()
                && tab
                    .target
                    .as_ref()
                    .is_some_and(|other| other.connection == target.connection)
        });
        let ordinary_table = self.tabs[index]
            .table_kind
            .as_deref()
            .is_some_and(|kind| kind.eq_ignore_ascii_case("BASE TABLE"));
        let id = self.tabs[index].id;
        let table = Table {
            schema: identity.schema.clone(),
            name: identity.name.clone(),
            kind: if ordinary_table {
                "BASE TABLE".into()
            } else {
                self.tabs[index].table_kind.clone().unwrap_or_default()
            },
        };
        let doc = self.tabs[index].design.as_mut().unwrap();
        let editable = ready && !busy && !doc.stale && !protected && ordinary_table;
        let save_shortcut = editable
            && doc.editor_open
            && doc.pending.is_none()
            && !egui::Popup::is_any_open(ui.ctx())
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::S));
        let mut refresh = false;
        let mut open_data = false;
        let mut apply = None;
        ui.push_id(("structure-document", id), |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
            ui.horizontal_wrapped(|ui| {
                open_data = ui.button("打开数据").clicked();
                refresh = ui.add_enabled(ready && !busy, egui::Button::new("刷新结构")).on_hover_text("重新读取实际结构，保留当前设计输入；原 SQL 预览需重新确认").clicked();
                if ui.add_enabled(doc.structure.is_some(), egui::Button::new("复制 DDL")).clicked() {
                    ui.ctx().copy_text(doc.structure.as_ref().unwrap().ddl.clone());
                }
                ui.separator();
                ui.add(egui::Label::new(RichText::new(format!("{connection_name} / {} / {}.{}", identity.database, identity.schema, identity.name)).weak()).truncate());
            });
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                for (section, label, count) in [
                    (DesignSection::Fields, "字段", doc.structure.as_ref().map_or(0, |s| s.columns.rows.len())),
                    (DesignSection::Indexes, "索引", doc.structure.as_ref().map_or(0, |s| s.indexes.rows.len())),
                    (DesignSection::ForeignKeys, "外键", doc.structure.as_ref().map_or(0, |s| s.foreign_keys.rows.len())),
                    (DesignSection::Relations, "关系图", 0),
                    (DesignSection::Ddl, "DDL", 0),
                ] {
                    let caption = if matches!(section, DesignSection::Ddl | DesignSection::Relations) { label.into() } else { format!("{label} {count}") };
                    ui.selectable_value(&mut doc.section, section, caption);
                }
            });
            if !matches!(doc.section, DesignSection::Ddl | DesignSection::Relations) {
                let search_id = egui::Id::new(("design-field-search", id));
                ui.horizontal(|ui| {
                    ui.label("查找");
                    let response = ui.add(egui::TextEdit::singleline(&mut doc.find).id(search_id)
                        .hint_text("名称、类型、定义…").desired_width(220.0));
                    if ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) { response.request_focus(); }
                    if !doc.find.is_empty() && ui.small_button("清除").clicked() { doc.find.clear(); }
                });
            }
            if let Some(error) = &doc.error { ui.colored_label(ui.visuals().error_fg_color, error); }
            if !ready { ui.label(RichText::new("绑定目标未连接或数据库不符；选择对应数据库后刷新结构。").weak()); }
            else if protected { ui.label(RichText::new("此连接有数据草稿，请先提交或撤销，再修改结构。").weak()); }
            else if doc.stale && !busy { ui.label(RichText::new("当前结构需要重新读取，设计输入保留。").weak()); }
            if doc.structure.is_some() && doc.pending.is_none() {
                ui.horizontal_wrapped(|ui| {
                    let actions: &[(usize, &str)] = match doc.section {
                        DesignSection::Fields => &[(0, "＋ 新增字段"), (4, "编辑字段"), (1, "− 删除字段")],
                        DesignSection::Indexes => &[(2, "＋ 新增索引"), (3, "− 删除索引")],
                        _ => &[],
                    };
                    for (action, label) in actions {
                        let selected = if matches!(*action, 1 | 4) { doc.selected_field.is_some() } else if *action == 3 { doc.selected_index.is_some() } else { true };
                        let supported = *action != 1 || engine == Some(Engine::Postgres);
                        if ui.add_enabled(editable && selected && supported && !doc.dirty(), egui::Button::new(*label)).clicked() {
                            if *action == 4 {
                                if let Some(engine) = engine { doc.edit_selected_field(engine); }
                                continue;
                            }
                            doc.draft = DesignDraft { action: *action, selected_column: if *action == 1 { doc.selected_field.clone().unwrap_or_default() } else { String::new() }, selected_index: if *action == 3 { doc.selected_index.clone().unwrap_or_default() } else { String::new() }, ..Default::default() };
                            doc.editor_open = true;
                        }
                    }
                });
            }
            if let Some(pending) = doc.pending.clone() {
                egui::Frame::group(ui.style()).inner_margin(8).show(ui, |ui| {
                    ui.strong("确认结构修改");
                    ui.label(format!("目标：{connection_name} / {} / {}.{}", identity.database, pending.table.schema, pending.table.name));
                    egui::ScrollArea::both().id_salt("ddl-confirm").max_height(100.0).show(ui, |ui| { ui.add(egui::Label::new(RichText::new(&pending.sql).monospace()).selectable(true)); });
                    ui.label(RichText::new("DDL 遵循数据库的锁与自动提交规则；删除字段会丢失该字段数据。").small().weak());
                    ui.horizontal(|ui| {
                        if ui.add_enabled(editable, egui::Button::new("确认执行修改")).clicked() { apply = Some(pending); }
                        if ui.add_enabled(!busy, egui::Button::new("返回编辑")).clicked() { doc.pending = None; }
                    });
                });
            } else if doc.editor_open {
                let structure = doc.structure.as_ref().unwrap();
                let column_names = structure.columns.columns.iter().position(|name| name == "name").map(|i| structure.columns.rows.iter().filter_map(|row| row.get(i).and_then(Option::as_deref)).map(str::to_owned).collect::<Vec<_>>()).unwrap_or_default();
                let index_names = structure.indexes.rows.iter().filter_map(|row| row.first().and_then(Option::as_deref)).map(str::to_owned).collect::<Vec<_>>();
                egui::Frame::group(ui.style()).inner_margin(8).show(ui, |ui| {
                    let draft = &mut doc.draft;
                    ui.strong(["新增字段", "删除字段", "新增普通索引", "删除索引", "编辑已有字段"][draft.action]);
                    ui.add_enabled_ui(editable, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            match draft.action {
                                0 | 4 => {
                                    ui.label("名称");
                                    if draft.action == 4 { ui.label(&draft.name); } else { ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(140.0)); }
                                    ui.label("类型");
                                    let previous_type = draft.data_type.clone();
                                    egui::ComboBox::from_id_salt("design-column-type")
                                        .width(155.0)
                                        .selected_text(if draft.data_type.is_empty() { "选择字段类型" } else { &draft.data_type })
                                        .show_ui(ui, |ui| {
                                            if let Some(engine) = engine {
                                                let types = if draft.action == 4 { column_type_options(engine).to_vec() } else { design_column_types(engine) };
                                                for data_type in types {
                                                    ui.selectable_value(&mut draft.data_type, data_type.to_owned(), data_type);
                                                }
                                            }
                                        });
                                    let needs_length = matches!(draft.data_type.as_str(), "VARCHAR" | "NVARCHAR" | "CHAR" | "NCHAR" | "VARBINARY" | "BINARY");
                                    if previous_type != draft.data_type && !needs_length { draft.size.clear(); }
                                    if needs_length {
                                        ui.label("长度");
                                        ui.add(egui::TextEdit::singleline(&mut draft.size).desired_width(65.0).hint_text("必填"));
                                    }
                                    if draft.action == 4 { ui.checkbox(&mut draft.nullable, "允许 NULL"); }
                                }
                                1 => { ui.label(&draft.selected_column); }
                                2 => {
                                    ui.label("索引名"); ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(160.0));
                                    egui::ComboBox::from_id_salt("index-column").selected_text(if draft.selected_column.is_empty() { "选择字段" } else { &draft.selected_column }).show_ui(ui, |ui| { for name in &column_names { ui.selectable_value(&mut draft.selected_column, name.clone(), name); } });
                                }
                                _ => { ui.label(&draft.selected_index); }
                            }
                        });
                    });
                    let change = match draft.action {
                        0 => Some(StructureChange::AddColumn { name: draft.name.clone(), data_type: draft.data_type.clone(), size: draft.size.clone() }),
                        1 if column_names.contains(&draft.selected_column) => Some(StructureChange::DropColumn { name: draft.selected_column.clone() }),
                        2 if column_names.contains(&draft.selected_column) => Some(StructureChange::CreateIndex { name: draft.name.clone(), columns: vec![draft.selected_column.clone()] }),
                        3 if index_names.contains(&draft.selected_index) => Some(StructureChange::DropIndex { name: draft.selected_index.clone() }),
                        4 if column_names.contains(&draft.name) => Some(StructureChange::AlterColumn { name: draft.name.clone(), data_type: draft.data_type.clone(), size: draft.size.clone(), nullable: draft.nullable, original_column: draft.original_column.clone() }),
                        _ => None,
                    };
                    let result = engine.zip(change).map(|(engine, change)| structure_change_sql(engine, &table, &change).map(|sql| PendingChange { target: target.clone(), table: table.clone(), change, sql }));
                    if draft.action == 4 && let Some(Err(error)) = &result { ui.label(RichText::new(error.to_string()).small().weak()); }
                    let editing_existing = draft.action == 4;
                    let changed = draft.baseline.as_ref().is_some_and(|(data_type, size, nullable)| data_type != &draft.data_type || size != &draft.size || nullable != &draft.nullable);
                    let preview = result.and_then(Result::ok).filter(|_| !editing_existing || changed);
                    ui.horizontal(|ui| {
                        if (ui.add_enabled(editable && preview.is_some(), egui::Button::new("预览修改 SQL")).on_hover_text("⌘ / Ctrl + S：生成 SQL 预览，确认后执行").clicked() || save_shortcut) && preview.is_some() { doc.pending = preview; }
                        if ui.add_enabled(!busy, egui::Button::new("取消编辑")).clicked() { doc.reset_draft(); }
                    });
                    ui.label(RichText::new(if editing_existing { "支持普通字段的安全扩大及可空设置；依赖、生成列和复杂属性按能力限制，原默认值不会静默删除。" } else { "当前支持可空无默认值的新字段、单字段普通索引；完整改列及约束编辑待补。" }).small().weak());
                });
            }
            ui.separator();
            if let Some(structure) = &doc.structure {
                match doc.section {
                    DesignSection::Fields => {
                        let double_clicked = metadata_grid(ui, id, doc.section, &structure.columns, &mut doc.selected_field, &doc.find);
                        let keyboard = editable && !ui.ctx().wants_keyboard_input() && !egui::Popup::is_any_open(ui.ctx()) && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F2));
                        if (double_clicked || keyboard) && editable && !doc.dirty() && doc.pending.is_none() && let Some(engine) = engine { doc.edit_selected_field(engine); }
                    }
                    DesignSection::Indexes => { metadata_grid(ui, id, doc.section, &structure.indexes, &mut doc.selected_index, &doc.find); }
                    DesignSection::ForeignKeys => { metadata_grid(ui, id, doc.section, &structure.foreign_keys, &mut doc.selected_foreign_key, &doc.find); }
                    DesignSection::Relations => { super::compare_ui::show_er(ui, &table, structure); }
                    DesignSection::Ddl => { egui::ScrollArea::both().id_salt("design-ddl").auto_shrink([false, false]).show(ui, |ui| { ui.add(egui::Label::new(RichText::new(&structure.ddl).monospace()).selectable(true)); }); }
                }
            } else { ui.label(if busy { "正在读取表结构…" } else { "暂无表结构，连接对应数据库后点击刷新结构。" }); }
        });
        if refresh {
            self.load_structure(index);
        }
        if open_data {
            self.handle_browser_action(object_browser_ui::BrowserAction::OpenData {
                connection: identity.connection,
                database: identity.database,
                table,
            });
        }
        if let Some(pending) = apply {
            let valid = self.tabs[index]
                .design
                .as_ref()
                .is_some_and(|doc| !doc.stale)
                && self.tabs[index].target.as_ref() == Some(&pending.target)
                && self.target_ready(&pending.target)
                && !self.tab_busy(index);
            if valid {
                self.request_on(
                    pending.target.connection,
                    Request::ChangeStructure {
                        tab: id,
                        target: pending.target,
                        table: pending.table,
                        change: pending.change,
                        preview_sql: pending.sql,
                    },
                    "正在修改表结构…",
                );
                if self.tab_busy(index) {
                    let doc = self.tabs[index].design.as_mut().unwrap();
                    doc.pending = None;
                    doc.stale = true;
                    doc.error = None;
                    doc.status = "正在修改表结构…".into();
                }
            } else {
                self.fail("设计文档目标或结构已变化，请刷新并重新预览 SQL".into());
            }
        }
    }
}

fn metadata_grid(
    ui: &mut egui::Ui,
    tab: Uuid,
    section: DesignSection,
    set: &ResultSet,
    selected: &mut Option<String>,
    find: &str,
) -> bool {
    let mut edit_requested = false;
    if set.columns.is_empty() {
        ui.label("暂无元数据");
        return false;
    }
    if set.rows.is_empty() {
        ui.label(RichText::new("此对象没有对应项目").weak());
    }
    let needle = find.to_lowercase();
    let rows: Vec<_> = set
        .rows
        .iter()
        .filter(|row| {
            needle.is_empty()
                || row
                    .iter()
                    .flatten()
                    .any(|value| value.to_lowercase().contains(&needle))
        })
        .collect();
    if !find.is_empty() {
        ui.label(
            RichText::new(format!("匹配 {} / {} 项", rows.len(), set.rows.len()))
                .small()
                .weak(),
        );
    }
    let height = ui.available_height().max(0.0);
    let name_index = set.columns.iter().position(|name| name == "name");
    egui::Frame::new()
        .fill(surface(ui))
        .corner_radius(3)
        .stroke(egui::Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            egui::ScrollArea::horizontal()
                .id_salt(("structure-scroll", tab, section))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let body_height =
                        (height - 28.0 - ui.spacing().scroll.bar_width - 2.0).max(0.0);
                    let mut builder = TableBuilder::new(ui)
                        .id_salt(("structure-table", tab, section))
                        .striped(true)
                        .resizable(true)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .min_scrolled_height(body_height)
                        .max_scroll_height(body_height);
                    for name in &set.columns {
                        let width = match name.as_str() {
                            "ordinal" | "nullable" | "primary" | "unique" | "non_unique" => 80.0,
                            "name" | "type" | "column" => 160.0,
                            "default" | "default_or_generation" | "definition" => 220.0,
                            _ => 140.0,
                        };
                        builder = builder.column(Column::initial(width).at_least(60.0).clip(true));
                    }
                    builder
                        .header(28.0, |mut header| {
                            for name in &set.columns {
                                header.col(|ui| {
                                    grid_cell(
                                        ui,
                                        RichText::new(metadata_label(name)).strong(),
                                        false,
                                        true,
                                    )
                                    .on_hover_text(name);
                                });
                            }
                        })
                        .body(|body| {
                            body.rows(28.0, rows.len(), |mut row| {
                                let values = rows[row.index()];
                                let name =
                                    name_index.and_then(|i| values.get(i).and_then(Option::as_ref));
                                let active = name.is_some() && name == selected.as_ref();
                                row.set_selected(active);
                                for i in 0..set.columns.len() {
                                    row.col(|ui| {
                                        let value = values
                                            .get(i)
                                            .and_then(Option::as_deref)
                                            .unwrap_or("NULL");
                                        let response =
                                            grid_cell(ui, RichText::new(value), active, false)
                                                .on_hover_text(value);
                                        if response.clicked() || response.double_clicked() {
                                            *selected = name.cloned();
                                        }
                                        edit_requested |= response.double_clicked();
                                    });
                                }
                            });
                        });
                });
        });
    edit_requested
}

fn design_column_types(engine: Engine) -> Vec<&'static str> {
    // Use the existing engine list, and only offer types accepted by the
    // actual ADD COLUMN generator. This is local validation, without DB I/O.
    column_type_options(engine)
        .iter()
        .copied()
        .filter(|data_type| {
            let sized = matches!(
                *data_type,
                "VARCHAR" | "NVARCHAR" | "CHAR" | "NCHAR" | "VARBINARY" | "BINARY"
            );
            structure_change_sql(
                engine,
                &Table {
                    schema: "schema".into(),
                    name: "table".into(),
                    kind: "BASE TABLE".into(),
                },
                &StructureChange::AddColumn {
                    name: "column".into(),
                    data_type: (*data_type).into(),
                    size: if sized { "32".into() } else { String::new() },
                },
            )
            .is_ok()
        })
        .collect()
}

fn metadata_label(name: &str) -> &str {
    match name {
        "ordinal" | "index_ordinal" => "序号",
        "name" => "名称",
        "type" => "类型",
        "nullable" => "允许空",
        "default" | "default_or_generation" => "默认 / 生成值",
        "extra" => "额外定义",
        "key" => "键",
        "primary" => "主键",
        "unique" => "唯一",
        "non_unique" => "非唯一",
        "column" => "字段",
        "definition" => "定义",
        "comment" => "注释",
        "collation" => "排序规则",
        "character_set" => "字符集",
        "referenced_schema" => "引用 Schema",
        "referenced_table" => "引用表",
        "referenced_column" => "引用字段",
        "on_update" => "更新规则",
        "on_delete" => "删除规则",
        "direction" | "descending" => "方向",
        "method" => "方法",
        "identity" => "标识列",
        "generated" | "generation_expression" => "生成表达式",
        _ => name,
    }
}
