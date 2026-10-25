use super::*;

pub(super) fn clear_account_passwords(doc: &mut AdvancedDocument) {
    doc.account_password.clear();
    doc.account_password_confirm.clear();
    doc.account_old_password.clear();
}

pub(super) fn preview_account(doc: &mut AdvancedDocument, change: AccountChange) {
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

pub(super) fn show_account_form(
    ui: &mut egui::Ui,
    doc: &mut AdvancedDocument,
    principals: &[AccessPrincipal],
) {
    let engine = doc.source.profile.engine;
    ui.add_enabled_ui(
        !doc.busy && !doc.outcome_unknown && doc.preview.is_none() && doc.account_preview.is_none(),
        |ui| {
            let labels: &[&str] = match engine {
                Engine::MySql => &["创建 User@Host", "修改密码", "登录锁定", "删除账号"],
                Engine::Postgres => &["创建登录用户", "修改密码", "LOGIN 权限", "删除用户"],
                Engine::SqlServer => &[
                    "创建 SQL 登录",
                    "修改登录密码",
                    "启禁登录",
                    "删除数据库用户",
                    "映射数据库用户",
                ],
            };
            ui.horizontal_wrapped(|ui| {
                for (index, label) in labels.iter().enumerate() {
                    if ui
                        .selectable_value(&mut doc.account_form, index, *label)
                        .changed()
                    {
                        clear_account_passwords(doc);
                    }
                }
            });
            let selected = doc.selected_principal.clone();
            if let Some(principal) = &selected {
                ui.label(format!(
                    "当前账号：{} · {}",
                    principal.display_name(),
                    principal.kind.label()
                ));
            }
            let mut preview = false;
            match doc.account_form {
                0 => {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(if engine == Engine::SqlServer {
                            "登录名"
                        } else {
                            "用户名"
                        });
                        ui.add(
                            egui::TextEdit::singleline(&mut doc.account_name).desired_width(180.),
                        );
                        if engine == Engine::MySql {
                            ui.label("Host");
                            ui.add(
                                egui::TextEdit::singleline(&mut doc.account_host)
                                    .desired_width(110.),
                            );
                        }
                    });
                    password_fields(ui, doc, false);
                    preview = ui.button("预览创建账号").clicked();
                }
                1 => {
                    let supported = selected.as_ref().is_some_and(|principal| {
                        engine != Engine::SqlServer
                            || (principal.kind == PrincipalKind::Login
                                && principal
                                    .identity
                                    .as_ref()
                                    .is_some_and(|id| id.starts_with("login:S:")))
                    });
                    if engine == Engine::SqlServer && !supported {
                        ui.label(
                            "请选择 SQL Server 的 SQL 登录；Windows/Entra 密码由认证提供方管理。",
                        );
                    }
                    password_fields(ui, doc, engine == Engine::SqlServer);
                    preview = ui
                        .add_enabled(supported, egui::Button::new("预览修改密码"))
                        .clicked();
                }
                2 => {
                    let supported = selected.as_ref().is_some_and(|principal| {
                        engine != Engine::SqlServer || principal.kind == PrincipalKind::Login
                    });
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(
                                supported,
                                egui::Button::new(if engine == Engine::MySql {
                                    "预览解除登录锁定"
                                } else {
                                    "预览允许登录"
                                }),
                            )
                            .clicked()
                            && let Some(principal) = selected.clone()
                        {
                            preview_account(
                                doc,
                                AccountChange::SetLogin {
                                    principal,
                                    enabled: true,
                                },
                            );
                        }
                        if ui
                            .add_enabled(
                                supported,
                                egui::Button::new(if engine == Engine::MySql {
                                    "预览锁定登录"
                                } else {
                                    "预览禁止登录"
                                }),
                            )
                            .clicked()
                            && let Some(principal) = selected.clone()
                        {
                            preview_account(
                                doc,
                                AccountChange::SetLogin {
                                    principal,
                                    enabled: false,
                                },
                            );
                        }
                    });
                }
                3 => {
                    let supported = selected.as_ref().is_some_and(|principal| {
                        engine != Engine::SqlServer || principal.kind == PrincipalKind::User
                    });
                    if ui
                        .add_enabled(supported, egui::Button::new("预览删除账号或用户"))
                        .on_hover_text(if engine == Engine::SqlServer {
                            "仅删除当前数据库 USER，不删除服务器 LOGIN。"
                        } else {
                            "预览删除所选用户及其授权。"
                        })
                        .clicked()
                        && let Some(principal) = selected.clone()
                    {
                        preview_account(doc, AccountChange::DropUser { principal });
                    }
                }
                _ => {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("数据库用户名");
                        ui.add(
                            egui::TextEdit::singleline(&mut doc.account_name).desired_width(170.),
                        );
                    });
                    let logins: Vec<_> = principals
                        .iter()
                        .filter(|principal| principal.kind == PrincipalKind::Login)
                        .cloned()
                        .collect();
                    principal_choice(
                        ui,
                        "服务器登录",
                        "user-for-login",
                        &mut doc.account_login,
                        &logins,
                    );
                    ui.horizontal_wrapped(|ui| {
                        ui.label("默认 Schema");
                        ui.add(
                            egui::TextEdit::singleline(&mut doc.account_schema).desired_width(150.),
                        );
                    });
                    if ui
                        .add_enabled(
                            doc.account_login.is_some(),
                            egui::Button::new("预览创建数据库用户映射"),
                        )
                        .clicked()
                        && let Some(login) = doc.account_login.clone()
                    {
                        let change = AccountChange::CreateDatabaseUser {
                            name: doc.account_name.clone(),
                            login,
                            default_schema: (!doc.account_schema.trim().is_empty())
                                .then(|| doc.account_schema.clone()),
                        };
                        preview_account(doc, change);
                    }
                }
            }
            if preview {
                if doc.account_password != doc.account_password_confirm {
                    doc.error = Some("两次输入的密码不一致。".into());
                    return;
                }
                let password = AccountPassword::new(std::mem::take(&mut doc.account_password));
                let change = if doc.account_form == 0 {
                    if engine == Engine::SqlServer {
                        AccountChange::CreateLogin {
                            name: doc.account_name.clone(),
                            password,
                        }
                    } else {
                        AccountChange::CreateUser {
                            name: doc.account_name.clone(),
                            host: (engine == Engine::MySql).then(|| doc.account_host.clone()),
                            password,
                        }
                    }
                } else if let Some(principal) = selected {
                    let old_password = (!doc.account_old_password.is_empty()).then(|| {
                        AccountPassword::new(std::mem::take(&mut doc.account_old_password))
                    });
                    AccountChange::ChangePassword {
                        principal,
                        password,
                        old_password,
                    }
                } else {
                    doc.error = Some("请选择目标账号。".into());
                    clear_account_passwords(doc);
                    return;
                };
                preview_account(doc, change);
            }
        },
    );
}

pub(super) fn password_fields(ui: &mut egui::Ui, doc: &mut AdvancedDocument, old: bool) {
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

pub(super) fn principal_choice(
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

pub(super) fn principal_state(engine: Engine, principal: &AccessPrincipal) -> Option<&'static str> {
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
