use super::*;
use std::collections::VecDeque;

pub(super) struct DatabaseTree {
    // Persist expansion only while this connection's in-memory contents exist.
    pub generation: Uuid,
    // Tree expansion outlives the management connection's current database.
    pub tables: HashMap<String, Vec<Table>>,
    pub errors: HashMap<String, String>,
    pub pending: VecDeque<String>,
    pub opening: Option<String>,
}

impl Default for DatabaseTree {
    fn default() -> Self {
        Self {
            generation: Uuid::new_v4(),
            tables: Default::default(),
            errors: Default::default(),
            pending: Default::default(),
            opening: None,
        }
    }
}

impl ConnectionSession {
    pub(super) fn tree_tables(&self, database: &str) -> Option<&[Table]> {
        if self.active_database.as_deref() == Some(database) {
            Some(&self.tables)
        } else {
            self.database_tree.tables.get(database).map(Vec::as_slice)
        }
    }
}

pub(super) fn database_status(
    ui: &mut egui::Ui,
    session: &ConnectionSession,
    database: &str,
) -> bool {
    let active = session.active_database.as_deref() == Some(database);
    let waiting = session
        .database_tree
        .pending
        .iter()
        .any(|name| name == database);
    let opening = session.database_tree.opening.as_deref() == Some(database);
    if waiting || opening || (active && session.catalog_loading) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(
                RichText::new(if waiting {
                    "正在等待当前操作完成…"
                } else {
                    "正在读取数据库对象…"
                })
                .small()
                .weak(),
            );
        });
        return false;
    }
    let error = session.database_tree.errors.get(database).or({
        if active {
            session.catalog_error.as_ref()
        } else {
            None
        }
    });
    if let Some(error) = error {
        ui.label(
            RichText::new(format!("读取失败：{error}"))
                .small()
                .color(ui.visuals().error_fg_color),
        );
        return ui
            .add_enabled(
                !session.busy,
                egui::Button::new(if active {
                    "重试读取对象"
                } else {
                    "重试打开数据库"
                }),
            )
            .clicked();
    }
    if !active && !session.database_tree.tables.contains_key(database) {
        ui.label(RichText::new("展开或双击打开数据库").small().weak());
        return ui
            .add_enabled(!session.busy, egui::Button::new("打开数据库"))
            .clicked();
    }
    false
}

impl App {
    pub(super) fn open_tree_database(&mut self, connection: Uuid, database: String) {
        if self.screenshot_path.is_some() {
            return;
        }
        let Some(session) = self.sessions.get_mut(&connection) else {
            return;
        };
        if session.connected.is_none() || session.database_tree.opening.as_ref() == Some(&database)
        {
            return;
        }
        let active = session.active_database.as_ref() == Some(&database);
        if active && session.object_catalog.is_some() && session.catalog_error.is_none() {
            return;
        }
        if session.busy {
            if !session.database_tree.pending.contains(&database) {
                session.database_tree.pending.push_back(database);
            }
            return;
        }
        let request = if active {
            Request::ObjectCatalog { database }
        } else {
            Request::SelectDatabase(database)
        };
        self.request_on(connection, request, "正在打开数据库…");
    }

    pub(super) fn poll_database_opens(&mut self) {
        if self.confirm_request.is_some() {
            return;
        }
        let ready: Vec<_> = self
            .sessions
            .iter()
            .filter_map(|(&connection, session)| {
                if session.busy || session.connected.is_none() {
                    return None;
                }
                (!session.database_tree.pending.is_empty()).then_some(connection)
            })
            .collect();
        for connection in ready {
            if let Some(database) = self
                .sessions
                .get_mut(&connection)
                .and_then(|session| session.database_tree.pending.pop_front())
            {
                self.open_tree_database(connection, database);
            }
            if self.confirm_request.is_some() {
                break;
            }
        }
    }
}
