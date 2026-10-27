use super::*;
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    ops::Range,
};

pub(super) const LARGE_SQL_BYTES: usize = 1024 * 1024;
const PAGE_BYTES: usize = 32 * 1024;

#[derive(Clone, Default)]
struct View {
    range: Range<usize>,
    fingerprint: u64,
}

fn boundary(sql: &str, offset: usize) -> usize {
    let mut offset = offset.min(sql.len());
    while !sql.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn page(sql: &str, start: usize) -> Range<usize> {
    let start = boundary(sql, start);
    start..boundary(sql, start.saturating_add(PAGE_BYTES))
}

fn fingerprint(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Only the visible segment is laid out and kept in the text widget's undo
/// buffer. The tab retains the full document for persistence and execution;
/// cursor and selection offsets are translated back to that document.
pub(super) fn show(ui: &mut egui::Ui, tab: &mut QueryTab, height: f32) {
    let view_id = egui::Id::new(("large-sql-view", tab.id));
    let editor_id = ui.make_persistent_id(("large-sql-editor", tab.id));
    let mut view = ui.ctx().data_mut(|data| {
        data.get_temp::<View>(view_id).unwrap_or_else(|| View {
            range: page(&tab.sql, 0),
            ..Default::default()
        })
    });
    let old_range = view.range.clone();
    view.range.start = boundary(&tab.sql, view.range.start);
    view.range.end = boundary(&tab.sql, view.range.end.max(view.range.start));
    if view.range.is_empty() {
        view.range = page(&tab.sql, view.range.start);
    }
    let requested = tab.select_range.take();
    if let Some(range) = &requested
        && (range.start < view.range.start || range.end > view.range.end)
    {
        view.range = page(&tab.sql, range.start);
    }
    let mut line_height = super::line_height(ui.ctx());
    let toolbar_top = ui.cursor().top();
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "大文件分段编辑 · {:.1} MiB",
            tab.sql.len() as f64 / 1024.0 / 1024.0
        ))
        .on_hover_text(
            "查找、执行全文与保存仍使用完整文档；选区限于当前段。切换段落会清空撤销记录。",
        );
        if ui
            .add_enabled(view.range.start > 0, egui::Button::new("开头"))
            .clicked()
        {
            view.range = page(&tab.sql, 0);
        }
        if ui
            .add_enabled(view.range.start > 0, egui::Button::new("上一段"))
            .clicked()
        {
            let end = view.range.start;
            view.range = page(&tab.sql, end.saturating_sub(PAGE_BYTES));
            view.range.end = end;
        }
        if ui
            .add_enabled(view.range.end < tab.sql.len(), egui::Button::new("下一段"))
            .clicked()
        {
            view.range = page(&tab.sql, view.range.end);
        }
        if ui
            .add_enabled(view.range.end < tab.sql.len(), egui::Button::new("末尾"))
            .clicked()
        {
            view.range = page(&tab.sql, tab.sql.len().saturating_sub(PAGE_BYTES));
        }
        ui.label(format!(
            "字节 {}–{} / {}",
            view.range.start + 1,
            view.range.end,
            tab.sql.len()
        ));
        ui.separator();
        line_height = line_height_control(ui);
    });
    let reset =
        old_range != view.range || view.fingerprint != fingerprint(&tab.sql[view.range.clone()]);
    if reset {
        egui::text_edit::TextEditState::default().store(ui.ctx(), editor_id);
        tab.selection.clear();
        tab.selection_char_start = None;
        tab.focus_editor = true;
    }
    let prefix_chars = byte_to_char_index(&tab.sql, view.range.start);
    if reset {
        tab.cursor = prefix_chars;
    }
    let mut text = tab.sql[view.range.clone()].to_owned();
    let content_height = (height - 28.0 - (ui.cursor().top() - toolbar_top)).max(40.0);
    egui::ScrollArea::both()
        .id_salt(("large-sql-scroll", tab.id, view.range.start))
        .max_height(content_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _: f32| {
                let job = layout_sql(ui, text.as_str(), line_height);
                ui.fonts_mut(|fonts| fonts.layout_job(job))
            };
            let mut output = egui::TextEdit::multiline(&mut text)
                .id(editor_id)
                .layouter(&mut layouter)
                .font(egui::TextStyle::Monospace)
                .code_editor()
                .desired_width(f32::INFINITY)
                .desired_rows(1)
                .frame(false)
                .show(ui);
            if let Some(range) = requested {
                let start = range.start.clamp(view.range.start, view.range.end) - view.range.start;
                let end = range.end.clamp(view.range.start, view.range.end) - view.range.start;
                output
                    .state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(byte_to_char_index(&text, start)),
                        egui::text::CCursor::new(byte_to_char_index(&text, end)),
                    )));
                output.state.clone().store(ui.ctx(), editor_id);
                tab.focus_editor = true;
            }
            if tab.focus_editor && ui.is_enabled() {
                output.response.request_focus();
                tab.focus_editor = false;
            }
            if let Some(range) = output.state.cursor.char_range() {
                tab.cursor = prefix_chars + range.primary.index;
                let selected = range.as_sorted_char_range();
                tab.selection_char_start =
                    (!selected.is_empty()).then_some(prefix_chars + selected.start);
                let start = char_to_byte_index(&text, selected.start);
                let end = char_to_byte_index(&text, selected.end);
                tab.selection = text[start..end].to_owned();
            }
            if output.response.changed() {
                tab.sql.replace_range(view.range.clone(), &text);
                view.range.end = view.range.start + text.len();
            }
        });
    view.fingerprint = fingerprint(&text);
    ui.ctx().data_mut(|data| data.insert_temp(view_id, view));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        ctx: &egui::Context,
        tab: &mut QueryTab,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 600.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    sql_editor(ui, tab, &mut None, false, &[], &[], Engine::MySql);
                });
            },
        )
    }

    #[test]
    fn unicode_segments_cover_the_document_without_gaps_or_truncation() {
        let sql = "SELECT '中文 🦀';\n".repeat(100_000);
        let mut reconstructed = String::new();
        let mut start = 0;
        while start < sql.len() {
            let range = page(&sql, start);
            assert!(range.len() <= PAGE_BYTES);
            assert!(range.end > start);
            reconstructed.push_str(&sql[range.clone()]);
            start = range.end;
        }
        assert_eq!(reconstructed, sql);
    }

    #[test]
    fn large_document_renders_one_segment_and_edits_use_global_unicode_offsets() {
        let ctx = egui::Context::default();
        let mut tab = QueryTab::new(1);
        tab.sql = "SELECT '中文 🦀';\n".repeat(100_000);
        let start = tab.sql.len();
        let marker = "末尾 🦀";
        tab.sql.push_str(marker);
        tab.saved_sql = tab.sql.clone();
        tab.select_range = Some(start..tab.sql.len());
        let output = frame(&ctx, &mut tab, Vec::new());
        assert_eq!(tab.selection, marker);
        assert_eq!(
            tab.selection_char_start,
            Some(byte_to_char_index(&tab.sql, start))
        );
        assert_eq!(tab.cursor, tab.sql.chars().count());
        assert!(output.shapes.iter().all(|shape| match &shape.shape {
            egui::Shape::Text(text) => text.galley.job.text.len() <= PAGE_BYTES,
            _ => true,
        }));

        frame(&ctx, &mut tab, vec![egui::Event::Text("修改 🦀".into())]);
        let expected = format!("{}修改 🦀", &tab.saved_sql[..start]);
        assert_eq!(tab.sql, expected);
        assert!(tab.selection.is_empty());
        assert_eq!(tab.cursor, tab.sql.chars().count());
        assert_eq!(&tab.saved_sql[start..], marker);

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("edited.sql");
        write_sql_file(&path, &tab.sql).unwrap();
        assert_eq!(read_sql_file(&path).unwrap(), expected);

        // Navigating resets the widget's local undo buffer, so undo cannot
        // transplant the previous segment over another part of the document.
        let output = frame(&ctx, &mut tab, Vec::new());
        let first = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == "开头" => {
                    Some(text.pos + text.galley.size() / 2.0)
                }
                _ => None,
            })
            .unwrap();
        for pressed in [true, false] {
            frame(
                &ctx,
                &mut tab,
                vec![
                    egui::Event::PointerMoved(first),
                    egui::Event::PointerButton {
                        pos: first,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        let view = ctx.data_mut(|data| {
            data.get_temp::<View>(egui::Id::new(("large-sql-view", tab.id)))
                .unwrap()
        });
        assert_eq!(view.range.start, 0);
        frame(
            &ctx,
            &mut tab,
            vec![egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }],
        );
        assert_eq!(tab.sql, expected);
    }
}
