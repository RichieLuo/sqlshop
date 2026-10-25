use super::*;

#[test]
fn table_tabs_bind_their_own_context() {
    let table = table();
    let target = TableTarget {
        connection: Uuid::new_v4(),
        database: "dev".into(),
        schema: table.schema.clone(),
        name: table.name.clone(),
    };
    let mut tabs = vec![QueryTab::new(1)];
    let mut next = 2;
    let (index, _) = open_table_tab(
        &mut tabs,
        &mut next,
        target.clone(),
        Engine::Postgres,
        &table,
    );
    assert_eq!(
        tabs[index].target,
        Some(QueryTarget {
            schema: Some(target.schema.clone()),
            connection: target.connection,
            database: Some(target.database.clone())
        })
    );
    assert!(!tabs[index].bind_for_execution(&QueryTarget {
        schema: None,
        connection: target.connection,
        database: Some("prod".into())
    }));
    assert_eq!(tabs[index].table_target.as_ref().unwrap().database, "dev");
}

#[test]
fn closing_tabs_keeps_the_active_query_and_results() {
    let mut tabs: Vec<_> = (1..=4).map(QueryTab::new).collect();
    let mut active = 2;
    let mut next = 5;
    let active_id = tabs[active].id;
    tabs[active].sql = "SELECT '保留我的查询'".into();
    tabs[active].output = Some(QueryOutput::default());
    apply_tab_action(&mut tabs, &mut active, &mut next, TabAction::Close(0));
    assert_eq!(active, 1);
    assert_eq!(tabs[active].id, active_id);
    assert_eq!(tabs[active].sql, "SELECT '保留我的查询'");
    assert!(tabs[active].output.is_some());
    apply_tab_action(&mut tabs, &mut active, &mut next, TabAction::CloseRight(1));
    assert_eq!(tabs.len(), 2);
    assert_eq!(tabs[active].id, active_id);
    apply_tab_action(&mut tabs, &mut active, &mut next, TabAction::CloseOthers(1));
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[active].id, active_id);
    apply_tab_action(&mut tabs, &mut active, &mut next, TabAction::Close(0));
    assert_eq!(tabs.len(), 1);
    assert_eq!(active, 0);
    assert_eq!(tabs[0].name, "查询 5");
    assert_eq!(next, 6);
    assert_ne!(tabs[0].id, active_id);
}

#[test]
fn closing_active_tab_selects_a_neighbor() {
    let mut tabs: Vec<_> = (1..=4).map(QueryTab::new).collect();
    let mut active = 3;
    let mut next = 5;
    let neighbor = tabs[2].id;
    apply_tab_action(&mut tabs, &mut active, &mut next, TabAction::Close(3));
    assert_eq!(tabs[active].id, neighbor);
    // Closing to the right of an inactive tab removes the old active tab.
    apply_tab_action(&mut tabs, &mut active, &mut next, TabAction::CloseRight(0));
    assert_eq!(active, 0);
    assert_eq!(tabs.len(), 1);
}
