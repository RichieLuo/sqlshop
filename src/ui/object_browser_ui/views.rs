use super::*;

pub(super) fn compact_style(ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
    ui.spacing_mut().button_padding = egui::vec2(7.0, 4.0);
    ui.spacing_mut().interact_size.y = 25.0;
    ui.visuals_mut().widgets.inactive.corner_radius = egui::CornerRadius::same(3);
    ui.visuals_mut().widgets.hovered.corner_radius = egui::CornerRadius::same(3);
    ui.visuals_mut().widgets.active.corner_radius = egui::CornerRadius::same(3);
    let color = accent(ui);
    ui.visuals_mut().selection.bg_fill =
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 32);
}

pub(super) fn category_controls(
    ui: &mut egui::Ui,
    data: &BrowserData<'_>,
    state: &mut BrowserState,
) {
    let Some(scope) = data.scope else {
        return;
    };
    let databases = scope.database.is_none();
    let last_category_id = ui.id().with("last-object-category");
    let category_changed = ui.ctx().data_mut(|data| {
        let previous = data.get_temp::<BrowserCategory>(last_category_id);
        data.insert_temp(last_category_id, state.category);
        previous != Some(state.category)
    });
    let rows: Vec<_> = catalog_rows(data)
        .into_iter()
        .filter(|entry| {
            scope
                .schema
                .as_deref()
                .is_none_or(|schema| schema == entry.schema())
        })
        .collect();
    let start = ui.available_rect_before_wrap().min;
    let width = ui.available_width();
    ui.painter().line_segment(
        [
            start + egui::vec2(0.0, 32.0),
            start + egui::vec2(width, 32.0),
        ],
        egui::Stroke::new(0.5, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );
    egui::ScrollArea::horizontal()
        .id_salt("object-category-scroll")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if databases {
                    ui.selectable_value(
                        &mut state.category,
                        BrowserCategory::All,
                        format!("数据库  {}", data.databases.len()),
                    );
                    let count = data
                        .queries
                        .iter()
                        .filter(|query| query_in_scope(query, Some(scope)))
                        .count();
                    ui.selectable_value(
                        &mut state.category,
                        BrowserCategory::Queries,
                        format!("查询  {count}"),
                    );
                } else {
                    for (category, name) in [
                        (BrowserCategory::All, "全部对象"),
                        (BrowserCategory::Tables, "表"),
                        (BrowserCategory::Views, "视图"),
                        (BrowserCategory::Functions, "函数"),
                        (BrowserCategory::Procedures, "过程"),
                        (BrowserCategory::Triggers, "触发器"),
                        (BrowserCategory::Sequences, "序列"),
                        (BrowserCategory::Queries, "查询"),
                    ] {
                        if category == BrowserCategory::Sequences
                            && data
                                .profile
                                .is_some_and(|profile| profile.engine == Engine::MySql)
                        {
                            continue;
                        }
                        let count = if category == BrowserCategory::Queries {
                            data.queries
                                .iter()
                                .filter(|query| query_in_scope(query, Some(scope)))
                                .count()
                        } else {
                            rows.iter()
                                .filter(|entry| {
                                    entry
                                        .kind()
                                        .is_some_and(|kind| category_matches(category, kind))
                                })
                                .count()
                        };
                        let unavailable = category != BrowserCategory::Queries
                            && data.catalog.is_some_and(|catalog| {
                                catalog
                                    .unavailable_kinds
                                    .iter()
                                    .any(|kind| category_matches(category, *kind))
                            });
                        let count_label = if unavailable {
                            "—".into()
                        } else {
                            count.to_string()
                        };
                        let label = format!("{name}  {count_label}");
                        let text_width = ui
                            .painter()
                            .layout_no_wrap(
                                label.clone(),
                                egui::FontId::proportional(13.0),
                                ui.visuals().text_color(),
                            )
                            .size()
                            .x;
                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(text_width + 34.0, 32.0),
                            egui::Sense::click(),
                        );
                        let selected = state.category == category;
                        if selected && category_changed {
                            response.scroll_to_me(Some(egui::Align::Center));
                        }
                        if selected || response.hovered() {
                            ui.painter().rect_filled(
                                rect.shrink2(egui::vec2(1.0, 1.0)),
                                2,
                                if selected {
                                    surface(ui)
                                } else {
                                    ui.visuals().widgets.hovered.bg_fill
                                },
                            );
                        }
                        let color = if selected {
                            accent(ui)
                        } else {
                            ui.visuals().text_color()
                        };
                        let icon = match category {
                            BrowserCategory::Tables => crate::ui::tab_bar_ui::TabIcon::Table,
                            BrowserCategory::Views => crate::ui::tab_bar_ui::TabIcon::View,
                            BrowserCategory::All => crate::ui::tab_bar_ui::TabIcon::Objects,
                            _ => crate::ui::tab_bar_ui::TabIcon::Query,
                        };
                        crate::ui::tab_bar_ui::paint_icon(
                            ui,
                            egui::Rect::from_min_size(
                                rect.min + egui::vec2(5.0, 8.0),
                                egui::vec2(16.0, 16.0),
                            ),
                            icon,
                            color,
                        );
                        ui.painter().text(
                            rect.min + egui::vec2(25.0, 16.0),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::FontId::proportional(13.0),
                            color,
                        );
                        if selected {
                            ui.painter().line_segment(
                                [
                                    rect.left_bottom() + egui::vec2(4.0, 0.0),
                                    rect.right_bottom() - egui::vec2(4.0, 0.0),
                                ],
                                egui::Stroke::new(2.0, color),
                            );
                        }
                        if response.clicked() {
                            state.category = category;
                        }
                    }
                }
            });
        });
    ui.horizontal(|ui| {
        let queries = state.category == BrowserCategory::Queries;
        let width = (ui.available_width()
            - if queries {
                120.0
            } else if databases {
                0.0
            } else {
                145.0
            })
        .max(80.0);
        ui.add(
            egui::TextEdit::singleline(&mut state.search)
                .hint_text(if queries {
                    "搜索查询名称 / SQL"
                } else if databases {
                    "搜索数据库"
                } else {
                    "搜索名称、Schema、注释或所有者"
                })
                .desired_width(width),
        );
        if queries {
            ui.checkbox(&mut state.all_queries, "全部连接")
                .on_hover_text("包括未绑定及连接已删除的查询");
        } else if !databases {
            ui.selectable_value(&mut state.view, BrowserView::Details, "列表");
            ui.selectable_value(&mut state.view, BrowserView::Icons, "图标");
            ui.menu_button("列", |ui| {
                ui.label(RichText::new("显示列").small().strong());
                let mut name_visible = true;
                ui.add_enabled(false, egui::Checkbox::new(&mut name_visible, "名称"));
                for (column, label) in object_column_labels(data).into_iter().enumerate().skip(1) {
                    let mut visible = !state.hidden_columns.contains(&column);
                    if ui.checkbox(&mut visible, label).changed() {
                        if visible {
                            state.hidden_columns.remove(&column);
                        } else {
                            state.hidden_columns.insert(column);
                            if state.sort_column == column {
                                state.sort_column = 0;
                            }
                        }
                    }
                }
                ui.separator();
                if ui.button("恢复默认列").clicked() {
                    state.hidden_columns.clear();
                    ui.close();
                }
            });
        }
    });
    ui.add_space(2.0);
}

pub(super) fn object_context_menu(
    ui: &mut egui::Ui,
    data: &BrowserData<'_>,
    item: &BrowserItem,
    action: &mut Option<BrowserAction>,
) {
    let caption = match item {
        BrowserItem::Database { .. } => "打开数据库",
        BrowserItem::Object { .. } => "打开数据",
        BrowserItem::CatalogObject { .. } => "查看定义",
    };
    if ui
        .add_enabled(data.item_ready(item), egui::Button::new(caption))
        .clicked()
    {
        *action = Some(open_action(item));
        ui.close();
    }
    if let Some(next) = structure_action(item)
        && ui
            .add_enabled(data.item_ready(item), egui::Button::new("打开结构"))
            .clicked()
    {
        *action = Some(next);
        ui.close();
    }
    if let BrowserItem::Object {
        connection,
        database,
        table,
    } = item
        && table.kind.eq_ignore_ascii_case("BASE TABLE")
        && ui
            .add_enabled(data.item_ready(item), egui::Button::new("重命名…"))
            .clicked()
    {
        *action = Some(BrowserAction::RenameTable {
            connection: *connection,
            database: database.clone(),
            table: table.clone(),
        });
        ui.close();
    }
    if ui.button("新建查询").clicked() {
        *action = query_action(data, Some(item));
        ui.close();
    }
    ui.separator();
    if ui.button("复制名称").clicked() {
        let name = match item {
            BrowserItem::Database { name, .. } => name,
            BrowserItem::Object { table, .. } => &table.name,
            BrowserItem::CatalogObject { object, .. } => &object.name,
        };
        ui.ctx().copy_text(name.clone());
        ui.close();
    }
    if !matches!(item, BrowserItem::Database { .. }) && ui.button("复制限定名称").clicked() {
        let (schema, name) = match item {
            BrowserItem::Object { table, .. } => (&table.schema, &table.name),
            BrowserItem::CatalogObject { object, .. } => (&object.schema, &object.name),
            _ => unreachable!(),
        };
        let engine = data.profile.map_or(Engine::MySql, |profile| profile.engine);
        let qualified = if schema.is_empty() {
            engine.quote(name)
        } else {
            format!("{}.{}", engine.quote(schema), engine.quote(name))
        };
        ui.ctx().copy_text(qualified);
        ui.close();
    }
}

pub(super) fn entry_icon(ui: &egui::Ui, rect: egui::Rect, entry: BrowserRow<'_>) {
    if matches!(
        entry.kind(),
        Some(
            ObjectKind::Function
                | ObjectKind::Procedure
                | ObjectKind::Trigger
                | ObjectKind::Sequence
        )
    ) {
        let symbol = match entry.kind() {
            Some(ObjectKind::Function) => "ƒ",
            Some(ObjectKind::Procedure) => "P",
            Some(ObjectKind::Trigger) => "T",
            _ => "#",
        };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            symbol,
            egui::FontId::monospace((rect.height() - 2.0).min(24.0)),
            accent(ui),
        );
    } else {
        paint_object_icon(
            ui,
            rect,
            matches!(entry, BrowserRow::Database(_)),
            matches!(
                entry.kind(),
                Some(ObjectKind::View | ObjectKind::MaterializedView)
            ),
        );
    }
}

pub(super) fn object_column_labels(data: &BrowserData<'_>) -> [&'static str; 10] {
    [
        "名称",
        "Schema",
        "类型",
        "估计行数",
        "目录大小",
        if data
            .profile
            .is_some_and(|profile| profile.engine == Engine::MySql)
        {
            "存储引擎"
        } else {
            "所有者"
        },
        "注释",
        "创建时间",
        "修改时间",
        "签名 / 所属对象",
    ]
}

pub(super) fn catalog_size_hint(data: &BrowserData<'_>) -> &'static str {
    match data.profile.map(|profile| profile.engine) {
        Some(Engine::Postgres) => {
            "服务器目录中的关系大小，仅包含关系本身，不包含索引或 TOAST；可能无法提供。"
        }
        Some(Engine::MySql) => "服务器目录中的数据与索引大小，为存储引擎提供的近似统计。",
        Some(Engine::SqlServer) => {
            "服务器目录中的行存储分配空间，包含索引，可能与实际磁盘文件大小不同。"
        }
        None => "服务器目录提供的大小统计，计算范围随数据库引擎而异。",
    }
}

pub(super) fn render_detail_view(
    ui: &mut egui::Ui,
    data: &BrowserData<'_>,
    state: &mut BrowserState,
    rows: &[BrowserRow<'_>],
    scope: &BrowserScope,
    action: &mut Option<BrowserAction>,
) {
    let databases = scope.database.is_none();
    let labels = if databases {
        vec!["名称", "Schema", "状态"]
    } else {
        object_column_labels(data).to_vec()
    };
    let columns: Vec<_> = (0..labels.len())
        .filter(|column| databases || !state.hidden_columns.contains(column))
        .collect();
    let widths: &[f32] = if databases {
        &[220.0, 125.0, 120.0]
    } else {
        &[
            220.0, 125.0, 100.0, 100.0, 105.0, 155.0, 240.0, 170.0, 170.0, 250.0,
        ]
    };
    let viewport_width = ui.available_width();
    let content_width = columns.iter().map(|column| widths[*column]).sum::<f32>()
        + ui.spacing().item_spacing.x * columns.len() as f32;
    let height = (ui.available_height() - 18.0).max(80.0);
    let column_mask = columns
        .iter()
        .fold(0_u16, |mask, column| mask | (1 << column));
    egui::Frame::new()
        .fill(chrome(ui.ctx()))
        .inner_margin(0)
        .show(ui, |ui| {
            egui::ScrollArea::horizontal()
                .id_salt(("object-detail-scroll", scope.connection, &scope.database))
                .scroll_source(egui::scroll_area::ScrollSource {
                    drag: false,
                    ..Default::default()
                })
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(content_width.max(viewport_width));
                    let mut table = TableBuilder::new(ui)
                        .drag_to_scroll(false)
                        .id_salt((
                            "object-browser",
                            scope.connection,
                            &scope.database,
                            &scope.schema,
                            column_mask,
                        ))
                        .striped(true)
                        .resizable(true)
                        .sense(egui::Sense::click())
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
                    for column in &columns {
                        table = table.column(Column::initial(widths[*column]).at_least(55.0));
                    }
                    table
                        .min_scrolled_height(80.0)
                        .max_scroll_height(height)
                        .header(25.0, |mut header| {
                            for &index in &columns {
                                let label = labels[index];
                                header.col(|ui| {
                                    let marker = if state.sort_column == index {
                                        if state.sort_descending {
                                            " ↓"
                                        } else {
                                            " ↑"
                                        }
                                    } else {
                                        ""
                                    };
                                    let header_response = ui.add(
                                        egui::Label::new(
                                            RichText::new(format!("{label}{marker}"))
                                                .small()
                                                .strong(),
                                        )
                                        .sense(egui::Sense::click()),
                                    );
                                    let header_response = if !databases && index == 3 {
                                        header_response
                                            .on_hover_text("服务器统计估计；— 表示未提供")
                                    } else if !databases && index == 4 {
                                        header_response.on_hover_text(catalog_size_hint(data))
                                    } else {
                                        header_response
                                    };
                                    if header_response.clicked() {
                                        if state.sort_column == index {
                                            state.sort_descending = !state.sort_descending;
                                        } else {
                                            state.sort_column = index;
                                            state.sort_descending = false;
                                        }
                                    }
                                });
                            }
                        })
                        .body(|body| {
                            body.rows(25.0, rows.len(), |mut row| {
                                let entry = rows[row.index()];
                                let item = entry.item(scope);
                                row.set_selected(state.selected.as_ref() == Some(&item));
                                let mut cell_response: Option<egui::Response> = None;
                                for &column in &columns {
                                    row.col(|ui| {
                                        if column == 0 {
                                            let (rect, _) = ui.allocate_exact_size(
                                                egui::vec2(17.0, 17.0),
                                                egui::Sense::hover(),
                                            );
                                            entry_icon(ui, rect, entry);
                                        }
                                        let value = row_value(entry, column, data);
                                        let response = ui
                                            .add(
                                                egui::Label::new(&value)
                                                    .truncate()
                                                    .sense(egui::Sense::click()),
                                            )
                                            .on_hover_text(&value);
                                        cell_response = Some(match cell_response.take() {
                                            Some(previous) => previous.union(response),
                                            None => response,
                                        });
                                    });
                                }
                                let response = cell_response.unwrap().union(row.response());
                                if response.clicked() || response.secondary_clicked() {
                                    state.selected = Some(item.clone());
                                }
                                if response.double_clicked() && data.item_ready(&item) {
                                    *action = Some(open_action(&item));
                                }
                                response.context_menu(|ui| {
                                    object_context_menu(ui, data, &item, action)
                                });
                            });
                        });
                });
        });
}

pub(super) fn render_icon_view(
    ui: &mut egui::Ui,
    data: &BrowserData<'_>,
    state: &mut BrowserState,
    rows: &[BrowserRow<'_>],
    scope: &BrowserScope,
    action: &mut Option<BrowserAction>,
) {
    let columns = (ui.available_width() / 145.0).floor().max(1.0) as usize;
    let row_count = rows.len().div_ceil(columns);
    egui::ScrollArea::vertical()
        .id_salt(("object-icons", scope.connection, &scope.database))
        .auto_shrink([false, false])
        .show_rows(ui, 96.0, row_count, |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    for entry in rows.iter().skip(row * columns).take(columns) {
                        let item = entry.item(scope);
                        let (rect, response) =
                            ui.allocate_exact_size(egui::vec2(138.0, 88.0), egui::Sense::click());
                        if state.selected.as_ref() == Some(&item) || response.hovered() {
                            ui.painter().rect_filled(
                                rect.shrink(1.0),
                                3,
                                if state.selected.as_ref() == Some(&item) {
                                    ui.visuals().selection.bg_fill
                                } else {
                                    ui.visuals().widgets.hovered.bg_fill
                                },
                            );
                        }
                        entry_icon(
                            ui,
                            egui::Rect::from_center_size(
                                rect.center_top() + egui::vec2(0.0, 25.0),
                                egui::vec2(28.0, 28.0),
                            ),
                            *entry,
                        );
                        let name = ui.painter().layout(
                            entry.name().into(),
                            egui::FontId::proportional(13.0),
                            ui.visuals().text_color(),
                            126.0,
                        );
                        let name_rect = egui::Rect::from_min_size(
                            egui::pos2(rect.center().x - name.size().x / 2.0, rect.top() + 44.0),
                            name.size(),
                        );
                        ui.painter()
                            .with_clip_rect(egui::Rect::from_min_max(
                                rect.min,
                                egui::pos2(rect.right(), rect.top() + 68.0),
                            ))
                            .galley(name_rect.min, name, ui.visuals().text_color());
                        ui.painter().text(
                            rect.center_bottom() - egui::vec2(0.0, 9.0),
                            egui::Align2::CENTER_CENTER,
                            entry.kind().map_or("数据库", |kind| kind.label()),
                            egui::FontId::proportional(11.0),
                            ui.visuals().weak_text_color(),
                        );
                        let response = response.on_hover_text(format!(
                            "{} / {}",
                            entry.schema(),
                            entry.name()
                        ));
                        if response.clicked() || response.secondary_clicked() {
                            state.selected = Some(item.clone());
                        }
                        if response.double_clicked() && data.item_ready(&item) {
                            *action = Some(open_action(&item));
                        }
                        response.context_menu(|ui| object_context_menu(ui, data, &item, action));
                    }
                });
            }
        });
}

pub(super) fn paint_object_icon(ui: &egui::Ui, rect: egui::Rect, database: bool, view: bool) {
    let color = if view {
        accent(ui)
    } else {
        ui.visuals().weak_text_color()
    };
    let stroke = egui::Stroke::new(1.0, color);
    let bounds = rect.shrink(2.0);
    ui.painter()
        .rect_stroke(bounds, 1.0, stroke, egui::StrokeKind::Inside);
    for part in [0.33, 0.66] {
        let y = bounds.top() + bounds.height() * part;
        ui.painter().line_segment(
            [egui::pos2(bounds.left(), y), egui::pos2(bounds.right(), y)],
            stroke,
        );
        if !database {
            let x = bounds.left() + bounds.width() * part;
            ui.painter().line_segment(
                [egui::pos2(x, bounds.top()), egui::pos2(x, bounds.bottom())],
                stroke,
            );
        }
    }
}
