use super::{TreeNode, TreeSelection};
use eframe::egui;
use egui::collapsing_header::{CollapsingState, HeaderResponse};

#[derive(Clone)]
struct NavigationRow {
    node: TreeNode,
    rect: egui::Rect,
    indent: f32,
    response_id: egui::Id,
    branch_id: Option<egui::Id>,
    open: bool,
}

#[derive(Default)]
pub(super) struct NavigationState {
    rows: Vec<NavigationRow>,
    focused: bool,
    focus_node: Option<TreeNode>,
    pending_focus: Option<TreeNode>,
    pending_open: Option<(egui::Id, bool)>,
    pending_activation: Option<TreeNode>,
    last_filter: String,
}

impl NavigationState {
    /// Expand matching branches once per nonempty filter change; clearing the
    /// filter preserves the user's expansion state.
    pub(super) fn search_changed(&mut self, filter: &str) -> bool {
        let changed = self.last_filter != filter;
        if changed {
            self.last_filter = filter.to_owned();
        }
        changed && !filter.is_empty()
    }
}

/// Process only keys belonging to the tree's own row/arrow focus domain.
pub(super) fn begin(ui: &egui::Ui, selection: &mut TreeSelection) {
    let navigation = &mut selection.navigator;
    navigation.pending_focus = None;
    navigation.pending_open = None;
    navigation.pending_activation = None;
    let mut rows = std::mem::take(&mut navigation.rows);
    rows.sort_by(|a, b| a.rect.top().total_cmp(&b.rect.top()));
    let focused_id = ui.memory(|memory| memory.focused());
    let owns_focus = focused_id.is_some_and(|id| {
        rows.iter()
            .any(|row| row.response_id == id || row.branch_id == Some(id))
    });
    let mut outside_click = false;
    if ui.input(|input| input.pointer.any_pressed()) {
        let inside = ui.input(|input| {
            input.pointer.interact_pos().is_some_and(|position| {
                rows.iter().any(|row| {
                    let mut rect = row.rect;
                    if row.branch_id.is_some() {
                        rect.min.x -= ui.spacing().indent;
                    }
                    rect.contains(position)
                })
            })
        });
        if !inside {
            navigation.focused = false;
            outside_click = true;
            if let Some(id) = focused_id.filter(|_| owns_focus) {
                ui.memory_mut(|memory| memory.surrender_focus(id));
            }
        }
    }
    navigation.focused = owns_focus && !outside_click;
    if !ui.is_enabled() || egui::Popup::is_any_open(ui.ctx()) || !navigation.focused {
        return;
    }
    let index = focused_id
        .and_then(|id| {
            rows.iter()
                .position(|row| row.response_id == id || row.branch_id == Some(id))
        })
        .or_else(|| {
            navigation
                .focus_node
                .as_ref()
                .and_then(|node| rows.iter().position(|row| &row.node == node))
        })
        .or_else(|| {
            selection
                .selected
                .as_ref()
                .and_then(|node| rows.iter().position(|row| &row.node == node))
        });
    let Some(index) = index else { return };
    let key = ui.input_mut(|input| {
        // egui's keyboard-generated Button clicks also read these events.
        // Consume Enter before drawing either the row or the arrow so only
        // pending_activation executes, without a second native toggle.
        if !input.modifiers.is_none() {
            return None;
        }
        [
            egui::Key::ArrowUp,
            egui::Key::ArrowDown,
            egui::Key::Home,
            egui::Key::End,
            egui::Key::ArrowLeft,
            egui::Key::ArrowRight,
            egui::Key::Enter,
        ]
        .into_iter()
        .find(|&key| input.consume_key(egui::Modifiers::NONE, key))
    });
    let Some(key) = key else { return };
    let row = &rows[index];
    if matches!(
        key,
        egui::Key::ArrowLeft | egui::Key::ArrowRight | egui::Key::Enter
    ) {
        selection.selected = Some(row.node.clone());
    }
    let next = match key {
        egui::Key::ArrowUp => Some(index.saturating_sub(1)),
        egui::Key::ArrowDown => Some((index + 1).min(rows.len() - 1)),
        egui::Key::Home => Some(0),
        egui::Key::End => Some(rows.len() - 1),
        egui::Key::ArrowLeft => {
            if let Some(id) = row.branch_id.filter(|_| row.open) {
                navigation.pending_open = Some((id, false));
                None
            } else {
                (0..index)
                    .rev()
                    .find(|&parent| rows[parent].indent < row.indent - 1.0)
            }
        }
        egui::Key::ArrowRight => {
            if let Some(id) = row.branch_id.filter(|_| !row.open) {
                navigation.pending_open = Some((id, true));
                None
            } else if row.open
                && rows
                    .get(index + 1)
                    .is_some_and(|child| child.indent > row.indent + 1.0)
            {
                Some(index + 1)
            } else {
                None
            }
        }
        egui::Key::Enter => {
            navigation.pending_activation = Some(row.node.clone());
            None
        }
        _ => None,
    };
    if let Some(index) = next {
        let node = rows[index].node.clone();
        selection.selected = Some(node.clone());
        navigation.focus_node = Some(node.clone());
        navigation.pending_focus = Some(node);
    }
    ui.ctx().request_repaint();
}

pub(super) struct Branch<'ui> {
    pub response: egui::Response,
    pub opened: bool,
    pub activated: bool,
    open: bool,
    header: HeaderResponse<'ui, ()>,
}

impl Branch<'_> {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn body<R>(
        self,
        add_body: impl FnOnce(&mut egui::Ui) -> R,
    ) -> Option<egui::InnerResponse<R>> {
        if self.open {
            self.header.body(add_body).2
        } else {
            // Closing animation must not register logically hidden keyboard rows.
            self.header.body(|_| ());
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn branch<'ui>(
    ui: &'ui mut egui::Ui,
    selection: &mut TreeSelection,
    node: TreeNode,
    id: egui::Id,
    title: egui::WidgetText,
    default_open: bool,
    force_open: Option<bool>,
    double_toggles: bool,
) -> Branch<'ui> {
    let context = ui.ctx().clone();
    let mut state = CollapsingState::load_with_default_open(&context, id, default_open);
    let was_open = state.is_open();
    if let Some(open) = force_open {
        state.set_open(open);
    }
    let mut keyboard_opened = false;
    if let Some((pending_id, open)) = selection.navigator.pending_open
        && pending_id == id
    {
        keyboard_opened = open;
        state.set_open(open);
        selection.navigator.pending_open = None;
    }
    let before_arrow = state.is_open();
    let indent = ui.cursor().left();
    let background = ui.painter().add(egui::Shape::Noop);
    let mut response = None;
    let mut activated = false;
    let mut double_clicked = false;
    let mut header = state.show_header(ui, |ui| {
        let row = ui.add(
            egui::Button::new((title, egui::Atom::grow()))
                .frame(false)
                .sense(egui::Sense::hover())
                .min_size(egui::vec2(
                    ui.available_width(),
                    ui.spacing().interact_size.y,
                )),
        );
        // Keep keyboard focus attached to object identity when refresh/filter
        // inserts siblings; the layout's automatic widget number can change.
        let row = ui.interact(row.rect, id.with("row"), egui::Sense::click());
        selection.row(&row, background, node.clone());
        register_focus(selection, &node, &row);
        double_clicked = row.double_clicked();
        activated = take_activation(selection, &node) || double_clicked;
        response = Some(row);
    });
    let arrow_toggled = header.is_open() != before_arrow;
    if activated && double_toggles {
        if double_clicked {
            header.toggle();
        } else {
            // Enter opens/reveals a container; Left/Right own keyboard folding.
            header.set_open(true);
        }
    }
    let open = header.is_open();
    if arrow_toggled {
        selection.navigator.focused = true;
        selection.navigator.focus_node = Some(node.clone());
        context.memory_mut(|memory| memory.request_focus(id));
    }
    let response = response.expect("tree header always renders its row");
    selection.navigator.rows.push(NavigationRow {
        node,
        rect: response.rect,
        indent,
        response_id: response.id,
        branch_id: Some(id),
        open,
    });
    Branch {
        response,
        // Programmatic reveal (search/restored focus) must never establish a
        // connection or switch databases. Only an explicit expansion loads.
        opened: !was_open && open && (arrow_toggled || keyboard_opened || activated),
        activated,
        open,
        header,
    }
}

pub(super) struct Leaf {
    pub response: egui::Response,
    pub activated: bool,
}

pub(super) fn leaf(
    ui: &mut egui::Ui,
    selection: &mut TreeSelection,
    node: TreeNode,
    title: egui::WidgetText,
) -> Leaf {
    let background = ui.painter().add(egui::Shape::Noop);
    let response = ui
        .push_id(("tree-leaf", format!("{node:?}")), |ui| {
            ui.add(
                egui::Button::new((title, egui::Atom::grow()))
                    .frame(false)
                    .min_size(egui::vec2(
                        ui.available_width(),
                        ui.spacing().interact_size.y,
                    )),
            )
        })
        .inner;
    selection.row(&response, background, node.clone());
    register_focus(selection, &node, &response);
    let activated = response.double_clicked() || take_activation(selection, &node);
    selection.navigator.rows.push(NavigationRow {
        node,
        rect: response.rect,
        indent: response.rect.left(),
        response_id: response.id,
        branch_id: None,
        open: false,
    });
    Leaf {
        response,
        activated,
    }
}

fn take_activation(selection: &mut TreeSelection, node: &TreeNode) -> bool {
    if selection.navigator.pending_activation.as_ref() == Some(node) {
        selection.navigator.pending_activation = None;
        true
    } else {
        false
    }
}

fn register_focus(selection: &mut TreeSelection, node: &TreeNode, response: &egui::Response) {
    if response.clicked() || response.secondary_clicked() {
        selection.navigator.focused = true;
        selection.navigator.focus_node = Some(node.clone());
        response.request_focus();
    }
    if selection.navigator.pending_focus.as_ref() == Some(node) {
        response.request_focus();
        response.scroll_to_me(Some(egui::Align::Center));
        selection.navigator.pending_focus = None;
    }
}
