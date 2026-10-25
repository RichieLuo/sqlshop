use super::*;

fn columns() -> Vec<String> {
    vec!["id".into(), "name".into()]
}

#[test]
fn draft_apply_excludes_disabled_rows_and_preserves_null_and_sort_order() {
    let mut tab = QueryTab::new(1);
    tab.browse.offset = 400;
    tab.browse.page_size = 100;
    tab.browse_draft.filters = vec![
        FilterDraft {
            enabled: true,
            column: "name".into(),
            operator: FilterOperator::Eq,
            value: String::new(),
        },
        FilterDraft {
            enabled: false,
            column: String::new(),
            operator: FilterOperator::Gt,
            value: "ignored".into(),
        },
        FilterDraft {
            enabled: true,
            column: "id".into(),
            operator: FilterOperator::IsNull,
            value: "stale".into(),
        },
        FilterDraft {
            enabled: true,
            column: "name".into(),
            operator: FilterOperator::IsNotNull,
            value: "stale".into(),
        },
    ];
    tab.browse_draft.sorts = vec![
        ColumnSort {
            column: "name".into(),
            descending: true,
        },
        ColumnSort {
            column: "id".into(),
            descending: false,
        },
    ];
    assert_eq!(tab.browse_draft.validation_error(&columns()), None);
    assert!(tab.browse.filters.is_empty());
    assert!(tab.browse.sorts.is_empty());
    assert_eq!(tab.browse.offset, 400);
    tab.apply_browse_draft();
    assert_eq!(tab.browse.filters.len(), 3);
    assert_eq!(tab.browse.filters[0].value, Some(String::new()));
    assert_eq!(tab.browse.filters[1].value, None);
    assert_eq!(tab.browse.filters[2].value, None);
    assert_eq!(tab.browse.sorts, tab.browse_draft.sorts);
    assert_eq!(tab.browse.offset, 0);
    assert_eq!(tab.browse.page_size, 100);
    assert_eq!(tab.browse_draft.filters.len(), 4);
}

#[test]
fn incomplete_enabled_rows_and_duplicate_sorts_block_apply() {
    let mut draft = TableBrowseDraft::default();
    assert!(draft.validation_error(&columns()).is_none());
    draft.filters.push(FilterDraft::new(String::new()));
    assert!(draft.validation_error(&columns()).is_some());
    draft.filters[0].enabled = false;
    assert!(draft.validation_error(&columns()).is_none());
    draft.sorts.push(ColumnSort {
        column: String::new(),
        descending: false,
    });
    assert!(draft.validation_error(&columns()).is_some());
    draft.sorts[0].column = "id".into();
    assert!(draft.validation_error(&columns()).is_none());
    draft.sorts.push(draft.sorts[0].clone());
    assert!(draft.validation_error(&columns()).is_some());
}

#[test]
fn reset_clears_applied_and_draft_conditions_without_affecting_other_tabs() {
    let mut tab = QueryTab::new(1);
    let other = QueryTab::new(2);
    tab.browse_draft
        .filters
        .push(FilterDraft::new("name".into()));
    tab.browse_draft.sorts.push(ColumnSort {
        column: "id".into(),
        descending: true,
    });
    tab.apply_browse_draft();
    tab.browse.offset = 200;
    tab.browse.page_size = 50;
    tab.reset_browse();
    assert!(tab.browse_draft.filters.is_empty());
    assert!(tab.browse_draft.sorts.is_empty());
    assert!(tab.browse.filters.is_empty());
    assert!(tab.browse.sorts.is_empty());
    assert_eq!(tab.browse.offset, 0);
    assert_eq!(tab.browse.page_size, 50);
    assert_eq!(other.browse, TableBrowseOptions::default());
    assert!(other.browse_draft.filters.is_empty());
}
