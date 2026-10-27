use super::*;
use egui::containers::panel::PanelState;
use serde::Deserialize;
use sqlshop::workspace::WorkspaceLayout;

pub(super) fn widths(context: &egui::Context) -> (Option<f32>, Option<f32>) {
    let width = |name| {
        PanelState::load(context, egui::Id::new(name))
            .map(|state| state.size().x)
            .filter(|width| width.is_finite() && *width > 0.0)
    };
    (width("sidebar"), width("object-information"))
}

fn restore_width(context: &egui::Context, name: &str, width: f32, maximum: f32) {
    if width.is_finite() && width > 0.0 {
        context.data_mut(|data| {
            data.insert_persisted(
                egui::Id::new(name),
                PanelState {
                    rect: egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width.clamp(200.0, maximum), 0.0),
                    ),
                },
            );
        });
    }
}

pub(super) fn restore(context: &egui::Context, layout: &WorkspaceLayout) {
    if let Some(width) = layout.navigation_width {
        restore_width(context, "sidebar", width, 800.0);
    }
    if let Some(width) = layout.information_width {
        restore_width(context, "object-information", width, 290.0);
    }
}

#[derive(Deserialize)]
struct LegacyMemory {
    data: Vec<(u64, LegacyElement)>,
}

#[derive(Deserialize)]
struct LegacyTypeId(u64);

#[derive(Deserialize)]
struct LegacyElement {
    type_id: LegacyTypeId,
    ron: String,
    generation: usize,
}

struct SerializedPanel<'a>(&'a str);

impl eframe::Storage for SerializedPanel<'_> {
    fn get_string(&self, _: &str) -> Option<String> {
        Some(self.0.to_owned())
    }
    fn set_string(&mut self, _: &str, _: String) {}
    fn flush(&mut self) {}
}

/// egui keys cached values by widget id XOR a compiler-generated TypeId. Decode
/// only the two known panel widths when an older binary's TypeId no longer matches.
/// Future launches use the named fields in workspace.json instead of these keys.
pub(super) fn migrate_legacy(context: &egui::Context, storage: Option<&dyn eframe::Storage>) {
    if let (Some(_), Some(_)) = widths(context) {
        return;
    }
    let Some(memory) =
        storage.and_then(|storage| eframe::get_value::<LegacyMemory>(storage, "egui"))
    else {
        return;
    };
    for (name, maximum) in [("sidebar", 800.0), ("object-information", 290.0)] {
        let id = egui::Id::new(name);
        if PanelState::load(context, id).is_some() {
            continue;
        }
        let newest = memory
            .data
            .iter()
            .filter_map(|(key, element)| {
                if key ^ element.type_id.0 != id.value() {
                    return None;
                }
                let state =
                    eframe::get_value::<PanelState>(&SerializedPanel(&element.ron), "panel")?;
                let width = state.size().x;
                (width.is_finite() && width > 0.0).then_some((element.generation, width))
            })
            .min_by_key(|(generation, _)| *generation);
        if let Some((_, width)) = newest {
            restore_width(context, name, width, maximum);
        }
    }
}
