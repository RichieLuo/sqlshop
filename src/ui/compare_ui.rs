use eframe::egui::{self, RichText};
use sqlshop::{
    compare::{CompareEndpoint, CompareReport, RowDifferenceKind, er_relations},
    compare_worker::{CompareResponse, CompareResponseKind},
    db::TableStructure,
    model::{Engine, Profile, Table},
};
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Data,
    Structure,
    SourceDdl,
    TargetDdl,
    Sync,
    Er,
}
pub struct CompareDocument {
    pub id: Uuid,
    pub source: CompareEndpoint,
    pub target: CompareEndpoint,
    pub pending: Option<Uuid>,
    pub report: Option<Box<CompareReport>>,
    pub error: Option<String>,
    pub status: String,
    section: Section,
    confirmed: bool,
    selected_structure: Option<usize>,
    structure_confirmed: bool,
}
impl CompareDocument {
    pub fn new(id: Uuid, source: CompareEndpoint, target: CompareEndpoint) -> Self {
        Self {
            id,
            source,
            target,
            pending: None,
            report: None,
            error: None,
            status: String::new(),
            section: Section::Data,
            confirmed: false,
            selected_structure: None,
            structure_confirmed: false,
        }
    }
    pub fn apply_response(&mut self, response: CompareResponse) -> bool {
        if response.document != self.id
            || self.pending != Some(response.generation)
            || response.source != self.source.fingerprint()
            || response.target != self.target.fingerprint()
        {
            return false;
        }
        self.pending = None;
        self.confirmed = false;
        self.structure_confirmed = false;
        self.selected_structure = None;
        match response.kind {
            CompareResponseKind::Compared(Ok(report)) => {
                self.report = Some(report);
                self.error = None;
                self.status = "比较完成".into();
            }
            CompareResponseKind::Compared(Err(error))
            | CompareResponseKind::Applied(Err(error))
            | CompareResponseKind::StructureApplied(Err(error)) => {
                self.error = Some(error);
                self.status.clear();
            }
            CompareResponseKind::StructureApplied(Ok(message)) => {
                self.status = message;
                self.report = None;
                self.error = None;
            }
            CompareResponseKind::Applied(Ok((updated, inserted))) => {
                self.status = format!(
                    "已提交 {updated} 个单元格修改、{inserted} 行新增；请重新比较查看当前结果。"
                );
                self.report = None;
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
// Actions own endpoint snapshots until dispatched; reports remain boxed.
#[allow(clippy::large_enum_variant)]
pub enum CompareAction {
    ApplyStructure {
        generation: Uuid,
        report: Box<CompareReport>,
        candidate: usize,
    },
    Compare {
        generation: Uuid,
        source: CompareEndpoint,
        target: CompareEndpoint,
    },
    Apply {
        generation: Uuid,
        report: Box<CompareReport>,
    },
}
fn endpoint_editor(
    ui: &mut egui::Ui,
    title: &str,
    endpoint: &mut CompareEndpoint,
    profiles: &[Profile],
) {
    ui.group(|ui| {
        ui.label(RichText::new(title).strong());
        egui::ComboBox::from_id_salt(title)
            .selected_text(&endpoint.profile.name)
            .show_ui(ui, |ui| {
                for profile in profiles {
                    if ui
                        .selectable_label(profile.id == endpoint.profile.id, &profile.name)
                        .clicked()
                    {
                        endpoint.profile = profile.clone();
                        endpoint.target.connection = profile.id;
                        endpoint.target.schema = None;
                    }
                }
            });
        egui::Grid::new((title, "target"))
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("数据库");
                let mut database = endpoint.target.database.clone().unwrap_or_default();
                if ui
                    .add(egui::TextEdit::singleline(&mut database).desired_width(190.0))
                    .changed()
                {
                    endpoint.target.database = Some(database.clone());
                    if endpoint.profile.engine == Engine::MySql {
                        endpoint.table.schema = database;
                    }
                }
                ui.end_row();
                ui.label("Schema");
                ui.add_enabled(
                    endpoint.profile.engine != Engine::MySql,
                    egui::TextEdit::singleline(&mut endpoint.table.schema).desired_width(190.0),
                );
                ui.end_row();
                ui.label("表");
                ui.add(egui::TextEdit::singleline(&mut endpoint.table.name).desired_width(190.0));
                ui.end_row();
            });
    });
}
fn display_value(value: &Option<String>) -> String {
    value
        .as_ref()
        .map(|s| serde_json::to_string(s).unwrap_or_default())
        .unwrap_or_else(|| "NULL".into())
}
fn display_key(key: &str, names: &[String]) -> String {
    serde_json::from_str::<Vec<String>>(key)
        .ok()
        .map(|values| {
            names
                .iter()
                .zip(values)
                .map(|(n, v)| format!("{n} = {}", v.split_once(':').map(|(_, v)| v).unwrap_or(&v)))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| key.into())
}

pub fn show(
    ui: &mut egui::Ui,
    document: &mut CompareDocument,
    profiles: &[Profile],
) -> Option<CompareAction> {
    let mut action = None;
    ui.push_id(document.id, |ui| {
        ui.heading("表数据与结构比较");
        let before = (document.source.fingerprint(),document.target.fingerprint());
        let busy = document.pending.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            ui.columns(2, |columns| {
                endpoint_editor(&mut columns[0], "来源", &mut document.source, profiles);
                endpoint_editor(&mut columns[1], "目标", &mut document.target, profiles);
            });
        });
        if before != (document.source.fingerprint(),document.target.fingerprint()) {
            document.report = None; document.confirmed = false; document.error = None; document.status.clear();
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new("开始比较")).on_hover_text("每侧最多扫描 1000 行。").clicked() {
                let generation = Uuid::new_v4(); document.pending = Some(generation); document.error = None; document.report = None; document.confirmed = false;
                action = Some(CompareAction::Compare { generation, source:document.source.clone(),target:document.target.clone() });
            }
            if busy { ui.spinner(); ui.label("读取或同步中"); }
            ui.label(&document.status);
        });
        if let Some(error) = &document.error { ui.colored_label(ui.visuals().error_fg_color,error); }
        let Some(report) = &document.report else { return; };
        ui.label(format!("来源读取 {} 行{} · 目标读取 {} 行{} · {} 项字段/索引/外键差异 · {} 项行/单元格差异", report.source.data.output.sets.first().map(|s|s.rows.len()).unwrap_or(0),if report.source.complete {"（完整）"}else{"（未完整）"},report.target.data.output.sets.first().map(|s|s.rows.len()).unwrap_or(0),if report.target.complete {"（完整）"}else{"（未完整）"},report.structure_differences.len(),report.data_differences.len()));
        for blocker in &report.blockers { ui.colored_label(ui.visuals().warn_fg_color,blocker); }
        ui.horizontal_wrapped(|ui| {
            for (section,label) in [(Section::Data,"数据差异"),(Section::Structure,"结构差异"),(Section::SourceDdl,"来源 DDL"),(Section::TargetDdl,"目标 DDL"),(Section::Sync,"同步预览"),(Section::Er,"目标外键图")] { ui.selectable_value(&mut document.section,section,label); }
        });
        ui.separator();
        match document.section {
            Section::Data => {
                if report.data_differences.is_empty() {
                    ui.label(if report.blockers.is_empty() { "未发现数据差异。" } else { "数据核对不完整。" });
                }
                egui::ScrollArea::vertical().id_salt("data_differences").max_height(ui.available_height().max(100.0)).show_rows(ui,64.0,report.data_differences.len(),|ui,range| {
                    for difference in &report.data_differences[range] {
                        ui.group(|ui| {
                            ui.label(RichText::new(format!("{} · {}{}", match difference.kind {RowDifferenceKind::Changed=>"值不同",RowDifferenceKind::SourceOnly=>"仅来源存在",RowDifferenceKind::TargetOnly=>"仅目标存在（保留）"}, display_key(&difference.key,&report.target.data.primary_keys), difference.column.as_ref().map(|c|format!(" · {c}")).unwrap_or_default())).strong());
                            ui.horizontal(|ui| {
                                let left = display_value(&difference.source); let right = display_value(&difference.target);
                                ui.label(format!("来源：{}",left.chars().take(180).collect::<String>())).on_hover_text(left);
                                ui.label(format!("目标：{}",right.chars().take(180).collect::<String>())).on_hover_text(right);
                            });
                        });
                    }
                });
            },
            Section::Structure => {
                ui.add_enabled_ui(!busy,|ui| {
                    for (index,candidate) in report.structure_candidates().iter().enumerate() {
                        if ui.selectable_label(document.selected_structure==Some(index),&candidate.label).clicked() {
                            document.selected_structure=Some(index); document.structure_confirmed=false;
                        }
                    }
                });
                if let Some((index,candidate))=document.selected_structure.and_then(|index|report.structure_candidates().get(index).map(|c|(index,c))) {
                    if let Some(reason)=&candidate.reason {ui.colored_label(ui.visuals().warn_fg_color,reason);}
                    if let Some(sql)=candidate.preview_sql() {
                        ui.monospace(sql);
                        ui.add_enabled_ui(!busy,|ui|{ui.checkbox(&mut document.structure_confirmed,"确认向目标表应用此条新增 SQL");});
                        if ui.add_enabled(!busy&&candidate.supported()&&document.structure_confirmed,egui::Button::new("应用此结构操作")).on_hover_text("仅新增普通可空字段或单列索引。").clicked() {
                            let generation=Uuid::new_v4(); document.pending=Some(generation); document.structure_confirmed=false;
                            action=Some(CompareAction::ApplyStructure {generation,report:report.clone(),candidate:index});
                        }
                    }
                }
                ui.separator();
                egui::ScrollArea::both().id_salt("structure_differences").show(ui, |ui| {
                    for difference in &report.structure_differences {
                        ui.group(|ui| {
                            ui.label(RichText::new(format!("{} · {}",difference.section,difference.name)).strong());
                            ui.monospace(format!("来源：{}",difference.source.as_deref().unwrap_or("不存在")));
                            ui.monospace(format!("目标：{}",difference.target.as_deref().unwrap_or("不存在")));
                        });
                    }
                    if report.structure_differences.is_empty() { ui.label("未发现结构差异。"); }
                });
            },
            Section::SourceDdl | Section::TargetDdl => {
                let ddl = if document.section == Section::SourceDdl { &report.source.structure.ddl } else { &report.target.structure.ddl };
                if ui.button("复制 DDL").on_hover_text("只读定义，不自动应用 DDL。").clicked() { ui.ctx().copy_text(ddl.clone()); }
                egui::ScrollArea::both().id_salt("compare_ddl").show(ui,|ui| { ui.monospace(ddl); });
            },
            Section::Sync => {
                ui.label(format!("更新 {} 个单元格，新增 {} 行；保留目标独有行。",report.update_count(),report.insert_count()));
                let preview = report.sync_preview();
                if ui.button("复制操作预览").clicked() { ui.ctx().copy_text(preview.clone()); }
                ui.add_enabled_ui(!busy && report.can_apply(),|ui| { ui.checkbox(&mut document.confirmed,"已核对来源、目标与修改，确认提交同步事务"); });
                if ui.add_enabled(!busy && report.can_apply() && document.confirmed,egui::Button::new("应用数据同步")).on_hover_text("应用前重读目标与元数据；发现变化时拒绝同步。").clicked() {
                    let generation = Uuid::new_v4(); document.pending = Some(generation); document.confirmed = false;
                    action = Some(CompareAction::Apply { generation,report:report.clone() });
                }
                egui::ScrollArea::both().id_salt("sync_preview").show(ui,|ui| { ui.monospace(preview); });
            },
            Section::Er => { show_er(ui,&report.target.endpoint.table,&report.target.structure); },
        }
    });
    action
}

/// Actual selected-table foreign keys. Compound columns are grouped by the
/// real constraint name; no inferred joins or fabricated relationships.
pub fn show_er(ui: &mut egui::Ui, table: &Table, structure: &TableStructure) {
    let relations = match er_relations(structure) {
        Ok(relations) => relations,
        Err(error) => {
            ui.colored_label(ui.visuals().error_fg_color, format!("{error:#}"));
            return;
        }
    };
    ui.label(format!(
        "{}.{} · {} 个外键",
        table.schema,
        table.name,
        relations.len()
    ));
    if relations.is_empty() {
        ui.label("暂无外键关系。");
        return;
    }
    egui::ScrollArea::both()
        .id_salt("actual_er")
        .show(ui, |ui| {
            let height = (relations.len() as f32 * 108.0).max(150.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(800.0, height), egui::Sense::hover());
            let painter = ui.painter_at(rect);
            let node_fill = ui.visuals().faint_bg_color;
            let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
            let text_color = ui.visuals().text_color();
            let source = egui::Rect::from_min_size(
                rect.left_top() + egui::vec2(8.0, height / 2.0 - 32.0),
                egui::vec2(245.0, 64.0),
            );
            painter.rect(source, 5.0, node_fill, stroke, egui::StrokeKind::Inside);
            painter.text(
                source.center(),
                egui::Align2::CENTER_CENTER,
                format!("{}.{}", table.schema, table.name),
                egui::FontId::proportional(14.0),
                text_color,
            );
            for (index, relation) in relations.iter().enumerate() {
                let target = egui::Rect::from_min_size(
                    rect.left_top() + egui::vec2(535.0, index as f32 * 108.0 + 18.0),
                    egui::vec2(250.0, 72.0),
                );
                painter.rect(target, 5.0, node_fill, stroke, egui::StrokeKind::Inside);
                painter.text(
                    target.center(),
                    egui::Align2::CENTER_CENTER,
                    format!(
                        "{}.{}\n{}",
                        relation.referenced_schema,
                        relation.referenced_table,
                        relation.referenced_columns.join(", ")
                    ),
                    egui::FontId::proportional(13.0),
                    text_color,
                );
                let start = source.right_center();
                let end = target.left_center();
                let middle = egui::pos2(rect.left() + 388.0, end.y);
                painter.line_segment(
                    [start, middle],
                    egui::Stroke::new(1.5, ui.visuals().selection.stroke.color),
                );
                painter.arrow(
                    middle,
                    end - middle,
                    egui::Stroke::new(1.5, ui.visuals().selection.stroke.color),
                );
                painter.text(
                    egui::pos2(rect.left() + 386.0, end.y - 8.0),
                    egui::Align2::CENTER_BOTTOM,
                    format!("{}\n{}", relation.name, relation.local_columns.join(", ")),
                    egui::FontId::proportional(11.0),
                    text_color,
                );
            }
        });
}
