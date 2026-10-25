use super::*;

#[test]
fn cell_context_value_actions_stage_null_empty_and_revert_only_original_cell() {
    let (mut app, target, change) = table_flow_fixture();
    app.tabs[0].stage_cell(CellUpdate {
        column: "id".into(),
        original: Some("7".into()),
        value: Some("8".into()),
        ..change.clone()
    });
    assert!(apply_grid_cell_draft(
        &mut app.tabs[0],
        0,
        1,
        CellDraftAction::SetNull,
        true
    ));
    assert_eq!(app.tabs[0].staged_cells[1].value, None);
    assert!(apply_grid_cell_draft(
        &mut app.tabs[0],
        0,
        1,
        CellDraftAction::SetEmptyString,
        true
    ));
    assert_eq!(app.tabs[0].staged_cells[1].value.as_deref(), Some(""));
    assert_eq!(
        app.tabs[0].staged_cells[1].original.as_deref(),
        Some("original")
    );
    assert_eq!(app.tabs[0].staged_cells[1].key_values, [Some("7".into())]);
    assert!(apply_grid_cell_draft(
        &mut app.tabs[0],
        0,
        1,
        CellDraftAction::Revert,
        true
    ));
    assert_eq!(app.tabs[0].staged_cells.len(), 1);
    assert_eq!(app.tabs[0].staged_cells[0].column, "id");
    assert_eq!(app.tabs[0].staged_cells[0].value.as_deref(), Some("8"));
    assert_eq!(
        app.tabs[0].table_data.as_ref().unwrap().keys[0],
        [Some("7".into())]
    );
    assert_eq!(
        app.tabs[0].output.as_ref().unwrap().sets[0].rows[0],
        [Some("7".into()), Some("original".into())]
    );
    assert!(!app.session_busy(target.connection));
}

#[test]
fn cell_context_menu_clicks_apply_draft_actions_without_database_requests() {
    fn click_menu_item(app: &mut App, label: &str, time: f64) {
        let ctx = app.context.clone();
        let mut frame = |time, events| {
            ctx.run(
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
                        result_grid(ui, &mut app.tabs[0], true);
                    });
                },
            )
        };
        let output = frame(time, vec![]);
        let position = grid_test_cell_position(&output, "note", "1");
        let pointer_event = |pos, button, pressed| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        let _ = frame(
            time + 0.02,
            pointer_event(position, egui::PointerButton::Secondary, true),
        );
        let _ = frame(
            time + 0.04,
            pointer_event(position, egui::PointerButton::Secondary, false),
        );
        let output = frame(time + 0.05, vec![egui::Event::PointerMoved(position)]);
        let item_position = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
                }
                _ => None,
            })
            .unwrap_or_else(|| {
                panic!(
                    "context item {label} should render: {:?}",
                    output
                        .shapes
                        .iter()
                        .filter_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                )
            });
        let _ = frame(
            time + 0.06,
            pointer_event(item_position, egui::PointerButton::Primary, true),
        );
        let _ = frame(
            time + 0.08,
            pointer_event(item_position, egui::PointerButton::Primary, false),
        );
    }
    let (mut app, target, change) = table_flow_fixture();
    app.tabs[0].stage_cell(CellUpdate {
        column: "id".into(),
        original: Some("7".into()),
        value: Some("8".into()),
        ..change
    });
    click_menu_item(&mut app, "设为 NULL", 0.0);
    assert_eq!(app.tabs[0].staged_cells.len(), 2);
    assert_eq!(app.tabs[0].staged_cells[1].value, None);
    click_menu_item(&mut app, "设为空字符串", 1.0);
    assert_eq!(app.tabs[0].staged_cells[1].value.as_deref(), Some(""));
    click_menu_item(&mut app, "撤销此格修改", 2.0);
    assert_eq!(app.tabs[0].staged_cells.len(), 1);
    assert_eq!(app.tabs[0].staged_cells[0].column, "id");
    assert!(!app.session_busy(target.connection));
}

#[test]
fn cell_context_actions_and_editor_share_snapshot_and_editability_guards() {
    for mode in 0..8 {
        let (mut app, _, _) = table_flow_fixture();
        let tab = &mut app.tabs[0];
        let can_edit = mode != 0;
        match mode {
            1 => tab.table_target = None,
            2 => tab.table_snapshot_stale = true,
            3 => tab.table_data.as_mut().unwrap().writable = false,
            4 => tab.table_data.as_mut().unwrap().primary_keys.clear(),
            5 => tab.table_data.as_mut().unwrap().columns[1].editable = false,
            6 => {
                tab.table_data.as_mut().unwrap().columns[0].kind =
                    sqlshop::model::CellKind::Unsupported
            }
            7 => tab.table_data.as_mut().unwrap().keys[0].clear(),
            _ => {}
        }
        assert!(
            grid_cell_editor(tab, 0, 1, can_edit).is_none(),
            "mode {mode}"
        );
        for action in [
            CellDraftAction::SetNull,
            CellDraftAction::SetEmptyString,
            CellDraftAction::Revert,
        ] {
            assert!(
                !apply_grid_cell_draft(tab, 0, 1, action, can_edit),
                "mode {mode}"
            );
        }
        assert!(tab.staged_cells.is_empty());
    }
    let (mut app, _, _) = table_flow_fixture();
    app.tabs[0].table_data.as_mut().unwrap().columns[1].kind = sqlshop::model::CellKind::Integer;
    assert!(!apply_grid_cell_draft(
        &mut app.tabs[0],
        0,
        1,
        CellDraftAction::SetEmptyString,
        true
    ));
    assert!(apply_grid_cell_draft(
        &mut app.tabs[0],
        0,
        1,
        CellDraftAction::SetNull,
        true
    ));
}
