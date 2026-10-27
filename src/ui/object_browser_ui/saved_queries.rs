use super::*;

pub(in crate::ui) fn query_in_scope(query: &SavedQuery, scope: Option<&BrowserScope>) -> bool {
    let Some(scope) = scope else { return true };
    query.target.as_ref().is_some_and(|target| {
        target.connection == scope.connection
            && scope
                .database
                .as_ref()
                .is_none_or(|database| target.database.as_ref() == Some(database))
            && scope
                .schema
                .as_ref()
                .is_none_or(|schema| target.schema.as_ref() == Some(schema))
    })
}

fn visible_queries<'a>(data: &BrowserData<'a>, state: &BrowserState) -> Vec<&'a SavedQuery> {
    let needle = state.search.trim().to_lowercase();
    let mut queries: Vec<_> = data
        .queries
        .iter()
        .filter(|query| {
            (state.all_queries || query_in_scope(query, data.scope))
                && (needle.is_empty()
                    || format!("{} {}", query.name, query.sql)
                        .to_lowercase()
                        .contains(&needle))
        })
        .collect();
    queries.sort_by_cached_key(|query| query.name.to_lowercase());
    queries
}

fn connection_name(data: &BrowserData<'_>, query: &SavedQuery) -> String {
    match &query.target {
        Some(target) => data
            .profiles
            .iter()
            .find(|profile| profile.id == target.connection)
            .map(|profile| profile.name.clone())
            .unwrap_or_else(|| "已删除的连接".into()),
        None => "未绑定".into(),
    }
}

pub(super) fn workspace(
    ui: &mut egui::Ui,
    data: &BrowserData<'_>,
    state: &mut BrowserState,
) -> Option<BrowserAction> {
    let rows = visible_queries(data, state);
    if state
        .selected_query
        .is_some_and(|id| !rows.iter().any(|query| query.id == id))
    {
        state.selected_query = None;
    }
    let selected = rows
        .iter()
        .find(|query| Some(query.id) == state.selected_query)
        .copied();
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        if ui.button("新建查询").clicked() {
            action = Some(BrowserAction::NewSavedQuery);
        }
        if ui
            .add_enabled(selected.is_some(), egui::Button::new("打开查询"))
            .clicked()
        {
            action = selected.map(|query| BrowserAction::OpenSavedQuery(query.id));
        }
        if ui
            .add_enabled(
                selected.is_some() && data.queries_writable,
                egui::Button::new("重命名…"),
            )
            .clicked()
        {
            action = selected.map(|query| BrowserAction::RenameSavedQuery(query.id));
        }
        if ui
            .add_enabled(
                selected.is_some() && data.queries_writable,
                egui::Button::new("删除…"),
            )
            .clicked()
        {
            action = selected.map(|query| BrowserAction::DeleteSavedQuery(query.id));
        }
    });
    ui.label(
        RichText::new(format!("{} 个查询", rows.len()))
            .small()
            .weak(),
    );
    if rows.is_empty() {
        ui.add_space(28.0);
        ui.vertical_centered(|ui| {
            ui.strong(if state.search.trim().is_empty() {
                "此位置还没有保存的查询"
            } else {
                "没有匹配的查询"
            });
            ui.label(RichText::new("在查询编辑器中保存后，将显示在所属连接和数据库中。").weak());
            if data.scope.is_some()
                && !state.all_queries
                && ui.button("查看全部连接的查询").clicked()
            {
                state.all_queries = true;
            }
        });
        return action;
    }
    egui::ScrollArea::horizontal()
        .id_salt("query-object-horizontal")
        .scroll_source(egui::scroll_area::ScrollSource {
            drag: false,
            ..Default::default()
        })
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width().max(560.0));
            TableBuilder::new(ui)
                .drag_to_scroll(false)
                .id_salt("query-object-list")
                .striped(false)
                .resizable(true)
                .sense(egui::Sense::click())
                .column(Column::remainder().at_least(160.0))
                .column(Column::initial(145.0).at_least(90.0))
                .column(Column::initial(140.0).at_least(90.0))
                .column(Column::initial(110.0).at_least(75.0))
                .header(27.0, |mut header| {
                    for label in ["名称", "连接", "数据库", "Schema"] {
                        header.col(|ui| {
                            ui.strong(label);
                        });
                    }
                })
                .body(|mut body| {
                    for query in rows {
                        body.row(25.0, |mut row| {
                            row.set_selected(state.selected_query == Some(query.id));
                            row.col(|ui| {
                                ui.horizontal(|ui| {
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(16.0, 16.0),
                                        egui::Sense::hover(),
                                    );
                                    crate::ui::tab_bar_ui::paint_icon(
                                        ui,
                                        rect,
                                        crate::ui::tab_bar_ui::TabIcon::Query,
                                        accent(ui),
                                    );
                                    ui.add(
                                        egui::Label::new(&query.name).truncate().selectable(false),
                                    );
                                });
                            });
                            row.col(|ui| {
                                ui.add(
                                    egui::Label::new(connection_name(data, query))
                                        .truncate()
                                        .selectable(false),
                                );
                            });
                            row.col(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        query
                                            .target
                                            .as_ref()
                                            .and_then(|t| t.database.as_deref())
                                            .unwrap_or("—"),
                                    )
                                    .truncate()
                                    .selectable(false),
                                );
                            });
                            row.col(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        query
                                            .target
                                            .as_ref()
                                            .and_then(|t| t.schema.as_deref())
                                            .unwrap_or("—"),
                                    )
                                    .truncate()
                                    .selectable(false),
                                );
                            });
                            let response = row.response();
                            if response.clicked() || response.secondary_clicked() {
                                state.selected_query = Some(query.id);
                            }
                            if response.double_clicked() {
                                action = Some(BrowserAction::OpenSavedQuery(query.id));
                            }
                            response
                                .on_hover_text(query.sql.chars().take(600).collect::<String>())
                                .context_menu(|ui| {
                                    if ui.button("打开查询").clicked() {
                                        action = Some(BrowserAction::OpenSavedQuery(query.id));
                                        ui.close();
                                    }
                                    ui.add_enabled_ui(data.queries_writable, |ui| {
                                        if ui.button("重命名…").clicked() {
                                            action =
                                                Some(BrowserAction::RenameSavedQuery(query.id));
                                            ui.close();
                                        }
                                        if ui.button("删除…").clicked() {
                                            action =
                                                Some(BrowserAction::DeleteSavedQuery(query.id));
                                            ui.close();
                                        }
                                    });
                                });
                        });
                    }
                });
        });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_scope_respects_connection_database_and_schema_and_keeps_unbound_accessible() {
        let connection = Uuid::new_v4();
        let mut scope = BrowserScope {
            connection,
            database: None,
            schema: None,
        };
        let query = SavedQuery {
            id: Uuid::new_v4(),
            name: "report".into(),
            sql: "SELECT 1".into(),
            target: Some(sqlshop::workspace::QueryTarget {
                connection,
                database: Some("sales".into()),
                schema: Some("public".into()),
            }),
        };
        assert!(query_in_scope(&query, Some(&scope)));
        scope.database = Some("sales".into());
        assert!(query_in_scope(&query, Some(&scope)));
        scope.schema = Some("public".into());
        assert!(query_in_scope(&query, Some(&scope)));
        scope.schema = Some("audit".into());
        assert!(!query_in_scope(&query, Some(&scope)));
        scope.schema = None;
        scope.database = Some("other".into());
        assert!(!query_in_scope(&query, Some(&scope)));
        scope.database = None;
        scope.connection = Uuid::new_v4();
        assert!(!query_in_scope(&query, Some(&scope)));
        let unbound = SavedQuery {
            target: None,
            ..query
        };
        assert!(!query_in_scope(&unbound, Some(&scope)));
        assert!(query_in_scope(&unbound, None));
    }
}
