use super::*;

impl App {
    pub(super) fn workspace_shortcuts(&mut self, ctx: &egui::Context) {
        if self.modal_open() || egui::Popup::is_any_open(ctx) {
            return;
        }
        let (navigation, information, previous, next) = ctx.input_mut(|input| {
            let shifted = egui::Modifiers {
                shift: true,
                ..egui::Modifiers::COMMAND
            };
            (
                input.consume_key(egui::Modifiers::COMMAND, egui::Key::Num1),
                input.consume_key(egui::Modifiers::COMMAND, egui::Key::Num2),
                input.consume_key(shifted, egui::Key::OpenBracket)
                    || input.consume_key(
                        egui::Modifiers {
                            ctrl: true,
                            shift: true,
                            ..Default::default()
                        },
                        egui::Key::Tab,
                    ),
                input.consume_key(shifted, egui::Key::CloseBracket)
                    || input.consume_key(egui::Modifiers::CTRL, egui::Key::Tab),
            )
        });
        if navigation {
            self.view.navigation = self.view.focus || !self.view.navigation;
            self.view.focus = false;
        }
        if information {
            let visible = self.inspector_visible
                && !self.view.focus
                && (self.view.information_force || ctx.content_rect().width() >= 1100.0);
            self.inspector_visible = !visible;
            self.view.information_force = self.inspector_visible;
            self.view.focus = false;
        }
        if previous || next {
            if self.cell_editor.is_some() {
                self.finish_inline_cell(false, CellMovement::Stay);
                if self.cell_editor.is_some() {
                    return;
                }
            }
            let position = if self.browser_visible {
                0
            } else {
                self.active_tab + 1
            };
            let count = self.tabs.len() + 1;
            let position = if previous {
                (position + count - 1) % count
            } else {
                (position + 1) % count
            };
            if position == 0 {
                self.show_selected_objects();
            } else {
                self.change_tab(TabAction::Select(position - 1));
            }
        }
    }

    pub(super) fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.visuals_mut().indent_has_left_vline = false;
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        ui.add_space(4.0);
        self.saved_queries_sidebar(ui);
        ui.separator();
        let search_width =
            (ui.available_width() - 56.0 - 2.0 * ui.spacing().item_spacing.x).max(60.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("搜索数据库、数据表…")
                    .desired_width(search_width)
                    .margin(egui::vec2(4.0, 2.0)),
            );
            let (filter_rect, filter_response) =
                ui.allocate_exact_size(egui::vec2(24.0, 20.0), egui::Sense::click());
            let color = if self.view.connected_only {
                accent(ui)
            } else {
                ui.visuals().weak_text_color()
            };
            if self.view.connected_only {
                ui.painter()
                    .rect_filled(filter_rect, 3.0, accent(ui).gamma_multiply(0.14));
            }
            let center = filter_rect.center();
            ui.painter()
                .circle_stroke(center, 5.0, egui::Stroke::new(1.3, color));
            ui.painter().line_segment(
                [
                    center + egui::vec2(0.0, -8.0),
                    center + egui::vec2(0.0, -2.0),
                ],
                egui::Stroke::new(1.3, color),
            );
            if filter_response
                .on_hover_text("仅显示已连接的服务器")
                .clicked()
            {
                self.view.connected_only = !self.view.connected_only;
            }
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(24.0, 20.0), egui::Sense::click());
            let visuals = ui.style().interact(&response);
            ui.painter().rect(
                rect,
                3.0,
                visuals.weak_bg_fill,
                visuals.bg_stroke,
                egui::StrokeKind::Inside,
            );
            let color = visuals.fg_stroke;
            for y in [8.0, 12.0] {
                let center = rect.left_top() + egui::vec2(12.0, y);
                ui.painter()
                    .line_segment([center + egui::vec2(-4.0, 2.0), center], color);
                ui.painter()
                    .line_segment([center, center + egui::vec2(4.0, 2.0)], color);
            }
            if response.on_hover_text("全部折叠").clicked() {
                self.filter.clear();
                self.expand_connection = None;
                self.expand_database = None;
                self.view.tree_generation = self.view.tree_generation.wrapping_add(1);
            }
        });
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        let tree_before = self.tree_selection.selected.clone();
        let mut tree_activation = None;
        tree_navigation_ui::begin(ui, &mut self.tree_selection);
        let filter = self.filter.to_lowercase();
        let searching = !filter.is_empty();
        let expand_search = self.tree_selection.navigator.search_changed(&filter);
        let connected_only = self.view.connected_only;
        let mut to_connect = None;
        let mut to_edit = None;
        let mut to_delete = None;
        let mut disconnect = None;
        let mut refresh_databases = None;
        let mut refresh_tables = None;
        let mut to_select = None;
        let mut preview = None;
        let mut schema_dialog = None;
        let mut table_create = None;
        let mut move_connection = None;
        let mut group_dialog = None;
        let mut buckets: Vec<_> = self
            .catalog
            .groups
            .iter()
            .map(|g| (Some(g.id), g.name.clone()))
            .collect();
        buckets.push((None, "未分组".into()));
        let tree_generation = self.view.tree_generation;
        let mut tree_render = table_tree_ui::TreeRender {
            generation: tree_generation,
            fixture: self.screenshot_path.is_some()
                && matches!(
                    std::env::var("SQLSHOP_SCREENSHOT_VIEW").as_deref(),
                    Ok("tree-details" | "tree-document")
                ),
            ..Default::default()
        };
        let expand_connection = self.expand_connection.take();
        let expand_database = self.expand_database.take();
        egui::ScrollArea::vertical()
            .id_salt("connection-tree")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
                ui.spacing_mut().button_padding = egui::vec2(4.0, 2.0);
                ui.spacing_mut().interact_size.y = 22.0;
                let mut visible_connections = 0usize;
                for (group_id, group_name) in &buckets {
                    let group_matches = searching && group_name.to_lowercase().contains(&filter);
                    let members = self.connection_tree_members(
                        *group_id,
                        &filter,
                        group_matches,
                        connected_only,
                    );
                    visible_connections += members.len();
                    if members.is_empty() && (searching || connected_only) {
                        continue;
                    }
                    let expand_group =
                        expand_connection.is_some_and(|id| members.iter().any(|p| p.id() == id));
                    let group_state_id =
                        ui.make_persistent_id(("connection-group", group_id, tree_generation));
                    let group_header = tree_navigation_ui::branch(
                        ui,
                        &mut self.tree_selection,
                        TreeNode::Group(*group_id),
                        group_state_id,
                        format!("{} ({})", group_name, members.len()).into(),
                        tree_generation == 0,
                        if expand_group || expand_search {
                            Some(true)
                        } else {
                            None
                        },
                        true,
                    );
                    let group_response = group_header.response.clone();
                    group_header.body(|ui| {
                        for member in members {
                            let profile = match member {
                                TreeConnection::Sql(profile) => profile,
                                TreeConnection::Redis(profile) => {
                                    let events = self.redis_connection_tree(
                                        ui,
                                        &profile,
                                        expand_connection == Some(profile.id) || expand_search,
                                        &buckets,
                                    );
                                    if events.connect.is_some() {
                                        to_connect = events.connect;
                                    }
                                    if events.disconnect.is_some() {
                                        disconnect = events.disconnect;
                                    }
                                    if events.edit.is_some() {
                                        to_edit = events.edit;
                                    }
                                    if events.delete.is_some() {
                                        to_delete = events.delete;
                                    }
                                    if events.select.is_some() {
                                        to_select = events.select;
                                    }
                                    if events.move_group.is_some() {
                                        move_connection = events.move_group;
                                    }
                                    continue;
                                }
                            };
                            let filter = if group_matches
                                || (searching && profile.name.to_lowercase().contains(&filter))
                            {
                                String::new()
                            } else {
                                filter.clone()
                            };
                            let session = self.sessions.get(&profile.id);
                            let connected = session.is_some_and(|s| s.connected.is_some());
                            let busy = session.is_some_and(|s| s.busy);
                            let databases =
                                session.map(|s| s.databases.as_slice()).unwrap_or_default();
                            let active_database = session.and_then(|s| s.active_database.as_ref());
                            let catalog_tables =
                                session.map(|s| s.tables.as_slice()).unwrap_or_default();
                            let empty_entries = HashMap::new();
                            let tree_entries = session
                                .map(|s| &s.tree_metadata.entries)
                                .unwrap_or(&empty_entries);
                            let mut title = egui::text::LayoutJob::default();
                            title.append(
                                &profile.name,
                                28.0,
                                egui::TextFormat::simple(
                                    egui::TextStyle::Button.resolve(ui.style()),
                                    ui.visuals().text_color(),
                                ),
                            );
                            let server_state_id =
                                ui.make_persistent_id(("server", profile.id, tree_generation));
                            let server_header = tree_navigation_ui::branch(
                                ui,
                                &mut self.tree_selection,
                                TreeNode::Connection(profile.id),
                                server_state_id,
                                title.into(),
                                false,
                                if expand_connection == Some(profile.id) || expand_search {
                                    Some(true)
                                } else {
                                    None
                                },
                                true,
                            );
                            let header_response = server_header.response.clone();
                            if (server_header.opened || server_header.activated)
                                && !busy
                                && !connected
                            {
                                to_connect = Some(profile.id);
                            }
                            if server_header.activated && (!connected || server_header.is_open()) {
                                tree_activation = Some(TreeNode::Connection(profile.id));
                            }
                            server_header.body(|ui| {
                                if !connected {
                                    return;
                                }
                                if databases.is_empty() {
                                    ui.label(
                                        RichText::new(if busy {
                                            "正在读取数据库列表…"
                                        } else {
                                            "未发现可访问的数据库"
                                        })
                                        .weak(),
                                    );
                                }
                                for database in databases {
                                    let active = active_database == Some(database);
                                    let protected_database = match profile.engine {
                                        Engine::MySql => [
                                            "mysql",
                                            "information_schema",
                                            "performance_schema",
                                            "sys",
                                        ]
                                        .iter()
                                        .any(|name| database.eq_ignore_ascii_case(name)),
                                        Engine::Postgres => ["postgres", "template0", "template1"]
                                            .iter()
                                            .any(|name| database.eq_ignore_ascii_case(name)),
                                        Engine::SqlServer => ["master", "model", "msdb", "tempdb"]
                                            .iter()
                                            .any(|name| database.eq_ignore_ascii_case(name)),
                                    };
                                    let matches_database =
                                        database.to_lowercase().contains(&filter);
                                    if !filter.is_empty() && !matches_database && !active {
                                        continue;
                                    }
                                    let title =
                                        RichText::new(database).color(ui.visuals().text_color());
                                    let database_state_id = ui.make_persistent_id((
                                        "database",
                                        profile.id,
                                        database,
                                        tree_generation,
                                    ));
                                    let database_header = tree_navigation_ui::branch(
                                        ui,
                                        &mut self.tree_selection,
                                        TreeNode::Database(profile.id, database.clone()),
                                        database_state_id,
                                        title.into(),
                                        false,
                                        if expand_database.as_ref()
                                            == Some(&(profile.id, database.clone()))
                                            || expand_search
                                        {
                                            Some(true)
                                        } else {
                                            None
                                        },
                                        true,
                                    );
                                    let database_response = database_header.response.clone();
                                    if (database_header.opened || database_header.activated)
                                        && !busy
                                        && !active
                                    {
                                        to_select = Some((profile.id, database.clone()));
                                    }
                                    if database_header.activated
                                        && (!active || database_header.is_open())
                                    {
                                        tree_activation =
                                            Some(TreeNode::Database(profile.id, database.clone()));
                                    }
                                    database_header.body(|ui| {
                                        if !active {
                                            ui.label(
                                                RichText::new(if busy {
                                                    "当前连接忙，请稍后打开数据库"
                                                } else {
                                                    "展开或双击打开数据库"
                                                })
                                                .small()
                                                .weak(),
                                            );
                                            if ui
                                                .add_enabled(!busy, egui::Button::new("打开数据库"))
                                                .clicked()
                                            {
                                                to_select = Some((profile.id, database.clone()));
                                            }
                                            return;
                                        }
                                        let tables: Vec<_> = catalog_tables
                                            .iter()
                                            .filter(|table| {
                                                matches_database
                                                    || format!("{}.{}", table.schema, table.name)
                                                        .to_lowercase()
                                                        .contains(&filter)
                                            })
                                            .collect();
                                        if profile.engine == Engine::MySql {
                                            if let Some(table) = table_tree_ui::tables(
                                                ui,
                                                &tables,
                                                &mut self.tree_selection,
                                                profile.id,
                                                database,
                                                busy,
                                                &mut schema_dialog,
                                                &mut tree_render,
                                                tree_entries,
                                            ) {
                                                preview = Some((
                                                    profile.id,
                                                    database.clone(),
                                                    profile.engine,
                                                    table,
                                                ));
                                            }
                                        } else {
                                            let mut schemas: Vec<_> =
                                                tables.iter().map(|table| &table.schema).collect();
                                            schemas.sort();
                                            schemas.dedup();
                                            for schema in schemas {
                                                let schema_state_id = ui.make_persistent_id((
                                                    profile.id,
                                                    database,
                                                    schema,
                                                    tree_generation,
                                                ));
                                                let schema_header = tree_navigation_ui::branch(
                                                    ui,
                                                    &mut self.tree_selection,
                                                    TreeNode::Schema(
                                                        profile.id,
                                                        database.clone(),
                                                        schema.to_string(),
                                                    ),
                                                    schema_state_id,
                                                    egui::WidgetText::from(schema.as_str()),
                                                    true,
                                                    expand_search.then_some(true),
                                                    true,
                                                );
                                                let schema_response =
                                                    schema_header.response.clone();
                                                if schema_header.activated
                                                    && schema_header.is_open()
                                                {
                                                    tree_activation = Some(TreeNode::Schema(
                                                        profile.id,
                                                        database.clone(),
                                                        schema.to_string(),
                                                    ));
                                                }
                                                schema_header.body(|ui| {
                                                    let schema_tables: Vec<_> = tables
                                                        .iter()
                                                        .copied()
                                                        .filter(|table| &table.schema == schema)
                                                        .collect();
                                                    if let Some(table) = table_tree_ui::tables(
                                                        ui,
                                                        &schema_tables,
                                                        &mut self.tree_selection,
                                                        profile.id,
                                                        database,
                                                        busy,
                                                        &mut schema_dialog,
                                                        &mut tree_render,
                                                        tree_entries,
                                                    ) {
                                                        preview = Some((
                                                            profile.id,
                                                            database.clone(),
                                                            profile.engine,
                                                            table,
                                                        ));
                                                    }
                                                });
                                                schema_response.context_menu(|ui| {
                                                    if ui
                                                        .add_enabled(
                                                            !busy && active,
                                                            egui::Button::new(
                                                                "在此 Schema 新建数据表…",
                                                            ),
                                                        )
                                                        .clicked()
                                                    {
                                                        table_create = Some(new_table_create_form(
                                                            profile.id,
                                                            database.clone(),
                                                            schema.to_string(),
                                                            profile.engine,
                                                        ));
                                                        ui.close();
                                                    }
                                                });
                                            }
                                        }
                                        if catalog_tables.is_empty() {
                                            ui.label(
                                                RichText::new(if busy {
                                                    "正在读取表目录…"
                                                } else {
                                                    "没有表或视图"
                                                })
                                                .weak(),
                                            );
                                        } else if tables.is_empty() {
                                            ui.label(RichText::new("没有匹配的表或视图").weak());
                                        }
                                    });
                                    database_response.context_menu(|ui| {
                                        database_context_menu(
                                            ui,
                                            profile.id,
                                            profile.engine,
                                            database,
                                            active,
                                            busy,
                                            protected_database,
                                            &mut schema_dialog,
                                            &mut table_create,
                                            &mut to_select,
                                            &mut refresh_tables,
                                        );
                                    });
                                }
                            });
                            icons::paint_connection(
                                ui,
                                egui::Rect::from_center_size(
                                    egui::pos2(
                                        header_response.rect.left() + 10.0,
                                        header_response.rect.center().y,
                                    ),
                                    egui::vec2(20.0, 20.0),
                                ),
                                profile.engine,
                                connected,
                            );
                            header_response.clone().on_hover_text(format!(
                                "{} · {}:{}{} · {}",
                                profile.engine.label(),
                                profile.host,
                                profile.port,
                                if profile.ssh.enabled { " · SSH" } else { "" },
                                if busy {
                                    "正在操作…"
                                } else if connected {
                                    "已连接"
                                } else {
                                    "未连接"
                                },
                            ));
                            header_response.context_menu(|ui| {
                                connection_context_menu(
                                    ui,
                                    profile.id,
                                    connected,
                                    busy,
                                    self.catalog_store.is_some(),
                                    self.catalog.memberships.get(&profile.id).copied(),
                                    &buckets,
                                    &mut schema_dialog,
                                    &mut disconnect,
                                    &mut refresh_databases,
                                    &mut to_connect,
                                    &mut move_connection,
                                    &mut to_edit,
                                    &mut to_delete,
                                );
                            });
                        }
                        if self
                            .profiles
                            .iter()
                            .map(|p| p.id)
                            .chain(self.redis_profiles.iter().map(|p| p.id))
                            .all(|id| self.catalog.memberships.get(&id).copied() != *group_id)
                        {
                            ui.label(RichText::new("空分组").small().weak());
                        }
                    });
                    if let Some(id) = group_id {
                        group_response.context_menu(|ui| {
                            ui.add_enabled_ui(self.catalog_store.is_some(), |ui| {
                                if ui.button("重命名分组…").clicked() {
                                    group_dialog = Some(CatalogDialog::Group {
                                        id: Some(*id),
                                        name: group_name.clone(),
                                    });
                                    ui.close();
                                }
                                if ui.button("删除分组…").clicked() {
                                    group_dialog =
                                        Some(CatalogDialog::DeleteGroup(*id, group_name.clone()));
                                    ui.close();
                                }
                            });
                        });
                    }
                }
                if visible_connections == 0
                    && (!self.profiles.is_empty() || !self.redis_profiles.is_empty())
                {
                    ui.label(
                        RichText::new(if searching {
                            "没有匹配的连接或已读对象"
                        } else {
                            "当前没有已连接的服务器"
                        })
                        .weak(),
                    );
                }
                self.tree_selection.paint(ui);
                if let Some(node) = &self.tree_selection.selected {
                    let connection = match node {
                        TreeNode::Group(_) => None,
                        TreeNode::Connection(id)
                        | TreeNode::Database(id, _)
                        | TreeNode::Schema(id, _, _)
                        | TreeNode::Table(id, _, _)
                        | TreeNode::TablePart(id, _, _, _, _) => Some(*id),
                    };
                    if let Some(connection) = connection {
                        self.selected = Some(connection);
                    }
                }
                if self.profiles.is_empty() && self.redis_profiles.is_empty() {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("⌁").size(38.0).color(accent(ui)));
                        ui.label(RichText::new("还没有数据库连接").size(16.0).strong());
                    });
                }
            });
        if let Some(node) = tree_activation.clone().or_else(|| {
            (self.tree_selection.selected != tree_before)
                .then(|| self.tree_selection.selected.clone())
                .flatten()
        }) {
            // Redis selection belongs only to the tree. It must never become a SQL BrowserScope.
            let redis_node = match &node {
                TreeNode::Connection(id)
                | TreeNode::Database(id, _)
                | TreeNode::Schema(id, _, _)
                | TreeNode::Table(id, _, _)
                | TreeNode::TablePart(id, _, _, _, _) => self.is_redis_connection(*id),
                TreeNode::Group(_) => false,
            };
            let scope = if redis_node {
                None
            } else {
                match node {
                    TreeNode::Group(_) => None,
                    TreeNode::Connection(connection) => Some((
                        object_browser_ui::BrowserScope {
                            connection,
                            database: None,
                            schema: None,
                        },
                        None,
                    )),
                    TreeNode::Database(connection, database) => Some((
                        object_browser_ui::BrowserScope {
                            connection,
                            database: Some(database),
                            schema: None,
                        },
                        None,
                    )),
                    TreeNode::Schema(connection, database, schema) => Some((
                        object_browser_ui::BrowserScope {
                            connection,
                            database: Some(database),
                            schema: Some(schema),
                        },
                        None,
                    )),
                    TreeNode::Table(connection, database, table)
                    | TreeNode::TablePart(connection, database, table, _, _) => Some((
                        object_browser_ui::BrowserScope {
                            connection,
                            database: Some(database.clone()),
                            schema: Some(table.schema.clone()),
                        },
                        Some(object_browser_ui::BrowserItem::Object {
                            connection,
                            database,
                            table,
                        }),
                    )),
                }
            };
            if let Some((scope, item)) = scope {
                if tree_activation.is_some() {
                    self.show_object_scope(scope, item);
                } else {
                    self.select_object_scope(scope, item);
                }
            }
        }
        if let Some((connection, group)) = move_connection {
            self.move_to_group(connection, group);
        }
        self.request_tree_details(tree_render.requests);
        if let Some(action) = tree_render.action {
            self.open_tree_part(action);
        }
        if let Some(action) = tree_render.trigger {
            self.handle_browser_action(action);
        }
        if let Some(dialog) = group_dialog {
            self.catalog_dialog = Some(dialog);
        }
        if let Some(dialog) = schema_dialog {
            self.schema_dialog = Some(dialog);
        }
        if let Some(form) = table_create {
            self.table_create = Some(form);
            self.schema_dialog = None;
        }
        if let Some(id) = to_edit {
            self.selected = Some(id);
            self.edit_connection(id);
        }
        if let Some(id) = to_delete {
            self.selected = Some(id);
            self.confirm_delete = Some(id);
        }
        if let Some(id) = disconnect {
            self.disconnect_connection(id);
        }
        if let Some(id) = refresh_databases {
            self.request_on(id, Request::Databases, "正在刷新数据库列表…");
        }
        if let Some(id) = refresh_tables {
            self.request_on(id, Request::Tables, "正在刷新表目录…");
        }
        if let Some(id) = to_connect {
            self.selected = Some(id);
            self.connect_selected();
        }
        if let Some((id, database)) = to_select {
            self.selected = Some(id);
            if self.is_redis_connection(id) {
                match database.parse::<u32>() {
                    Ok(database) => {
                        self.select_redis_database(id, database);
                    }
                    Err(_) => self.fail("Redis 数据库编号无效".into()),
                }
            } else {
                self.request_on(id, Request::SelectDatabase(database), "正在选择数据库…");
            }
        }
        if let Some((connection, database, engine, table)) = preview {
            if self.cell_editor.is_some() {
                self.finish_inline_cell(false, CellMovement::Stay);
                if self.cell_editor.is_some() {
                    return;
                }
            }
            self.selected = Some(connection);
            let target = TableTarget {
                connection,
                database: database.clone(),
                schema: table.schema.clone(),
                name: table.name.clone(),
            };
            let (index, created) =
                open_table_tab(&mut self.tabs, &mut self.next_tab, target, engine, &table);
            self.active_tab = index;
            self.browser_visible = false;
            let tab = &self.tabs[index];
            if created
                || ((tab.output.is_none() || tab.table_snapshot_stale)
                    && !tab.has_table_drafts()
                    && self.cell_editor.is_none())
            {
                self.load_table(index);
            }
        }
    }
}
