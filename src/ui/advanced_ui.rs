//! Inline diagnostics, execution plans and permission editor documents.
//! All database work is returned as a typed host action.
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
                    ui.label(RichText::new("点击刷新，读取当前数据库的服务器诊断快照。").weak());
                }
            }
            AdvancedKind::Security => show_security(ui, doc, &mut action),
        }
    });
    action
}

fn show_plan(ui: &mut egui::Ui, doc: &mut AdvancedDocument) {
    ui.label(
        RichText::new("单个 SELECT / 只读 WITH · 估算计划 · 不启用 ANALYZE")
            .small()
            .weak(),
    );
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
        ui.label(RichText::new("输入查询后点击“解释查询”。").weak());
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
        ui.label(RichText::new("点击刷新，读取账号、数据库角色及当前账号可见的权限。").weak());
        return;
    };
    let principals = catalog.principals.clone();
    for warning in &catalog.warnings {
        ui.add(egui::Label::new(RichText::new(warning).small().weak()).wrap());
    }
    let height = ui.available_height().max(180.);
    let side_width = (ui.available_width() * 0.3).clamp(190., 260.);
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(egui::vec2(side_width,height),egui::Layout::top_down(egui::Align::Min),|ui| {
            ui.add(egui::TextEdit::singleline(&mut doc.search).hint_text("搜索账号或角色").desired_width(f32::INFINITY));
            ui.label(RichText::new(format!("{} 个可见账号与角色",principals.len())).small().weak());
            let search=doc.search.trim().to_lowercase();
            egui::ScrollArea::vertical().id_salt("security-principals").auto_shrink([false,false]).max_height((height-56.).max(100.)).show(ui,|ui| {
                for principal in &principals {
                    if !search.is_empty() && !principal.display_name().to_lowercase().contains(&search) && !principal.kind.label().contains(&search) {continue;}
                    let selected=doc.selected_principal.as_ref()==Some(principal);
                    if ui.add_enabled(!doc.busy,egui::Button::new(format!("{}  {}",principal.kind.label(),principal.display_name())).selected(selected).frame(selected)).clicked() {
                        doc.selected_principal=Some(principal.clone());doc.output=None;doc.preview=None;doc.account_preview=None;clear_account_passwords(doc);
                        if principal.kind!=PrincipalKind::Login {doc.member=Some(principal.clone());if doc.source.profile.engine!=Engine::SqlServer || principal.kind==PrincipalKind::Role {doc.role=Some(principal.clone());}}
                        *action=Some(AdvancedAction::Run(AdvancedOperation::LoadGrants(principal.clone())));
                    }
                }
            });
        });
        ui.separator();
        ui.vertical(|ui| {
            ui.set_max_width(ui.available_width());
            egui::ScrollArea::vertical().id_salt("security-details").auto_shrink([false,false]).max_height(height).show(ui,|ui| {
                if let Some(principal)=doc.selected_principal.clone() {
                    ui.horizontal_wrapped(|ui| {ui.colored_label(accent(ui),RichText::new(principal.display_name()).strong());ui.label(principal.kind.label());if let Some(state)=principal_state(doc.source.profile.engine,&principal) {ui.label(RichText::new(state).weak());}
                        if let Some(schema)=&principal.default_schema {ui.label(format!("默认 Schema：{schema}"));}});
                    if ui.add_enabled(!doc.busy,egui::Button::new("读取最新授权")).clicked() {*action=Some(AdvancedAction::Run(AdvancedOperation::LoadGrants(principal)));}
                } else {ui.label(RichText::new("选择左侧账号查看授权；可在下方创建角色。").weak());}
                egui::CollapsingHeader::new("角色与权限操作").id_salt("security-edit").default_open(false).show(ui,|ui|show_security_form(ui,doc,&principals));
                egui::CollapsingHeader::new("账号与登录管理").id_salt("account-edit").default_open(false).show(ui,|ui|show_account_form(ui,doc,&principals));
                if let Some(preview)=doc.account_preview.take() {
                    let mut confirm=false;let mut cancel=false;
                    egui::Frame::group(ui.style()).inner_margin(8).show(ui,|ui| {
                        ui.strong("确认账号修改");
                        ui.label(format!("目标：{} / {}",preview.source.profile.name,preview.source.target.database.as_deref().unwrap_or("未选择数据库")));
                        ui.label(RichText::new(if matches!(preview.change,AccountChange::CreateDatabaseUser{..}) {"数据库用户映射仅作用于当前数据库。"} else {"账号与登录状态作用于服务器或集群；密码已从 SQL 预览中遮盖。"}).small().weak());
                        egui::ScrollArea::both().id_salt("account-preview").max_height(110.).show(ui,|ui|{ui.add(egui::Label::new(RichText::new(&preview.sql).monospace()).selectable(true));});
                        if matches!(preview.change,AccountChange::DropUser{..}) {ui.label(RichText::new("删除账号或数据库用户会影响其授权；数据库依赖与所有权由服务器核实。").small().weak());}
                        ui.horizontal(|ui| {
                            if ui.add_enabled(!doc.busy&&!doc.outcome_unknown,egui::Button::new("确认执行账号修改")).clicked() {
                                confirm=true;
                            }
                            if ui.add_enabled(!doc.busy,egui::Button::new("取消修改")).clicked(){cancel=true;}
                        });
                    });
                    if confirm {
                        if !doc.source.same_binding(&preview.source) {doc.error=Some("预览目标已变化，请重新预览。".into());} else {*action=Some(AdvancedAction::Run(AdvancedOperation::ApplyAccount{change:preview.change,preview_sql:preview.sql,preview_source:Box::new(preview.source)}));}
                        clear_account_passwords(doc);
                    }else if cancel {clear_account_passwords(doc);}else {doc.account_preview=Some(preview);}
                }
                if let Some(preview)=doc.preview.clone() {
                    egui::Frame::group(ui.style()).inner_margin(8).show(ui,|ui| {
                        ui.strong("确认权限修改");ui.label(format!("目标：{} / {}",doc.source.profile.name,doc.source.target.database.as_deref().unwrap_or("未选择数据库")));
                        egui::ScrollArea::both().id_salt("security-preview").max_height(110.).show(ui,|ui| {ui.add(egui::Label::new(RichText::new(&preview.sql).monospace()).selectable(true));});
                        if matches!(preview.change,SecurityChange::DropRole{..}) {ui.label(RichText::new(if doc.source.profile.engine==Engine::MySql {"删除角色会删除该 User@Host 账号及其授权和角色关系。"} else {"删除角色遵循数据库依赖与所有权规则；请核对角色名称。"}).small().weak());}
                        ui.horizontal(|ui| {
                            if ui.add_enabled(!doc.busy&&!doc.outcome_unknown,egui::Button::new("确认执行权限修改")).clicked() {*action=Some(AdvancedAction::Run(AdvancedOperation::ApplySecurity{change:preview.change.clone(),preview_sql:preview.sql.clone()}));}
                            if ui.add_enabled(!doc.busy,egui::Button::new("返回编辑")).clicked() {doc.preview=None;}
                            if ui.button("复制 SQL").clicked() {ui.ctx().copy_text(preview.sql.clone());}
                        });
                    });
                }
                ui.separator();
                if let Some(output)=&doc.output {show_output(ui,doc.id,&mut doc.result_index,output,260.);} else if !doc.busy {ui.label(RichText::new("暂无授权快照。直接授权与成员关系不等于全部有效权限。").weak());}
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
                        ui.add(egui::TextEdit::singleline(&mut doc.role_name).desired_width(180.));
                        if ui.button("预览创建").clicked() {
                            let change = SecurityChange::CreateRole {
                                name: doc.role_name.clone(),
                            };
                            preview_change(doc, change);
                        }
                    });
                    if engine == Engine::MySql {
                        ui.label(
                            RichText::new("新角色使用 Host %；不创建密码或登录凭据。")
                                .small()
                                .weak(),
                        );
                    }
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

fn clear_account_passwords(doc: &mut AdvancedDocument) {
    doc.account_password.clear();
    doc.account_password_confirm.clear();
    doc.account_old_password.clear();
}

fn preview_account(doc: &mut AdvancedDocument, change: AccountChange) {
    match account_change_preview(doc.source.profile.engine, &change) {
        Ok(sql) => {
            doc.account_preview = Some(AccountPreview {
                change,
                sql,
                source: doc.source.clone(),
            });
            doc.error = None;
        }
        Err(error) => doc.error = Some(error.to_string()),
    }
    clear_account_passwords(doc);
}

fn show_account_form(
    ui: &mut egui::Ui,
    doc: &mut AdvancedDocument,
    principals: &[AccessPrincipal],
) {
    let engine = doc.source.profile.engine;
    ui.add_enabled_ui(!doc.busy&&!doc.outcome_unknown&&doc.preview.is_none()&&doc.account_preview.is_none(),|ui| {
        let labels:&[&str]=match engine {Engine::MySql=>&["创建 User@Host","修改密码","登录锁定","删除账号"],Engine::Postgres=>&["创建登录用户","修改密码","LOGIN 权限","删除用户"],Engine::SqlServer=>&["创建 SQL 登录","修改登录密码","启禁登录","删除数据库用户","映射数据库用户"]};
        ui.horizontal_wrapped(|ui| {for(index,label)in labels.iter().enumerate(){if ui.selectable_value(&mut doc.account_form,index,*label).changed(){clear_account_passwords(doc);}}});
        let selected=doc.selected_principal.clone();
        if let Some(principal)=&selected {ui.label(format!("当前账号：{} · {}",principal.display_name(),principal.kind.label()));}
        let mut preview=false;
        match doc.account_form {
            0=>{
                ui.horizontal_wrapped(|ui|{ui.label(if engine==Engine::SqlServer{"登录名"}else{"用户名"});ui.add(egui::TextEdit::singleline(&mut doc.account_name).desired_width(180.));if engine==Engine::MySql {ui.label("Host");ui.add(egui::TextEdit::singleline(&mut doc.account_host).desired_width(110.));}});
                password_fields(ui,doc,false);
                preview=ui.button("预览创建账号").clicked();
            }
            1=>{
                let supported=selected.as_ref().is_some_and(|principal|engine!=Engine::SqlServer || (principal.kind==PrincipalKind::Login && principal.identity.as_ref().is_some_and(|id|id.starts_with("login:S:"))));
                if engine==Engine::SqlServer && !supported {ui.label("请选择 SQL Server 的 SQL 登录；Windows/Entra 密码由认证提供方管理。");}
                password_fields(ui,doc,engine==Engine::SqlServer);
                preview=ui.add_enabled(supported,egui::Button::new("预览修改密码")).clicked();
                ui.label(RichText::new("密码修改后，保存的连接凭据需要手动更新。").small().weak());
            }
            2=>{
                let supported=selected.as_ref().is_some_and(|principal|engine!=Engine::SqlServer || principal.kind==PrincipalKind::Login);
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(supported,egui::Button::new(if engine==Engine::MySql{"预览解除登录锁定"}else{"预览允许登录"})).clicked() && let Some(principal)=selected.clone() {preview_account(doc,AccountChange::SetLogin{principal,enabled:true});}
                    if ui.add_enabled(supported,egui::Button::new(if engine==Engine::MySql{"预览锁定登录"}else{"预览禁止登录"})).clicked() && let Some(principal)=selected.clone() {preview_account(doc,AccountChange::SetLogin{principal,enabled:false});}
                });
                ui.label(RichText::new("登录状态影响后续连接；角色成员关系与登录锁定分别管理。").small().weak());
            }
            3=>{
                let supported=selected.as_ref().is_some_and(|principal|engine!=Engine::SqlServer || principal.kind==PrincipalKind::User);
                if engine==Engine::SqlServer {ui.label(RichText::new("此操作删除当前数据库 USER；服务器 LOGIN 删除暂不支持。").small().weak());}
                if ui.add_enabled(supported,egui::Button::new("预览删除账号或用户")).clicked() && let Some(principal)=selected.clone() {preview_account(doc,AccountChange::DropUser{principal});}
            }
            _=>{
                ui.horizontal_wrapped(|ui|{ui.label("数据库用户名");ui.add(egui::TextEdit::singleline(&mut doc.account_name).desired_width(170.));});
                let logins:Vec<_>=principals.iter().filter(|principal|principal.kind==PrincipalKind::Login).cloned().collect();
                principal_choice(ui,"服务器登录","user-for-login",&mut doc.account_login,&logins);
                ui.horizontal_wrapped(|ui|{ui.label("默认 Schema");ui.add(egui::TextEdit::singleline(&mut doc.account_schema).desired_width(150.));});
                if ui.add_enabled(doc.account_login.is_some(),egui::Button::new("预览创建数据库用户映射")).clicked() && let Some(login)=doc.account_login.clone() {
                    let change=AccountChange::CreateDatabaseUser{name:doc.account_name.clone(),login,default_schema:(!doc.account_schema.trim().is_empty()).then(||doc.account_schema.clone())};preview_account(doc,change);
                }
            }
        }
        if preview {
            if doc.account_password!=doc.account_password_confirm {doc.error=Some("两次输入的密码不一致。".into());return;}
            let password=AccountPassword::new(std::mem::take(&mut doc.account_password));
            let change=if doc.account_form==0 {
                if engine==Engine::SqlServer {AccountChange::CreateLogin{name:doc.account_name.clone(),password}}else{AccountChange::CreateUser{name:doc.account_name.clone(),host:(engine==Engine::MySql).then(||doc.account_host.clone()),password}}
            }else if let Some(principal)=selected {
                let old_password=(!doc.account_old_password.is_empty()).then(||AccountPassword::new(std::mem::take(&mut doc.account_old_password)));
                AccountChange::ChangePassword{principal,password,old_password}
            }else{doc.error=Some("请选择目标账号。".into());clear_account_passwords(doc);return;};
            preview_account(doc,change);
        }
        ui.label(RichText::new("基础账号流程支持密码和登录状态；高级认证插件、DENY 与 GRANT OPTION 仍需独立管理。").small().weak());
    });
}

fn password_fields(ui: &mut egui::Ui, doc: &mut AdvancedDocument, old: bool) {
    ui.horizontal_wrapped(|ui| {
        ui.label("新密码");
        ui.add(
            egui::TextEdit::singleline(&mut doc.account_password)
                .password(true)
                .desired_width(190.)
                .char_limit(4096),
        );
        ui.label("确认密码");
        ui.add(
            egui::TextEdit::singleline(&mut doc.account_password_confirm)
                .password(true)
                .desired_width(190.)
                .char_limit(4096),
        );
    });
    if old {
        ui.horizontal_wrapped(|ui| {
            ui.label("旧密码（按服务器权限需要填写）");
            ui.add(
                egui::TextEdit::singleline(&mut doc.account_old_password)
                    .password(true)
                    .desired_width(190.)
                    .char_limit(128),
            );
        });
    }
}

fn principal_choice(
    ui: &mut egui::Ui,
    label: &str,
    id: &str,
    selected: &mut Option<AccessPrincipal>,
    principals: &[AccessPrincipal],
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(id)
            .selected_text(
                selected
                    .as_ref()
                    .map_or_else(|| "请选择".into(), AccessPrincipal::display_name),
            )
            .width(220.)
            .show_ui(ui, |ui| {
                for principal in principals {
                    let chosen = selected.as_ref() == Some(principal);
                    if ui
                        .selectable_label(chosen, principal.display_name())
                        .clicked()
                    {
                        *selected = Some(principal.clone());
                    }
                }
            });
    });
}

fn principal_state(engine: Engine, principal: &AccessPrincipal) -> Option<&'static str> {
    principal.enabled.map(|enabled| match engine {
        Engine::MySql => {
            if enabled {
                "登录未锁定"
            } else {
                "登录已锁定"
            }
        }
        Engine::Postgres => {
            if enabled {
                "允许 LOGIN"
            } else {
                "无 LOGIN 属性"
            }
        }
        Engine::SqlServer => {
            if enabled {
                "登录已启用"
            } else {
                "登录已禁用"
            }
        }
    })
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
