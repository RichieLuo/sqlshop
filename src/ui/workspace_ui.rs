mod dialogs;

mod table_controls;

mod query_controls;

mod documents;

mod persistence;

use super::*;
use sqlshop::workspace::SavedEngineDocument;

pub(super) enum SqlReplacement {
    Generated(String),
    File { path: PathBuf, sql: String },
}

impl SqlReplacement {
    fn sql(&self) -> &str {
        match self {
            Self::Generated(sql) | Self::File { sql, .. } => sql,
        }
    }

    pub(super) fn apply(
        self,
        sql: &mut String,
        saved_sql: &mut String,
        file_path: &mut Option<PathBuf>,
    ) {
        match self {
            Self::Generated(replacement) => *sql = replacement,
            Self::File {
                path,
                sql: replacement,
            } => {
                *saved_sql = replacement.clone();
                *sql = replacement;
                *file_path = Some(path);
            }
        }
    }

    pub(super) fn confirm(&self, ui: &mut egui::Ui) -> Option<bool> {
        let mut action = None;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label("当前 SQL 有未保存修改。确认替换会丢弃这些修改。");
            ui.collapsing("待替换的 SQL", |ui| {
                egui::ScrollArea::both().max_height(120.0).show(ui, |ui| {
                    ui.monospace(self.sql());
                });
            });
            ui.horizontal(|ui| {
                if ui.button("保留当前 SQL").clicked() {
                    action = Some(false);
                }
                if ui.button("确认替换 SQL").clicked() {
                    action = Some(true);
                }
            });
        });
        action
    }
}

#[derive(Clone)]
struct FilterDraft {
    enabled: bool,
    column: String,
    operator: FilterOperator,
    value: String,
}

impl FilterDraft {
    fn new(column: String) -> Self {
        Self {
            enabled: true,
            column,
            operator: FilterOperator::Eq,
            value: String::new(),
        }
    }

    fn needs_value(&self) -> bool {
        !matches!(
            self.operator,
            FilterOperator::IsNull | FilterOperator::IsNotNull
        )
    }

    fn applied(&self) -> ColumnFilter {
        ColumnFilter {
            column: self.column.clone(),
            operator: self.operator,
            value: self.needs_value().then(|| self.value.clone()),
        }
    }
}

/// Editable conditions are local to the tab; paging and refresh use only `browse`.
#[derive(Clone, Default)]
pub(super) struct TableBrowseDraft {
    filters: Vec<FilterDraft>,
    sorts: Vec<ColumnSort>,
}

impl TableBrowseDraft {
    pub(super) fn add_filter(
        &mut self,
        column: String,
        operator: FilterOperator,
        value: Option<String>,
    ) -> bool {
        if self.filters.len() < 100 {
            self.filters.push(FilterDraft {
                enabled: true,
                column,
                operator,
                value: value.unwrap_or_default(),
            });
            true
        } else {
            false
        }
    }

    pub(super) fn sync_sorts(&mut self, sorts: &[ColumnSort]) {
        self.sorts = sorts.to_vec();
    }

    fn matches(&self, browse: &TableBrowseOptions) -> bool {
        self.sorts == browse.sorts
            && self
                .filters
                .iter()
                .filter(|row| row.enabled)
                .map(FilterDraft::applied)
                .collect::<Vec<_>>()
                == browse.filters
    }

    fn validation_error(&self, columns: &[String]) -> Option<&'static str> {
        let filters: Vec<_> = self.filters.iter().filter(|row| row.enabled).collect();
        if filters.len() > 100 {
            return Some("最多支持 100 个启用的筛选条件");
        }
        if filters.iter().any(|row| !columns.contains(&row.column)) {
            return Some("请为每个启用的筛选条件选择有效列");
        }
        if self.sorts.iter().any(|row| !columns.contains(&row.column)) {
            return Some("请为每个排序条件选择有效列");
        }
        for (index, row) in self.sorts.iter().enumerate() {
            if self.sorts[..index]
                .iter()
                .any(|previous| previous.column == row.column)
            {
                return Some("排序列不能重复");
            }
        }
        None
    }

    fn apply_to(&self, browse: &mut TableBrowseOptions) {
        browse.filters = self
            .filters
            .iter()
            .filter(|row| row.enabled)
            .map(FilterDraft::applied)
            .collect();
        browse.sorts = self.sorts.clone();
        browse.offset = 0;
    }
}

impl QueryTab {
    fn apply_browse_draft(&mut self) {
        self.browse_draft.apply_to(&mut self.browse);
    }

    fn reset_browse(&mut self) {
        self.browse_draft = TableBrowseDraft::default();
        self.apply_browse_draft();
    }

    pub(super) fn bind_for_execution(&mut self, current: &QueryTarget) -> bool {
        if self.target.as_ref().is_some_and(|target| target != current) {
            return false;
        }
        self.target = Some(current.clone());
        true
    }

    pub(super) fn dirty(&self) -> bool {
        self.sql_dirty()
            || self.local_database.as_ref().is_some_and(|doc| doc.dirty())
            || self
                .oracle
                .as_ref()
                .is_some_and(|doc| doc.is_dirty() || doc.may_have_transaction())
            || self.redis.as_ref().is_some_and(|doc| doc.is_dirty())
            || self.jobs.as_ref().is_some_and(|doc| doc.dirty())
            || self
                .advanced
                .as_ref()
                .is_some_and(|doc| doc.preview.is_some() || doc.account_preview.is_some())
    }

    fn workspace_saved_tab(&self) -> SavedTab {
        if let Some(doc) = &self.local_database {
            return doc.saved_tab(self.id, &self.name);
        }
        if let Some(doc) = &self.oracle {
            return doc.saved_tab(self.id, &self.name);
        }
        SavedTab {
            id: self.id,
            name: self.name.clone(),
            sql: self.sql.clone(),
            file_path: self.file_path.clone(),
            saved_sql: self.saved_sql.clone(),
            target: self.target.clone(),
            saved_query: self.saved_query.clone(),
            document: None,
        }
    }

    fn matches_saved_tab(&self, saved: &SavedTab) -> bool {
        if saved.id != self.id || saved.name != self.name {
            return false;
        }
        if let Some(doc) = &self.local_database {
            return doc.matches_saved(saved);
        }
        if let Some(doc) = &self.oracle {
            return doc.matches_saved(saved);
        }
        saved.document.is_none()
            && saved.sql == self.sql
            && saved.file_path == self.file_path
            && saved.saved_sql == self.saved_sql
            && saved.target == self.target
            && saved.saved_query == self.saved_query
    }
}

#[derive(Clone, Copy)]
struct TableActionState {
    idle: bool,
    ready: bool,
    stale: bool,
    has_drafts: bool,
}

impl TableActionState {
    fn can_read(self) -> bool {
        self.idle && self.ready && !self.has_drafts
    }

    fn can_write(self) -> bool {
        self.can_read() && !self.stale
    }

    fn can_submit(self) -> bool {
        self.idle && self.ready && !self.stale && self.has_drafts
    }

    fn can_revert(self) -> bool {
        self.idle && self.has_drafts
    }
}

impl App {
    pub(super) fn can_add_table_row(&self, index: usize) -> bool {
        let state = self.table_action_state(index);
        let tab = &self.tabs[index];
        state.idle
            && state.ready
            && !state.stale
            && tab.table_target.is_some()
            && tab.table_data.as_ref().is_some_and(|data| data.writable)
            && tab.insert_drafts.len() < 1000
    }

    pub(super) fn can_browse_grid(&self, index: usize) -> bool {
        let state = self.table_action_state(index);
        state.can_read() && !state.stale
    }

    fn table_action_state(&self, index: usize) -> TableActionState {
        let tab = &self.tabs[index];
        TableActionState {
            idle: !self.tab_busy(index) && self.cell_editor.is_none(),
            ready: tab
                .target
                .as_ref()
                .is_some_and(|target| self.target_ready(target)),
            stale: tab.table_snapshot_stale,
            has_drafts: tab.has_table_drafts(),
        }
    }

    pub(super) fn current_target(&self) -> Option<QueryTarget> {
        self.selected_session().and_then(|s| {
            s.connected.as_ref().map(|p| QueryTarget {
                connection: p.id,
                database: s.active_database.clone(),
                schema: None,
            })
        })
    }
}

fn draft_value_label(value: &Option<String>) -> String {
    match value {
        None => "NULL".into(),
        Some(value) if value.is_empty() => "空字符串".into(),
        Some(value) => value.replace(['\n', '\r'], " "),
    }
}

/// Resolve aliases for document identity while retaining the user's file path
/// for I/O. New save-as destinations can resolve their existing parent folder.
fn sql_file_identity(path: &std::path::Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        path.parent()
            .and_then(|parent| parent.canonicalize().ok())
            .zip(path.file_name())
            .map(|(parent, name)| parent.join(name))
            .unwrap_or_else(|| path.to_path_buf())
    })
}

fn table_footer_status(ui: &mut egui::Ui, status: &str, source: &str, ready: bool) {
    let color = if ready {
        ui.visuals().weak_text_color()
    } else {
        Color32::from_rgb(235, 150, 70)
    };
    ui.add_sized(
        [ui.available_width().max(0.0), 24.0],
        egui::Label::new(RichText::new(status).small().color(color)).truncate(),
    )
    .on_hover_text(format!("{status}\n{source}"));
}

fn table_page_controls(ui: &mut egui::Ui, tab: &mut QueryTab, enabled: bool) -> (bool, bool) {
    let mut reload = false;
    let mut last_page = false;
    ui.add_enabled_ui(enabled, |ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.spacing_mut().button_padding.x = 8.0;
        let step = u64::from(tab.browse.page_size.max(1));
        if ui
            .add_enabled(tab.browse.offset > 0, egui::Button::new("首页"))
            .clicked()
        {
            tab.browse.offset = 0;
            reload = true;
        }
        if ui
            .add_enabled(tab.browse.offset > 0, egui::Button::new("上一页"))
            .clicked()
        {
            tab.browse.offset = tab.browse.offset.saturating_sub(step);
            reload = true;
        }
        ui.label("第");
        let mut page = tab.browse.offset / step + 1;
        ui.add(
            egui::DragValue::new(&mut page)
                .range(1..=((i64::MAX as u64 - step - 1) / step + 1))
                .speed(1)
                .update_while_editing(false),
        )
        .on_hover_text("输入页码并按 Enter 或离开输入框跳转");
        let page_changed = page != tab.browse.offset / step + 1;
        if page_changed && let Some(offset) = page.saturating_sub(1).checked_mul(step) {
            tab.browse.offset = offset;
            reload = true;
        }
        ui.label("页");
        if ui
            .add_enabled(
                tab.output.is_some()
                    && tab.has_more
                    && tab
                        .browse
                        .offset
                        .checked_add(step)
                        .is_some_and(|offset| offset < i64::MAX as u64 - step),
                egui::Button::new("下一页"),
            )
            .clicked()
        {
            tab.browse.offset = tab.browse.offset.saturating_add(step);
            reload = true;
        }
        last_page = ui
            .button("末页")
            .on_hover_text("按当前筛选计算总条数并读取末页；并发新增或删除记录时，末页位置可能变化")
            .clicked();
        ui.label("每页");
        let previous_size = tab.browse.page_size;
        egui::ComboBox::from_id_salt(("page-size", tab.id))
            .width(60.0)
            .selected_text(tab.browse.page_size.to_string())
            .show_ui(ui, |ui| {
                for size in [50, 100, 200, 500, 1000] {
                    ui.selectable_value(&mut tab.browse.page_size, size, size.to_string());
                }
            });
        if tab.browse.page_size != previous_size {
            tab.browse.offset = 0;
            reload = true;
        }
    });
    (reload, last_page)
}

enum BrowseDraftAction {
    Apply,
    Reset,
}

const FILTER_OPERATORS: [(FilterOperator, &str); 7] = [
    (FilterOperator::Eq, "="),
    (FilterOperator::NotEq, "≠"),
    (FilterOperator::Contains, "包含"),
    (FilterOperator::Gt, ">"),
    (FilterOperator::Lt, "<"),
    (FilterOperator::IsNull, "IS NULL"),
    (FilterOperator::IsNotNull, "IS NOT NULL"),
];

fn draft_column(ui: &mut egui::Ui, column: &mut String, columns: &[String]) {
    egui::ComboBox::from_id_salt("column")
        .width(150.0)
        .selected_text(if column.is_empty() {
            "选择列"
        } else {
            column.as_str()
        })
        .show_ui(ui, |ui| {
            for name in columns {
                ui.selectable_value(column, name.clone(), name);
            }
        });
}

fn browse_draft_controls(
    ui: &mut egui::Ui,
    tab: Uuid,
    draft: &mut TableBrowseDraft,
    columns: &[String],
) -> Option<BrowseDraftAction> {
    let mut action = None;
    ui.push_id(("table-filters", tab), |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
        egui::Frame::group(ui.style())
            .inner_margin(8)
            .show(ui, |ui| {
                // Bound the condition list so Apply and the data grid remain reachable.
                let max_height = (ui.available_height() * 0.45).clamp(80.0, 240.0);
                egui::ScrollArea::both()
                    .id_salt("draft-rows")
                    .max_height(max_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.strong("筛选");
                        let mut remove = None;
                        let filter_count = draft.filters.len();
                        for (index, row) in draft.filters.iter_mut().enumerate() {
                            ui.push_id(("filter", index), |ui| {
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut row.enabled, "")
                                        .on_hover_text("启用此条件；未启用的条件不会提交");
                                    draft_column(ui, &mut row.column, columns);
                                    let label = FILTER_OPERATORS
                                        .iter()
                                        .find(|(operator, _)| *operator == row.operator)
                                        .map(|(_, label)| *label)
                                        .unwrap_or("=");
                                    egui::ComboBox::from_id_salt("operator")
                                        .width(110.0)
                                        .selected_text(label)
                                        .show_ui(ui, |ui| {
                                            for (operator, label) in FILTER_OPERATORS {
                                                ui.selectable_value(
                                                    &mut row.operator,
                                                    operator,
                                                    label,
                                                );
                                            }
                                        });
                                    ui.add_enabled(
                                        row.needs_value(),
                                        egui::TextEdit::singleline(&mut row.value)
                                            .desired_width(170.0)
                                            .hint_text("筛选值"),
                                    )
                                    .on_hover_text("空文本是有效筛选值；查找 NULL 请使用 IS NULL");
                                    if ui
                                        .small_button("−")
                                        .on_hover_text("删除此筛选条件")
                                        .clicked()
                                    {
                                        remove = Some(index);
                                    }
                                    if index + 1 < filter_count {
                                        ui.label(RichText::new("AND").small().weak());
                                    }
                                });
                            });
                        }
                        if let Some(index) = remove {
                            draft.filters.remove(index);
                        }
                        if ui
                            .add_enabled(
                                draft.filters.len() < 100,
                                egui::Button::new("＋ 添加条件"),
                            )
                            .clicked()
                        {
                            draft.filters.push(FilterDraft::new(String::new()));
                        }
                        ui.separator();
                        ui.strong("排序方式");
                        let mut remove = None;
                        for (index, row) in draft.sorts.iter_mut().enumerate() {
                            ui.push_id(("sort", index), |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(format!("{}.", index + 1)).weak());
                                    draft_column(ui, &mut row.column, columns);
                                    egui::ComboBox::from_id_salt("direction")
                                        .width(110.0)
                                        .selected_text(if row.descending {
                                            "降序 DESC"
                                        } else {
                                            "升序 ASC"
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut row.descending,
                                                false,
                                                "升序 ASC",
                                            );
                                            ui.selectable_value(
                                                &mut row.descending,
                                                true,
                                                "降序 DESC",
                                            );
                                        });
                                    if ui
                                        .small_button("−")
                                        .on_hover_text("删除此排序条件")
                                        .clicked()
                                    {
                                        remove = Some(index);
                                    }
                                });
                            });
                        }
                        if let Some(index) = remove {
                            draft.sorts.remove(index);
                        }
                        if ui
                            .add_enabled(
                                draft.sorts.len() < columns.len(),
                                egui::Button::new("＋ 添加排序"),
                            )
                            .clicked()
                        {
                            draft.sorts.push(ColumnSort {
                                column: String::new(),
                                descending: false,
                            });
                        }
                    });
                ui.separator();
                let error = draft.validation_error(columns);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(error.is_none(), egui::Button::new("应用筛选与排序"))
                        .on_hover_text("应用后重新读取数据；筛选条件以 AND 组合")
                        .clicked()
                    {
                        action = Some(BrowseDraftAction::Apply);
                    }
                    if ui
                        .button("重置")
                        .on_hover_text("清空草稿和已应用条件，返回第一页")
                        .clicked()
                    {
                        action = Some(BrowseDraftAction::Reset);
                    }
                    if let Some(error) = error {
                        ui.label(RichText::new(error).small().weak());
                    }
                });
            });
    });
    action
}

fn find_controls(ui: &mut egui::Ui, tab: &mut QueryTab) {
    if !tab.find_open {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        if ui
            .add(
                egui::TextEdit::singleline(&mut tab.find_text)
                    .hint_text("查找文本")
                    .desired_width(160.0),
            )
            .changed()
        {
            tab.find_index = usize::MAX;
        }
        ui.checkbox(&mut tab.find_case, "区分大小写");
        let matches = find_matches(&tab.sql, &tab.find_text, tab.find_case);
        ui.label(format!("{} 处匹配", matches.len()));
        if !matches.is_empty() {
            if ui.button("上一处").clicked() {
                tab.find_index = if tab.find_index >= matches.len() {
                    matches.len() - 1
                } else {
                    (tab.find_index + matches.len() - 1) % matches.len()
                };
                tab.select_range = Some(matches[tab.find_index].clone());
            }
            if ui.button("下一处").clicked() {
                tab.find_index = if tab.find_index >= matches.len() {
                    0
                } else {
                    (tab.find_index + 1) % matches.len()
                };
                tab.select_range = Some(matches[tab.find_index].clone());
            }
        }
        if ui.button("关闭查找").clicked() {
            tab.find_open = false;
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut tab.replace_text)
                .hint_text("替换为")
                .desired_width(160.0),
        );
        if ui.button("替换一处").clicked() {
            let matches = find_matches(&tab.sql, &tab.find_text, tab.find_case);
            if !matches.is_empty() {
                if tab.find_index >= matches.len() {
                    tab.find_index = 0;
                }
                let range = matches[tab.find_index].clone();
                replace_match(&mut tab.sql, range, &tab.replace_text);
                tab.selection.clear();
            }
        }
        if ui.button("全部替换").clicked() {
            tab.sql = replace_all(&tab.sql, &tab.find_text, &tab.replace_text, tab.find_case).0;
            tab.selection.clear();
            tab.find_index = 0;
        }
    });
}

#[cfg(test)]
mod browse_draft_tests;

#[cfg(test)]
mod engine_draft_tests;
