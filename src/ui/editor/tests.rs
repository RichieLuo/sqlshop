use super::*;

struct EditorHarness {
    ctx: egui::Context,
    tab: QueryTab,
    other: String,
    time: f64,
}

impl EditorHarness {
    fn new(sql: &str) -> Self {
        let mut tab = QueryTab::new(73);
        tab.sql = sql.into();
        tab.focus_editor = true;
        tab.select_range = Some(0..0);
        Self {
            ctx: egui::Context::default(),
            tab,
            other: "search text".into(),
            time: 0.0,
        }
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
        self.time += 0.1;
        let modifiers = events
            .iter()
            .find_map(|event| match event {
                egui::Event::Key { modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or_default();
        self.ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(760.0, 460.0),
                )),
                time: Some(self.time),
                modifiers,
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.other)
                            .id(egui::Id::new("editor-test-other")),
                    );
                    sql_editor(ui, &mut self.tab, &mut None, false, &[], &[], Engine::MySql);
                });
            },
        )
    }

    fn settled(&mut self) -> egui::FullOutput {
        for _ in 0..15 {
            self.frame(Vec::new());
        }
        self.frame(Vec::new())
    }

    fn scroll(&mut self) -> egui::FullOutput {
        let output = self.settled();
        let text = sql_shape(&output, &self.tab.sql);
        let pointer = output
            .shapes
            .iter()
            .find(|shape| matches!(&shape.shape, egui::Shape::Text(t) if std::ptr::eq(t, text)))
            .unwrap()
            .clip_rect
            .center();
        self.frame(vec![egui::Event::PointerMoved(pointer)]);
        self.frame(vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(-210.0, -350.0),
            modifiers: egui::Modifiers::NONE,
        }]);
        self.settled()
    }
}

fn select_all() -> egui::Event {
    egui::Event::Key {
        key: egui::Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        },
    }
}

fn sql_shape<'a>(output: &'a egui::FullOutput, sql: &str) -> &'a egui::epaint::TextShape {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == sql => Some(text),
            _ => None,
        })
        .expect("SQL text should render")
}

fn numbers(output: &egui::FullOutput) -> Vec<(usize, egui::Rect)> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => text.galley.job.text.parse().ok().map(|line| {
                (
                    line,
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )
            }),
            _ => None,
        })
        .collect()
}

#[test]
fn select_all_preserves_both_scroll_axes_and_remains_copyable_and_replaceable() {
    let sql = format!("-- 中文 {}\n", "long column ".repeat(35)).repeat(90);
    let mut editor = EditorHarness::new(&sql);
    let initial = editor.settled();
    let initial_pos = sql_shape(&initial, &sql).pos;
    let scrolled = editor.scroll();
    let scrolled_pos = sql_shape(&scrolled, &sql).pos;
    assert!(scrolled_pos.x < initial_pos.x - 100.0);
    assert!(scrolled_pos.y < initial_pos.y - 100.0);

    let selected = editor.frame(vec![select_all()]);
    assert_eq!(editor.tab.selection, sql);
    assert_eq!(editor.tab.selection_char_start, Some(0));
    assert!((sql_shape(&selected, &sql).pos - scrolled_pos).length() < 1.0);
    let later = editor.settled();
    assert!((sql_shape(&later, &sql).pos - scrolled_pos).length() < 1.0);

    let copied = editor.frame(vec![egui::Event::Copy]);
    assert!(
        copied
            .platform_output
            .commands
            .iter()
            .any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == &sql))
    );
    editor.frame(vec![egui::Event::Text("SELECT '替换';".into())]);
    assert_eq!(editor.tab.sql, "SELECT '替换';");
    assert!(editor.tab.selection.is_empty());
}

#[test]
fn select_all_in_another_input_does_not_select_sql() {
    let mut editor = EditorHarness::new("SELECT '中文';");
    editor.settled();
    let other_id = egui::Id::new("editor-test-other");
    editor
        .ctx
        .memory_mut(|memory| memory.request_focus(other_id));
    editor.frame(vec![select_all()]);
    assert!(editor.tab.selection.is_empty());
    let selected = egui::TextEdit::load_state(&editor.ctx, other_id)
        .unwrap()
        .cursor
        .char_range()
        .unwrap()
        .as_sorted_char_range();
    assert_eq!(selected, 0..editor.other.chars().count());
}

#[test]
fn select_all_preserves_text_input_order_within_a_frame() {
    let mut editor = EditorHarness::new("-- original");
    editor.settled();
    editor.frame(vec![egui::Event::Text("中文".into()), select_all()]);
    assert_eq!(editor.tab.sql, "中文-- original");
    assert_eq!(editor.tab.selection, editor.tab.sql);

    editor.frame(vec![
        select_all(),
        egui::Event::Text("-- replacement".into()),
        select_all(),
    ]);
    assert_eq!(editor.tab.sql, "-- replacement");
    assert_eq!(editor.tab.selection, editor.tab.sql);
}

#[test]
fn line_numbers_include_empty_document_and_trailing_blank_lines() {
    for (sql, expected) in [("", vec![1]), ("-- first\n\n", vec![1, 2, 3])] {
        let mut editor = EditorHarness::new(sql);
        let output = editor.settled();
        let actual = numbers(&output)
            .into_iter()
            .map(|(line, _)| line)
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "line numbers for {sql:?}");
    }
}

#[test]
fn line_numbers_align_with_rows_at_each_line_height_and_stay_fixed_horizontally() {
    let sql = format!("-- 中文 {}\n", "long column ".repeat(35)).repeat(90);
    for height in [1.0, 1.25, 1.5, 1.75, 2.0] {
        let mut editor = EditorHarness::new(&sql);
        set_line_height(&editor.ctx, height);
        let initial = editor.settled();
        let initial_right = numbers(&initial)[0].1.right();
        let output = editor.scroll();
        let text = sql_shape(&output, &sql);
        let labels = numbers(&output);
        assert!(!labels.is_empty());
        assert!(labels.iter().any(|(line, _)| *line > 1));
        for (line, rect) in labels {
            let row = &text.galley.rows[line - 1];
            let expected_baseline = text.pos.y + row.pos.y + row.glyphs[0].pos.y;
            let number_baseline = output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(number) if number.galley.job.text == line.to_string() => {
                    Some(number.pos.y + number.galley.rows[0].glyphs[0].pos.y)
                }
                _ => None,
            });
            assert!((number_baseline.unwrap() - expected_baseline).abs() < 1.1);
            assert!((rect.right() - initial_right).abs() < 1.0);
        }
    }
}
