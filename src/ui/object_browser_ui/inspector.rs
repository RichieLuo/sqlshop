use super::*;

pub(super) fn property(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).small().weak());
    ui.add(egui::Label::new(value).wrap()).on_hover_text(value);
    ui.add_space(5.0);
}

pub(in crate::ui) fn inspector(
    ui: &mut egui::Ui,
    data: &InspectorData<'_>,
) -> Option<InspectorAction> {
    compact_style(ui);
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("对象信息").strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("×").on_hover_text("收起信息面板").clicked() {
                action = Some(InspectorAction::Close);
            }
        });
    });
    let object = matches!(data.selected, Some(BrowserItem::Object { .. }));
    let definition_object = matches!(
        data.selected,
        Some(BrowserItem::Object { .. } | BrowserItem::CatalogObject { .. })
    );
    let page_id = egui::Id::new("object-inspector-page");
    let mut page = ui
        .ctx()
        .data_mut(|state| state.get_temp::<InspectorPage>(page_id).unwrap_or_default());
    if (!object && page == InspectorPage::Fields)
        || (!definition_object && page == InspectorPage::Ddl)
        || (data.query_preview.is_none() && page == InspectorPage::Query)
    {
        page = InspectorPage::General;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        for (value, icon, caption, enabled) in [
            (InspectorPage::General, "常规", "常规", true),
            (InspectorPage::Fields, "字段", "字段", object),
            (InspectorPage::Ddl, "DDL", "定义 / DDL", definition_object),
            (
                InspectorPage::Query,
                "SQL",
                "查询预览",
                data.query_preview.is_some(),
            ),
        ] {
            if ui
                .add_enabled(
                    enabled,
                    egui::Button::selectable(page == value, icon).min_size(egui::vec2(36.0, 26.0)),
                )
                .on_hover_text(caption)
                .clicked()
            {
                page = value;
            }
        }
    });
    ui.ctx().data_mut(|state| state.insert_temp(page_id, page));
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt(("object-inspector", page as u8))
        .auto_shrink([false, false])
        .show(ui, |ui| match page {
            InspectorPage::General => inspector_general(ui, data, &mut action),
            InspectorPage::Fields => inspector_fields(ui, data),
            InspectorPage::Ddl => {
                ui.label(RichText::new("定义 / DDL").strong())
                    .on_hover_text(
                        if data.definition.is_none()
                            && data.structure.is_some()
                            && data
                                .browser
                                .profile
                                .is_some_and(|profile| profile.engine != Engine::MySql)
                        {
                            "由结构元数据生成的有限 DDL"
                        } else {
                            "对象定义"
                        },
                    );
                if let Some(definition) = data.definition {
                    inspector_sql(ui, definition, "复制定义");
                } else if let Some((structure, stale)) = data.structure {
                    inspector_cache_notice(ui, stale);
                    if structure.ddl.is_empty() {
                        ui.label(RichText::new("当前缓存没有 DDL。").weak());
                    } else {
                        inspector_sql(ui, &structure.ddl, "复制 DDL");
                    }
                } else if let Some(item @ BrowserItem::CatalogObject { .. }) = data.selected {
                    ui.label(RichText::new("尚未读取此对象的定义。").weak());
                    if ui
                        .add_enabled(data.browser.item_ready(item), egui::Button::new("读取定义"))
                        .clicked()
                    {
                        action = Some(inspector_browser_action(open_action(item)));
                    }
                } else {
                    ui.label(RichText::new("尚未读取 DDL").weak());
                }
            }
            InspectorPage::Query => {
                if let Some(preview) = &data.query_preview {
                    property(ui, "查询", preview.name);
                    ui.label(RichText::new("当前文稿").strong())
                        .on_hover_text("编辑器中的 SQL，包含未执行的改动");
                    if preview.sql.is_empty() {
                        ui.label(RichText::new("文稿为空。").weak());
                    } else {
                        inspector_sql(ui, preview.sql, "复制当前文稿");
                    }
                    ui.separator();
                    ui.label(RichText::new("最近执行").strong())
                        .on_hover_text("最近发送给服务器的 SQL，可能是选区或当前语句");
                    if let Some(sql) = preview.executed_sql {
                        inspector_sql(ui, sql, "复制最近执行 SQL");
                    } else {
                        ui.label(RichText::new("此文档尚无执行记录。").weak());
                    }
                }
            }
        });
    action
}

pub(super) fn inspector_cache_notice(ui: &mut egui::Ui, stale: bool) {
    if stale {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "缓存已过期，请在源文档刷新后核对。",
        );
    }
}

pub(super) fn inspector_sql(ui: &mut egui::Ui, sql: &str, copy_caption: &str) {
    if ui.button(copy_caption).clicked() {
        ui.ctx().copy_text(sql.to_owned());
    }
    ui.add(
        egui::Label::new(RichText::new(sql).monospace())
            .selectable(true)
            .wrap(),
    );
}

pub(super) fn inspector_general(
    ui: &mut egui::Ui,
    data: &InspectorData<'_>,
    action: &mut Option<InspectorAction>,
) {
    let Some(profile) = data.browser.profile else {
        ui.label(RichText::new("选择连接或数据库对象查看信息").weak());
        return;
    };
    property(ui, "连接", &profile.name);
    property(ui, "数据库引擎", profile.engine.label());
    property(ui, "服务器", &format!("{}:{}", profile.host, profile.port));
    property(
        ui,
        "状态",
        if data.browser.busy {
            "正在操作"
        } else if data.browser.connected {
            "已连接"
        } else {
            "未连接"
        },
    );
    let database = match data.selected {
        Some(BrowserItem::Database { name, .. }) => Some(name.as_str()),
        Some(
            BrowserItem::Object { database, .. } | BrowserItem::CatalogObject { database, .. },
        ) => Some(database.as_str()),
        None => data
            .browser
            .scope
            .and_then(|scope| scope.database.as_deref()),
    };
    if let Some(database) = database {
        property(ui, "数据库", database);
    }
    if let Some(BrowserItem::Object { table, .. }) = data.selected {
        ui.separator();
        property(ui, "名称", &table.name);
        property(ui, "Schema", &table.schema);
        property(ui, "类型", object_kind(table));
        if let Some(object) = data.browser.catalog.and_then(|catalog| {
            catalog.objects.iter().find(|object| {
                object.schema == table.schema
                    && object.name == table.name
                    && matches!(
                        object.kind,
                        ObjectKind::Table | ObjectKind::View | ObjectKind::MaterializedView
                    )
            })
        }) {
            inspector_catalog_properties(ui, object, data.browser);
        }
        ui.horizontal_wrapped(|ui| {
            let ready = data.browser.item_ready(data.selected.unwrap());
            if ui
                .add_enabled(ready, egui::Button::new("打开数据"))
                .clicked()
            {
                *action = Some(inspector_browser_action(open_action(
                    data.selected.unwrap(),
                )));
            }
            if ui.add_enabled(ready, egui::Button::new("结构")).clicked()
                && let Some(next) = structure_action(data.selected.unwrap())
            {
                *action = Some(inspector_browser_action(next));
            }
        });
        if let Some((structure, stale)) = data.structure {
            property(ui, "字段数", &structure.columns.rows.len().to_string());
            property(ui, "索引明细", &structure.indexes.rows.len().to_string());
            property(
                ui,
                "外键明细",
                &structure.foreign_keys.rows.len().to_string(),
            );
            property(ui, "结构状态", if stale { "需要刷新" } else { "已读取" });
        }
        if let Some(metadata) = &data.metadata {
            property(ui, "已加载字段", &metadata.columns.len().to_string());
            property(
                ui,
                "主键",
                &if metadata.primary_keys.is_empty() {
                    "无主键".into()
                } else {
                    metadata.primary_keys.join(", ")
                },
            );
            property(
                ui,
                "表数据编辑",
                if metadata.stale {
                    "快照已过期，请刷新后编辑"
                } else if !metadata.writable {
                    "此对象不支持事务编辑"
                } else if metadata.primary_keys.is_empty() {
                    "无主键，表数据只读"
                } else {
                    "可暂存修改后提交"
                },
            );
        } else if data.structure.is_none() {
            ui.label(RichText::new("尚未读取结构").small().weak());
        }
    } else if let Some(item @ BrowserItem::CatalogObject { object, .. }) = data.selected {
        ui.separator();
        property(ui, "名称", &object.name);
        property(ui, "Schema", &object.schema);
        property(ui, "类型", object.kind.label());
        inspector_catalog_properties(ui, object, data.browser);
        if ui
            .add_enabled(data.browser.item_ready(item), egui::Button::new("查看定义"))
            .clicked()
        {
            *action = Some(inspector_browser_action(open_action(item)));
        }
    } else if let Some(scope) = data.browser.scope {
        if let Some(schema) = &scope.schema {
            property(ui, "Schema", schema);
        }
        if scope.database.as_deref() == data.browser.active_database && scope.database.is_some() {
            property(
                ui,
                "目录中的对象",
                &catalog_rows(data.browser)
                    .into_iter()
                    .filter(|entry| {
                        scope
                            .schema
                            .as_deref()
                            .is_none_or(|schema| schema == entry.schema())
                    })
                    .count()
                    .to_string(),
            );
        }
    }
}

pub(super) fn inspector_catalog_properties(
    ui: &mut egui::Ui,
    object: &CatalogObject,
    browser: &BrowserData<'_>,
) {
    if let Some(owner) = &object.owner {
        property(ui, "所有者", owner);
    }
    if let Some(engine) = &object.engine {
        property(ui, "存储引擎", engine);
    }
    if let Some(rows) = object.estimated_rows {
        ui.scope(|ui| property(ui, "估计行数", &rows.to_string()))
            .response
            .on_hover_text("来自服务器统计信息，可能与实际记录数不同");
    }
    if let Some(size) = object.size_bytes {
        ui.scope(|ui| property(ui, "目录大小", &format_bytes(size)))
            .response
            .on_hover_text(catalog_size_hint(browser));
    }
    if let Some(created) = &object.created_at {
        property(ui, "创建时间", created);
    }
    if let Some(updated) = &object.updated_at {
        property(ui, "修改时间", updated);
    }
    if let Some(signature) = &object.signature {
        property(ui, "参数 / 签名", signature);
    }
    if let Some(parent) = &object.parent {
        property(ui, "所属对象", parent);
    }
    if let Some(comment) = &object.comment {
        property(
            ui,
            "注释",
            if comment.is_empty() {
                "（无注释）"
            } else {
                comment
            },
        );
    }
}

pub(super) fn inspector_fields(ui: &mut egui::Ui, data: &InspectorData<'_>) {
    ui.label(RichText::new("字段").strong());
    if let Some((structure, stale)) = data.structure {
        inspector_cache_notice(ui, stale);
        let name_index = structure
            .columns
            .columns
            .iter()
            .position(|name| name == "name");
        let type_index = structure
            .columns
            .columns
            .iter()
            .position(|name| name == "type");
        if structure.columns.rows.is_empty() {
            ui.label(RichText::new("当前结构缓存没有字段。").weak());
        }
        for (index, row) in structure.columns.rows.iter().enumerate() {
            let name = name_index
                .and_then(|index| row.get(index).and_then(Option::as_deref))
                .unwrap_or("未命名字段");
            let data_type = type_index
                .and_then(|index| row.get(index).and_then(Option::as_deref))
                .unwrap_or("未知类型");
            egui::CollapsingHeader::new(format!("{name} · {data_type}"))
                .id_salt(("inspector-field", index, name))
                .default_open(data.focused_column == Some(name))
                .show(ui, |ui| {
                    for (column, value) in structure.columns.columns.iter().zip(row) {
                        let value = match value {
                            None => "NULL",
                            Some(value) if value.is_empty() => "（空字符串）",
                            Some(value) => value,
                        };
                        property(ui, field_property_label(column), value);
                    }
                });
        }
    } else if let Some(metadata) = &data.metadata {
        inspector_cache_notice(ui, metadata.stale);
        if metadata.columns.is_empty() {
            ui.label(RichText::new("当前数据缓存没有字段。").weak());
        }
        for (index, column) in metadata.columns.iter().enumerate() {
            egui::CollapsingHeader::new(format!("{} · {}", column.name, column.declared_type))
                .id_salt(("inspector-field", index, &column.name))
                .default_open(data.focused_column == Some(column.name.as_str()))
                .show(ui, |ui| {
                    property(ui, "名称", &column.name);
                    property(ui, "类型", &column.declared_type);
                    property(ui, "主键", if column.primary_key { "是" } else { "否" });
                    property(ui, "可编辑", if column.editable { "是" } else { "否" });
                });
        }
    } else {
        ui.label(RichText::new("尚未读取字段信息").weak());
    }
}

pub(super) fn field_property_label(name: &str) -> &str {
    match name {
        "ordinal" => "序号",
        "name" => "名称",
        "type" => "类型",
        "nullable" => "允许 NULL",
        "default" => "默认值",
        "default_or_generation" => "默认值 / 生成表达式",
        "comment" => "注释",
        "extra" => "额外属性",
        "key" => "键",
        "character_set" => "字符集",
        "collation" => "排序规则",
        "identity" => "标识列",
        "generated" | "generated_always" => "生成属性",
        "generation_expression" => "生成表达式",
        "computed" => "计算表达式",
        "default_name" => "默认约束",
        "identity_seed" => "标识初值",
        "identity_increment" => "标识增量",
        "persisted" => "持久化",
        "rowguid" => "行 GUID",
        "sparse" => "稀疏列",
        "hidden" => "隐藏列",
        "privileges" => "权限",
        "storage" => "存储",
        "statistics_target" => "统计目标",
        _ => name,
    }
}

pub(super) fn inspector_browser_action(action: BrowserAction) -> InspectorAction {
    InspectorAction::Browser(Box::new(action))
}
