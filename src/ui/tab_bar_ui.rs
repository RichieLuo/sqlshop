use super::*;

pub(super) enum BarAction {
    Objects,
    Document(TabAction),
    Information,
    Reorder { from: usize, to: usize },
}

#[derive(Clone, Copy)]
pub(super) enum TabIcon {
    Objects,
    Query,
    Table,
    View,
    Design,
}

pub(super) fn paint_icon(ui: &egui::Ui, rect: egui::Rect, icon: TabIcon, color: Color32) {
    let stroke = egui::Stroke::new(1.1, color);
    let r = rect.shrink(2.0);
    match icon {
        TabIcon::Objects => {
            for x in 0..2 {
                for y in 0..2 {
                    let min = r.min + egui::vec2(x as f32 * 7.0, y as f32 * 7.0);
                    ui.painter().rect_stroke(
                        egui::Rect::from_min_size(min, egui::vec2(4.5, 4.5)),
                        0.5,
                        stroke,
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
        TabIcon::Query => {
            ui.painter()
                .rect_stroke(r, 1.0, stroke, egui::StrokeKind::Inside);
            ui.painter().line_segment(
                [r.min + egui::vec2(3.0, 3.5), r.min + egui::vec2(5.5, 6.0)],
                stroke,
            );
            ui.painter().line_segment(
                [r.min + egui::vec2(5.5, 6.0), r.min + egui::vec2(3.0, 8.5)],
                stroke,
            );
            ui.painter().line_segment(
                [r.min + egui::vec2(7.0, 8.5), r.min + egui::vec2(10.0, 8.5)],
                stroke,
            );
        }
        TabIcon::Table | TabIcon::View | TabIcon::Design => {
            ui.painter()
                .rect_stroke(r, 0.5, stroke, egui::StrokeKind::Inside);
            for part in [0.33, 0.66] {
                let y = r.top() + r.height() * part;
                ui.painter()
                    .line_segment([egui::pos2(r.left(), y), egui::pos2(r.right(), y)], stroke);
                if matches!(icon, TabIcon::Table | TabIcon::Design) {
                    let x = r.left() + r.width() * part;
                    ui.painter()
                        .line_segment([egui::pos2(x, r.top()), egui::pos2(x, r.bottom())], stroke);
                }
            }
            if matches!(icon, TabIcon::Design) {
                ui.painter().line_segment(
                    [
                        r.left_bottom() + egui::vec2(2.0, -1.0),
                        r.right_top() + egui::vec2(-1.0, 2.0),
                    ],
                    egui::Stroke::new(2.2, color),
                );
            }
        }
    }
}

fn paint_tab(ui: &egui::Ui, rect: egui::Rect, selected: bool, hovered: bool) {
    let fill = if selected {
        surface(ui)
    } else if hovered {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(
        rect,
        egui::CornerRadius {
            nw: 3,
            ne: 3,
            sw: 0,
            se: 0,
        },
        fill,
    );
    if selected {
        ui.painter().line_segment(
            [
                rect.left_top() + egui::vec2(1.0, 1.0),
                rect.right_top() + egui::vec2(-1.0, 1.0),
            ],
            egui::Stroke::new(2.0, accent(ui)),
        );
    } else {
        ui.painter().line_segment(
            [
                rect.right_top() + egui::vec2(0.0, 8.0),
                rect.right_bottom() - egui::vec2(0.0, 8.0),
            ],
            egui::Stroke::new(0.5, ui.visuals().widgets.noninteractive.bg_stroke.color),
        );
    }
}

pub(super) fn tab_bar(
    ui: &mut egui::Ui,
    tabs: &[QueryTab],
    profiles: &[Profile],
    active: usize,
    objects_active: bool,
    editor_open: bool,
    information_open: bool,
) -> Option<BarAction> {
    let mut action = None;
    let drag_id = egui::Id::new("document-tab-drag");
    let mut dragged = ui.data(|data| data.get_temp::<Uuid>(drag_id));
    let released = ui.input(|input| input.pointer.any_released());
    let active_id = (!objects_active).then_some(tabs[active].id);
    let previous =
        ui.data(|data| data.get_temp::<(Uuid, f32)>(egui::Id::new("visible-document-tab")));
    let reveal = active_id.is_some_and(|id| {
        previous.is_none_or(|(old_id, width)| {
            old_id != id || (width - ui.available_width()).abs() > 2.0
        })
    });
    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
    ui.spacing_mut().button_padding = egui::vec2(5.0, 3.0);
    ui.spacing_mut().interact_size.y = 24.0;
    ui.spacing_mut().interact_size.x = 24.0;
    ui.visuals_mut().widgets.inactive.corner_radius = egui::CornerRadius::same(2);
    ui.visuals_mut().widgets.hovered.corner_radius = egui::CornerRadius::same(2);
    let available = ui.available_rect_before_wrap();
    let bar = egui::Rect::from_min_size(available.min, egui::vec2(available.width(), 32.0));
    ui.painter().rect_filled(bar, 0, chrome(ui.ctx()));
    ui.painter().line_segment(
        [bar.left_bottom(), bar.right_bottom()],
        egui::Stroke::new(0.5, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );
    ui.horizontal(|ui| {
        let (home, response) = ui.allocate_exact_size(egui::vec2(86.0, 32.0), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                ui.is_enabled(),
                objects_active,
                "对象工作区",
            )
        });
        paint_tab(ui, home, objects_active, response.hovered());
        paint_icon(
            ui,
            egui::Rect::from_min_size(home.min + egui::vec2(10.0, 8.0), egui::vec2(16.0, 16.0)),
            TabIcon::Objects,
            ui.visuals().text_color(),
        );
        ui.painter().text(
            home.min + egui::vec2(32.0, 16.0),
            egui::Align2::LEFT_CENTER,
            "对象",
            egui::FontId::proportional(13.0),
            ui.visuals().text_color(),
        );
        if response.on_hover_text("数据库对象工作区").clicked() {
            action = Some(BarAction::Objects);
        }
        let width = (ui.available_width() - 84.0).max(80.0);
        ui.allocate_ui_with_layout(
            egui::vec2(width, 32.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("document-tabs")
                    .max_width(width)
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        // A nested horizontal row starts with the 24px widget
                        // height and centers it inside this 32px viewport.
                        // Allocate the document tabs directly from its top edge.
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                            for (index, tab) in tabs.iter().enumerate() {
                                ui.push_id(tab.id, |ui| {
                                    let connection_name = tab.target.as_ref().map(|target| {
                                        profiles
                                            .iter()
                                            .find(|profile| profile.id == target.connection)
                                            .map_or_else(
                                                || format!("已删除的连接 {}", target.connection),
                                                |profile| profile.name.clone(),
                                            )
                                    });
                                    let caption = if tabs
                                        .iter()
                                        .any(|other| other.id != tab.id && other.name == tab.name)
                                    {
                                        connection_name.as_ref().map_or_else(
                                            || tab.name.clone(),
                                            |name| format!("{} · {name}", tab.name),
                                        )
                                    } else {
                                        tab.name.clone()
                                    };
                                    let label = format!(
                                        "{}{}",
                                        caption,
                                        if tab.document_dirty() { " •" } else { "" }
                                    );
                                    let text_width = ui
                                        .painter()
                                        .layout_no_wrap(
                                            label.clone(),
                                            egui::FontId::proportional(13.0),
                                            ui.visuals().text_color(),
                                        )
                                        .size()
                                        .x;
                                    let tab_width = (text_width + 58.0).clamp(104.0, 184.0);
                                    let (rect, response) = ui.allocate_exact_size(
                                        egui::vec2(tab_width, 32.0),
                                        egui::Sense::click_and_drag(),
                                    );
                                    let selected = !objects_active && index == active;
                                    if response.drag_started() && !editor_open {
                                        dragged = Some(tab.id);
                                    }
                                    if dragged.is_some()
                                        && !editor_open
                                        && response.contains_pointer()
                                    {
                                        let after = ui
                                            .input(|input| input.pointer.hover_pos())
                                            .is_some_and(|pointer| pointer.x > rect.center().x);
                                        let x = if after { rect.right() } else { rect.left() };
                                        ui.painter().vline(
                                            x,
                                            rect.y_range(),
                                            egui::Stroke::new(2.0, accent(ui)),
                                        );
                                        if released
                                            && let Some(from) =
                                                tabs.iter().position(|tab| Some(tab.id) == dragged)
                                        {
                                            let insertion = index + usize::from(after);
                                            let to = if insertion > from {
                                                insertion - 1
                                            } else {
                                                insertion
                                            };
                                            if from != to {
                                                action = Some(BarAction::Reorder { from, to });
                                            }
                                        }
                                    }
                                    response.widget_info(|| {
                                        egui::WidgetInfo::selected(
                                            egui::WidgetType::SelectableLabel,
                                            ui.is_enabled(),
                                            selected,
                                            &tab.name,
                                        )
                                    });
                                    let close_rect = egui::Rect::from_center_size(
                                        egui::pos2(rect.right() - 14.0, rect.center().y),
                                        egui::vec2(20.0, 20.0),
                                    );
                                    let close = ui.interact(
                                        close_rect,
                                        ui.id().with("close"),
                                        egui::Sense::click(),
                                    );
                                    close.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::WidgetType::Button,
                                            ui.is_enabled() && !editor_open,
                                            format!("关闭 {}", tab.name),
                                        )
                                    });
                                    paint_tab(
                                        ui,
                                        rect,
                                        selected,
                                        response.hovered() || close.hovered(),
                                    );
                                    if dragged.is_some()
                                        && !editor_open
                                        && response.contains_pointer()
                                    {
                                        let after = ui
                                            .input(|input| input.pointer.hover_pos())
                                            .is_some_and(|pointer| pointer.x > rect.center().x);
                                        ui.painter().vline(
                                            if after { rect.right() } else { rect.left() },
                                            rect.y_range(),
                                            egui::Stroke::new(2.0, accent(ui)),
                                        );
                                    }
                                    let icon =
                                        if tab.definition.is_some() || tab.management_document() {
                                            TabIcon::Objects
                                        } else if tab.design.is_some() {
                                            TabIcon::Design
                                        } else if tab.table_target.is_none() {
                                            TabIcon::Query
                                        } else if tab.table_kind.as_deref().is_some_and(|kind| {
                                            kind.to_ascii_uppercase().contains("VIEW")
                                        }) {
                                            TabIcon::View
                                        } else {
                                            TabIcon::Table
                                        };
                                    paint_icon(
                                        ui,
                                        egui::Rect::from_min_size(
                                            rect.min + egui::vec2(8.0, 8.0),
                                            egui::vec2(16.0, 16.0),
                                        ),
                                        icon,
                                        accent(ui),
                                    );
                                    let text_rect = egui::Rect::from_min_max(
                                        rect.min + egui::vec2(29.0, 0.0),
                                        egui::pos2(close_rect.left() - 2.0, rect.bottom()),
                                    );
                                    let mut job = egui::text::LayoutJob::simple(
                                        label,
                                        egui::FontId::proportional(13.0),
                                        ui.visuals().text_color(),
                                        text_rect.width(),
                                    );
                                    job.wrap.max_rows = 1;
                                    job.wrap.overflow_character = Some('…');
                                    let galley = ui.painter().layout_job(job);
                                    ui.painter().galley(
                                        egui::pos2(
                                            text_rect.left(),
                                            rect.center().y - galley.size().y * 0.5,
                                        ),
                                        galley,
                                        ui.visuals().text_color(),
                                    );
                                    if selected || response.hovered() || close.hovered() {
                                        let center = close_rect.center();
                                        let stroke =
                                            egui::Stroke::new(1.0, ui.visuals().weak_text_color());
                                        ui.painter().line_segment(
                                            [
                                                center - egui::vec2(3.0, 3.0),
                                                center + egui::vec2(3.0, 3.0),
                                            ],
                                            stroke,
                                        );
                                        ui.painter().line_segment(
                                            [
                                                center + egui::vec2(-3.0, 3.0),
                                                center + egui::vec2(3.0, -3.0),
                                            ],
                                            stroke,
                                        );
                                    }
                                    if selected && reveal {
                                        response.scroll_to_me(Some(egui::Align::Center));
                                    }
                                    let tooltip = if let Some(target) = &tab.table_target {
                                        format!(
                                            "{} / {}.{}",
                                            target.database, target.schema, target.name
                                        )
                                    } else {
                                        tab.target.as_ref().map_or_else(
                                            || tab.name.clone(),
                                            |target| {
                                                format!(
                                                    "{} / {}{}",
                                                    tab.name,
                                                    target.database.as_deref().unwrap_or("服务器"),
                                                    target
                                                        .schema
                                                        .as_ref()
                                                        .map_or(String::new(), |schema| format!(
                                                            " / {schema}"
                                                        ))
                                                )
                                            },
                                        )
                                    };
                                    response.clone().on_hover_text(format!(
                                        "{}\n{tooltip}\n右键管理标签 · 中键关闭",
                                        connection_name.as_deref().unwrap_or("尚未绑定连接")
                                    ));
                                    if !editor_open
                                        && (close.clicked()
                                            || close.clicked_by(egui::PointerButton::Middle)
                                            || response.clicked_by(egui::PointerButton::Middle))
                                    {
                                        action = Some(BarAction::Document(TabAction::Close(index)));
                                    } else if response.clicked() && !close.hovered() {
                                        action =
                                            Some(BarAction::Document(TabAction::Select(index)));
                                    }
                                    close.on_hover_text(if editor_open {
                                        "请先结束单元格编辑"
                                    } else {
                                        "关闭标签"
                                    });
                                    response.context_menu(|ui| {
                                        for (label, enabled, next) in [
                                            ("关闭标签页", !editor_open, TabAction::Close(index)),
                                            (
                                                "关闭其他标签页",
                                                !editor_open && tabs.len() > 1,
                                                TabAction::CloseOthers(index),
                                            ),
                                            (
                                                "关闭右侧标签页",
                                                !editor_open && index + 1 < tabs.len(),
                                                TabAction::CloseRight(index),
                                            ),
                                        ] {
                                            if ui
                                                .add_enabled(enabled, egui::Button::new(label))
                                                .clicked()
                                            {
                                                action = Some(BarAction::Document(next));
                                                ui.close();
                                            }
                                        }
                                    });
                                });
                            }
                        });
                    });
            },
        );
        if ui
            .add_sized([28.0, 28.0], egui::Button::new("+").frame(false))
            .on_hover_text("新建查询")
            .clicked()
        {
            action = Some(BarAction::Document(TabAction::New));
        }
        let menu_response = ui.add_sized([28.0, 28.0], egui::Button::new("").frame(false));
        egui::Popup::menu(&menu_response).show(|ui| {
            if ui.selectable_label(objects_active, "对象工作区").clicked() {
                action = Some(BarAction::Objects);
                ui.close();
            }
            ui.separator();
            for (index, tab) in tabs.iter().enumerate() {
                let location = tab
                    .target
                    .as_ref()
                    .map(|target| {
                        let connection = profiles
                            .iter()
                            .find(|profile| profile.id == target.connection)
                            .map(|profile| profile.name.as_str())
                            .unwrap_or("已删除的连接");
                        format!(
                            "{} · {connection} / {}",
                            tab.name,
                            target.database.as_deref().unwrap_or("服务器")
                        )
                    })
                    .unwrap_or_else(|| tab.name.clone());
                if ui
                    .selectable_label(!objects_active && index == active, location)
                    .clicked()
                {
                    action = Some(BarAction::Document(TabAction::Select(index)));
                    ui.close();
                }
            }
        });
        let center = menu_response.rect.center();
        ui.painter().line_segment(
            [
                center + egui::vec2(-3.0, -1.5),
                center + egui::vec2(0.0, 1.5),
            ],
            egui::Stroke::new(1.2, ui.visuals().text_color()),
        );
        ui.painter().line_segment(
            [
                center + egui::vec2(0.0, 1.5),
                center + egui::vec2(3.0, -1.5),
            ],
            egui::Stroke::new(1.2, ui.visuals().text_color()),
        );
        menu_response.on_hover_text("全部标签");
        let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
        let bounds = rect.shrink(6.0);
        ui.painter().rect_stroke(
            bounds,
            1.0,
            egui::Stroke::new(1.0, ui.visuals().text_color()),
            egui::StrokeKind::Inside,
        );
        let x = bounds.right() - 4.0;
        ui.painter().line_segment(
            [egui::pos2(x, bounds.top()), egui::pos2(x, bounds.bottom())],
            egui::Stroke::new(
                1.0,
                if information_open {
                    accent(ui)
                } else {
                    ui.visuals().text_color()
                },
            ),
        );
        if response
            .on_hover_text(if information_open {
                "收起对象信息"
            } else {
                "展开对象信息（窄窗口自动收起）"
            })
            .clicked()
        {
            action = Some(BarAction::Information);
        }
    });

    ui.data_mut(|data| {
        if let Some(id) = active_id
            && !matches!(&action, Some(BarAction::Document(TabAction::Select(_))))
        {
            data.insert_temp(egui::Id::new("visible-document-tab"), (id, bar.width()));
        } else {
            data.remove::<(Uuid, f32)>(egui::Id::new("visible-document-tab"));
        }
    });
    if released || editor_open {
        dragged = None;
    }
    ui.data_mut(|data| {
        if let Some(id) = dragged {
            data.insert_temp(drag_id, id);
        } else {
            data.remove::<Uuid>(drag_id);
        }
    });
    action
}
