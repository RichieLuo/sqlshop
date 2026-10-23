use super::*;

#[derive(Clone, Debug)]
pub(super) enum BrowseAction {
    Sort {
        column: String,
        descending: Option<bool>,
        additive: bool,
    },
    Filter {
        column: String,
        operator: FilterOperator,
        value: Option<String>,
    },
}

pub(super) fn header_controls(
    response: egui::Response,
    name: &str,
    browse: &TableBrowseOptions,
    enabled: bool,
) -> Option<BrowseAction> {
    let mut action = None;
    let response = if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    };
    if enabled && response.clicked() {
        let descending = match browse.sorts.iter().find(|sort| sort.column == name) {
            None => Some(false),
            Some(sort) if !sort.descending => Some(true),
            Some(_) => None,
        };
        action = Some(BrowseAction::Sort {
            column: name.into(),
            descending,
            additive: response.ctx.input(|input| input.modifiers.shift),
        });
    }
    response.context_menu(|ui| {
        ui.add_enabled_ui(enabled, |ui| {
            for (label, descending, additive) in [
                ("升序排序", Some(false), false),
                ("降序排序", Some(true), false),
                ("追加升序排序", Some(false), true),
                ("追加降序排序", Some(true), true),
                ("移除此列排序", None, true),
            ] {
                if ui.button(label).clicked() {
                    action = Some(BrowseAction::Sort {
                        column: name.into(),
                        descending,
                        additive,
                    });
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("筛选此列…").clicked() {
                action = Some(BrowseAction::Filter {
                    column: name.into(),
                    operator: FilterOperator::Eq,
                    value: Some(String::new()),
                });
                ui.close();
            }
        });
        if !enabled {
            ui.label("请先完成编辑并提交或撤销暂存修改；确认连接和数据有效后操作。");
        }
    });
    action
}

pub(super) fn cell_filter_controls(
    ui: &mut egui::Ui,
    name: &str,
    value: &Option<String>,
    text_column: bool,
    enabled: bool,
) -> Option<BrowseAction> {
    let mut action = None;
    ui.add_enabled_ui(enabled, |ui| {
        ui.menu_button("按此值筛选", |ui| {
            let choices = if value.is_none() {
                vec![
                    ("是 NULL", FilterOperator::IsNull),
                    ("不是 NULL", FilterOperator::IsNotNull),
                ]
            } else {
                let mut choices = vec![
                    ("等于此值", FilterOperator::Eq),
                    ("不等于此值", FilterOperator::NotEq),
                ];
                if text_column {
                    choices.push(("包含此文本", FilterOperator::Contains));
                }
                choices
            };
            for (label, operator) in choices {
                if ui.button(label).clicked() {
                    action = Some(BrowseAction::Filter {
                        column: name.into(),
                        operator,
                        value: value.clone(),
                    });
                    ui.close();
                }
            }
        }).response.on_hover_text("添加筛选条件，应用后重新读取数据");
    });
    action
}

impl App {
    pub(super) fn apply_grid_browse(&mut self, action: BrowseAction) {
        let index = self.active_tab;
        if !self.can_browse_grid(index) {
            self.fail("请先完成编辑并提交或撤销暂存修改，刷新有效数据后再筛选或排序".into());
            return;
        }
        let tab = &mut self.tabs[index];
        let name = match &action {
            BrowseAction::Sort { column, .. } | BrowseAction::Filter { column, .. } => column,
        };
        if tab.table_target.is_none()
            || !tab
                .table_data
                .as_ref()
                .is_some_and(|data| data.columns.iter().any(|column| column.name == *name))
        {
            return;
        }
        match action {
            BrowseAction::Sort {
                column,
                descending,
                additive,
            } => {
                if !additive {
                    tab.browse.sorts.clear();
                }
                if let Some(position) = tab
                    .browse
                    .sorts
                    .iter()
                    .position(|sort| sort.column == column)
                {
                    if let Some(descending) = descending {
                        tab.browse.sorts[position].descending = descending;
                    } else {
                        tab.browse.sorts.remove(position);
                    }
                } else if let Some(descending) = descending {
                    tab.browse.sorts.push(ColumnSort { column, descending });
                }
                tab.browse.offset = 0;
                tab.browse_draft.sync_sorts(&tab.browse.sorts);
                self.load_table(index);
            }
            BrowseAction::Filter {
                column,
                operator,
                value,
            } => {
                if tab.table_data.as_ref().is_some_and(|data| {
                    data.columns.iter().any(|field| {
                        field.name == column && field.kind == sqlshop::model::CellKind::Unsupported
                    })
                }) {
                    self.fail("此列类型暂不支持值筛选，请在筛选面板中选择 NULL 条件".into());
                    return;
                }
                let added = tab.browse_draft.add_filter(column, operator, value);
                tab.filters_open = true;
                if !added {
                    self.fail("筛选条件已达 100 个，请先删除不需要的条件".into());
                }
            }
        }
    }
}
