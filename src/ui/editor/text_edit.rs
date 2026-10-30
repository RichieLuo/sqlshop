use super::*;

/// Keep line numbers outside the scroll area so horizontal scrolling only
/// moves SQL, while row positions come from the text widget's actual layout.
pub(super) fn show(
    ui: &mut egui::Ui,
    scroll_id: impl std::hash::Hash,
    height: f32,
    first_line: usize,
    line_count: usize,
    add_contents: impl FnOnce(&mut egui::Ui) -> egui::text_edit::TextEditOutput,
) {
    let font = egui::FontId::monospace(14.0);
    let color = ui.visuals().weak_text_color();
    let last_line = first_line + line_count.saturating_sub(1);
    let width = ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(last_line.to_string(), font.clone(), color)
            .size()
            .x
    }) + 16.0;
    ui.horizontal_top(|ui| {
        let (gutter, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let output = egui::ScrollArea::both()
            .id_salt(scroll_id)
            .min_scrolled_height(height)
            .max_height(height)
            .auto_shrink([false, false])
            .show(ui, add_contents);
        let gutter = gutter.intersect(egui::Rect::from_min_max(
            egui::pos2(gutter.left(), output.inner_rect.top()),
            egui::pos2(gutter.right(), output.inner_rect.bottom()),
        ));
        let painter = ui.painter_at(gutter);
        painter.vline(
            gutter.right(),
            gutter.y_range(),
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        let mut line = first_line;
        let mut starts_line = true;
        for row in &output.inner.galley.rows {
            let rect = row.rect().translate(output.inner.galley_pos.to_vec2());
            if starts_line && rect.bottom() >= gutter.top() && rect.top() <= gutter.bottom() {
                let number = ui
                    .fonts_mut(|fonts| fonts.layout_no_wrap(line.to_string(), font.clone(), color));
                let number_baseline = number.rows[0].glyphs[0].pos.y;
                let baseline = row
                    .glyphs
                    .first()
                    .map_or(number_baseline, |glyph| glyph.pos.y);
                painter.galley(
                    egui::pos2(
                        gutter.right() - 8.0 - number.size().x,
                        output.inner.galley_pos.y + row.pos.y + baseline - number_baseline,
                    ),
                    number,
                    color,
                );
            }
            starts_line = row.ends_with_newline;
            if starts_line {
                line += 1;
            }
        }
    });
}

pub(super) fn edit(
    ui: &mut egui::Ui,
    text: &mut String,
    id: egui::Id,
    line_height: f32,
) -> egui::text_edit::TextEditOutput {
    if ui.is_enabled()
        && ui.memory(|memory| memory.has_focus(id))
        && ui.input_mut(|input| {
            // Mixed input batches must retain their original event order:
            // preselecting before an earlier Text/Paste event could replace SQL.
            let only_select_all = !input.pointer.any_down()
                && input.events.iter().all(|event| match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => *key == egui::Key::A && modifiers.command,
                    egui::Event::Key { pressed: false, .. }
                    | egui::Event::PointerMoved(_)
                    | egui::Event::MouseMoved(_)
                    | egui::Event::MouseWheel { .. } => true,
                    _ => false,
                });
            only_select_all && input.consume_key(egui::Modifiers::COMMAND, egui::Key::A)
        })
    {
        // Preload the selection so TextEdit paints it immediately without
        // treating it as cursor movement and scrolling to the document end.
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(text.chars().count()),
            )));
        state.store(ui.ctx(), id);
    }
    let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _: f32| {
        let job = layout_sql(ui, text.as_str(), line_height);
        ui.fonts_mut(|fonts| fonts.layout_job(job))
    };
    egui::TextEdit::multiline(text)
        .id(id)
        .layouter(&mut layouter)
        .font(egui::TextStyle::Monospace)
        .code_editor()
        .desired_width(f32::INFINITY)
        .desired_rows(1)
        .frame(false)
        .show(ui)
}
