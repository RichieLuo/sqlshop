use super::*;

impl App {
    pub(in crate::ui) fn restore_workspace(&mut self) {
        let Some(store) = &self.store else { return };
        let store = WorkspaceStore::new(store.directory.clone());
        match store.load() {
            Ok(workspace) => {
                self.tabs = workspace
                    .tabs
                    .into_iter()
                    .map(|saved| {
                        let mut tab = QueryTab::new(1);
                        tab.id = saved.id;
                        tab.name = saved.name.clone();
                        match &saved.document {
                            Some(SavedEngineDocument::Sqlite { database_path }) => {
                                tab.local_database =
                                    Some(local_database_ui::LocalDocument::restore(
                                        database_path.clone(),
                                        &saved,
                                    ));
                            }
                            Some(SavedEngineDocument::Oracle { .. }) => {
                                tab.oracle = Some(oracle_database_ui::OracleDocument::restore(
                                    &self.context,
                                    &saved,
                                ));
                            }
                            None => {
                                tab.sql = saved.sql;
                                tab.saved_sql = saved.saved_sql;
                                tab.file_path = saved.file_path;
                                tab.target = saved.target;
                                tab.saved_query = saved.saved_query;
                            }
                        }
                        tab
                    })
                    .collect();
                self.active_tab = workspace.active_tab.min(self.tabs.len().saturating_sub(1));
                self.browser_visible |= self.tabs.is_empty();
                self.next_tab = if self.tabs.is_empty() {
                    1
                } else {
                    workspace.next_tab.max(self.tabs.len() + 1)
                };
                self.editor_height = workspace.editor_height;
                self.history = workspace.history;
                self.table_grid_layouts = workspace.table_grid_layouts;
                if let Some(layout) = workspace.layout {
                    panel_layout_ui::restore(&self.context, &layout);
                    editor::set_line_height(&self.context, layout.sql_line_height);
                    self.view.navigation = layout.navigation;
                    self.inspector_visible = layout.information;
                    self.view.information_force = layout.information_force;
                    self.view.toolbar = layout.toolbar;
                    self.view.status = layout.status;
                    self.view.focus = layout.focus;
                    self.view.connected_only = layout.connected_only;
                    self.theme = match layout.theme.as_str() {
                        "light" => egui::ThemePreference::Light,
                        "dark" => egui::ThemePreference::Dark,
                        _ => egui::ThemePreference::System,
                    };
                    self.context.set_theme(self.theme);
                }
                self.workspace_store = Some(store);
                self.status =
                    "草稿已恢复 · 未连接、未执行 SQL · 草稿和历史以明文保存在本机，可能含敏感信息"
                        .into();
            }
            Err(error) => {
                // Do not overwrite an unreadable workspace with an empty one.
                self.fail(format!(
                    "工作区恢复失败，已停用自动保存以保护原文件：{error:#}"
                ));
            }
        }
    }

    pub(super) fn workspace_tabs(&self) -> impl Iterator<Item = &QueryTab> {
        self.tabs.iter().filter(|t| {
            (t.local_database.is_some() || t.oracle.is_some() || !t.management_document())
                && t.definition.is_none()
                && (t.table_target.is_none() || t.dirty())
        })
    }

    pub(super) fn workspace_theme(&self) -> &'static str {
        match self.theme {
            egui::ThemePreference::Light => "light",
            egui::ThemePreference::Dark => "dark",
            _ => "system",
        }
    }

    /// Compare persisted fields by reference before cloning SQL or history. Keep
    /// this projection in sync with `workspace_snapshot`, including filtered tabs.
    pub(in crate::ui) fn workspace_matches(&self, saved: &Workspace) -> bool {
        let Some(layout) = &saved.layout else {
            return false;
        };
        let (navigation_width, information_width) = panel_layout_ui::widths(&self.context);
        if saved.version != sqlshop::workspace::WORKSPACE_VERSION
            || saved.next_tab != self.next_tab
            || saved.editor_height != self.editor_height
            || layout.navigation != self.view.navigation
            || layout.navigation_width != navigation_width
            || layout.information != self.inspector_visible
            || layout.information_width != information_width
            || layout.information_force != self.view.information_force
            || layout.toolbar != self.view.toolbar
            || layout.status != self.view.status
            || layout.focus != self.view.focus
            || layout.connected_only != self.view.connected_only
            || layout.theme != self.workspace_theme()
            || layout.sql_line_height != editor::line_height(&self.context)
            || saved.history != self.history
            || saved.table_grid_layouts != self.table_grid_layouts
        {
            return false;
        }
        let active_id = self.tabs.get(self.active_tab).map(|tab| tab.id);
        let active_tab = self
            .workspace_tabs()
            .position(|tab| Some(tab.id) == active_id)
            .unwrap_or(0);
        if saved.active_tab != active_tab {
            return false;
        }
        let mut saved_tabs = saved.tabs.iter();
        for tab in self.workspace_tabs() {
            let Some(saved) = saved_tabs.next() else {
                return false;
            };
            if !tab.matches_saved_tab(saved) {
                return false;
            }
        }
        saved_tabs.next().is_none()
    }

    pub(in crate::ui) fn workspace_snapshot(&self) -> Workspace {
        let (navigation_width, information_width) = panel_layout_ui::widths(&self.context);
        let tabs: Vec<_> = self
            .workspace_tabs()
            .map(QueryTab::workspace_saved_tab)
            .collect();
        let active_id = self.tabs.get(self.active_tab).map(|tab| tab.id);
        let active_tab = tabs
            .iter()
            .position(|t| Some(t.id) == active_id)
            .unwrap_or(0);
        Workspace {
            tabs,
            active_tab,
            next_tab: self.next_tab,
            history: self.history.clone(),
            table_grid_layouts: self.table_grid_layouts.clone(),
            editor_height: self.editor_height,
            layout: Some(sqlshop::workspace::WorkspaceLayout {
                navigation: self.view.navigation,
                navigation_width,
                information: self.inspector_visible,
                information_width,
                information_force: self.view.information_force,
                toolbar: self.view.toolbar,
                status: self.view.status,
                focus: self.view.focus,
                connected_only: self.view.connected_only,
                theme: self.workspace_theme().into(),
                sql_line_height: editor::line_height(&self.context),
            }),
            ..Workspace::default()
        }
    }

    pub(in crate::ui) fn autosave_workspace(&mut self, force: bool) -> bool {
        if self.screenshot_path.is_some() {
            return true;
        }
        if !force && self.last_autosave.elapsed() < std::time::Duration::from_secs(2) {
            return true;
        }
        self.last_autosave = std::time::Instant::now();
        let Some(store) = &self.workspace_store else {
            return false;
        };
        if self
            .last_workspace
            .as_ref()
            .is_some_and(|saved| self.workspace_matches(saved))
        {
            return true;
        }
        let snapshot = self.workspace_snapshot();
        match store.save(&snapshot) {
            Ok(()) => {
                self.last_workspace = Some(snapshot);
                true
            }
            Err(error) => {
                self.fail(format!("草稿自动保存失败：{error:#}"));
                false
            }
        }
    }

    pub(in crate::ui) fn finish_history(&mut self, tab: Uuid, success: bool) {
        if let Some(mut entry) = self.pending_history.remove(&tab) {
            entry.success = success;
            let mut workspace = Workspace {
                history: std::mem::take(&mut self.history),
                ..Workspace::default()
            };
            workspace.push_history(entry);
            self.history = workspace.history;
        }
    }
}
