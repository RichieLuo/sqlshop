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
    start: Option<(usize, usize)>,
    blocked_press: bool,
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
            start: None,
            blocked_press: false,
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
            // ScrollArea registers its floating bars after rendering the cells.
            // Defer mutation until finish can reject presses owned by those bars.
            self.start = Some((row, column));
        }
    }

    pub fn exclude_scrollbar<R>(
        &mut self,
        ui: &egui::Ui,
        scroll: &egui::scroll_area::ScrollAreaOutput<R>,
        axis: usize,
    ) {
        // egui 0.33 uses ScrollArea's id.with(axis) for the bar interaction.
        // Use its actual clipped hit rectangle, including floating/nested bars,
        // rather than subtracting a guessed width from the content viewport.
        // Check the original press, not the endpoint: input may be coalesced.
        let bar_id = scroll.id.with(axis);
        let rect_id = self.id.with(("scrollbar-hit-rect", bar_id));
        if let Some(response) = ui.ctx().read_response(bar_id) {
            let frame = ui.ctx().cumulative_frame_nr();
            let previous_rect = ui.data_mut(|data| {
                let previous = data.get_temp::<(egui::Rect, egui::Rect, u64)>(rect_id);
                data.insert_temp(rect_id, (response.interact_rect, scroll.inner_rect, frame));
                previous
                    .filter(|(_, viewport, previous_frame)| {
                        *viewport == scroll.inner_rect && previous_frame.saturating_add(1) >= frame
                    })
                    .map(|(rect, _, _)| rect)
            });
            if self.pressed
                && response.enabled()
                && response.sense.senses_drag()
                && self.press_position.is_some_and(|position| {
                    // A floating bar may shrink when a coalesced move leaves it.
                    // Its previous hit rect is the one visible at the press.
                    (response.interact_rect.contains(position)
                        || previous_rect.is_some_and(|rect| rect.contains(position)))
                        && ui.ctx().layer_id_at(position) == Some(response.layer_id)
                })
            {
                self.blocked_press = true;
            }
        }
    }

    pub fn finish(self, ui: &egui::Ui, mut selection: SelectionRefs<'_>) {
        if self.blocked_press {
            ui.data_mut(|data| data.remove::<DragSelection>(self.id));
            return;
        }
        if let Some(mut drag) = self.drag {
            if let Some((row, column)) = self.start {
                // Initialize even when press/move/release arrived in one frame.
                apply(&mut selection, drag.anchor, drag.rows, row, column);
                ui.ctx().request_repaint();
            }
            if (self.down || self.released)
                && let Some((row, column, _)) = self.hovered
            {
                apply(&mut selection, drag.anchor, drag.rows, row, column);
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
    selection: &mut SelectionRefs<'_>,
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
