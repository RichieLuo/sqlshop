use super::*;

#[test]
fn connection_scroll_area_uses_full_sidebar_width() {
    for width in [265.0, 302.0, 450.0] {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::SidePanel::left("test-sidebar")
                .exact_width(width)
                .show(ctx, |ui| {
                    let available = ui.available_rect_before_wrap();
                    let scroll = egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for _ in 0..100 {
                                ui.label("短表名");
                            }
                        });
                    let gap = available.right() - scroll.inner_rect.right();
                    assert!(gap >= 0.0 && gap <= ui.spacing().scroll.allocated_width() + 1.0);
                    assert!(scroll.inner_rect.width() > width - 50.0);
                });
        });
    }
}

#[test]
fn tree_selection_moves_between_connection_database_and_table() {
    let ctx = egui::Context::default();
    let connection = Uuid::new_v4();
    let table = table();
    let nodes = [
        TreeNode::Connection(connection),
        TreeNode::Database(connection, "app_dev".into()),
        TreeNode::Table(connection, "app_dev".into(), table.clone()),
    ];
    let mut selection = TreeSelection::default();
    let mut positions = [egui::Pos2::ZERO; 3];
    let mut time = 0.0;
    let mut frame = |click: Option<(usize, bool, egui::PointerButton)>| {
        time += 0.1;
        let events = click.map_or_else(Vec::new, |(index, pressed, button)| {
            let pos = positions[index];
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        });
        let mut fill = egui::Color32::TRANSPARENT;
        let mut row_right = 0.0;
        let mut schema_dialog = None;
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
                    for (index, label) in ["server", "app_dev"].into_iter().enumerate() {
                        let background = ui.painter().add(egui::Shape::Noop);
                        let header = egui::CollapsingHeader::new(
                            RichText::new(label).color(ui.visuals().text_color()),
                        )
                        .show(ui, |_| {});
                        positions[index] = header.header_response.rect.center();
                        selection.row(&header.header_response, background, nodes[index].clone());
                    }
                    // An active database still has selectable children while work is busy.
                    table_buttons(
                        ui,
                        &[&table],
                        &mut selection,
                        connection,
                        "app_dev",
                        true,
                        &mut schema_dialog,
                    );
                    positions[2] = selection.rows.last().unwrap().1.center();
                    fill =
                        accent(ui).gamma_multiply(if ui.visuals().dark_mode { 0.30 } else { 0.14 });
                    row_right = ui.max_rect().right();
                    selection.paint(ui);
                });
            },
        );
        let table_text = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == table.name => Some(text),
                _ => None,
            })
            .expect("table label must be rendered");
        assert!(
            table_text.pos.x < positions[2].x - 100.0,
            "table label stays left-aligned"
        );
        let backgrounds = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.fill == fill => Some(rect),
                _ => None,
            })
            .inspect(|rect| {
                assert_eq!(
                    rect.rect.right(),
                    row_right,
                    "selection fills the row width"
                );
            })
            .count();
        (selection.selected.clone(), backgrounds)
    };
    assert_eq!(frame(None), (None, 0));
    // Returning to the already-active database after a table click must move the
    // background back to the database, not keep either parent highlighted.
    for index in [0, 1, 2, 1, 2, 0] {
        frame(Some((index, true, egui::PointerButton::Primary)));
        assert_eq!(
            frame(Some((index, false, egui::PointerButton::Primary))),
            (Some(nodes[index].clone()), 1),
        );
    }
    frame(Some((1, true, egui::PointerButton::Secondary)));
    assert_eq!(
        frame(Some((1, false, egui::PointerButton::Secondary))),
        (Some(nodes[1].clone()), 1),
    );
}

#[test]
fn tree_selection_background_covers_nested_indentation() {
    let ctx = egui::Context::default();
    let node = TreeNode::Connection(Uuid::new_v4());
    let mut selection = TreeSelection {
        navigator: Default::default(),
        selected: Some(node.clone()),
        ..Default::default()
    };
    let mut bounds = egui::Rect::NOTHING;
    let mut fill = Color32::TRANSPARENT;
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            bounds = ui.max_rect();
            fill = accent(ui).gamma_multiply(if ui.visuals().dark_mode { 0.30 } else { 0.14 });
            ui.indent("group", |ui| {
                ui.indent("database", |ui| {
                    let background = ui.painter().add(egui::Shape::Noop);
                    let response = ui.button("nested table");
                    assert!(response.rect.left() > bounds.left());
                    selection.row(&response, background, node.clone());
                });
            });
            selection.paint(ui);
        });
    });
    let background = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect) if rect.fill == fill => Some(rect),
            _ => None,
        })
        .expect("selected row background must be rendered");
    assert_eq!(background.rect.left(), bounds.left());
    assert_eq!(background.rect.right(), bounds.right());
}

#[test]
fn tree_node_identity_includes_connection_database_and_schema() {
    let connection = Uuid::new_v4();
    let node = TreeNode::Table(connection, "app_dev".into(), table());
    assert_ne!(
        node,
        TreeNode::Table(Uuid::new_v4(), "app_dev".into(), table())
    );
    assert_ne!(
        node,
        TreeNode::Table(connection, "app_prod".into(), table())
    );
    let mut other_schema = table();
    other_schema.schema = "archive".into();
    assert_ne!(
        node,
        TreeNode::Table(connection, "app_dev".into(), other_schema)
    );
}

#[test]
fn table_single_click_selects_and_double_click_opens() {
    let ctx = egui::Context::default();
    let table = table();
    let connection = Uuid::new_v4();
    let mut selection = TreeSelection::default();
    let mut frame = |time, pressed: Option<bool>, busy| {
        let position = egui::pos2(50.0, 18.0);
        let mut events = vec![egui::Event::PointerMoved(position)];
        if let Some(pressed) = pressed {
            events.push(egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        let mut opened = None;
        let mut schema_dialog = None;
        let _ = ctx.run(
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
                    opened = table_buttons(
                        ui,
                        &[&table],
                        &mut selection,
                        connection,
                        "app_dev",
                        busy,
                        &mut schema_dialog,
                    );
                    selection.paint(ui);
                });
            },
        );
        (opened, selection.selected.clone())
    };
    frame(0.0, None, false);
    frame(0.02, Some(true), false);
    let (opened, selected) = frame(0.03, Some(false), false);
    assert!(opened.is_none());
    assert_eq!(
        selected,
        Some(TreeNode::Table(connection, "app_dev".into(), table.clone()))
    );
    frame(0.12, Some(true), false);
    assert_eq!(frame(0.13, Some(false), false).0, Some(table.clone()));
    // Selection remains usable during work, but cannot launch a query.
    frame(1.0, Some(true), true);
    assert!(frame(1.01, Some(false), true).0.is_none());
    frame(1.1, Some(true), true);
    assert!(frame(1.11, Some(false), true).0.is_none());
}

#[test]
fn reopening_table_preserves_tab_and_distinguishes_database_and_schema() {
    let table = table();
    let target = TableTarget {
        connection: Uuid::new_v4(),
        database: "app_dev".into(),
        schema: table.schema.clone(),
        name: table.name.clone(),
    };
    let mut tabs = vec![QueryTab::new(1)];
    let mut next = 2;
    let (index, created) = open_table_tab(
        &mut tabs,
        &mut next,
        target.clone(),
        Engine::Postgres,
        &table,
    );
    assert!(created);
    let id = tabs[index].id;
    tabs[index].sql = "SELECT id FROM public.users LIMIT 10".into();
    tabs[index].output = Some(QueryOutput::default());
    let reopened = open_table_tab(
        &mut tabs,
        &mut next,
        target.clone(),
        Engine::Postgres,
        &table,
    );
    assert_eq!(reopened, (index, false));
    assert_eq!(tabs.len(), 2);
    assert_eq!(tabs[index].id, id);
    assert_eq!(tabs[index].sql, "SELECT id FROM public.users LIMIT 10");
    assert!(tabs[index].output.is_some());
    for distinct in [
        TableTarget {
            connection: Uuid::new_v4(),
            ..target.clone()
        },
        TableTarget {
            database: "app_prod".into(),
            ..target.clone()
        },
        TableTarget {
            schema: "audit".into(),
            ..target.clone()
        },
    ] {
        assert!(open_table_tab(&mut tabs, &mut next, distinct, Engine::Postgres, &table).1);
    }
    tabs.remove(index);
    let (index, created) = open_table_tab(&mut tabs, &mut next, target, Engine::Postgres, &table);
    assert!(created);
    assert_ne!(tabs[index].id, id);
}
