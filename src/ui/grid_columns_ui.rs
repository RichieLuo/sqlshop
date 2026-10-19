use super::*;
use sqlshop::workspace::{ColumnIdentity, GridColumnLayout, GridColumnPreference, TableGridLayout};

pub(super) fn reconcile(layout: &mut GridColumnLayout, columns: &[String]) {
    let mut seen = HashMap::<&str, usize>::new();
    layout.columns = columns
        .iter()
        .map(|name| {
            let occurrence = seen.entry(name.as_str()).or_default();
            let identity = ColumnIdentity {
                name: name.clone(),
                occurrence: *occurrence,
            };
            *occurrence += 1;
            let mut preference = layout
                .columns
                .iter()
                .find(|entry| entry.identity == identity)
                .cloned()
                .unwrap_or(GridColumnPreference {
                    identity,
                    hidden: false,
                    width: 160.0,
                });
            if !preference.width.is_finite() {
                preference.width = 160.0;
            }
            preference.width = preference.width.clamp(70.0, 1000.0);
            preference
        })
        .collect();
    if !layout.columns.is_empty() && layout.columns.iter().all(|entry| entry.hidden) {
        layout.columns[0].hidden = false;
    }
    if layout
        .frozen_through
        .as_ref()
        .is_some_and(|key| !layout.columns.iter().any(|entry| &entry.identity == key))
    {
        layout.frozen_through = None;
    }
}

pub(super) fn visible(layout: &GridColumnLayout, count: usize) -> Vec<usize> {
    (0..count)
        .filter(|&index| !layout.columns.get(index).is_some_and(|entry| entry.hidden))
        .collect()
}

pub(super) fn tab_visible(tab: &QueryTab, count: usize) -> Vec<usize> {
    tab.grid_layouts
        .get(&tab.result_index)
        .map(|layout| visible(layout, count))
        .unwrap_or_else(|| (0..count).collect())
}

pub(super) fn frozen_count(layout: &GridColumnLayout, visible: &[usize]) -> usize {
    layout
        .frozen_through
        .as_ref()
        .and_then(|key| {
            layout
                .columns
                .iter()
                .position(|entry| &entry.identity == key)
        })
        .map_or(0, |through| {
            visible
                .iter()
                .take_while(|&&index| index <= through)
                .count()
        })
}

fn auto_fit(ui: &egui::Ui, set: &ResultSet, index: usize) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let header = set.columns.get(index).map(String::as_str).unwrap_or("");
    // Bound measurement cost; long values still remain available in hover/editor.
    let width = std::iter::once(header)
        .chain(
            set.rows
                .iter()
                .take(1000)
                .filter_map(|row| row.get(index))
                .map(|value| value.as_deref().unwrap_or("NULL")),
        )
        .map(|text| {
            ui.painter()
                .layout_no_wrap(
                    text.chars().take(500).collect(),
                    font.clone(),
                    Color32::WHITE,
                )
                .size()
                .x
        })
        .fold(0.0_f32, f32::max);
    (width + 32.0).clamp(70.0, 650.0)
}

pub(super) fn controls(
    ui: &mut egui::Ui,
    set: &ResultSet,
    layout: &mut GridColumnLayout,
    enabled: bool,
) -> bool {
    let before = layout.clone();
    ui.horizontal(|ui| {
        ui.add_enabled_ui(enabled, |ui| {
            ui.menu_button("列设置", |ui| {
                if ui.button("显示全部列").clicked() {
                    for entry in &mut layout.columns {
                        entry.hidden = false;
                    }
                }
                if ui
                    .button("全部列自动适宽")
                    .on_hover_text("根据表头与当前页前 1000 行测量")
                    .clicked()
                {
                    for (index, entry) in layout.columns.iter_mut().enumerate() {
                        entry.width = auto_fit(ui, set, index);
                    }
                }
                if ui.button("取消冻结").clicked() {
                    layout.frozen_through = None;
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for index in 0..layout.columns.len() {
                            let visible_count =
                                layout.columns.iter().filter(|entry| !entry.hidden).count();
                            let entry = &mut layout.columns[index];
                            ui.horizontal(|ui| {
                                let mut shown = !entry.hidden;
                                if ui
                                    .add_enabled(
                                        entry.hidden || visible_count > 1,
                                        egui::Checkbox::new(&mut shown, &entry.identity.name),
                                    )
                                    .changed()
                                {
                                    entry.hidden = !shown;
                                }
                                if ui.small_button("适宽").clicked() {
                                    entry.width = auto_fit(ui, set, index);
                                }
                                if ui.small_button("冻结至此").clicked() {
                                    layout.frozen_through = Some(entry.identity.clone());
                                }
                            });
                        }
                    });
            });
        });
        let hidden = layout.columns.iter().filter(|entry| entry.hidden).count();
        if hidden > 0 {
            ui.label(RichText::new(format!("已隐藏 {hidden} 列")).small().weak());
        }
        if layout.frozen_through.is_some() {
            ui.label(RichText::new("左侧列已冻结").small().weak());
        }
    });
    *layout != before
}

/// Shift frozen cells into the viewport, and clip the scrolling cells behind them.
/// The table still shares one vertical scroll and one row index space.
pub(super) fn cell<R>(
    ui: &mut egui::Ui,
    frozen: bool,
    offset: f32,
    boundary: f32,
    viewport: egui::Rect,
    render: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let rect = ui.available_rect_before_wrap();
    let rect = if frozen {
        rect.translate(egui::vec2(offset, 0.0))
    } else {
        rect
    };
    let mut clip = ui.clip_rect().intersect(viewport);
    if frozen {
        clip.min.x = viewport.left();
        clip.max.x = boundary;
    } else {
        clip.min.x = clip.min.x.max(boundary);
    }
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.set_clip_rect(clip);
        if frozen {
            ui.painter().rect_filled(rect, 0, surface(ui));
        }
        render(ui)
    })
    .inner
}

fn matches(entry: &TableGridLayout, target: &TableTarget) -> bool {
    entry.connection == target.connection
        && entry.database == target.database
        && entry.schema == target.schema
        && entry.table == target.name
}
pub(super) fn load_table_layout(tab: &mut QueryTab, layouts: &[TableGridLayout]) {
    if let Some(target) = &tab.table_target
        && !tab.grid_layouts.contains_key(&tab.result_index)
        && let Some(saved) = layouts.iter().find(|entry| matches(entry, target))
    {
        tab.grid_layouts
            .insert(tab.result_index, saved.layout.clone());
    }
}
pub(super) fn save_table_layout(tab: &QueryTab, layouts: &mut Vec<TableGridLayout>) {
    let Some(target) = &tab.table_target else {
        return;
    };
    let Some(layout) = tab.grid_layouts.get(&tab.result_index) else {
        return;
    };
    if let Some(saved) = layouts.iter_mut().find(|entry| matches(entry, target)) {
        saved.layout = layout.clone();
    } else {
        layouts.push(TableGridLayout {
            connection: target.connection,
            database: target.database.clone(),
            schema: target.schema.clone(),
            table: target.name.clone(),
            layout: layout.clone(),
        });
    }
}

pub(super) fn selected_tsv(tab: &QueryTab, set: &ResultSet, rows: Option<&[usize]>) -> String {
    let columns = tab_visible(tab, set.columns.len());
    let (start_row, end_row, selected_columns) = if rows.is_some() {
        (0, 0, columns.clone())
    } else if let Some((end_row, end_column)) = tab.selected_cell {
        let (start_row, start_column) = tab.cell_anchor.unwrap_or((end_row, end_column));
        (
            start_row.min(end_row),
            start_row.max(end_row),
            columns
                .into_iter()
                .filter(|column| {
                    (start_column.min(end_column)..=start_column.max(end_column)).contains(column)
                })
                .collect(),
        )
    } else {
        return String::new();
    };
    let row_indices: Vec<_> = rows
        .map(<[usize]>::to_vec)
        .unwrap_or_else(|| (start_row..=end_row).collect());
    let projected = ResultSet {
        columns: selected_columns
            .iter()
            .map(|&index| set.columns[index].clone())
            .collect(),
        rows: row_indices
            .into_iter()
            .filter_map(|row| set.rows.get(row))
            .map(|row| {
                selected_columns
                    .iter()
                    .map(|&index| row.get(index).cloned().unwrap_or(None))
                    .collect()
            })
            .collect(),
        ..Default::default()
    };
    super::selected_tsv(
        &projected,
        Some((0, 0)),
        Some((
            projected.rows.len().saturating_sub(1),
            projected.columns.len().saturating_sub(1),
        )),
    )
}

pub(super) fn rows_tsv(set: &ResultSet, columns: &[usize], rows: &[usize]) -> String {
    let projected = ResultSet {
        columns: columns
            .iter()
            .map(|&index| set.columns[index].clone())
            .collect(),
        rows: rows
            .iter()
            .filter_map(|&row| set.rows.get(row))
            .map(|row| {
                columns
                    .iter()
                    .map(|&index| row.get(index).cloned().unwrap_or(None))
                    .collect()
            })
            .collect(),
        ..Default::default()
    };
    super::selected_tsv(
        &projected,
        Some((0, 0)),
        Some((
            projected.rows.len().saturating_sub(1),
            projected.columns.len().saturating_sub(1),
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editable_tab() -> QueryTab {
        let mut tab = QueryTab::new(1);
        tab.table_target = Some(TableTarget {
            connection: Uuid::new_v4(),
            database: "db".into(),
            schema: "public".into(),
            name: "items".into(),
        });
        let set = ResultSet {
            columns: vec!["a".into(), "hidden".into(), "c".into()],
            rows: vec![vec![
                Some("7".into()),
                Some("secret".into()),
                Some("x\ty".into()),
            ]],
            ..Default::default()
        };
        let output = QueryOutput {
            sets: vec![set.clone()],
            ..Default::default()
        };
        tab.table_data = Some(TableData {
            output: output.clone(),
            primary_keys: vec!["a".into()],
            keys: vec![vec![Some("7".into())]],
            writable: true,
            columns: set
                .columns
                .iter()
                .map(|name| sqlshop::model::TableColumn {
                    name: name.clone(),
                    declared_type: "text".into(),
                    sql_type: "text".into(),
                    kind: sqlshop::model::CellKind::Text,
                    editable: true,
                    primary_key: name == "a",
                })
                .collect(),
        });
        tab.output = Some(output);
        let mut layout = GridColumnLayout::default();
        reconcile(&mut layout, &set.columns);
        layout.columns[1].hidden = true;
        tab.grid_layouts.insert(0, layout);
        tab.selected_cell = Some((0, 2));
        tab.cell_anchor = Some((0, 0));
        tab
    }

    #[test]
    fn grid_columns_copy_paste_and_drafts_keep_original_identity() {
        let mut tab = editable_tab();
        let set = &tab.output.as_ref().unwrap().sets[0];
        assert_eq!(selected_tsv(&tab, set, None), "7\t\"x\ty\"");
        assert_eq!(selected_grid_cells(&tab, 0, 0), vec![(0, 0), (0, 2)]);
        assert_eq!(
            paste_grid_values(&mut tab, "new-a\tnew-c", true).unwrap(),
            2
        );
        assert_eq!(
            tab.staged_cells
                .iter()
                .map(|change| change.column.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "c"]
        );
        let previous = tab
            .staged_cells
            .iter()
            .map(|change| change.value.clone())
            .collect::<Vec<_>>();
        assert!(paste_grid_values(&mut tab, "1\t2\t3", true).is_err());
        assert_eq!(
            tab.staged_cells
                .iter()
                .map(|change| change.value.clone())
                .collect::<Vec<_>>(),
            previous
        );
        tab.table_data.as_mut().unwrap().columns[2].editable = false;
        assert!(paste_grid_values(&mut tab, "other-a\tother-c", true).is_err());
        assert_eq!(
            tab.staged_cells
                .iter()
                .map(|change| change.value.clone())
                .collect::<Vec<_>>(),
            previous
        );
    }

    #[test]
    fn grid_columns_reconcile_duplicates_and_schema_changes_by_identity() {
        let mut layout = GridColumnLayout::default();
        reconcile(&mut layout, &["a".into(), "a".into(), "b".into()]);
        layout.columns[1].hidden = true;
        layout.columns[2].width = 330.0;
        layout.frozen_through = Some(layout.columns[1].identity.clone());
        reconcile(
            &mut layout,
            &["b".into(), "a".into(), "a".into(), "new".into()],
        );
        assert_eq!(visible(&layout, 4), vec![0, 1, 3]);
        assert_eq!(layout.columns[0].width, 330.0);
        assert_eq!(frozen_count(&layout, &visible(&layout, 4)), 2);
        reconcile(&mut layout, &["new".into()]);
        assert!(layout.frozen_through.is_none());
        layout.columns[0].hidden = true;
        layout.columns[0].width = f32::NAN;
        reconcile(&mut layout, &["new".into()]);
        assert!(!layout.columns[0].hidden);
        assert!(layout.columns[0].width.is_finite());
    }

    #[test]
    fn grid_columns_roundtrip_preferences_without_results_and_isolate_tables() {
        let tab = editable_tab();
        let mut workspace = Workspace::default();
        save_table_layout(&tab, &mut workspace.table_grid_layouts);
        let directory = tempfile::tempdir().unwrap();
        let store = WorkspaceStore::new(directory.path().into());
        store.save(&workspace).unwrap();
        let saved = store.load().unwrap();
        assert_eq!(workspace.table_grid_layouts, saved.table_grid_layouts);
        let json = std::fs::read_to_string(directory.path().join("workspace.json")).unwrap();
        assert!(!json.contains("secret"));
        let mut restored = QueryTab::new(1);
        restored.table_target = tab.table_target.clone();
        load_table_layout(&mut restored, &saved.table_grid_layouts);
        assert!(restored.grid_layouts[&0].columns[1].hidden);
        restored.grid_layouts.clear();
        restored.table_target.as_mut().unwrap().schema = "other".into();
        load_table_layout(&mut restored, &saved.table_grid_layouts);
        assert!(restored.grid_layouts.is_empty());
        restored.table_target = None;
        save_table_layout(&restored, &mut workspace.table_grid_layouts);
        assert_eq!(workspace.table_grid_layouts.len(), 1);
    }

    #[test]
    fn grid_columns_frozen_cells_keep_rect_and_clip_when_scrolled() {
        let context = egui::Context::default();
        let _ = context.run(Default::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                let viewport =
                    egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(500.0, 300.0));
                let original =
                    egui::Rect::from_min_size(egui::pos2(-90.0, 40.0), egui::vec2(160.0, 32.0));
                ui.scope_builder(egui::UiBuilder::new().max_rect(original), |ui| {
                    ui.set_clip_rect(original.intersect(viewport));
                    cell(ui, true, 100.0, 170.0, viewport, |ui| {
                        assert_eq!(ui.available_rect_before_wrap().left(), 10.0);
                        assert_eq!(ui.clip_rect().left(), 10.0);
                        assert_eq!(ui.clip_rect().right(), 170.0);
                    });
                });
            });
        });
    }
    #[test]
    fn grid_columns_actual_grid_freezes_and_keeps_scrollable_cells_clipped() {
        let context = egui::Context::default();
        let mut tab = editable_tab();
        let layout = tab.grid_layouts.get_mut(&0).unwrap();
        layout.columns[1].hidden = false;
        layout.frozen_through = Some(layout.columns[0].identity.clone());
        let mut render = |events| {
            context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 260.0),
                    )),
                    events,
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        super::super::result_grid(ui, &mut tab, false);
                    });
                },
            )
        };
        render(vec![]);
        let before = render(vec![]);
        let position = |output: &egui::FullOutput, value: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == value => {
                        Some((text.pos.x, shape.clip_rect))
                    }
                    _ => None,
                })
                .unwrap()
        };
        let first_before = position(&before, "7");
        let scrolling_before = position(&before, "secret");
        render(vec![
            egui::Event::PointerMoved(egui::pos2(300.0, 110.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(-120.0, 0.0),
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        let after = render(vec![]);
        let first_after = position(&after, "7");
        let scrolling_after = position(&after, "secret");
        assert!(
            (first_before.0 - first_after.0).abs() < 0.1,
            "before={first_before:?} after={first_after:?} scrolling={scrolling_before:?}/{scrolling_after:?}"
        );
        assert!(scrolling_after.0 < scrolling_before.0);
        assert!(scrolling_after.1.left() >= first_after.1.right() - 0.1);
    }
}
