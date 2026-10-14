use eframe::egui::{self, RichText};
use sqlshop::model::{QueryDiagnostic, QueryOutput};
use uuid::Uuid;

pub(super) struct QueryResultContext<'a> {
    pub tab: Uuid,
    pub output: Option<&'a QueryOutput>,
    pub error: Option<&'a str>,
    pub diagnostic: Option<&'a QueryDiagnostic>,
    pub elapsed_ms: Option<u128>,
    pub busy: bool,
    /// Navigation is safe only while the host can map the executed SQL back
    /// into the unchanged document. The result panel never guesses that map.
    pub can_locate_error: bool,
}

pub(super) struct QueryResultAction {
    pub show_grid: bool,
    pub locate_error: Option<usize>,
}

/// Draw the per-document result strip and its message page. Ordinary statement
/// completions belong to Messages; only row sets receive data tabs.
pub(super) fn show_results(
    ui: &mut egui::Ui,
    result_index: &mut usize,
    context: QueryResultContext<'_>,
) -> QueryResultAction {
    let cancelled = context
        .error
        .is_some_and(|message| message.starts_with("查询已停止"));
    let error_color = if cancelled {
        ui.visuals().text_color()
    } else if ui.visuals().dark_mode {
        egui::Color32::from_rgb(235, 120, 130)
    } else {
        egui::Color32::from_rgb(180, 55, 66)
    };
    let selection_id = egui::Id::new(("query-result-messages", context.tab));
    let mut selected_messages = ui
        .data(|data| data.get_temp::<bool>(selection_id))
        .unwrap_or(false);
    let sets = context
        .output
        .map(|output| output.sets.as_slice())
        .unwrap_or(&[]);
    let data_indices: Vec<_> = sets
        .iter()
        .enumerate()
        .filter_map(|(index, set)| (!set.columns.is_empty()).then_some(index))
        .collect();
    if !data_indices.contains(result_index)
        && let Some(first) = data_indices.first()
    {
        *result_index = *first;
    }
    // Loading/error-only renders must not overwrite the user's selection. A
    // successful execution with rows should open its data tab by default.
    let mut messages = selected_messages || data_indices.is_empty();
    egui::ScrollArea::horizontal()
        .id_salt(("query-result-strip", context.tab))
        .max_height(30.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for (number, index) in data_indices.iter().enumerate() {
                    let selected = !messages && *result_index == *index;
                    if ui
                        .selectable_label(selected, format!("结果 {}", number + 1))
                        .clicked()
                    {
                        *result_index = *index;
                        selected_messages = false;
                        messages = false;
                    }
                }
                if ui.selectable_label(messages, "消息").clicked() {
                    selected_messages = true;
                    messages = true;
                }
            });
        });
    ui.data_mut(|data| data.insert_temp(selection_id, selected_messages));
    ui.separator();
    let mut action = QueryResultAction {
        show_grid: !messages,
        locate_error: None,
    };
    if !messages {
        if context.error.is_some() {
            ui.label(
                RichText::new(if cancelled {
                    "已停止 · 显示此前已完成的结果，详情见消息"
                } else {
                    "执行失败 · 显示此前已完成的结果，详情见消息"
                })
                .small()
                .color(error_color),
            );
        }
        return action;
    }
    egui::ScrollArea::vertical()
        .id_salt(("query-result-messages-scroll", context.tab))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if context.busy {
                    ui.spinner();
                    ui.label("正在执行查询…");
                } else if context.error.is_some() {
                    ui.colored_label(
                        error_color,
                        if cancelled {
                            "服务器已确认停止"
                        } else {
                            "执行失败"
                        },
                    );
                } else if context.output.is_some() {
                    ui.strong("执行完成");
                } else {
                    ui.label(RichText::new("暂无执行信息").weak());
                }
                if let Some(elapsed) = context
                    .output
                    .map(|output| output.elapsed_ms)
                    .or(context.elapsed_ms)
                {
                    ui.label(RichText::new(format!("耗时 {elapsed} ms")).weak());
                }
            });
            if let Some(error) = context.error {
                let label = ui.add(
                    egui::Label::new(RichText::new(error).color(error_color))
                        .selectable(true)
                        .wrap(),
                );
                label.context_menu(|ui| {
                    if ui
                        .button(if cancelled {
                            "复制停止信息"
                        } else {
                            "复制错误信息"
                        })
                        .clicked()
                    {
                        ui.ctx().copy_text(error.to_owned());
                        ui.close();
                    }
                });
                if let Some(diagnostic) = context.diagnostic {
                    if let Some(code) = &diagnostic.code {
                        ui.label(format!("错误代码：{code}"));
                    }
                    if let Some(position) = &diagnostic.position {
                        let text = match position.column {
                            Some(column) => format!("第 {} 行，第 {column} 列", position.line),
                            None => format!("第 {} 行", position.line),
                        };
                        if ui
                            .add_enabled(context.can_locate_error, egui::Button::new(text))
                            .on_hover_text("定位到此次执行 SQL 的错误位置")
                            .clicked()
                        {
                            action.locate_error = Some(position.char_offset);
                        }
                    }
                    for (title, text) in [
                        ("详情", &diagnostic.detail),
                        ("建议", &diagnostic.hint),
                        ("上下文", &diagnostic.context),
                    ] {
                        if let Some(text) = text {
                            ui.add(egui::Label::new(format!("{title}：{text}")).selectable(true));
                        }
                    }
                }
            }
            if let Some(output) = context.output {
                for message in &output.messages {
                    ui.separator();
                    let code = message
                        .code
                        .as_deref()
                        .map(|code| format!(" · {code}"))
                        .unwrap_or_default();
                    ui.label(
                        RichText::new(format!("{}{code}", message.severity))
                            .small()
                            .weak(),
                    );
                    let label = ui.add(egui::Label::new(&message.text).selectable(true).wrap());
                    label.context_menu(|ui| {
                        if ui.button("复制服务器消息").clicked() {
                            let mut text =
                                format!("{}{}\n{}", message.severity, code, message.text);
                            for extra in [&message.detail, &message.hint, &message.context]
                                .into_iter()
                                .flatten()
                            {
                                text.push('\n');
                                text.push_str(extra);
                            }
                            ui.ctx().copy_text(text);
                            ui.close();
                        }
                    });
                    for (title, text) in [
                        ("详情", &message.detail),
                        ("建议", &message.hint),
                        ("上下文", &message.context),
                    ] {
                        if let Some(text) = text {
                            ui.add(
                                egui::Label::new(format!("{title}：{text}"))
                                    .selectable(true)
                                    .wrap(),
                            );
                        }
                    }
                }
            }
            for (index, set) in sets.iter().enumerate() {
                let text = if set.columns.is_empty() {
                    match set.affected {
                        Some(affected) => format!("语句 {}：受影响行数 {affected}", index + 1),
                        None => format!("语句 {}：已完成", index + 1),
                    }
                } else {
                    let suffix = if set.truncated {
                        "（已达到显示上限）"
                    } else {
                        ""
                    };
                    format!(
                        "结果 {}：已读取 {} 行，{} 列{suffix}",
                        data_indices.iter().position(|item| *item == index).unwrap() + 1,
                        set.rows.len(),
                        set.columns.len(),
                    )
                };
                ui.add(egui::Label::new(text).selectable(true));
            }
            if context.output.is_some_and(|output| output.sets.is_empty()) {
                ui.label("服务器未返回行结果或受影响行数。");
            }
        });
    action
}

pub(super) fn reset_result_view(context: &egui::Context, tab: Uuid) {
    context.data_mut(|data| {
        data.remove::<bool>(egui::Id::new(("query-result-messages", tab)));
    });
}

/// Select the error page once when its response arrives. Completed results
/// remain selectable afterwards, even while this execution has an error.
pub(super) fn show_error_view(context: &egui::Context, tab: Uuid) {
    context.data_mut(|data| {
        data.insert_temp(egui::Id::new(("query-result-messages", tab)), true);
    });
}
