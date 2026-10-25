use super::*;

#[test]
fn inline_cell_input_fills_cell_with_square_outline() {
    let (app, _, _) = table_flow_fixture();
    let mut editor = grid_cell_editor(&app.tabs[0], 0, 1, true).unwrap();
    let ctx = app.context.clone();
    let cell = egui::Rect::from_min_size(egui::pos2(20.0, 40.0), egui::vec2(160.0, 32.0));
    let mut outline_color = egui::Color32::TRANSPARENT;
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.scope_builder(egui::UiBuilder::new().max_rect(cell), |ui| {
                outline_color = accent(ui);
                let response = grid_cell_input(ui, app.tabs[0].id, &mut editor, true);
                assert!((response.rect.width() - cell.width()).abs() < 0.5);
                assert!((response.rect.height() - cell.height()).abs() < 0.5);
            });
        });
    });
    assert!(output.shapes.iter().any(|shape| matches!(
        &shape.shape,
        egui::Shape::Rect(rect)
            if rect.rect == cell
                && rect.corner_radius == egui::CornerRadius::ZERO
                && rect.stroke.color == outline_color
                && rect.stroke.width == 1.0
    )));
}

#[test]
fn inline_cell_editing_ends_on_outside_click_for_null_and_text() {
    for value in [None, Some("original".to_owned())] {
        for expanded in [false, true] {
            for destination in ["7", "note", "blank"] {
                let (mut app, _, _) = table_flow_fixture();
                install_font(&app.context);
                app.tabs[0].output.as_mut().unwrap().sets[0].rows[0][1] = value.clone();
                let output = inline_grid_frame(&mut app, vec![]);
                let position = grid_text_position(&output, value.as_deref().unwrap_or("NULL"));
                click_inline_grid(&mut app, position);
                click_inline_grid(&mut app, position);
                assert!(app.cell_editor.is_some(), "double click must start editing");
                inline_grid_frame(&mut app, vec![]);
                if expanded {
                    let output = inline_grid_frame(&mut app, vec![]);
                    let position = grid_text_position(&output, "展开编辑");
                    click_inline_grid(&mut app, position);
                    assert!(app.cell_editor.as_ref().unwrap().expanded);
                }
                let output = inline_grid_frame(&mut app, vec![]);
                let position = if destination == "blank" {
                    egui::pos2(300.0, 600.0)
                } else {
                    grid_text_position(&output, destination)
                };
                click_inline_grid(&mut app, position);
                assert!(
                    app.cell_editor.is_none(),
                    "outside click must finish editing: value={value:?}, expanded={expanded}, destination={destination}",
                );
                assert!(app.tabs[0].staged_cells.is_empty());
                assert_eq!(
                    app.tabs[0].output.as_ref().unwrap().sets[0].rows[0][1],
                    value,
                );
            }
        }
    }
}

#[test]
fn inline_cell_outside_click_stages_input_without_changing_snapshot() {
    for value in [None, Some("original".to_owned())] {
        for expanded in [false, true] {
            let (mut app, _, _) = table_flow_fixture();
            install_font(&app.context);
            app.tabs[0].output.as_mut().unwrap().sets[0].rows[0][1] = value.clone();
            app.cell_editor = grid_cell_editor(&app.tabs[0], 0, 1, true);
            app.cell_editor.as_mut().unwrap().expanded = expanded;
            inline_grid_frame(&mut app, vec![]);
            if value.is_none() {
                let output = inline_grid_frame(&mut app, vec![]);
                let position = grid_text_position(&output, "NULL");
                click_inline_grid(&mut app, position);
                assert!(!app.cell_editor.as_ref().unwrap().is_null);
            }
            inline_grid_frame(&mut app, vec![egui::Event::Text("changed".into())]);
            let text = app.cell_editor.as_ref().unwrap().text.clone();
            assert!(text.contains("changed"));
            let output = inline_grid_frame(&mut app, vec![]);
            let position = grid_text_position(&output, "7");
            click_inline_grid(&mut app, position);
            assert!(app.cell_editor.is_none());
            assert_eq!(app.tabs[0].staged_cells.len(), 1);
            assert_eq!(
                app.tabs[0].staged_cells[0].value.as_deref(),
                Some(text.as_str()),
            );
            assert_eq!(
                app.tabs[0].output.as_ref().unwrap().sets[0].rows[0][1],
                value,
            );
        }
    }
}

#[test]
fn inline_cell_toolbar_interactions_keep_editing_active() {
    for value in [None, Some("original".to_owned())] {
        let (mut app, _, _) = table_flow_fixture();
        install_font(&app.context);
        app.tabs[0].output.as_mut().unwrap().sets[0].rows[0][1] = value;
        app.cell_editor = grid_cell_editor(&app.tabs[0], 0, 1, true);
        inline_grid_frame(&mut app, vec![]);
        for label in ["NULL", "展开编辑", "NULL", "展开编辑"] {
            let output = inline_grid_frame(&mut app, vec![]);
            let position = grid_text_position(&output, label);
            click_inline_grid(&mut app, position);
            assert!(
                app.cell_editor.is_some(),
                "clicking {label} must not finish editing",
            );
        }
        assert!(app.tabs[0].staged_cells.is_empty());
    }
}

#[test]
fn inline_editing_replaces_table_toolbar_without_moving_grid_header() {
    let (mut app, _, _) = table_flow_fixture();
    let ctx = app.context.clone();
    install_font(&ctx);
    let mut header_positions = Vec::new();
    for editing in [false, true, false] {
        app.cell_editor = editing.then(|| grid_cell_editor(&app.tabs[0], 0, 0, true).unwrap());
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 820.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.workspace_content(ui);
                });
            },
        );
        let labels = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            labels
                .iter()
                .any(|text| text.galley.job.text == "结构 / DDL"),
            !editing,
        );
        assert_eq!(
            labels.iter().any(|text| text.galley.job.text == "展开编辑"),
            editing,
        );
        header_positions.push(
            labels
                .iter()
                .find(|text| text.galley.job.text == "#")
                .expect("grid header should render")
                .pos
                .y,
        );
    }
    assert!(
        header_positions
            .iter()
            .all(|y| (y - header_positions[0]).abs() < 0.5),
        "grid header moved while toggling editing: {header_positions:?}",
    );
}

#[test]
fn selected_cell_f2_and_enter_open_editor_with_staged_value_and_snapshot_key() {
    for key in [egui::Key::F2, egui::Key::Enter] {
        for readonly in [false, true] {
            let (mut app, _, change) = table_flow_fixture();
            app.tabs[0].selected_cell = Some((0, 1));
            app.tabs[0].stage_cell(change);
            let mut editor = None;
            let _ = app.context.run(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        editor = result_grid(ui, &mut app.tabs[0], !readonly).0;
                    });
                },
            );
            if readonly {
                assert!(editor.is_none());
            } else {
                let editor =
                    editor.expect("selected cell should open through the shared editor helper");
                assert_eq!(editor.text, "changed");
                assert_eq!(editor.change.original.as_deref(), Some("original"));
                assert_eq!(editor.change.key_values, [Some("7".into())]);
            }
        }
    }
}

#[test]
fn reverting_a_staged_cell_in_the_editor_removes_only_that_change() {
    for original in [None, Some("A".to_owned())] {
        let (mut app, target, _) = table_flow_fixture();
        let tab_id = app.tabs[0].id;
        let set = &mut app.tabs[0].output.as_mut().unwrap().sets[0];
        set.rows[0][1] = original.clone();
        set.rows.push(vec![Some("8".into()), original.clone()]);
        app.tabs[0]
            .table_data
            .as_mut()
            .unwrap()
            .keys
            .push(vec![Some("8".into())]);
        let change = CellUpdate {
            database: "db_a".into(),
            table: Table {
                schema: "public".into(),
                name: "items".into(),
                kind: "BASE TABLE".into(),
            },
            primary_keys: vec!["id".into()],
            key_values: vec![Some("7".into())],
            column: "note".into(),
            original: original.clone(),
            value: Some("B".into()),
        };
        let other = CellUpdate {
            key_values: vec![Some("8".into())],
            ..change.clone()
        };
        app.tabs[0].stage_cell(change.clone());
        app.tabs[0].stage_cell(other);
        app.cell_editor = Some(CellEditor {
            tab: tab_id,
            connection: target.connection,
            change,
            text: original.clone().unwrap_or_default(),
            is_null: original.is_none(),
            focus: false,
            expanded: false,
        });
        app.finish_inline_cell(false, CellMovement::Stay);
        assert!(app.cell_editor.is_none());
        assert_eq!(app.tabs[0].staged_cells.len(), 1);
        assert_eq!(app.tabs[0].staged_cells[0].key_values, [Some("8".into())]);
    }
}

#[test]
fn result_cell_double_click_uses_snapshot_keys_and_respects_read_only_mode() {
    let ctx = egui::Context::default();
    let mut tab = QueryTab::new(1);
    let connection = Uuid::new_v4();
    tab.table_target = Some(TableTarget {
        connection,
        database: "app_dev".into(),
        schema: "public".into(),
        name: "users".into(),
    });
    tab.output = Some(QueryOutput {
        sets: vec![sqlshop::model::ResultSet {
            columns: vec!["id".into(), "name".into()],
            rows: vec![vec![Some("7".into()), Some("original".into())]],
            ..Default::default()
        }],
        ..Default::default()
    });
    tab.table_data = Some(TableData {
        writable: true,
        output: QueryOutput::default(),
        primary_keys: vec!["id".into()],
        keys: vec![vec![Some("7".into())]],
        columns: ["id", "name"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| sqlshop::model::TableColumn {
                name: name.into(),
                declared_type: "text".into(),
                sql_type: "text".into(),
                kind: sqlshop::model::CellKind::Text,
                editable: true,
                primary_key: index == 0,
            })
            .collect(),
    });
    let mut click_position = None;
    let mut frame = |time, pressed: Option<bool>, can_edit| {
        let position = click_position.unwrap_or(egui::Pos2::ZERO);
        let mut events = vec![egui::Event::PointerMoved(position)];
        if let Some(pressed) = pressed {
            events.push(egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        let mut editor = None;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    editor = result_grid(ui, &mut tab, can_edit).0;
                });
            },
        );
        click_position = Some(grid_test_cell_position(&output, "id", "1"));
        editor
    };
    frame(0.0, None, true);
    frame(0.02, Some(true), true);
    assert!(frame(0.03, Some(false), true).is_none());
    frame(0.12, Some(true), true);
    let editor = frame(0.13, Some(false), true).expect("double click should edit the clicked cell");
    assert_eq!(editor.connection, connection);
    assert_eq!(editor.change.column, "id");
    assert_eq!(editor.change.key_values, [Some("7".into())]);
    assert_eq!(editor.change.original.as_deref(), Some("7"));
    frame(1.0, Some(true), false);
    assert!(frame(1.01, Some(false), false).is_none());
    frame(1.1, Some(true), false);
    assert!(frame(1.11, Some(false), false).is_none());
}
