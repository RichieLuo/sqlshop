use super::*;

#[test]
fn table_draft_overlay_preserves_snapshot_null_empty_and_primary_key() {
    for new_note in [None, Some(String::new()), Some("changed".into())] {
        let (mut app, _, mut change) = table_flow_fixture();
        change.value = new_note.clone();
        let tab = &mut app.tabs[0];
        tab.stage_cell(CellUpdate {
            column: "id".into(),
            original: Some("7".into()),
            value: Some("8".into()),
            ..change.clone()
        });
        tab.stage_cell(change);
        let output = app.context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                result_grid(ui, tab, true);
            });
        });
        let labels = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(labels.contains(&"● 8"), "{labels:?}");
        let expected = format!("● {}", new_note.as_deref().unwrap_or("NULL"));
        assert!(labels.contains(&expected.as_str()), "{labels:?}");
        let snapshot = &tab.output.as_ref().unwrap().sets[0];
        assert_eq!(
            snapshot.rows[0],
            [Some("7".into()), Some("original".into())]
        );
        assert_eq!(tab.table_data.as_ref().unwrap().keys[0], [Some("7".into())]);
        assert_eq!(
            selected_tsv(snapshot, Some((0, 0)), Some((0, 1))),
            "7\toriginal"
        );
        assert!(app.discard_staged_cells(0));
        assert!(app.tabs[0].staged_cells.is_empty());
    }
}

#[test]
fn table_drafts_cannot_be_discarded_during_commit_or_cell_editing() {
    let (mut app, target, change) = table_flow_fixture();
    app.tabs[0].stage_cell(change.clone());
    app.sessions.get_mut(&target.connection).unwrap().busy = true;
    assert!(!app.discard_staged_cells(0));
    assert_eq!(app.tabs[0].staged_cells.len(), 1);
    app.handle_response(
        target.connection,
        Response::OperationError {
            tab: app.tabs[0].id,
            message: "conflict rolled back".into(),
            disconnected: false,
        },
    );
    assert_eq!(app.tabs[0].staged_cells.len(), 1);
    app.cell_editor = Some(CellEditor {
        tab: app.tabs[0].id,
        connection: target.connection,
        change: change.clone(),
        text: "editing".into(),
        is_null: false,
        focus: false,
        expanded: false,
    });
    app.request_on(
        target.connection,
        Request::SaveCells {
            tab: app.tabs[0].id,
            target: target.clone(),
            changes: vec![change],
        },
        "save",
    );
    assert!(!app.session_busy(target.connection));
    assert_eq!(app.tabs[0].staged_cells.len(), 1);
    // Explicit idle revert cancels the uncommitted input and all drafts.
    // During a commit both remain intact.
    app.sessions.get_mut(&target.connection).unwrap().busy = true;
    assert!(!app.discard_staged_cells(0));
    assert_eq!(app.cell_editor.as_ref().unwrap().text, "editing");
    assert_eq!(app.tabs[0].staged_cells.len(), 1);
    app.sessions.get_mut(&target.connection).unwrap().busy = false;
    assert!(app.discard_staged_cells(0));
    assert!(app.cell_editor.is_none());
    assert!(app.tabs[0].staged_cells.is_empty());
}

#[test]
fn table_expanded_filters_cannot_change_applied_conditions_with_drafts() {
    let (mut app, target, change) = table_flow_fixture();
    app.tabs[0].filters_open = true;
    app.tabs[0].browse.filters = vec![sqlshop::db::ColumnFilter {
        column: "id".into(),
        operator: sqlshop::db::FilterOperator::Eq,
        value: Some("7".into()),
    }];
    app.tabs[0].browse.offset = 200;
    let original = app.tabs[0].browse.clone();
    app.tabs[0].stage_cell(change);
    let ctx = app.context.clone();
    let mut frame = |time, events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 820.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.table_controls(ui));
            },
        )
    };
    let output = frame(0.0, vec![]);
    let positions = ["应用筛选与排序", "重置"].map(|label| {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
                }
                _ => None,
            })
            .expect("expanded filter buttons should render")
    });
    for (index, position) in positions.into_iter().enumerate() {
        for pressed in [true, false] {
            let _ = frame(
                1.0 + index as f64 + f64::from(!pressed) * 0.1,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    }
    assert_eq!(app.tabs[0].browse, original);
    assert_eq!(app.tabs[0].staged_cells.len(), 1);
    assert!(!app.session_busy(target.connection));
}

#[test]
fn table_refresh_failure_preserves_snapshot_and_blocks_every_data_write() {
    let (mut app, target, change) = table_flow_fixture();
    let tab_id = app.tabs[0].id;
    app.load_table(0);
    assert!(app.tabs[0].output.is_some());
    assert!(app.tabs[0].table_data.is_some());
    assert!(app.tabs[0].table_snapshot_stale);
    app.handle_response(
        target.connection,
        Response::OperationError {
            tab: tab_id,
            message: "read failed".into(),
            disconnected: false,
        },
    );
    assert!(app.status.contains("保留上次快照"));
    assert!(!app.tabs[0].table_read_pending);
    assert_eq!(
        app.tabs[0].output.as_ref().unwrap().sets[0].rows[0][1].as_deref(),
        Some("original")
    );
    for request in [
        Request::SaveCell {
            tab: tab_id,
            target: target.clone(),
            change: change.clone(),
        },
        Request::SaveCells {
            tab: tab_id,
            target: target.clone(),
            changes: vec![change.clone()],
        },
        Request::DeleteRows {
            tab: tab_id,
            target: target.clone(),
            table: change.table.clone(),
            primary_keys: change.primary_keys.clone(),
            key_values: vec![change.key_values.clone()],
        },
        Request::InsertDefaultRow {
            tab: tab_id,
            target: target.clone(),
            table: change.table.clone(),
        },
        Request::InsertRows {
            tab: tab_id,
            target: target.clone(),
            table: change.table.clone(),
            columns: vec!["note".into()],
            rows: vec![vec![InsertValue::Text("new".into())]],
        },
    ] {
        app.request_on(target.connection, request, "write");
        assert!(!app.session_busy(target.connection));
        assert!(app.status.contains("表数据已过期"));
    }
    app.cell_editor = Some(CellEditor {
        tab: tab_id,
        connection: target.connection,
        change,
        text: "must not stage".into(),
        is_null: false,
        focus: false,
        expanded: false,
    });
    let ctx = app.context.clone();
    let _ = ctx.run(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }],
            ..Default::default()
        },
        |ctx| app.cell_editor_window(ctx),
    );
    assert!(app.tabs[0].staged_cells.is_empty());
    assert!(app.cell_editor.is_some());
    app.cell_editor = None;
    app.load_table(0);
    app.handle_response(
        target.connection,
        Response::TablePage {
            tab: tab_id,
            page: table_flow_page(),
        },
    );
    assert!(!app.tabs[0].table_snapshot_stale);
    assert!(!app.tabs[0].table_read_pending);
}

#[test]
fn table_commit_success_then_refresh_failure_keeps_success_evidence() {
    let (mut app, target, change) = table_flow_fixture();
    let tab_id = app.tabs[0].id;
    app.tabs[0].stage_cell(change);
    app.handle_response(
        target.connection,
        Response::CellsSaved {
            tab: tab_id,
            count: 1,
        },
    );
    assert!(app.tabs[0].staged_cells.is_empty());
    assert!(app.tabs[0].table_snapshot_stale);
    assert!(app.tabs[0].table_read_pending);
    app.handle_response(
        target.connection,
        Response::OperationError {
            tab: tab_id,
            message: "refresh unavailable".into(),
            disconnected: false,
        },
    );
    assert!(app.status.contains("已成功提交 1 格修改"));
    assert!(app.status.contains("刷新失败"));
    assert!(app.tabs[0].table_snapshot_stale);
    assert!(app.tabs[0].table_write_notice.is_some());
    app.load_table(0);
    app.handle_response(
        target.connection,
        Response::TablePage {
            tab: tab_id,
            page: table_flow_page(),
        },
    );
    assert!(app.tabs[0].table_write_notice.is_none());
    assert!(!app.tabs[0].table_snapshot_stale);
}

#[test]
fn table_draft_details_start_collapsed_and_top_toolbar_has_no_write_buttons() {
    let (mut app, _, change) = table_flow_fixture();
    app.tabs[0].stage_cell(change);
    let mut height = 0.0;
    let ctx = app.context.clone();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            height = ui.scope(|ui| app.table_controls(ui)).response.rect.height();
        });
    });
    let labels = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(height <= 76.0, "collapsed table toolbar used {height}px");
    assert!(labels.iter().any(|label| label.contains("筛选与排序")));
    assert!(!labels.contains(&"提交全部"));
    assert!(!labels.contains(&"撤销暂存"));
    assert!(!labels.contains(&"刷新"));
    assert!(
        !labels
            .iter()
            .any(|label| label.contains("original") || label.contains("changed"))
    );
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            result_grid(ui, &mut app.tabs[0], true);
        });
    });
    let labels: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
            _ => None,
        })
        .collect();
    assert!(labels.contains(&"暂存 1 格"));
    assert!(labels.contains(&"提交全部"));
    assert!(labels.contains(&"撤销暂存"));
}

#[test]
fn table_draft_actions_render_above_grid_after_cell_editing_ends() {
    let (mut app, _, change) = table_flow_fixture();
    app.tabs[0].stage_cell(change);
    let ctx = app.context.clone();
    install_font(&ctx);
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let actions = app.table_draft_action_state();
            result_grid_inline(ui, &mut app.tabs[0], true, None, actions);
        });
    });
    let positions = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text)
                if ["提交全部", "撤销暂存"].contains(&text.galley.job.text.as_str()) =>
            {
                Some(text.pos.y)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(positions.len(), 2);
    assert_eq!(positions[0], positions[1]);
    assert!(positions[0] < 40.0, "draft actions must precede the grid");
    let footer = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| app.table_pagination(ui));
    });
    assert!(!footer.shapes.iter().any(|shape| matches!(
        &shape.shape,
        egui::Shape::Text(text)
            if ["提交全部", "撤销暂存"].contains(&text.galley.job.text.as_str())
    )));
}

#[test]
fn table_draft_actions_above_grid_share_busy_stale_and_offline_guards() {
    fn click_button(app: &mut App, label: &str, time: f64) {
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
                        let draft_actions = app.table_draft_action_state();
                        let (_, action) = result_grid_inline(
                            ui,
                            &mut app.tabs[0],
                            draft_actions.0,
                            app.cell_editor.as_mut(),
                            draft_actions,
                        );
                        match action {
                            Some(GridAction::SubmitDrafts) => {
                                app.submit_staged_cells(0);
                            }
                            Some(GridAction::RevertDrafts) => {
                                app.discard_staged_cells(0);
                            }
                            _ => {}
                        }
                    });
                },
            )
        };
        let output = frame(time, vec![]);
        let position = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
                }
                _ => None,
            })
            .expect("draft action should render above the grid");
        for pressed in [true, false] {
            let _ = frame(
                time + if pressed { 0.02 } else { 0.04 },
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    }
    let (mut app, target, change) = table_flow_fixture();
    let tab_id = app.tabs[0].id;
    app.tabs[0].stage_cell(change.clone());
    click_button(&mut app, "提交全部", 0.0);
    assert!(app.session_busy(target.connection));
    click_button(&mut app, "撤销暂存", 1.0);
    assert_eq!(app.tabs[0].staged_cells.len(), 1);
    app.handle_response(
        target.connection,
        Response::OperationError {
            tab: tab_id,
            message: "rolled back".into(),
            disconnected: false,
        },
    );
    assert!(app.error);
    app.tabs[0].table_snapshot_stale = true;
    click_button(&mut app, "提交全部", 2.0);
    assert!(!app.session_busy(target.connection));
    app.sessions.get_mut(&target.connection).unwrap().connected = None;
    click_button(&mut app, "撤销暂存", 3.0);
    assert!(app.tabs[0].staged_cells.is_empty());
    assert!(app.status.contains("本地暂存"));
    assert!(!app.error);
    assert!(app.tabs[0].table_snapshot_stale);
    app.sessions.get_mut(&target.connection).unwrap().connected = Some(app.profiles[0].clone());
    app.load_table(0);
    assert!(app.session_busy(target.connection));
    assert!(app.tabs[0].table_read_pending);
    assert!(app.tabs[0].output.is_some());
}

#[test]
fn table_snapshot_stays_stale_after_connection_or_database_context_changes() {
    for event in 0..7 {
        let (mut app, target, change) = table_flow_fixture();
        let other = add_test_session(&mut app, "B", "db_b");
        let other_index = app.tabs.len();
        app.tabs.push(QueryTab::new(other_index + 1));
        app.tabs[other_index].table_target = Some(TableTarget {
            connection: other.connection,
            database: "db_b".into(),
            schema: "public".into(),
            name: "other".into(),
        });
        app.tabs[0].stage_cell(change);
        let tab_id = app.tabs[0].id;
        let response = match event {
            0 => Response::OperationError {
                tab: tab_id,
                message: "transport lost".into(),
                disconnected: true,
            },
            1 => Response::Error {
                message: "transport lost".into(),
                disconnected: true,
            },
            2 => Response::Connected(target.connection),
            3 => Response::Disconnected,
            4 => Response::DatabaseContext(Some("another_db".into())),
            5 => Response::QueryResult {
                tab: Uuid::new_v4(),
                output: QueryOutput::default(),
                database: Some("another_db".into()),
            },
            _ => Response::Query {
                output: QueryOutput::default(),
                database: Some("another_db".into()),
            },
        };
        app.handle_response(target.connection, response);
        assert!(app.tabs[0].table_snapshot_stale, "event {event}");
        assert!(app.tabs[0].output.is_some());
        assert_eq!(app.tabs[0].staged_cells.len(), 1);
        assert!(!app.tabs[other_index].table_snapshot_stale);
        // Returning to the old context must not make the old result editable.
        app.sessions.get_mut(&target.connection).unwrap().connected = Some(app.profiles[0].clone());
        app.handle_response(target.connection, Response::DatabaseSelected("db_a".into()));
        assert!(app.target_ready(&target));
        assert!(app.tabs[0].table_snapshot_stale);
        assert_eq!(app.tabs[0].staged_cells.len(), 1);
        assert!(!app.tabs[other_index].table_snapshot_stale);
    }
}
