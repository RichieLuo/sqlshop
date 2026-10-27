use super::*;

impl App {
    pub(super) fn table_rename_rejection(
        &self,
        connection: Uuid,
        database: &str,
        table: &Table,
    ) -> Option<String> {
        let Some(session) = self.sessions.get(&connection) else {
            return Some("请先连接并选择此表所属数据库".into());
        };
        if session.connected.is_none() || session.active_database.as_deref() != Some(database) {
            return Some("请先连接并选择此表所属数据库".into());
        }
        if session.busy {
            return Some("请等待当前数据库操作结束".into());
        }
        if !table.kind.eq_ignore_ascii_case("BASE TABLE") || !session.tables.contains(table) {
            return Some("目标不存在或不是普通数据表，请刷新目录".into());
        }
        let identity = TableTarget {
            connection,
            database: database.into(),
            schema: table.schema.clone(),
            name: table.name.clone(),
        };
        if self.tabs.iter().enumerate().any(|(index, tab)| {
            tab.table_target.as_ref() == Some(&identity)
                && (tab.has_table_drafts()
                    || tab.design.as_ref().is_some_and(|doc| doc.dirty())
                    || self.tab_busy(index))
        }) || self.cell_editor.as_ref().is_some_and(|editor| {
            editor.connection == connection
                && editor.change.database == database
                && editor.change.table == *table
        }) || self.csv_import.as_ref().is_some_and(|form| {
            form.target.connection == connection
                && form.target.database.as_deref() == Some(database)
                && form.table == *table
        }) {
            return Some(
                "此表有未保存的修改或正在执行操作；请先完成操作，并提交或撤销数据及结构草稿".into(),
            );
        }
        None
    }

    pub(super) fn apply_table_rename(
        &mut self,
        connection: Uuid,
        database: &str,
        table: &Table,
        new_name: &str,
        engine: Engine,
    ) {
        let renamed = Table {
            name: new_name.into(),
            ..table.clone()
        };
        for tab in &mut self.tabs {
            let Some(target) = &mut tab.table_target else {
                continue;
            };
            if target.connection != connection
                || target.database != database
                || target.schema != table.schema
                || target.name != table.name
            {
                continue;
            }
            target.name = new_name.into();
            tab.name = if tab.design.is_some() {
                format!("{new_name} · 设计")
            } else {
                new_name.into()
            };
            tab.table_snapshot_stale = true;
            tab.table_write_notice = Some("表已重命名，请刷新后继续操作".into());
            if let Some(doc) = &mut tab.design {
                let section = doc.section;
                *doc = structure_ui::DesignDocument::default();
                doc.section = section;
                doc.status = "表已重命名，请重新读取实际结构".into();
            } else {
                tab.sql = engine.preview_sql(&renamed);
                tab.saved_sql = tab.sql.clone();
            }
        }
        match &mut self.tree_selection.selected {
            Some(TreeNode::Table(id, db, selected) | TreeNode::TablePart(id, db, selected, ..))
                if *id == connection && db == database && selected == table =>
            {
                *selected = renamed.clone()
            }
            _ => {}
        }
        if let Some(object_browser_ui::BrowserItem::Object {
            connection: id,
            database: db,
            table: selected,
        }) = &mut self.browser_state.selected
            && *id == connection
            && db == database
            && selected == table
        {
            *selected = renamed;
        }
    }
}
