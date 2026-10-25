use super::*;

#[test]
fn row_selection_extends_from_anchor_in_both_directions() {
    assert_eq!(row_selection(None, 4, false), (4, 4));
    assert_eq!(row_selection(None, 4, true), (4, 4));
    assert_eq!(row_selection(Some((4, 6)), 1, true), (4, 1));
    assert_eq!(row_selection(Some((4, 6)), 1, false), (1, 1));
    for row in 0..7 {
        assert_eq!(row_in_selection(Some((4, 1)), row), (1..=4).contains(&row));
    }
    assert!(!row_in_selection(None, 0));
}

#[test]
fn grid_headers_render_names_types_and_visible_cell_borders_in_both_themes() {
    for dark in [false, true] {
        let ctx = egui::Context::default();
        ctx.set_visuals(if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(180.0, GRID_HEADER_HEIGHT),
                    egui::Sense::hover(),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                    grid_header_cell(ui, "CustomerName", "varchar(100)");
                });
            });
        });
        let texts: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::epaint::Shape::Text(text) = &shape.shape {
                    Some(text.galley.job.text.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(texts.contains(&"CustomerName"));
        assert!(texts.contains(&"varchar(100)"));
        let borders: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::epaint::Shape::LineSegment { stroke, .. } = &shape.shape {
                    Some(stroke)
                } else {
                    None
                }
            })
            .collect();
        assert!(borders.len() >= 2);
        assert!(borders.iter().all(|stroke| stroke.color.a() == 255));
    }
}

#[test]
fn grid_skips_text_layout_for_horizontally_hidden_cells() {
    let ctx = egui::Context::default();
    let mut tab = QueryTab::new(1);
    tab.output = Some(QueryOutput {
        sets: vec![ResultSet {
            columns: (0..40).map(|column| format!("column-{column}")).collect(),
            rows: vec![
                (0..40)
                    .map(|column| Some(format!("hidden-value-{column}")))
                    .collect(),
            ],
            ..Default::default()
        }],
        ..Default::default()
    });
    for _ in 0..2 {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 260.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| result_grid(ui, &mut tab, false));
            },
        );
        let cells: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text.starts_with("hidden-value-") => {
                    Some(text.galley.job.text.as_str())
                }
                _ => None,
            })
            .collect();
        assert!(cells.contains(&"hidden-value-0"));
        assert!(
            cells.len() < 10,
            "screen-external cells should not construct text shapes"
        );
        assert!(!cells.contains(&"hidden-value-39"));
    }
}

#[test]
fn borrowed_tsv_matches_existing_copy_for_ragged_nullable_rows() {
    let set = ResultSet {
        columns: vec!["a".into(), "b".into(), "c".into()],
        rows: vec![
            vec![None, Some("中文\t\"quoted\"\r\n".into())],
            vec![],
            vec![
                Some(String::new()),
                Some("NULL".into()),
                Some("last".into()),
            ],
        ],
        ..Default::default()
    };
    let materialized = set
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| value.clone().unwrap_or_else(|| "NULL".into()))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    for start_row in 0..5 {
        for end_row in 0..5 {
            for start_column in 0..5 {
                for end_column in 0..5 {
                    assert_eq!(
                        selected_tsv(
                            &set,
                            Some((start_row, start_column)),
                            Some((end_row, end_column))
                        ),
                        sqlshop::sql_editor::copy_tsv(
                            &materialized,
                            start_row.min(end_row)..=start_row.max(end_row),
                            start_column.min(end_column)..=start_column.max(end_column)
                        ),
                    );
                }
            }
        }
    }
    assert_eq!(
        selected_rows_tsv(&set, &[2, 99, 0, 2]),
        "\tNULL\tlast\nNULL\t\"中文\t\"\"quoted\"\"\r\n\"\t\n\tNULL\tlast"
    );
    assert!(selected_rows_tsv(&set, &[99]).is_empty());
}

#[test]
fn grid_scrolled_rows_are_clipped_below_headers_in_both_themes() {
    for dark in [false, true] {
        let ctx = egui::Context::default();
        ctx.set_visuals(if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        let mut tab = QueryTab::new(1);
        tab.output = Some(QueryOutput {
            sets: vec![ResultSet {
                columns: vec!["CustomerName".into()],
                column_types: vec!["varchar(100)".into()],
                rows: (0..200)
                    .map(|row| vec![Some(format!("customer-{row}"))])
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        });
        tab.selected_rows = Some((110, 125));
        // Centering in an odd-height viewport leaves a partially visible first row.
        tab.grid_scroll_row = Some(118);
        for frame in 0..4 {
            let output = ctx.run(
                egui::RawInput {
                    time: Some(frame as f64),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(480.0, 301.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        result_grid(ui, &mut tab, false);
                        // The stricter clipping must stay local to the grid.
                        assert_eq!(ui.visuals().clip_rect_margin, 3.0);
                    });
                },
            );
            let header = output
                .shapes
                .iter()
                .find(|shape| {
                    matches!(&shape.shape, egui::epaint::Shape::Text(text)
                    if text.galley.job.text == "CustomerName")
                })
                .expect("column header should be drawn");
            let rows: Vec<_> = output
                .shapes
                .iter()
                .filter(|shape| {
                    matches!(&shape.shape, egui::epaint::Shape::Text(text)
                    if text.galley.job.text.starts_with("customer-"))
                })
                .collect();
            assert!(!rows.is_empty());
            assert!(
                rows.iter()
                    .all(|shape| shape.clip_rect.top() >= header.clip_rect.bottom()),
                "scrolled rows must not paint over the header (dark={dark})"
            );
            // ScrollArea applies programmatic scrolling after layout, then animates it.
            if frame >= 2 {
                assert!(
                    rows.iter().any(|shape| {
                        matches!(&shape.shape, egui::epaint::Shape::Text(text)
                    if text.galley.job.text == "customer-118")
                    }),
                    "the grid must actually scroll to the requested row: {:?}",
                    rows.iter()
                        .filter_map(|shape| match &shape.shape {
                            egui::epaint::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                );
            }
        }
    }
}

#[test]
fn grid_scrollbar_drag_does_not_resize_columns() {
    for dark in [false, true] {
        let ctx = egui::Context::default();
        ctx.set_visuals(if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        let mut widths = vec![302.0, 110.0];
        let mut frame = |events| {
            let mut offset = 0.0;
            let mut header = egui::Rect::NOTHING;
            let mut body_rect = egui::Rect::NOTHING;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(316.0, 300.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                        egui::ScrollArea::horizontal()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let mut headers = Vec::new();
                                let scroll = TableBuilder::new(ui)
                                    .resizable(false)
                                    .column(Column::exact(widths[0]).clip(true))
                                    .column(Column::exact(widths[1]).clip(true))
                                    .min_scrolled_height(200.0)
                                    .max_scroll_height(200.0)
                                    .header(GRID_HEADER_HEIGHT, |mut row| {
                                        for _ in 0..2 {
                                            row.col(|ui| {
                                                headers.push(
                                                    grid_header_cell(ui, "column", "int").rect,
                                                );
                                            });
                                        }
                                    })
                                    .body(|body| {
                                        body.rows(32.0, 100, |mut row| {
                                            for _ in 0..2 {
                                                row.col(|ui| {
                                                    ui.label("value");
                                                });
                                            }
                                        });
                                    });
                                offset = scroll.state.offset.y;
                                body_rect = scroll.inner_rect;
                                header = headers[0];
                                resize_grid_columns(
                                    ui,
                                    ui.id().with("test-widths"),
                                    &headers,
                                    &mut widths,
                                );
                            });
                    });
                },
            );
            (
                output.platform_output.cursor_icon,
                offset,
                header,
                body_rect,
                widths.clone(),
            )
        };
        let (_, _, header, body, _) = frame(Vec::new());
        let start = egui::pos2(310.0, body.top() + 6.0);
        let (cursor, _, _, _, _) = frame(vec![egui::Event::PointerMoved(start)]);
        assert_ne!(cursor, egui::CursorIcon::ResizeColumn);
        frame(vec![egui::Event::PointerButton {
            pos: start,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }]);
        let end = start + egui::vec2(0.0, 60.0);
        let (_, offset, _, _, after) = frame(vec![egui::Event::PointerMoved(end)]);
        assert!(
            offset > 0.0,
            "the vertical scrollbar must receive the drag: header={header:?}, body={body:?}, cursor={cursor:?}, offset={offset}"
        );
        assert_eq!(after, vec![302.0, 110.0]);
        frame(vec![egui::Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        let start = egui::pos2(header.right() - 3.0, header.center().y);
        let (cursor, _, _, _, _) = frame(vec![egui::Event::PointerMoved(start)]);
        assert_eq!(cursor, egui::CursorIcon::ResizeColumn);
        frame(vec![egui::Event::PointerButton {
            pos: start,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }]);
        let (_, _, _, _, after) = frame(vec![egui::Event::PointerMoved(
            start - egui::vec2(40.0, 0.0),
        )]);
        assert!(
            after[0] < 302.0,
            "header dragging must still resize the column"
        );
    }
}

#[test]
fn grid_scrollbar_drags_preserve_selection_even_when_dragged_into_cells() {
    for floating in [false, true] {
        for axis in [0_usize, 1] {
            for preselected in [false, true] {
                for coalesced in [false, true] {
                    let ctx = egui::Context::default();
                    ctx.style_mut(|style| {
                        style.animation_time = 0.0;
                        style.spacing.scroll = if floating {
                            egui::style::ScrollStyle::floating()
                        } else {
                            egui::style::ScrollStyle::solid()
                        };
                    });
                    let mut tab = QueryTab::new(1);
                    if preselected {
                        tab.cell_anchor = Some((2, 0));
                        tab.selected_cell = Some((4, 1));
                    }
                    let expected = (
                        tab.selected_rows,
                        tab.selected_cell,
                        tab.cell_anchor,
                        tab.selected_insert,
                    );
                    let mut frame = |events| {
                        let mut bars = [egui::Rect::NOTHING; 2];
                        let mut content = egui::Rect::NOTHING;
                        let mut offset = egui::Vec2::ZERO;
                        let _ = ctx.run(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO, egui::vec2(316.0, 300.0),
                                )),
                                events,
                                ..Default::default()
                            },
                            |ctx| {
                                egui::CentralPanel::default().show(ctx, |ui| {
                                    ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                                    ui.visuals_mut().clip_rect_margin = 0.0;
                                    let mut pointer = grid_selection_ui::PointerSelection::new(ui, &tab);
                                    let horizontal = egui::ScrollArea::horizontal()
                                        .scroll_source(egui::scroll_area::ScrollSource { drag: false, ..Default::default() })
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            let vertical = TableBuilder::new(ui)
                                                .drag_to_scroll(false)
                                                .resizable(false)
                                                .column(Column::exact(44.0))
                                                .column(Column::exact(302.0).clip(true))
                                                .column(Column::exact(110.0).clip(true))
                                                .min_scrolled_height(200.0)
                                                .max_scroll_height(200.0)
                                                .header(GRID_HEADER_HEIGHT, |mut row| {
                                                    for _ in 0..3 {
                                                        row.col(|ui| { grid_header_cell(ui, "column", "int"); });
                                                    }
                                                })
                                                .body(|body| {
                                                    body.rows(32.0, 100, |mut row| {
                                                        let index = row.index();
                                                        for column in 0_usize..3 {
                                                            row.col(|ui| {
                                                                let response = grid_cell(ui, RichText::new("value"), false, false);
                                                                pointer.cell(ui, &response, grid_selection_ui::SelectionRefs {
                                                                    rows: &mut tab.selected_rows,
                                                                    cell: &mut tab.selected_cell,
                                                                    anchor: &mut tab.cell_anchor,
                                                                    insert: &mut tab.selected_insert,
                                                                }, (index, column.saturating_sub(1)), column == 0);
                                                            });
                                                        }
                                                    });
                                                });
                                            pointer.exclude_scrollbar(ui, &vertical, 1);
                                            bars[1] = ctx.read_response(vertical.id.with(1_usize)).unwrap().interact_rect;
                                            content = vertical.inner_rect.intersect(ui.clip_rect());
                                            offset.y = vertical.state.offset.y;
                                        });
                                    pointer.exclude_scrollbar(ui, &horizontal, 0);
                                    bars[0] = ctx.read_response(horizontal.id.with(0_usize)).unwrap().interact_rect;
                                    offset.x = horizontal.state.offset.x;
                                    pointer.finish(ui, grid_selection_ui::SelectionRefs {
                                        rows: &mut tab.selected_rows,
                                        cell: &mut tab.selected_cell,
                                        anchor: &mut tab.cell_anchor,
                                        insert: &mut tab.selected_insert,
                                    });
                                });
                            },
                        );
                        assert_eq!(
                            (
                                tab.selected_rows,
                                tab.selected_cell,
                                tab.cell_anchor,
                                tab.selected_insert
                            ),
                            expected,
                            "scrollbar changed selection: floating={floating}, axis={axis}, preselected={preselected}, coalesced={coalesced}",
                        );
                        (bars, content, offset)
                    };
                    frame(Vec::new());
                    let (bars, content, _) = frame(Vec::new());
                    let start = bars[axis].lerp_inside(egui::vec2(0.1, 0.1));
                    // Hover expands floating bars before pressing their handle.
                    frame(vec![egui::Event::PointerMoved(start)]);
                    let button = |pos, pressed| egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    };
                    if coalesced {
                        frame(vec![
                            button(start, true),
                            egui::Event::PointerMoved(content.center()),
                            button(content.center(), false),
                        ]);
                    } else {
                        frame(vec![button(start, true)]);
                        let delta = if axis == 0 {
                            egui::vec2(60.0, 0.0)
                        } else {
                            egui::vec2(0.0, 60.0)
                        };
                        let (_, _, offset) = frame(vec![egui::Event::PointerMoved(start + delta)]);
                        assert!(
                            offset[axis] > 0.0,
                            "scrollbar must actually scroll: {offset:?}"
                        );
                        frame(vec![egui::Event::PointerMoved(content.center())]);
                        frame(vec![button(content.center(), false)]);
                    }
                }
            }
        }
    }
}

#[test]
fn grid_pointer_selection_preserves_cell_row_shift_and_coalesced_drags() {
    for rows in [false, true] {
        for shift in [false, true] {
            for coalesced in [false, true] {
                let ctx = egui::Context::default();
                let mut tab = QueryTab::new(1);
                if shift {
                    if rows {
                        tab.selected_rows = Some((1, 2));
                    } else {
                        tab.cell_anchor = Some((1, 0));
                        tab.selected_cell = Some((2, 1));
                    }
                }
                let modifiers = if shift {
                    egui::Modifiers::SHIFT
                } else {
                    egui::Modifiers::NONE
                };
                let mut frame = |events| {
                    let mut centers = [[egui::Pos2::ZERO; 3]; 4];
                    let _ = ctx.run(
                        egui::RawInput {
                            events,
                            modifiers,
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                let mut pointer =
                                    grid_selection_ui::PointerSelection::new(ui, &tab);
                                TableBuilder::new(ui)
                                    .drag_to_scroll(false)
                                    .column(Column::exact(44.0))
                                    .column(Column::exact(110.0))
                                    .column(Column::exact(110.0))
                                    .body(|body| {
                                        body.rows(32.0, 4, |mut row| {
                                            let index = row.index();
                                            for (column, center) in
                                                centers[index].iter_mut().enumerate()
                                            {
                                                row.col(|ui| {
                                                    let response = grid_cell(
                                                        ui,
                                                        RichText::new("value"),
                                                        false,
                                                        false,
                                                    );
                                                    *center = response.rect.center();
                                                    pointer.cell(
                                                        ui,
                                                        &response,
                                                        grid_selection_ui::SelectionRefs {
                                                            rows: &mut tab.selected_rows,
                                                            cell: &mut tab.selected_cell,
                                                            anchor: &mut tab.cell_anchor,
                                                            insert: &mut tab.selected_insert,
                                                        },
                                                        (index, column.saturating_sub(1)),
                                                        column == 0,
                                                    );
                                                });
                                            }
                                        });
                                    });
                                pointer.finish(
                                    ui,
                                    grid_selection_ui::SelectionRefs {
                                        rows: &mut tab.selected_rows,
                                        cell: &mut tab.selected_cell,
                                        anchor: &mut tab.cell_anchor,
                                        insert: &mut tab.selected_insert,
                                    },
                                );
                            });
                        },
                    );
                    (
                        centers,
                        tab.selected_rows,
                        tab.selected_cell,
                        tab.cell_anchor,
                    )
                };
                let (centers, ..) = frame(Vec::new());
                let start = centers[3][if rows { 0 } else { 2 }];
                let end = centers[0][1]; // Row dragging also continues over data cells.
                let button = |pos, pressed| egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers,
                };
                let (_, selected_rows, selected_cell, anchor) = if coalesced {
                    frame(vec![
                        egui::Event::PointerMoved(start),
                        button(start, true),
                        egui::Event::PointerMoved(end),
                        button(end, false),
                    ])
                } else {
                    frame(vec![egui::Event::PointerMoved(start), button(start, true)]);
                    frame(vec![egui::Event::PointerMoved(end)]);
                    frame(vec![button(end, false)])
                };
                if rows {
                    assert_eq!(selected_rows, Some((if shift { 1 } else { 3 }, 0)));
                    assert_eq!(selected_cell, None);
                    assert_eq!(anchor, None);
                } else {
                    assert_eq!(selected_rows, None);
                    assert_eq!(selected_cell, Some((0, 0)));
                    assert_eq!(anchor, Some(if shift { (1, 0) } else { (3, 1) }));
                }
            }
        }
    }
}

#[test]
fn dirty_sql_and_rectangular_clipboard_preserve_unicode_and_null() {
    let mut tab = QueryTab::new(1);
    assert!(!tab.dirty());
    tab.sql.push_str("\nSELECT '中文';");
    assert!(tab.dirty());
    tab.saved_sql = tab.sql.clone();
    assert!(!tab.dirty());
    let set = sqlshop::model::ResultSet {
        columns: vec!["a".into(), "b".into()],
        rows: vec![
            vec![Some("中文".into()), None],
            vec![Some("a\tb".into()), Some(String::new())],
        ],
        ..Default::default()
    };
    assert_eq!(
        selected_tsv(&set, Some((1, 1)), Some((0, 0))),
        "中文\tNULL\n\"a\tb\"\t"
    );
    assert_eq!(
        selected_tsv(&set, Some((1, 0)), Some((1, set.columns.len() - 1))),
        "\"a\tb\"\t"
    );
    assert!(cell_in_selection(Some((2, 2)), Some((0, 0)), 1, 1));
    assert!(!cell_in_selection(Some((2, 2)), Some((0, 0)), 3, 1));
}
