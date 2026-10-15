use super::*;

#[derive(Clone, Copy)]
struct DragSelection {
    anchor: (usize, usize),
    rows: bool,
    press_origin: egui::Pos2,
    frame: u64,
}

pub(super) struct SelectionRefs<'a> {
    pub rows: &'a mut Option<(usize, usize)>,
    pub cell: &'a mut Option<(usize, usize)>,
    pub anchor: &'a mut Option<(usize, usize)>,
    pub insert: &'a mut Option<Uuid>,
}

pub(super) struct PointerSelection {
    id: egui::Id,
    drag: Option<DragSelection>,
    position: Option<egui::Pos2>,
    press_position: Option<egui::Pos2>,
    hovered: Option<(usize, usize, bool)>,
    pressed: bool,
    down: bool,
    released: bool,
    shift: bool,
}

impl PointerSelection {
    pub fn new(ui: &egui::Ui, tab: &QueryTab) -> Self {
        let id = egui::Id::new(("grid-pointer-selection", tab.id, tab.result_index));
        let (position, press_position, pressed, down, released, shift) = ui.input(|input| {
            let origin = input
                .events
                .iter()
                .find_map(|event| match event {
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        ..
                    } => Some(*pos),
                    _ => None,
                })
                .or(input.pointer.press_origin());
            (
                input.pointer.interact_pos(),
                origin,
                input.pointer.primary_pressed(),
                input.pointer.primary_down(),
                input.pointer.primary_released(),
                input.modifiers.shift,
            )
        });
        let drag = if !pressed && (down || released) {
            ui.data(|data| data.get_temp::<DragSelection>(id))
                .filter(|drag| drag.frame.saturating_add(1) >= ui.ctx().cumulative_frame_nr())
                .filter(|drag| released || press_position == Some(drag.press_origin))
                .filter(|drag| {
                    if drag.rows {
                        tab.selected_rows.is_some()
                    } else {
                        tab.selected_cell.is_some()
                    }
                })
        } else {
            None
        };
        Self {
            id,
            drag,
            position,
            press_position,
            hovered: None,
            pressed,
            down,
            released,
            shift,
        }
    }

    pub fn cell(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        selection: SelectionRefs<'_>,
        location: (usize, usize),
        row_heading: bool,
    ) {
        if !response.enabled() {
            return;
        }
        let (row, column) = location;
        let rect = response.rect.intersect(ui.clip_rect());
        if self
            .position
            .is_some_and(|position| rect.contains(position))
        {
            self.hovered = Some((row, column, row_heading));
        }
        if self.pressed
            && self.press_position.is_some_and(|position| {
                rect.contains(position) && ui.ctx().layer_id_at(position) == Some(response.layer_id)
            })
            && !egui::Popup::is_any_open(ui.ctx())
        {
            let anchor = if self.shift {
                if row_heading {
                    (row_selection(*selection.rows, row, true).0, 0)
                } else {
                    selection
                        .anchor
                        .or(*selection.cell)
                        .unwrap_or((row, column))
                }
            } else {
                (row, column)
            };
            self.drag = Some(DragSelection {
                anchor,
                rows: row_heading,
                press_origin: self.press_position.unwrap(),
                frame: ui.ctx().cumulative_frame_nr(),
            });
            // Initialize at the press location even when press/move/release were
            // coalesced into one frame. Finish resolves the endpoint after all
            // visible cells have been visited, including reverse-direction drags.
            apply(selection, anchor, row_heading, row, column);
            ui.ctx().request_repaint();
        }
    }

    pub fn finish(self, ui: &egui::Ui, selection: SelectionRefs<'_>) {
        if let Some(mut drag) = self.drag {
            if (self.down || self.released)
                && let Some((row, column, _)) = self.hovered
            {
                apply(selection, drag.anchor, drag.rows, row, column);
                ui.ctx().request_repaint();
            }
            if self.down {
                drag.frame = ui.ctx().cumulative_frame_nr();
                ui.data_mut(|data| data.insert_temp(self.id, drag));
                return;
            }
        }
        ui.data_mut(|data| data.remove::<DragSelection>(self.id));
    }
}

fn apply(
    selection: SelectionRefs<'_>,
    anchor: (usize, usize),
    rows: bool,
    row: usize,
    column: usize,
) {
    *selection.insert = None;
    if rows {
        *selection.rows = Some((anchor.0, row));
        *selection.cell = None;
        *selection.anchor = None;
    } else {
        *selection.rows = None;
        *selection.anchor = Some(anchor);
        *selection.cell = Some((row, column));
    }
}
