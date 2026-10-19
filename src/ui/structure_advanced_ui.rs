use super::*;
use sqlshop::db::{ConstraintDesign, ConstraintKind, DesignerOperation, ReferenceAction};

#[derive(Default)]
pub(super) struct AdvancedDraft {
    pub open: bool,
    action: usize,
    column: String,
    name: String,
    data_type: String,
    expression: String,
    using: String,
    columns: Vec<String>,
    reference_schema: String,
    reference_table: String,
    reference_columns: String,
    on_delete: ReferenceAction,
    on_update: ReferenceAction,
    deferrable: bool,
    deferred: bool,
    stored: bool,
    always: bool,
    replace: bool,
    previous: String,
    error: Option<String>,
}
fn kind(name: &str) -> Option<ConstraintKind> {
    match name {
        "PRIMARY KEY" => Some(ConstraintKind::PrimaryKey),
        "UNIQUE" => Some(ConstraintKind::Unique),
        "CHECK" => Some(ConstraintKind::Check),
        "FOREIGN KEY" => Some(ConstraintKind::ForeignKey),
        _ => None,
    }
}
fn action_combo(ui: &mut egui::Ui, label: &str, value: &mut ReferenceAction) {
    egui::ComboBox::from_id_salt(label)
        .selected_text(format!("{label}: {}", value.sql()))
        .show_ui(ui, |ui| {
            for action in [
                ReferenceAction::NoAction,
                ReferenceAction::Restrict,
                ReferenceAction::Cascade,
                ReferenceAction::SetNull,
                ReferenceAction::SetDefault,
            ] {
                ui.selectable_value(value, action, action.sql());
            }
        });
}
impl AdvancedDraft {
    pub(super) fn example(table: &Table) -> Self {
        Self {
            open: true,
            action: 9,
            name: "fk_customer".into(),
            columns: vec!["id".into()],
            reference_schema: table.schema.clone(),
            reference_table: "customers".into(),
            reference_columns: "id".into(),
            ..Default::default()
        }
    }
    fn operation(&self, structure: &TableStructure) -> anyhow::Result<DesignerOperation> {
        let previous = || -> anyhow::Result<(String, ConstraintKind)> {
            let row = structure
                .constraints
                .rows
                .iter()
                .find(|r| r.first().and_then(Option::as_deref) == Some(&self.previous))
                .ok_or_else(|| anyhow::anyhow!("请选择现有约束"))?;
            let k = kind(row.get(1).and_then(Option::as_deref).unwrap_or(""))
                .ok_or_else(|| anyhow::anyhow!("此约束类型不支持编辑"))?;
            Ok((self.previous.clone(), k))
        };
        Ok(match self.action {
            0 => DesignerOperation::RenameColumn {
                old: self.column.clone(),
                new: self.name.clone(),
            },
            1 | 2 => DesignerOperation::SetDefault {
                column: self.column.clone(),
                expression: if self.action == 1 {
                    Some(self.expression.clone())
                } else {
                    None
                },
            },
            3 => DesignerOperation::AlterType {
                column: self.column.clone(),
                data_type: self.data_type.clone(),
                using: (!self.using.trim().is_empty()).then(|| self.using.clone()),
            },
            4 => DesignerOperation::AddGenerated {
                column: self.name.clone(),
                data_type: self.data_type.clone(),
                expression: self.expression.clone(),
                stored: self.stored,
            },
            5 => DesignerOperation::AddIdentity {
                column: self.column.clone(),
                always: self.always,
            },
            6..=9 => DesignerOperation::PutConstraint {
                previous: if self.replace {
                    Some(previous()?)
                } else {
                    None
                },
                definition: ConstraintDesign {
                    name: self.name.clone(),
                    kind: [
                        ConstraintKind::PrimaryKey,
                        ConstraintKind::Unique,
                        ConstraintKind::Check,
                        ConstraintKind::ForeignKey,
                    ][self.action - 6],
                    columns: self.columns.clone(),
                    expression: self.expression.clone(),
                    referenced_schema: self.reference_schema.clone(),
                    referenced_table: self.reference_table.clone(),
                    referenced_columns: self
                        .reference_columns
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect(),
                    on_delete: self.on_delete,
                    on_update: self.on_update,
                    deferrable: self.deferrable,
                    initially_deferred: self.deferred,
                },
            },
            _ => {
                let (name, kind) = previous()?;
                DesignerOperation::DropConstraint { name, kind }
            }
        })
    }
}
pub(super) fn show(
    ui: &mut egui::Ui,
    draft: &mut AdvancedDraft,
    structure: &TableStructure,
    engine: Engine,
    table: &Table,
    enabled: bool,
) -> Option<(StructureChange, String)> {
    let mut preview = None;
    let names: Vec<_> = structure
        .columns
        .rows
        .iter()
        .filter_map(|r| r.get(1).and_then(Option::as_deref))
        .collect();
    egui::ScrollArea::vertical().id_salt("advanced-table-design-form").max_height((ui.available_height() - 110.0).max(160.0)).auto_shrink([false, true]).show(ui, |ui| {
    egui::Frame::group(ui.style()).show(ui,|ui|{
        ui.strong("字段与约束设计");
        ui.add_enabled_ui(enabled,|ui|{
            ui.horizontal_wrapped(|ui|{
                egui::ComboBox::from_id_salt("advanced-design-operation").selected_text(["重命名字段","设置默认值","移除默认值","转换字段类型","新增生成/计算列","设置标识属性","主键","唯一约束","CHECK 约束","外键","删除约束"][draft.action]).show_ui(ui,|ui|{
                    for (i,name) in ["重命名字段","设置默认值","移除默认值","转换字段类型","新增生成/计算列","设置标识属性","主键","唯一约束","CHECK 约束","外键","删除约束"].iter().enumerate(){if ui.selectable_value(&mut draft.action,i,*name).changed(){draft.error=None;}}
                });
                if matches!(draft.action,0..=3|5){egui::ComboBox::from_id_salt("advanced-design-column").selected_text(if draft.column.is_empty(){"选择字段"}else{&draft.column}).show_ui(ui,|ui|{for name in &names{ui.selectable_value(&mut draft.column,(*name).to_owned(),*name);}});}
                if matches!(draft.action,0|4|6..=9){ui.label(if draft.action<6{"新字段名"}else{"约束名"});ui.text_edit_singleline(&mut draft.name);}
                if matches!(draft.action,3|4) && !(draft.action==4 && engine==Engine::SqlServer){ui.label("字段类型");ui.add(egui::TextEdit::singleline(&mut draft.data_type).hint_text("例如 VARCHAR(255)、NUMERIC(12,2)").desired_width(230.));}
                if draft.action==4{ui.checkbox(&mut draft.stored,if engine==Engine::SqlServer{"持久化 PERSISTED"}else{"存储 STORED"});}
                if draft.action==5{ui.checkbox(&mut draft.always,"GENERATED ALWAYS");}
            });
            if matches!(draft.action,1|4|8){ui.label("表达式（默认值的文本请加单引号）");ui.add(egui::TextEdit::multiline(&mut draft.expression).desired_rows(2).code_editor());}
            if draft.action==3 && engine==Engine::Postgres {ui.label("USING 转换表达式（可选）");ui.text_edit_singleline(&mut draft.using);}
            if matches!(draft.action,6|7|9){
                ui.label("字段（勾选顺序为约束内顺序）");
                ui.horizontal_wrapped(|ui|{for name in &names{let mut selected=draft.columns.iter().any(|c|c==name);if ui.checkbox(&mut selected,*name).changed(){if selected{draft.columns.push((*name).to_owned());}else{draft.columns.retain(|c|c!=name);}}}});
                ui.label(format!("顺序：{}",draft.columns.join(", ")));
            }
            if draft.action==9 {
                ui.horizontal_wrapped(|ui|{ui.label("引用 Schema");ui.text_edit_singleline(&mut draft.reference_schema);ui.label("引用表");ui.text_edit_singleline(&mut draft.reference_table);});
                ui.label("引用字段（按对应顺序，用逗号分隔）");ui.text_edit_singleline(&mut draft.reference_columns);
                ui.horizontal_wrapped(|ui|{action_combo(ui,"ON DELETE",&mut draft.on_delete);action_combo(ui,"ON UPDATE",&mut draft.on_update);});
            }
            if matches!(draft.action,6..=9) {
                if engine==Engine::Postgres && draft.action!=8 {ui.horizontal(|ui|{ui.checkbox(&mut draft.deferrable,"可延迟");ui.add_enabled_ui(draft.deferrable,|ui|{ui.checkbox(&mut draft.deferred,"初始延迟");});});}
                ui.checkbox(&mut draft.replace,"替换现有约束（原定义复核后执行删除与重建）");
            }
            if draft.action==10 || (matches!(draft.action,6..=9) && draft.replace) {
                egui::ComboBox::from_id_salt("advanced-existing-constraint").selected_text(if draft.previous.is_empty(){"选择现有约束"}else{&draft.previous}).show_ui(ui,|ui|{for row in &structure.constraints.rows {if let (Some(Some(name)),Some(Some(k)))=(row.first(),row.get(1)){ui.selectable_value(&mut draft.previous,name.clone(),format!("{name} · {k}"));}}});
            }
            ui.horizontal(|ui|{
                if ui.button("预览修改 SQL").clicked(){
                    let snapshot=sqlshop::db::DesignSnapshot::from(structure);
                    match draft.operation(structure).and_then(|operation|{let change=StructureChange::Designer{operation: Box::new(operation),snapshot};let sql=structure_change_sql(engine,table,&change)?;Ok((change,sql))}){
                        Ok(p)=>{preview=Some(p);draft.error=None;},Err(error)=>draft.error=Some(error.to_string()),
                    }
                }
                if ui.button("取消设计").clicked(){*draft=AdvancedDraft::default();}
            });
        });
        if let Some(error)=&draft.error{ui.colored_label(ui.visuals().error_fg_color,error);}
        ui.label(RichText::new("变更按服务器版本和依赖执行检查；转换类型可能失败或改变数据，请核对生成的 SQL。已有草稿不会自动提交。").small().weak());
    });
    });
    preview
}
