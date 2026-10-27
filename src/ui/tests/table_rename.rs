use super::*;

#[test]
fn table_rename_blocks_drafts_and_other_database_without_losing_edits() {
    let (mut app, target, change) = table_flow_fixture();
    let database = target.database.as_deref().unwrap();
    assert!(
        app.table_rename_rejection(target.connection, database, &change.table)
            .is_none()
    );
    app.tabs[0].stage_cell(change.clone());
    app.request_on(
        target.connection,
        Request::RenameTable {
            database: database.into(),
            table: change.table.clone(),
            new_name: "renamed".into(),
        },
        "rename",
    );
    assert!(!app.session_busy(target.connection));
    assert!(app.status.contains("未保存"));
    assert_eq!(app.tabs[0].staged_cells.len(), 1);
    assert_eq!(app.tabs[0].staged_cells[0].value, change.value);
    assert_eq!(app.tabs[0].staged_cells[0].table, change.table);
    app.tabs[0].clear_table_drafts();
    assert!(
        app.table_rename_rejection(target.connection, "other_database", &change.table)
            .is_some()
    );
    app.cell_editor = Some(CellEditor {
        tab: app.tabs[0].id,
        connection: target.connection,
        change: change.clone(),
        text: "unsaved".into(),
        is_null: false,
        focus: false,
        expanded: false,
    });
    assert!(
        app.table_rename_rejection(target.connection, database, &change.table)
            .is_some()
    );
    app.cell_editor = None;
    // A draft on a different table must not prevent renaming this one.
    app.tabs[0].table_target.as_mut().unwrap().name = "unrelated".into();
    app.tabs[0].stage_cell(change.clone());
    assert!(
        app.table_rename_rejection(target.connection, database, &change.table)
            .is_none()
    );
}

#[test]
fn table_renamed_response_updates_matching_documents_and_selection_only() {
    let (mut app, target, change) = table_flow_fixture();
    let table = change.table;
    let database = target.database.clone().unwrap();
    let identity = app.tabs[0].table_target.clone().unwrap();
    let mut design = QueryTab::new(2);
    design.table_target = Some(identity.clone());
    design.target = Some(target.clone());
    design.design = Some(structure_ui::DesignDocument::default());
    app.tabs.push(design);
    let mut other = QueryTab::new(3);
    other.table_target = Some(TableTarget {
        schema: "other_schema".into(),
        ..identity.clone()
    });
    other.sql = "SELECT 'other table'".into();
    app.tabs.push(other);
    let mut query = QueryTab::new(4);
    query.target = Some(target.clone());
    query.sql = "SELECT 'keep my SQL draft'".into();
    app.tabs.push(query);
    app.browser_state.selected = Some(object_browser_ui::BrowserItem::Object {
        connection: target.connection,
        database: database.clone(),
        table: table.clone(),
    });
    app.tree_selection.selected = Some(TreeNode::TablePart(
        target.connection,
        database.clone(),
        table.clone(),
        table_tree_ui::TablePart::Fields,
        Some("id".into()),
    ));
    app.sessions
        .get_mut(&target.connection)
        .unwrap()
        .column_cache
        .insert(
            (table.schema.clone(), table.name.clone()),
            vec!["id".into()],
        );
    app.handle_response(
        target.connection,
        Response::TableRenamed {
            database,
            table,
            new_name: "renamed".into(),
        },
    );
    assert_eq!(app.tabs[0].table_target.as_ref().unwrap().name, "renamed");
    assert_eq!(app.tabs[0].name, "renamed");
    assert!(app.tabs[0].sql.contains("`renamed`"));
    assert!(app.tabs[0].table_snapshot_stale);
    assert!(app.tabs[0].table_data.is_some());
    assert_eq!(app.tabs[1].name, "renamed · 设计");
    assert!(app.tabs[1].design.as_ref().unwrap().stale);
    assert_eq!(
        app.tabs[2].table_target.as_ref().unwrap().name,
        identity.name
    );
    assert!(!app.tabs[2].table_snapshot_stale);
    assert_eq!(app.tabs[2].sql, "SELECT 'other table'");
    assert_eq!(app.tabs[3].sql, "SELECT 'keep my SQL draft'");
    assert!(
        matches!(&app.tree_selection.selected, Some(TreeNode::TablePart(_, _, table, _, _)) if table.name == "renamed")
    );
    assert!(
        matches!(&app.browser_state.selected, Some(object_browser_ui::BrowserItem::Object { table, .. }) if table.name == "renamed")
    );
    let session = &app.sessions[&target.connection];
    assert!(session.column_cache.is_empty());
    assert_eq!(session.tables[0].name, "renamed");
    assert!(session.catalog_loading);
}

fn frame(
    ctx: &egui::Context,
    time: f64,
    events: Vec<egui::Event>,
    mut render: impl FnMut(&mut egui::Ui),
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| render(ui));
        },
    )
}

fn position(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == label => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing label {label}"))
}

fn pointer(pos: egui::Pos2, button: egui::PointerButton, pressed: bool) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

#[test]
fn tree_context_menu_opens_rename_dialog_with_current_name() {
    let ctx = egui::Context::default();
    let table = table();
    let connection = Uuid::new_v4();
    let mut selection = TreeSelection::default();
    let mut dialog = None;
    let mut render_state = table_tree_ui::TreeRender::default();
    let entries = HashMap::new();
    let mut render = |ui: &mut egui::Ui| {
        table_tree_ui::tables(
            ui,
            &[&table],
            &mut selection,
            connection,
            "app_dev",
            false,
            &mut dialog,
            &mut render_state,
            &entries,
        );
    };
    let output = frame(&ctx, 0.0, vec![], &mut render);
    let table_pos = position(&output, "users");
    for (step, pressed) in [true, false].into_iter().enumerate() {
        frame(
            &ctx,
            0.1 + step as f64 * 0.1,
            pointer(table_pos, egui::PointerButton::Secondary, pressed),
            &mut render,
        );
    }
    let output = frame(&ctx, 0.3, vec![], &mut render);
    let rename_pos = position(&output, "重命名…");
    for (step, pressed) in [true, false].into_iter().enumerate() {
        frame(
            &ctx,
            0.4 + step as f64 * 0.1,
            pointer(rename_pos, egui::PointerButton::Primary, pressed),
            &mut render,
        );
    }
    let dialog = dialog.expect("rename dialog");
    assert_eq!(dialog.name, "users");
    assert!(
        matches!(dialog.action, SchemaDialogAction::RenameTable { connection: id, database, table: selected } if id == connection && database == "app_dev" && selected == table)
    );
}
