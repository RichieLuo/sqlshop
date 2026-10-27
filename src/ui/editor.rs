use super::*;

mod appearance;
mod large_file;
use appearance::{layout_sql, line_height_control};
pub(super) use appearance::{line_height, set_line_height};

pub(super) fn sql_editor(
    ui: &mut egui::Ui,
    tab: &mut QueryTab,
    height: &mut Option<f32>,
    show_results: bool,
    columns: &[String],
    tables: &[sqlshop::sql_editor::CompletionTable],
    engine: Engine,
) -> (egui::Response, bool) {
    let minimum = if ui.available_height() < 320.0 {
        60.0
    } else {
        100.0
    };
    let maximum = (ui.available_height() - 240.0).max(minimum);
    let editor_height = if show_results {
        height
            .unwrap_or(ui.available_height() * 0.45)
            .clamp(minimum, maximum)
    } else {
        (ui.available_height() - 2.0).max(100.0)
    };
    let mut needs_columns = false;
    let editor = egui::Frame::new()
        .fill(surface(ui))
        .corner_radius(10)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            toolbar_style(ui);
            if tab.sql.len() > large_file::LARGE_SQL_BYTES {
                large_file::show(ui, tab, editor_height);
                return;
            }
            let fold_id = egui::Id::new(("sql-fold-mode", tab.id));
            let error_id = egui::Id::new(("sql-format-error", tab.id));
            let mut folded = ui
                .ctx()
                .data_mut(|data| data.get_temp::<bool>(fold_id).unwrap_or(false));
            let mut line_height = line_height(ui.ctx());
            ui.horizontal_wrapped(|ui| {
                for (label, compact) in [("格式化 SQL", false), ("压缩 SQL", true)] {
                    if ui
                        .button(label)
                        .on_hover_text("有选区时处理选区，否则处理全文；保留字符串和注释")
                        .clicked()
                    {
                        let range = tab
                            .selection_char_start
                            .filter(|_| !tab.selection.is_empty())
                            .map(|start| {
                                let begin = char_to_byte_index(&tab.sql, start);
                                begin..begin + tab.selection.len()
                            })
                            .filter(|range| {
                                tab.sql.get(range.clone()) == Some(tab.selection.as_str())
                            })
                            .unwrap_or(0..tab.sql.len());
                        match sqlshop::sql_editor::format_sql(
                            &tab.sql[range.clone()],
                            engine,
                            compact,
                        ) {
                            Ok(formatted) => {
                                let end = range.start + formatted.len();
                                tab.sql.replace_range(range.clone(), &formatted);
                                tab.selection.clear();
                                tab.select_range = Some(range.start..end);
                                tab.focus_editor = true;
                                folded = false;
                                ui.ctx().data_mut(|data| data.remove::<String>(error_id));
                            }
                            Err(error) => {
                                ui.ctx().data_mut(|data| data.insert_temp(error_id, error))
                            }
                        }
                    }
                }
                ui.checkbox(&mut folded, "折叠视图")
                    .on_hover_text("折叠多行括号和注释；点击编辑可返回全文并定位源代码");
                ui.separator();
                line_height = line_height_control(ui);
            });
            ui.ctx().data_mut(|data| data.insert_temp(fold_id, folded));
            if let Some(error) = ui.ctx().data_mut(|data| data.get_temp::<String>(error_id)) {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.separator();
            ui.add_space(4.0);
            let content_height =
                (editor_height - 28.0 - (ui.cursor().top() - ui.min_rect().top())).max(40.0);
            if folded {
                egui::ScrollArea::both()
                    .id_salt(("folded-sql", tab.id))
                    .max_height(content_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        match sqlshop::sql_editor::folding_ranges(&tab.sql, engine) {
                            Ok(ranges) => {
                                let mut previous = 0;
                                for range in ranges {
                                    if previous < range.start {
                                        ui.label(layout_sql(
                                            ui,
                                            &tab.sql[previous..range.start],
                                            line_height,
                                        ));
                                    }
                                    let line = tab.sql[..range.start]
                                        .bytes()
                                        .filter(|byte| *byte == b'\n')
                                        .count()
                                        + 1;
                                    if ui.small_button(format!("编辑第 {line} 行")).clicked() {
                                        tab.select_range = Some(range.clone());
                                        tab.focus_editor = true;
                                        ui.ctx().data_mut(|data| data.insert_temp(fold_id, false));
                                    }
                                    egui::CollapsingHeader::new(format!(
                                        "第 {line} 行 · {} 行",
                                        tab.sql[range.clone()].lines().count()
                                    ))
                                    .id_salt((tab.id, range.start))
                                    .show(ui, |ui| {
                                        ui.label(layout_sql(
                                            ui,
                                            &tab.sql[range.clone()],
                                            line_height,
                                        ));
                                    })
                                    .header_response
                                    .context_menu(|ui| {
                                        if ui.button("编辑此段").clicked() {
                                            tab.select_range = Some(range.clone());
                                            tab.focus_editor = true;
                                            ui.ctx()
                                                .data_mut(|data| data.insert_temp(fold_id, false));
                                            ui.close();
                                        }
                                    });
                                    previous = range.end;
                                }
                                if previous < tab.sql.len() {
                                    ui.label(layout_sql(ui, &tab.sql[previous..], line_height));
                                }
                            }
                            Err(error) => {
                                ui.colored_label(ui.visuals().error_fg_color, error);
                            }
                        }
                    });
                return;
            }
            egui::ScrollArea::both()
                .id_salt(("editor", tab.id))
                .min_scrolled_height(content_height)
                .max_height(content_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut layouter =
                        |ui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap_width: f32| {
                            let job = layout_sql(ui, text.as_str(), line_height);
                            ui.fonts_mut(|fonts| fonts.layout_job(job))
                        };
                    let editor_id = ui.make_persistent_id(egui::Id::new(tab.id));
                    let completion_key = if ui.is_enabled()
                        && !tab.find_open
                        && ui.ctx().memory(|m| m.has_focus(editor_id))
                    {
                        egui::TextEdit::load_state(ui.ctx(), editor_id)
                            .and_then(|state| state.cursor.char_range())
                            .filter(|range| range.is_empty())
                            .and_then(|range| {
                                sqlshop::sql_editor::semantic_completions(
                                    &tab.sql,
                                    range.primary.index,
                                    engine,
                                    tables,
                                    columns,
                                )
                                .map(|(word, _)| (tab.sql.clone(), range.primary.index, word))
                            })
                            .filter(|(sql, cursor, _)| {
                                tab.completion_dismissed.as_ref() != Some(&(sql.clone(), *cursor))
                            })
                    } else {
                        None
                    };
                    let accepting = completion_key.as_ref().is_some_and(|(_, cursor, _word)| {
                        sqlshop::sql_editor::semantic_completions(
                            &tab.sql, *cursor, engine, tables, columns,
                        )
                        .is_some_and(|(_, choices)| !choices.is_empty())
                    });
                    let completion_events = if accepting {
                        ui.input(|input| {
                            input
                                .events
                                .iter()
                                .filter_map(|event| {
                                    if let egui::Event::Key {
                                        key,
                                        pressed: true,
                                        modifiers,
                                        ..
                                    } = event
                                        && matches!(
                                            key,
                                            egui::Key::Tab
                                                | egui::Key::Enter
                                                | egui::Key::ArrowUp
                                                | egui::Key::ArrowDown
                                                | egui::Key::Escape
                                        )
                                        && !modifiers.command
                                        && !modifiers.ctrl
                                        && !modifiers.alt
                                        && !modifiers.shift
                                    {
                                        return Some(*key);
                                    }
                                    None
                                })
                                .collect::<Vec<_>>()
                        })
                    } else {
                        Vec::new()
                    };
                    if accepting {
                        for key in &completion_events {
                            ui.input_mut(|input| {
                                input.consume_key(egui::Modifiers::NONE, *key);
                            });
                        }
                    }
                    let mut output = egui::TextEdit::multiline(&mut tab.sql)
                        .id(editor_id)
                        .layouter(&mut layouter)
                        .font(egui::TextStyle::Monospace)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(1)
                        .frame(false)
                        .show(ui);
                    if tab.focus_editor && ui.is_enabled() {
                        output.response.request_focus();
                        tab.focus_editor = false;
                    }
                    if let Some(range) = tab.select_range.take() {
                        let selected = egui::text::CCursorRange::two(
                            egui::text::CCursor::new(byte_to_char_index(&tab.sql, range.start)),
                            egui::text::CCursor::new(byte_to_char_index(&tab.sql, range.end)),
                        );
                        output.state.cursor.set_char_range(Some(selected));
                        output.state.clone().store(ui.ctx(), output.response.id);
                        output.response.request_focus();
                    }
                    let cursor_range = output.state.cursor.char_range();
                    if ui.is_enabled()
                        && output.response.has_focus()
                        && !tab.find_open
                        && let Some((range, choices)) = cursor_range
                            .filter(|range| range.is_empty())
                            .and_then(|range| {
                                sqlshop::sql_editor::semantic_completions(
                                    &tab.sql,
                                    range.primary.index,
                                    engine,
                                    tables,
                                    columns,
                                )
                            })
                    {
                        let cursor = cursor_range.unwrap().primary.index;
                        needs_columns =
                            tab.completion_dismissed.as_ref() != Some(&(tab.sql.clone(), cursor));
                        let key = (tab.sql.clone(), cursor);
                        if tab.completion_dismissed.as_ref() != Some(&key) && !choices.is_empty() {
                            tab.completion_selected =
                                tab.completion_selected.min(choices.len() - 1);
                            if completion_events.contains(&egui::Key::ArrowDown) {
                                tab.completion_selected =
                                    (tab.completion_selected + 1) % choices.len();
                            } else if completion_events.contains(&egui::Key::ArrowUp) {
                                tab.completion_selected =
                                    (tab.completion_selected + choices.len() - 1) % choices.len();
                            }
                            let mut chosen = None;
                            let cursor_rect = output
                                .galley
                                .pos_from_cursor(egui::text::CCursor::new(cursor))
                                .translate(output.galley_pos.to_vec2());
                            let popup = egui::Popup::from_response(&output.response)
                                .anchor(cursor_rect)
                                .id(egui::Id::new(("sql-completion", tab.id)))
                                .open(true)
                                .show(|ui| {
                                    ui.set_min_width(200.0);
                                    for (index, (name, kind)) in choices.iter().enumerate() {
                                        let label = format!("{}  · {}", name, kind.label());
                                        if ui
                                            .selectable_label(
                                                index == tab.completion_selected,
                                                label,
                                            )
                                            .clicked()
                                        {
                                            chosen = Some(index);
                                        }
                                    }
                                });
                            let _ = popup;
                            if chosen.is_none()
                                && (completion_events.contains(&egui::Key::Tab)
                                    || completion_events.contains(&egui::Key::Enter))
                            {
                                chosen = Some(tab.completion_selected);
                            }
                            if completion_events.contains(&egui::Key::Escape) {
                                tab.completion_dismissed = Some(key);
                            } else if let Some(index) = chosen
                                && let Some(cursor) =
                                    insert_completion(&mut tab.sql, range, &choices[index].0)
                            {
                                output.state.cursor.set_char_range(Some(
                                    egui::text::CCursorRange::one(egui::text::CCursor::new(cursor)),
                                ));
                                output.state.clone().store(ui.ctx(), output.response.id);
                                output.response.request_focus();
                                tab.completion_dismissed = Some((tab.sql.clone(), cursor));
                            }
                        }
                    }
                    tab.cursor = output
                        .state
                        .cursor
                        .char_range()
                        .map_or(0, |range| range.primary.index);
                    tab.selection_char_start = output
                        .state
                        .cursor
                        .char_range()
                        .filter(|range| !range.is_empty())
                        .map(|range| range.primary.index.min(range.secondary.index));
                    tab.selection = output
                        .state
                        .cursor
                        .char_range()
                        .map(|range| {
                            let range = range.as_sorted_char_range();
                            tab.sql
                                .chars()
                                .skip(range.start)
                                .take(range.len())
                                .collect()
                        })
                        .unwrap_or_default();
                });
        });
    if !show_results {
        return (editor.response, needs_columns);
    }
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 8.0),
        egui::Sense::click_and_drag(),
    );
    let response = response
        .on_hover_cursor(egui::CursorIcon::ResizeVertical)
        .on_hover_text("上下拖动，调整 SQL 编辑区与结果区高度；双击恢复默认");
    let color = if response.hovered() || response.dragged() {
        accent(ui)
    } else {
        ui.visuals().weak_text_color().gamma_multiply(0.45)
    };
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        egui::Stroke::new(1.0, color),
    );
    ui.painter().hline(
        rect.center().x - 18.0..=rect.center().x + 18.0,
        rect.center().y,
        egui::Stroke::new(3.0, color),
    );
    if response.dragged() {
        *height = Some((editor_height + response.drag_delta().y).clamp(minimum, maximum));
    }
    if response.double_clicked() {
        *height = None;
    }
    (response, needs_columns)
}
