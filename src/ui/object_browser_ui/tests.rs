use super::*;

fn tables() -> Vec<Table> {
    vec![
        Table {
            schema: "public".into(),
            name: "users".into(),
            kind: "BASE TABLE".into(),
        },
        Table {
            schema: "audit".into(),
            name: "users".into(),
            kind: "BASE TABLE".into(),
        },
        Table {
            schema: "public".into(),
            name: "active_users".into(),
            kind: "VIEW".into(),
        },
    ]
}

fn data<'a>(scope: &'a BrowserScope, profile: &'a Profile, tables: &'a [Table]) -> BrowserData<'a> {
    BrowserData {
        scope: Some(scope),
        profile: Some(profile),
        connected: true,
        busy: false,
        active_database: Some("app_dev"),
        databases: &[],
        tables,
        catalog: None,
        catalog_loading: false,
        catalog_error: None,
    }
}

fn input(time: f64, events: Vec<egui::Event>) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        )),
        time: Some(time),
        events,
        ..Default::default()
    }
}

fn text_position(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
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

fn click(pos: egui::Pos2, button: egui::PointerButton, pressed: bool) -> Vec<egui::Event> {
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
fn filtering_and_scope_identity_keep_same_named_objects_separate() {
    let profile = Profile::default();
    let tables = tables();
    let scope = BrowserScope {
        connection: profile.id,
        database: Some("app_dev".into()),
        schema: None,
    };
    let data = data(&scope, &profile, &tables);
    let mut state = BrowserState::default();
    state.set_scope(Some(&scope));
    assert_eq!(visible_rows(&data, &state).len(), 3);
    state.category = BrowserCategory::Views;
    assert_eq!(visible_rows(&data, &state).len(), 1);
    state.category = BrowserCategory::Tables;
    state.search = "AUDIT".into();
    let rows = visible_rows(&data, &state);
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0], BrowserRow::Object(table) if table.schema == "audit"));
    state.selected = Some(rows[0].item(&scope));
    let another = BrowserScope {
        connection: Uuid::new_v4(),
        ..scope.clone()
    };
    state.set_scope(Some(&another));
    assert!(state.selected.is_none());
    assert!(state.search.is_empty());
    assert_eq!(state.category, BrowserCategory::All);
    let schema_scope = BrowserScope {
        schema: Some("public".into()),
        ..scope.clone()
    };
    let schema_data = BrowserData {
        scope: Some(&schema_scope),
        ..data
    };
    assert_eq!(visible_rows(&schema_data, &state).len(), 2);
}

#[test]
fn inactive_database_never_displays_another_databases_cached_objects() {
    let profile = Profile::default();
    let tables = tables();
    let scope = BrowserScope {
        connection: profile.id,
        database: Some("other_db".into()),
        schema: None,
    };
    let data = data(&scope, &profile, &tables);
    let mut state = BrowserState::default();
    assert!(visible_rows(&data, &state).is_empty());
    let ctx = egui::Context::default();
    let mut action = None;
    let output = ctx.run(input(0.0, vec![]), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            action = object_browser(ui, &data, &mut state);
        });
    });
    assert!(action.is_none());
    let position = text_position(&output, "选择数据库");
    for pressed in [true, false] {
        let _ = ctx.run(
            input(
                if pressed { 0.02 } else { 0.04 },
                click(position, egui::PointerButton::Primary, pressed),
            ),
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    action = object_browser(ui, &data, &mut state);
                });
            },
        );
    }
    assert_eq!(
        action,
        Some(BrowserAction::SelectDatabase {
            connection: profile.id,
            database: "other_db".into()
        })
    );
    assert_eq!(data.active_database, Some("app_dev"));
}

#[test]
fn object_single_click_selects_double_click_opens_with_complete_target() {
    for busy in [false, true] {
        let profile = Profile::default();
        let tables = tables();
        let scope = BrowserScope {
            connection: profile.id,
            database: Some("app_dev".into()),
            schema: Some("public".into()),
        };
        let data = BrowserData {
            busy,
            ..data(&scope, &profile, &tables)
        };
        let ctx = egui::Context::default();
        let mut state = BrowserState::default();
        let mut frame = |time, events| {
            let mut action = None;
            let output = ctx.run(input(time, events), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    action = object_browser(ui, &data, &mut state);
                });
            });
            (output, action)
        };
        frame(0.0, vec![]);
        let position = text_position(&frame(0.01, vec![]).0, "users");
        frame(0.02, click(position, egui::PointerButton::Primary, true));
        assert!(
            frame(0.04, click(position, egui::PointerButton::Primary, false))
                .1
                .is_none()
        );
        frame(0.12, click(position, egui::PointerButton::Primary, true));
        let action = frame(0.14, click(position, egui::PointerButton::Primary, false)).1;
        if busy {
            assert!(action.is_none());
        } else {
            assert_eq!(
                action,
                Some(BrowserAction::OpenData {
                    connection: profile.id,
                    database: "app_dev".into(),
                    table: tables[0].clone()
                })
            );
        }
        assert_eq!(
            state.selected,
            Some(BrowserItem::Object {
                connection: profile.id,
                database: "app_dev".into(),
                table: tables[0].clone()
            })
        );
    }
}

#[test]
fn inspector_reports_existing_metadata_and_returns_close_action() {
    let profile = Profile::default();
    let tables = tables();
    let scope = BrowserScope {
        connection: profile.id,
        database: Some("app_dev".into()),
        schema: None,
    };
    let browser = data(&scope, &profile, &tables);
    let selected = BrowserItem::Object {
        connection: profile.id,
        database: "app_dev".into(),
        table: tables[0].clone(),
    };
    let metadata = TableMetadata {
        columns: &[],
        primary_keys: &[],
        writable: false,
        stale: true,
    };
    let data = InspectorData {
        structure: None,
        query_preview: None,
        focused_column: None,
        definition: None,
        browser: &browser,
        selected: Some(&selected),
        metadata: Some(metadata),
    };
    let ctx = egui::Context::default();
    let mut action = None;
    let output = ctx.run(input(0.0, vec![]), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            action = inspector(ui, &data);
        });
    });
    assert!(action.is_none());
    text_position(&output, "快照已过期，请刷新后编辑");
    text_position(&output, "无主键");
    let position = text_position(&output, "×");
    for pressed in [true, false] {
        let _ = ctx.run(
            input(
                if pressed { 0.02 } else { 0.04 },
                click(position, egui::PointerButton::Primary, pressed),
            ),
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    action = inspector(ui, &data);
                });
            },
        );
    }
    assert_eq!(action, Some(InspectorAction::Close));
}
