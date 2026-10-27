use super::*;

fn panel_frame(context: &egui::Context, exact: Option<(f32, f32)>) -> (f32, f32) {
    let mut widths = (0.0, 0.0);
    let _ = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 820.0),
            )),
            ..Default::default()
        },
        |ctx| {
            let mut left = egui::SidePanel::left("sidebar")
                .default_width(240.0)
                .width_range(200.0..=800.0);
            let mut right = egui::SidePanel::right("object-information")
                .default_width(240.0)
                .width_range(200.0..=290.0);
            if let Some((navigation, information)) = exact {
                left = left.exact_width(navigation);
                right = right.exact_width(information);
            }
            widths.0 = left
                .show(ctx, |ui| ui.set_min_width(ui.available_width()))
                .response
                .rect
                .width();
            widths.1 = right
                .show(ctx, |ui| ui.set_min_width(ui.available_width()))
                .response
                .rect
                .width();
        },
    );
    widths
}

#[test]
fn panel_widths_survive_an_upgrade_without_egui_cache() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = session_test_app();
    app.workspace_store = Some(WorkspaceStore::new(directory.path().into()));
    panel_frame(&app.context, Some((640.0, 270.0)));
    assert!(app.autosave_workspace(true));

    let mut upgraded = session_test_app();
    // A new binary can have different egui type identifiers. Its old cached
    // panel state is unavailable, even though the user's workspace remains.
    assert_eq!(panel_frame(&upgraded.context, None), (240.0, 240.0));
    upgraded.store = Some(Store {
        directory: directory.path().into(),
    });
    upgraded.restore_workspace();
    assert_eq!(panel_frame(&upgraded.context, None), (640.0, 270.0));
}

#[test]
fn resizing_panels_triggers_workspace_autosave_and_hidden_panels_keep_their_width() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = session_test_app();
    app.workspace_store = Some(WorkspaceStore::new(directory.path().into()));
    panel_frame(&app.context, Some((400.0, 250.0)));
    assert!(app.autosave_workspace(true));
    panel_frame(&app.context, Some((590.0, 280.0)));
    assert!(!app.workspace_matches(app.last_workspace.as_ref().unwrap()));
    app.view.navigation = false;
    app.inspector_visible = false;
    assert!(app.autosave_workspace(true));

    let mut upgraded = session_test_app();
    upgraded.store = Some(Store {
        directory: directory.path().into(),
    });
    upgraded.restore_workspace();
    assert!(!upgraded.view.navigation);
    assert!(!upgraded.inspector_visible);
    assert_eq!(panel_frame(&upgraded.context, None), (590.0, 280.0));
}

#[derive(Default)]
struct MemoryStorage(HashMap<String, String>);

impl eframe::Storage for MemoryStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.into(), value);
    }
    fn flush(&mut self) {}
}

#[derive(serde::Serialize)]
struct LegacyMemory {
    data: Vec<(u64, LegacyElement)>,
}

#[derive(serde::Serialize)]
struct LegacyTypeId(u64);

#[derive(serde::Serialize)]
struct LegacyElement {
    type_id: LegacyTypeId,
    ron: String,
    generation: usize,
}

fn legacy_entry(name: &str, width: f32, type_id: u64, generation: usize) -> (u64, LegacyElement) {
    let mut storage = MemoryStorage::default();
    eframe::set_value(
        &mut storage,
        "panel",
        &egui::containers::panel::PanelState {
            rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 820.0)),
        },
    );
    (
        egui::Id::new(name).value() ^ type_id,
        LegacyElement {
            type_id: LegacyTypeId(type_id),
            ron: storage.0.remove("panel").unwrap(),
            generation,
        },
    )
}

#[test]
fn old_binary_panel_widths_migrate_once_into_the_stable_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let old_workspace = session_test_app().workspace_snapshot();
    let mut old_json = serde_json::to_value(&old_workspace).unwrap();
    let layout = old_json["layout"].as_object_mut().unwrap();
    layout.remove("navigation_width");
    layout.remove("information_width");
    std::fs::write(
        directory.path().join("workspace.json"),
        serde_json::to_vec(&old_json).unwrap(),
    )
    .unwrap();
    let mut storage = MemoryStorage::default();
    eframe::set_value(
        &mut storage,
        "egui",
        &LegacyMemory {
            data: vec![
                legacy_entry("sidebar", 350.0, 17, 4),
                legacy_entry("sidebar", 625.0, 29, 0),
                legacy_entry("object-information", 275.0, 29, 0),
                legacy_entry("another-panel", 777.0, 29, 0),
            ],
        },
    );
    let original = storage.0.clone();
    let mut upgraded = session_test_app();
    upgraded.store = Some(Store {
        directory: directory.path().into(),
    });
    upgraded.restore_workspace();
    assert!(
        egui::containers::panel::PanelState::load(&upgraded.context, egui::Id::new("sidebar"))
            .is_none()
    );
    panel_layout_ui::migrate_legacy(&upgraded.context, Some(&storage));
    assert_eq!(panel_frame(&upgraded.context, None), (625.0, 275.0));
    assert_eq!(
        storage.0, original,
        "migration must leave the legacy storage intact"
    );
    assert!(upgraded.autosave_workspace(true));

    let mut next_release = session_test_app();
    next_release.store = Some(Store {
        directory: directory.path().into(),
    });
    next_release.restore_workspace();
    assert_eq!(panel_frame(&next_release.context, None), (625.0, 275.0));
}

#[test]
fn saved_workspace_widths_take_precedence_over_old_framework_cache() {
    let mut storage = MemoryStorage::default();
    eframe::set_value(
        &mut storage,
        "egui",
        &LegacyMemory {
            data: vec![
                legacy_entry("sidebar", 350.0, 29, 0),
                legacy_entry("object-information", 220.0, 29, 0),
            ],
        },
    );
    let context = egui::Context::default();
    panel_layout_ui::restore(
        &context,
        &sqlshop::workspace::WorkspaceLayout {
            navigation_width: Some(680.0),
            information_width: Some(280.0),
            ..Default::default()
        },
    );
    panel_layout_ui::migrate_legacy(&context, Some(&storage));
    assert_eq!(panel_frame(&context, None), (680.0, 280.0));
}

#[test]
fn invalid_saved_widths_are_ignored_and_oversized_widths_fit_panel_limits() {
    let directory = tempfile::tempdir().unwrap();
    let mut saved = session_test_app().workspace_snapshot();
    let layout = saved.layout.as_mut().unwrap();
    layout.navigation_width = Some(-42.0);
    layout.information_width = Some(f32::INFINITY);
    let store = WorkspaceStore::new(directory.path().into());
    store.save(&saved).unwrap();
    let loaded = store.load().unwrap();
    let layout = loaded.layout.unwrap();
    assert_eq!(
        (layout.navigation_width, layout.information_width),
        (None, None)
    );
    let context = egui::Context::default();
    panel_layout_ui::restore(
        &context,
        &sqlshop::workspace::WorkspaceLayout {
            navigation_width: Some(9000.0),
            information_width: Some(9000.0),
            ..Default::default()
        },
    );
    assert_eq!(panel_frame(&context, None), (800.0, 290.0));
}
