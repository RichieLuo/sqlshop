//! Inline diagnostics, execution plans and permission editor documents.
//! All database work is returned as a typed host action.
mod accounts;
use accounts::*;

use super::{accent, chrome};
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use sqlshop::{
    advanced::{
        AccountPreview, AdvancedDocument, AdvancedKind, AdvancedOperation, SecurityPreview,
    },
    db::{account_change_preview, security_change_sql},
    diagnostics::DiagnosticKind,
    model::{Engine, QueryOutput, Table},
    security::{
        AccessPrincipal, AccountChange, AccountPassword, PrincipalKind, SecurityChange,
        TablePrivilege,
    },
};

pub(super) enum AdvancedAction {
    Run(AdvancedOperation),
}

pub(super) fn show(ui: &mut egui::Ui, doc: &mut AdvancedDocument) -> Option<AdvancedAction> {
    let mut action = None;
    ui.push_id(doc.id, |ui| {
        egui::Frame::new()
            .fill(chrome(ui.ctx()))
            .inner_margin(7)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(doc.kind.label());
                    ui.separator();
                    ui.label(
                        RichText::new(format!(
                            "{} / {}{}",
                            doc.source.profile.name,
                            doc.source
                                .target
                                .database
                                .as_deref()
                                .unwrap_or("未选择数据库"),
                            doc.source
                                .target
                                .schema
                                .as_ref()
                                .map_or(String::new(), |schema| format!(" / {schema}"))
                        ))
                        .weak(),
                    );
                    if let AdvancedKind::Diagnostics(kind) = doc.kind {
                        let mut selected = kind;
                        egui::ComboBox::from_id_salt("diagnostic-kind")
                            .selected_text(selected.label())
                            .show_ui(ui, |ui| {
                                for kind in DiagnosticKind::ALL {
                                    ui.selectable_value(&mut selected, kind, kind.label());
                                }
                            });
                        if selected != kind && !doc.busy {
                            doc.kind = AdvancedKind::Diagnostics(selected);
                            action = Some(AdvancedAction::Run(
                                AdvancedOperation::RefreshDiagnostics(selected),
                            ));
                        }
                    }
                    if ui
                        .add_enabled(
                            !doc.busy,
                            egui::Button::new(if doc.kind == AdvancedKind::Plan {
                                "解释查询"
                            } else {
                                "刷新"
                            }),
                        )
                        .on_hover_text(if doc.kind == AdvancedKind::Plan {
                            "单个 SELECT / 只读 WITH；估算计划，不启用 ANALYZE。"
                        } else {
                            "读取最新快照。"
                        })
                        .clicked()
                    {
                        action = Some(AdvancedAction::Run(doc.refresh_operation()));
                    }
                    if doc.busy {
                        ui.spinner();
                        ui.label("正在处理…");
                    }
                });
            });
        if let Some(error) = &doc.error {
            ui.add(
                egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color))
                    .selectable(true)
                    .wrap(),
            );
        }
        if let Some(notice) = &doc.notice {
            ui.add(egui::Label::new(notice).selectable(true).wrap());
        }
        if doc.output.is_some() && (doc.busy || doc.error.is_some()) {
            ui.label(RichText::new("以下保留上次成功读取的快照。").small().weak());
        }
        if doc.outcome_unknown {
            egui::Frame::group(ui.style())
                .inner_margin(7)
                .show(ui, |ui| {
                    ui.strong("需要核实上次账号或权限变更");
                    ui.label("先刷新账号与授权，核对实际状态；确认前不能再次提交权限修改。");
                    let mut verified = false;
                    if ui
                        .checkbox(&mut verified, "我已读取最新授权并核实实际结果")
                        .changed()
                        && verified
                    {
                        doc.outcome_unknown = false;
                        doc.error = None;
                        doc.notice =
                            Some("上次变更结果已由用户核实；可以重新预览新的修改。".into());
                    }
                });
        }
        match doc.kind {
            AdvancedKind::Plan => show_plan(ui, doc),
            AdvancedKind::Diagnostics(_) => {
                if let Some(output) = &doc.output {
                    let height = ui.available_height().max(140.);
                    show_output(ui, doc.id, &mut doc.result_index, output, height);
                } else {
                    ui.label(RichText::new("暂无诊断快照。").weak());
                }
            }
            AdvancedKind::Security => show_security(ui, doc, &mut action),
        }
    });
    action
}

fn show_plan(ui: &mut egui::Ui, doc: &mut AdvancedDocument) {
    ui.add_enabled(
        !doc.busy,
        egui::TextEdit::multiline(&mut doc.sql)
            .code_editor()
            .desired_rows(5)
            .desired_width(f32::INFINITY),
    );
    ui.separator();
    if doc.last_plan_sql.as_deref() != Some(doc.sql.as_str()) && doc.output.is_some() {
        ui.label(
            RichText::new("当前 SQL 已修改；显示上次读取的执行计划。")
                .small()
                .weak(),
        );
    }
    let Some(output) = &doc.output else {
        ui.label(RichText::new("暂无执行计划。").weak());
        return;
    };
    let raw = output
        .sets
        .iter()
        .flat_map(|set| &set.rows)
        .flat_map(|row| row.iter().flatten())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let json = serde_json::from_str::<serde_json::Value>(&raw).ok();
    ui.horizontal(|ui| {
        if json.is_some() {
            ui.selectable_value(&mut doc.plan_tree, true, "计划树");
        }
        ui.selectable_value(&mut doc.plan_tree, false, "原始计划");
        if ui.button("复制计划").clicked() {
            ui.ctx().copy_text(raw.clone());
        }
        ui.label(
            RichText::new(format!("规划耗时 {} ms", output.elapsed_ms))
                .small()
                .weak(),
        );
    });
    egui::ScrollArea::both()
        .id_salt(("plan-result", doc.id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if doc.plan_tree
                && let Some(json) = &json
            {
                let mut budget = 1000usize;
                json_tree(ui, "计划", json, 0, &mut budget);
                if budget == 0 {
                    ui.label(
                        RichText::new("计划树显示达到 1000 个节点；完整内容见原始计划。").weak(),
                    );
                }
            } else {
                let pretty = json
                    .as_ref()
                    .and_then(|value| serde_json::to_string_pretty(value).ok())
                    .unwrap_or(raw);
                ui.add(egui::Label::new(RichText::new(pretty).monospace()).selectable(true));
            }
        });
}

fn json_tree(
    ui: &mut egui::Ui,
    name: &str,
    value: &serde_json::Value,
    depth: usize,
    budget: &mut usize,
) {
    if *budget == 0 || depth > 32 {
        return;
    }
    *budget -= 1;
    match value {
        serde_json::Value::Object(map) => {
            let title = map
                .get("Node Type")
                .and_then(|value| value.as_str())
                .map_or_else(
                    || name.to_owned(),
                    |node| {
                        format!(
                            "{node}{}{}",
                            map.get("Relation Name")
                                .and_then(|value| value.as_str())
                                .map_or(String::new(), |name| format!(" · {name}")),
                            map.get("Total Cost")
                                .map_or(String::new(), |cost| format!(" · 成本 {cost}"))
                        )
                    },
                );
            egui::CollapsingHeader::new(title)
                .id_salt((depth, name))
                .default_open(depth < 3)
                .show(ui, |ui| {
                    for (key, value) in map {
                        json_tree(ui, key, value, depth + 1, budget);
                    }
                });
        }
        serde_json::Value::Array(values) => {
            egui::CollapsingHeader::new(format!("{name} [{}]", values.len()))
                .id_salt((depth, name))
                .default_open(depth < 3)
                .show(ui, |ui| {
                    for (index, value) in values.iter().enumerate() {
                        ui.push_id(index, |ui| {
                            json_tree(ui, &format!("节点 {}", index + 1), value, depth + 1, budget)
                        });
                    }
                });
        }
        _ => {
            ui.add(egui::Label::new(format!("{name}: {value}")).selectable(true));
        }
    }
}

fn show_security(
    ui: &mut egui::Ui,
    doc: &mut AdvancedDocument,
    action: &mut Option<AdvancedAction>,
) {
    let Some(catalog) = doc.catalog.as_ref() else {
        ui.label(RichText::new("暂无账号与权限快照。").weak());
        return;
    };
    let principals = catalog.principals.clone();
    for warning in &catalog.warnings {
        ui.add(egui::Label::new(RichText::new(warning).small().weak()).wrap());
    }
    let height = ui.available_height().max(180.);
    let side_width = (ui.available_width() * 0.3).clamp(190., 260.);
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(side_width, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut doc.search)
                        .hint_text("搜索账号或角色")
                        .desired_width(f32::INFINITY),
                );
                ui.label(
                    RichText::new(format!("{} 个可见账号与角色", principals.len()))
                        .small()
                        .weak(),
                );
                let search = doc.search.trim().to_lowercase();
                egui::ScrollArea::vertical()
                    .id_salt("security-principals")
                    .auto_shrink([false, false])
                    .max_height((height - 56.).max(100.))
                    .show(ui, |ui| {
                        for principal in &principals {
                            if !search.is_empty()
                                && !principal.display_name().to_lowercase().contains(&search)
                                && !principal.kind.label().contains(&search)
                            {
                                continue;
                            }
                            let selected = doc.selected_principal.as_ref() == Some(principal);
                            if ui
                                .add_enabled(
                                    !doc.busy,
                                    egui::Button::new(format!(
                                        "{}  {}",
                                        principal.kind.label(),
                                        principal.display_name()
                                    ))
                                    .selected(selected)
                                    .frame(selected),
                                )
                                .clicked()
                            {
                                doc.selected_principal = Some(principal.clone());
                                doc.output = None;
                                doc.preview = None;
                                doc.account_preview = None;
                                clear_account_passwords(doc);
                                if principal.kind != PrincipalKind::Login {
                                    doc.member = Some(principal.clone());
                                    if doc.source.profile.engine != Engine::SqlServer
                                        || principal.kind == PrincipalKind::Role
                                    {
                                        doc.role = Some(principal.clone());
                                    }
                                }
                                *action = Some(AdvancedAction::Run(AdvancedOperation::LoadGrants(
                                    principal.clone(),
                                )));
                            }
                        }
                    });
            },
        );
        ui.separator();
        ui.vertical(|ui| {
            ui.set_max_width(ui.available_width());
            egui::ScrollArea::vertical()
                .id_salt("security-details")
                .auto_shrink([false, false])
                .max_height(height)
                .show(ui, |ui| {
                    if let Some(principal) = doc.selected_principal.clone() {
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(
                                accent(ui),
                                RichText::new(principal.display_name()).strong(),
                            );
                            ui.label(principal.kind.label());
                            if let Some(state) =
                                principal_state(doc.source.profile.engine, &principal)
                            {
                                ui.label(RichText::new(state).weak());
                            }
                            if let Some(schema) = &principal.default_schema {
                                ui.label(format!("默认 Schema：{schema}"));
                            }
                        });
                        if ui
                            .add_enabled(!doc.busy, egui::Button::new("读取最新授权"))
                            .clicked()
                        {
                            *action = Some(AdvancedAction::Run(AdvancedOperation::LoadGrants(
                                principal,
                            )));
                        }
                    } else {
                        ui.label(RichText::new("未选择账号。").weak());
                    }
                    egui::CollapsingHeader::new("角色与权限操作")
                        .id_salt("security-edit")
                        .default_open(false)
                        .show(ui, |ui| show_security_form(ui, doc, &principals));
                    egui::CollapsingHeader::new("账号与登录管理")
                        .id_salt("account-edit")
                        .default_open(false)
                        .show(ui, |ui| show_account_form(ui, doc, &principals));
                    if let Some(preview) = doc.account_preview.take() {
                        let mut confirm = false;
                        let mut cancel = false;
                        egui::Frame::group(ui.style())
                            .inner_margin(8)
                            .show(ui, |ui| {
                                ui.strong("确认账号修改");
                                ui.label(format!(
                                    "目标：{} / {}",
                                    preview.source.profile.name,
                                    preview
                                        .source
                                        .target
                                        .database
                                        .as_deref()
                                        .unwrap_or("未选择数据库")
                                ));
                                if matches!(
                                    preview.change,
                                    AccountChange::CreateDatabaseUser { .. }
                                ) {
                                    ui.label("仅修改当前数据库用户映射。");
                                }
                                egui::ScrollArea::both()
                                    .id_salt("account-preview")
                                    .max_height(110.)
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&preview.sql).monospace(),
                                            )
                                            .selectable(true),
                                        );
                                    });
                                if matches!(preview.change, AccountChange::DropUser { .. }) {
                                    ui.label(RichText::new("删除此用户及其授权。").small().weak());
                                }
                                if matches!(preview.change, AccountChange::ChangePassword { .. }) {
                                    ui.label("修改密码后需更新保存的连接凭据。");
                                }
                                if matches!(preview.change, AccountChange::SetLogin { .. }) {
                                    ui.label("登录状态影响后续连接。");
                                }
                                ui.horizontal(|ui| {
                                    if ui
                                        .add_enabled(
                                            !doc.busy && !doc.outcome_unknown,
                                            egui::Button::new("确认执行账号修改"),
                                        )
                                        .clicked()
                                    {
                                        confirm = true;
                                    }
                                    if ui
                                        .add_enabled(!doc.busy, egui::Button::new("取消修改"))
                                        .clicked()
                                    {
                                        cancel = true;
                                    }
                                });
                            });
                        if confirm {
                            if !doc.source.same_binding(&preview.source) {
                                doc.error = Some("预览目标已变化，请重新预览。".into());
                            } else {
                                *action =
                                    Some(AdvancedAction::Run(AdvancedOperation::ApplyAccount {
                                        change: preview.change,
                                        preview_sql: preview.sql,
                                        preview_source: Box::new(preview.source),
                                    }));
                            }
                            clear_account_passwords(doc);
                        } else if cancel {
                            clear_account_passwords(doc);
                        } else {
                            doc.account_preview = Some(preview);
                        }
                    }
                    if let Some(preview) = doc.preview.clone() {
                        egui::Frame::group(ui.style())
                            .inner_margin(8)
                            .show(ui, |ui| {
                                ui.strong("确认权限修改");
                                ui.label(format!(
                                    "目标：{} / {}",
                                    doc.source.profile.name,
                                    doc.source
                                        .target
                                        .database
                                        .as_deref()
                                        .unwrap_or("未选择数据库")
                                ));
                                egui::ScrollArea::both()
                                    .id_salt("security-preview")
                                    .max_height(110.)
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&preview.sql).monospace(),
                                            )
                                            .selectable(true),
                                        );
                                    });
                                if matches!(preview.change, SecurityChange::DropRole { .. }) {
                                    ui.label(
                                        RichText::new(
                                            if doc.source.profile.engine == Engine::MySql {
                                                "删除此 User@Host 及其授权和角色关系。"
                                            } else {
                                                "删除此角色；服务器将检查依赖与所有权。"
                                            },
                                        )
                                        .small()
                                        .weak(),
                                    );
                                }
                                ui.horizontal(|ui| {
                                    if ui
                                        .add_enabled(
                                            !doc.busy && !doc.outcome_unknown,
                                            egui::Button::new("确认执行权限修改"),
                                        )
                                        .clicked()
                                    {
                                        *action = Some(AdvancedAction::Run(
                                            AdvancedOperation::ApplySecurity {
                                                change: preview.change.clone(),
                                                preview_sql: preview.sql.clone(),
                                            },
                                        ));
                                    }
                                    if ui
                                        .add_enabled(!doc.busy, egui::Button::new("返回编辑"))
                                        .clicked()
                                    {
                                        doc.preview = None;
                                    }
                                    if ui.button("复制 SQL").clicked() {
                                        ui.ctx().copy_text(preview.sql.clone());
                                    }
                                });
                            });
                    }
                    ui.separator();
                    if let Some(output) = &doc.output {
                        show_output(ui, doc.id, &mut doc.result_index, output, 260.);
                    } else if !doc.busy {
                        ui.label(RichText::new("暂无授权快照。").weak());
                    }
                });
        });
    });
}

fn show_security_form(
    ui: &mut egui::Ui,
    doc: &mut AdvancedDocument,
    principals: &[AccessPrincipal],
) {
    let engine = doc.source.profile.engine;
    ui.add_enabled_ui(
        !doc.busy && !doc.outcome_unknown && doc.preview.is_none() && doc.account_preview.is_none(),
        |ui| {
            ui.horizontal_wrapped(|ui| {
                for (index, label) in ["创建角色", "删除角色", "角色成员", "表授权"]
                    .iter()
                    .enumerate()
                {
                    ui.selectable_value(&mut doc.security_form, index, *label);
                }
            });
            let roles: Vec<_> = principals
                .iter()
                .filter(|principal| {
                    principal.kind != PrincipalKind::Login
                        && (engine != Engine::SqlServer || principal.kind == PrincipalKind::Role)
                })
                .cloned()
                .collect();
            let members: Vec<_> = principals
                .iter()
                .filter(|principal| principal.kind != PrincipalKind::Login)
                .cloned()
                .collect();
            match doc.security_form {
                0 => {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("角色名称");
                        let name = ui.add(
                            egui::TextEdit::singleline(&mut doc.role_name).desired_width(180.),
                        );
                        if engine == Engine::MySql {
                            name.on_hover_text("新角色使用 Host %，不创建登录凭据。");
                        }
                        if ui.button("预览创建").clicked() {
                            let change = SecurityChange::CreateRole {
                                name: doc.role_name.clone(),
                            };
                            preview_change(doc, change);
                        }
                    });
                }
                1 => {
                    principal_choice(ui, "删除角色", "role-drop", &mut doc.role, &roles);
                    if ui
                        .add_enabled(doc.role.is_some(), egui::Button::new("预览删除"))
                        .clicked()
                        && let Some(role) = doc.role.clone()
                    {
                        preview_change(doc, SecurityChange::DropRole { role });
                    }
                }
                2 => {
                    principal_choice(ui, "角色", "role-membership", &mut doc.role, &roles);
                    principal_choice(ui, "成员", "role-member", &mut doc.member, &members);
                    ui.horizontal(|ui| {
                        for (grant, label) in [(true, "预览授予角色"), (false, "预览撤销角色")]
                        {
                            if ui
                                .add_enabled(
                                    doc.role.is_some() && doc.member.is_some(),
                                    egui::Button::new(label),
                                )
                                .clicked()
                                && let (Some(role), Some(member)) =
                                    (doc.role.clone(), doc.member.clone())
                            {
                                preview_change(
                                    doc,
                                    if grant {
                                        SecurityChange::GrantRole { role, member }
                                    } else {
                                        SecurityChange::RevokeRole { role, member }
                                    },
                                );
                            }
                        }
                    });
                }
                _ => {
                    let principal = doc
                        .selected_principal
                        .clone()
                        .filter(|principal| principal.kind != PrincipalKind::Login);
                    ui.label(format!(
                        "授予对象：{}",
                        principal.as_ref().map_or_else(
                            || "请选择数据库用户或角色".into(),
                            AccessPrincipal::display_name
                        )
                    ));
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Schema");
                        ui.add(
                            egui::TextEdit::singleline(&mut doc.table_schema).desired_width(100.),
                        );
                        ui.label("表");
                        ui.add(egui::TextEdit::singleline(&mut doc.table_name).desired_width(150.));
                    });
                    egui::ComboBox::from_id_salt("table-privilege")
                        .selected_text(doc.privilege.sql())
                        .show_ui(ui, |ui| {
                            for privilege in TablePrivilege::ALL {
                                ui.selectable_value(&mut doc.privilege, privilege, privilege.sql());
                            }
                        });
                    ui.horizontal(|ui| {
                        for (grant, label) in [(true, "预览授予权限"), (false, "预览撤销权限")]
                        {
                            if ui
                                .add_enabled(
                                    principal.is_some() && !doc.table_name.trim().is_empty(),
                                    egui::Button::new(label),
                                )
                                .clicked()
                            {
                                let principal = principal.clone().unwrap();
                                let table = Table {
                                    schema: doc.table_schema.clone(),
                                    name: doc.table_name.clone(),
                                    kind: "BASE TABLE".into(),
                                };
                                let privilege = doc.privilege;
                                preview_change(
                                    doc,
                                    if grant {
                                        SecurityChange::GrantTablePrivilege {
                                            principal,
                                            table,
                                            privilege,
                                        }
                                    } else {
                                        SecurityChange::RevokeTablePrivilege {
                                            principal,
                                            table,
                                            privilege,
                                        }
                                    },
                                );
                            }
                        }
                    });
                }
            }
        },
    );
}

fn preview_change(doc: &mut AdvancedDocument, change: SecurityChange) {
    match security_change_sql(doc.source.profile.engine, &change) {
        Ok(sql) => {
            doc.preview = Some(SecurityPreview { change, sql });
            doc.error = None;
        }
        Err(error) => {
            doc.error = Some(error.to_string());
        }
    }
}

fn show_output(
    ui: &mut egui::Ui,
    id: uuid::Uuid,
    result_index: &mut usize,
    output: &QueryOutput,
    height: f32,
) {
    for message in &output.messages {
        ui.add(egui::Label::new(RichText::new(&message.text).small().weak()).wrap());
    }
    if output.sets.is_empty() {
        ui.label(RichText::new("服务器未返回数据结果。").weak());
        return;
    }
    *result_index = (*result_index).min(output.sets.len() - 1);
    ui.horizontal_wrapped(|ui| {
        for (index, set) in output.sets.iter().enumerate() {
            ui.selectable_value(
                result_index,
                index,
                format!("结果 {} · {} 行", index + 1, set.rows.len()),
            );
        }
        ui.label(
            RichText::new(format!("{} ms", output.elapsed_ms))
                .small()
                .weak(),
        );
    });
    let set = &output.sets[*result_index];
    ui.horizontal(|ui| {
        if set.truncated {
            ui.label(RichText::new("已达到 1000 行显示上限。").weak());
        }
        if ui.button("复制当前结果").clicked() {
            let mut text = set.columns.join("\t");
            for row in &set.rows {
                text.push('\n');
                text.push_str(
                    &row.iter()
                        .map(|cell| cell.as_deref().unwrap_or("NULL"))
                        .collect::<Vec<_>>()
                        .join("\t"),
                );
            }
            ui.ctx().copy_text(text);
        }
    });
    if set.columns.is_empty() {
        ui.label("此结果没有列元数据。");
        return;
    }
    let width = ui.available_width();
    egui::ScrollArea::horizontal()
        .id_salt(("advanced-grid-x", id, *result_index))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width((set.columns.len() as f32 * 145.).max(width));
            let mut table = TableBuilder::new(ui)
                .id_salt(("advanced-grid", id, *result_index))
                .striped(true)
                .resizable(true)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
            for _ in &set.columns {
                table = table.column(Column::initial(145.).at_least(65.));
            }
            table
                .max_scroll_height(height.max(80.))
                .min_scrolled_height(80.)
                .header(25., |mut header| {
                    for column in &set.columns {
                        header.col(|ui| {
                            ui.label(RichText::new(column).strong().small());
                        });
                    }
                })
                .body(|body| {
                    body.rows(25., set.rows.len(), |mut row| {
                        let index = row.index();
                        for column in 0..set.columns.len() {
                            row.col(|ui| {
                                let value = set.rows[index].get(column).and_then(Option::as_deref);
                                let text = value.unwrap_or("NULL");
                                let response = ui
                                    .add(
                                        egui::Label::new(if value.is_none() {
                                            RichText::new(text).weak()
                                        } else {
                                            RichText::new(text)
                                        })
                                        .truncate(),
                                    )
                                    .on_hover_text(text);
                                response.context_menu(|ui| {
                                    if ui.button("复制单元格").clicked() {
                                        ui.ctx().copy_text(text.to_owned());
                                        ui.close();
                                    }
                                });
                            });
                        }
                    });
                });
        });
}
