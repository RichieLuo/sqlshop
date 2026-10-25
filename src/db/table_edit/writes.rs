use super::*;

impl Session {
    pub async fn update_cell(&mut self, change: &CellUpdate) -> Result<()> {
        self.ensure_edit_transaction_safe().await?;
        if self.current_database().await?.as_deref() != Some(change.database.as_str()) {
            return Err(
                EditRejected("当前数据库已改变，请选择此表所属数据库后再保存".into()).into(),
            );
        }
        let columns = self.table_columns(&change.table).await?;
        let keys: Vec<_> = columns.iter().filter(|column| column.primary_key).collect();
        let column = columns
            .iter()
            .find(|column| column.name == change.column)
            .context("该列已不存在，请刷新表数据")?;
        if !column.editable
            || keys.is_empty()
            || keys.iter().any(|key| key.kind == CellKind::Unsupported)
            || keys
                .iter()
                .map(|key| &key.name)
                .ne(change.primary_keys.iter())
            || keys.len() != change.key_values.len()
            || change.key_values.iter().any(Option::is_none)
        {
            return Err(EditRejected(
                "表结构已改变或缺少可用主键，该单元格不可编辑；请刷新表数据".into(),
            )
            .into());
        }
        if change.original == change.value {
            return Ok(());
        }
        let mut parameters = vec![change.value.clone()];
        parameters.extend(change.key_values.iter().cloned());
        parameters.push(change.original.clone());
        for (column, value) in std::iter::once((column, &parameters[0])).chain(
            keys.iter()
                .zip(change.key_values.iter())
                .map(|(column, value)| (*column, value)),
        ) {
            if column.kind == CellKind::Binary
                && value.as_deref().is_some_and(|value| !valid_hex(value))
            {
                return Err(EditRejected("二进制值请使用 0x 开头的完整十六进制文本".into()).into());
            }
        }
        let engine = self.profile.engine;
        let predicates = keys
            .iter()
            .enumerate()
            .map(|(index, column)| {
                format!(
                    "{} = {}",
                    engine.quote(&column.name),
                    parameter_expression(engine, column, index + 2)
                )
            })
            .collect::<Vec<_>>();
        let name = engine.quote(&column.name);
        validate_assignment_value(engine, column, change.value.as_deref())?;
        self.ensure_sqlserver_text_inputs(
            &change.table,
            &columns,
            std::slice::from_ref(change),
            &[],
            &[],
        )
        .await?;
        let optimistic = original_value_predicate(engine, column, parameters.len());
        // SQL Server references the same positional parameter twice; MySQL's ? is used once.
        let sql = format!(
            "UPDATE {}.{} SET {name} = {} WHERE {} AND ({optimistic})",
            engine.quote(&change.table.schema),
            engine.quote(&change.table.name),
            assignment_expression(engine, column, 1),
            predicates.join(" AND ")
        );
        match &mut self.driver {
            Driver::MySql(connection) => {
                let mut tx = connection.begin().await?;
                let mut query = sqlx::query(&sql);
                for parameter in &parameters {
                    query = query.bind(parameter.as_deref());
                }
                match query
                    .execute(&mut *tx)
                    .await
                    .map(|result| result.rows_affected())
                {
                    Ok(1) => tx.commit().await.context(CommitOutcomeUnknown)?,
                    result => {
                        tx.rollback().await.context("回滚修改失败")?;
                        return Err(rejected_update(result).into());
                    }
                }
            }
            Driver::Postgres(connection) => {
                let mut tx = connection.begin().await?;
                let mut query = sqlx::query(&sql);
                for parameter in &parameters {
                    query = query.bind(parameter.as_deref());
                }
                match query
                    .execute(&mut *tx)
                    .await
                    .map(|result| result.rows_affected())
                {
                    Ok(1) => tx.commit().await.context(CommitOutcomeUnknown)?,
                    result => {
                        tx.rollback().await.context("回滚修改失败")?;
                        return Err(rejected_update(result).into());
                    }
                }
            }
            Driver::SqlServer(client) => {
                client
                    .simple_query("BEGIN TRANSACTION")
                    .await?
                    .into_results()
                    .await?;
                let mut query = tiberius::Query::new(&sql);
                for parameter in &parameters {
                    query.bind(parameter.as_deref());
                }
                match query.execute(client).await.map(|result| result.total()) {
                    Ok(1) => {
                        client
                            .simple_query("COMMIT TRANSACTION")
                            .await
                            .context(CommitOutcomeUnknown)?
                            .into_results()
                            .await
                            .context(CommitOutcomeUnknown)?;
                    }
                    result => {
                        client
                            .simple_query("IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION")
                            .await?
                            .into_results()
                            .await?;
                        return Err(rejected_update(result).into());
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn delete_rows(
        &mut self,
        database: &str,
        table: &Table,
        primary_keys: &[String],
        key_values: &[Vec<Option<String>>],
    ) -> Result<usize> {
        self.ensure_edit_transaction_safe().await?;
        if self.current_database().await?.as_deref() != Some(database) {
            return Err(
                EditRejected("当前数据库已改变，请选择此表所属数据库后再删除".into()).into(),
            );
        }
        anyhow::ensure!(!key_values.is_empty(), "没有选中要删除的行");
        self.ensure_table_writable(table).await?;
        let columns = self.table_columns(table).await?;
        let keys: Vec<_> = columns.iter().filter(|column| column.primary_key).collect();
        if keys.is_empty()
            || keys.iter().any(|key| key.kind == CellKind::Unsupported)
            || keys
                .iter()
                .map(|key| key.name.as_str())
                .ne(primary_keys.iter().map(String::as_str))
            || key_values
                .iter()
                .any(|values| values.len() != keys.len() || values.iter().any(Option::is_none))
        {
            return Err(EditRejected(
                "表结构已改变或缺少可用主键，无法安全删除；请刷新表数据".into(),
            )
            .into());
        }
        let engine = self.profile.engine;
        let predicates = keys
            .iter()
            .enumerate()
            .map(|(index, column)| {
                format!(
                    "{} = {}",
                    engine.quote(&column.name),
                    parameter_expression(engine, column, index + 1)
                )
            })
            .collect::<Vec<_>>();
        let sql = format!(
            "DELETE FROM {}.{} WHERE {}",
            engine.quote(&table.schema),
            engine.quote(&table.name),
            predicates.join(" AND ")
        );
        let mut deleted = 0;
        match &mut self.driver {
            Driver::MySql(connection) => {
                let mut tx = connection.begin().await?;
                for values in key_values {
                    let mut query = sqlx::query(&sql);
                    for value in values {
                        query = query.bind(value.as_deref());
                    }
                    match query
                        .execute(&mut *tx)
                        .await
                        .map(|result| result.rows_affected())
                    {
                        Ok(1) => deleted += 1,
                        result => {
                            tx.rollback().await.context("回滚删除失败")?;
                            return Err(rejected_delete(result).into());
                        }
                    }
                }
                tx.commit().await.context(CommitOutcomeUnknown)?;
            }
            Driver::Postgres(connection) => {
                let mut tx = connection.begin().await?;
                for values in key_values {
                    let mut query = sqlx::query(&sql);
                    for value in values {
                        query = query.bind(value.as_deref());
                    }
                    match query
                        .execute(&mut *tx)
                        .await
                        .map(|result| result.rows_affected())
                    {
                        Ok(1) => deleted += 1,
                        result => {
                            tx.rollback().await.context("回滚删除失败")?;
                            return Err(rejected_delete(result).into());
                        }
                    }
                }
                tx.commit().await.context(CommitOutcomeUnknown)?;
            }
            Driver::SqlServer(client) => {
                client
                    .simple_query("BEGIN TRANSACTION")
                    .await?
                    .into_results()
                    .await?;
                for values in key_values {
                    let mut query = tiberius::Query::new(&sql);
                    for value in values {
                        query.bind(value.as_deref());
                    }
                    match query.execute(client).await.map(|result| result.total()) {
                        Ok(1) => deleted += 1,
                        result => {
                            client
                                .simple_query("IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION")
                                .await?
                                .into_results()
                                .await?;
                            return Err(rejected_delete(result).into());
                        }
                    }
                }
                client
                    .simple_query("COMMIT TRANSACTION")
                    .await
                    .context(CommitOutcomeUnknown)?
                    .into_results()
                    .await
                    .context(CommitOutcomeUnknown)?;
            }
        }
        Ok(deleted)
    }

    pub async fn insert_rows(
        &mut self,
        database: &str,
        table: &Table,
        names: &[String],
        rows: &[Vec<InsertValue>],
    ) -> Result<usize> {
        self.ensure_edit_transaction_safe().await?;
        if self.current_database().await?.as_deref() != Some(database) {
            return Err(EditRejected("当前数据库已改变，导入或新增未执行".into()).into());
        }
        self.ensure_table_writable(table).await?;
        let columns = self.table_columns(table).await?;
        let engine = self.profile.engine;
        anyhow::ensure!(!rows.is_empty(), "没有需要新增的数据");
        anyhow::ensure!(names.len() <= columns.len(), "列映射数量超过表列数");
        let prepared = rows
            .iter()
            .map(|row| insert_statement(engine, table, &columns, names, row))
            .collect::<Result<Vec<_>>>()?;
        self.ensure_sqlserver_text_inputs(table, &columns, &[], names, rows)
            .await?;
        match &mut self.driver {
            Driver::MySql(connection) => {
                let mut tx = connection.begin().await?;
                for (sql, parameters) in &prepared {
                    let mut query = sqlx::query(sql);
                    for value in parameters {
                        query = query.bind(value.as_deref());
                    }
                    if let Err(error) = query.execute(&mut *tx).await {
                        tx.rollback().await.context("回滚导入失败")?;
                        return Err(EditRejected(format!("新增失败，全部已回滚：{error}")).into());
                    }
                }
                tx.commit().await.context(CommitOutcomeUnknown)?;
            }
            Driver::Postgres(connection) => {
                let mut tx = connection.begin().await?;
                for (sql, parameters) in &prepared {
                    let mut query = sqlx::query(sql);
                    for value in parameters {
                        query = query.bind(value.as_deref());
                    }
                    if let Err(error) = query.execute(&mut *tx).await {
                        tx.rollback().await.context("回滚导入失败")?;
                        return Err(EditRejected(format!("新增失败，全部已回滚：{error}")).into());
                    }
                }
                tx.commit().await.context(CommitOutcomeUnknown)?;
            }
            Driver::SqlServer(client) => {
                client
                    .simple_query("BEGIN TRANSACTION")
                    .await?
                    .into_results()
                    .await?;
                for (sql, parameters) in &prepared {
                    let mut query = tiberius::Query::new(sql);
                    for value in parameters {
                        query.bind(value.as_deref());
                    }
                    if let Err(error) = query.execute(client).await {
                        client
                            .simple_query("IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION")
                            .await?
                            .into_results()
                            .await?;
                        return Err(EditRejected(format!("新增失败，全部已回滚：{error}")).into());
                    }
                }
                client
                    .simple_query("COMMIT TRANSACTION")
                    .await
                    .context(CommitOutcomeUnknown)?
                    .into_results()
                    .await
                    .context(CommitOutcomeUnknown)?;
            }
        }
        Ok(prepared.len())
    }

    pub async fn update_cells(&mut self, changes: &[CellUpdate]) -> Result<usize> {
        let first = changes
            .first()
            .ok_or_else(|| EditRejected("没有待提交的修改".into()))?;
        let (updated_cells, _) = self
            .save_table_draft(&first.database, &first.table, changes, &[], &[])
            .await?;
        Ok(updated_cells)
    }

    /// Apply the complete local table draft in one transaction. Prepare every
    /// statement before beginning, and retain the original row optimistic checks.
    pub async fn save_table_draft(
        &mut self,
        database: &str,
        table: &Table,
        changes: &[CellUpdate],
        names: &[String],
        rows: &[Vec<InsertValue>],
    ) -> Result<(usize, usize)> {
        if changes.is_empty() && rows.is_empty() {
            return Err(EditRejected("没有待提交的新增或修改".into()).into());
        }
        if changes
            .iter()
            .any(|change| change.database != database || change.table != *table)
        {
            return Err(EditRejected("暂存修改与新增行的目标不一致，未执行写入".into()).into());
        }
        self.ensure_edit_transaction_safe().await?;
        if self.current_database().await?.as_deref() != Some(database) {
            return Err(EditRejected("当前数据库已改变，暂存提交未执行".into()).into());
        }
        self.ensure_table_writable(table).await?;
        let columns = self.table_columns(table).await?;
        let engine = self.profile.engine;
        if names.len() > columns.len() {
            return Err(EditRejected("新增行的列映射数量超过表列数".into()).into());
        }
        let mut prepared = Vec::new();
        if let Some(first) = changes.first() {
            let mut groups: Vec<Vec<&CellUpdate>> = Vec::new();
            for change in changes {
                if let Some(group) = groups
                    .iter_mut()
                    .find(|group| group[0].key_values == change.key_values)
                {
                    group.push(change);
                } else {
                    groups.push(vec![change]);
                }
            }
            for group in groups {
                let (sql, parameters) = update_row_statement(engine, &columns, first, &group)?;
                prepared.push((sql, parameters, true));
            }
        }
        for row in rows {
            let (sql, parameters) = insert_statement(engine, table, &columns, names, row)
                .map_err(|error| EditRejected(format!("新增草稿无效，未执行写入：{error:#}")))?;
            prepared.push((sql, parameters, false));
        }
        self.ensure_sqlserver_text_inputs(table, &columns, changes, names, rows)
            .await?;
        match &mut self.driver {
            Driver::MySql(connection) => {
                let mut tx = connection.begin().await?;
                for (sql, parameters, update) in &prepared {
                    let mut query = sqlx::query(sql);
                    for value in parameters {
                        query = query.bind(value.as_deref());
                    }
                    let result = query
                        .execute(&mut *tx)
                        .await
                        .map(|result| result.rows_affected())
                        .map_err(Into::into);
                    if let Some(error) = table_draft_statement_error(result, *update) {
                        tx.rollback().await.context("回滚表格暂存提交失败")?;
                        return Err(error);
                    }
                }
                // A failed commit has an unknown outcome; propagate its transport
                // error so the worker closes the connection and preserves drafts.
                tx.commit().await.context(CommitOutcomeUnknown)?;
            }
            Driver::Postgres(connection) => {
                let mut tx = connection.begin().await?;
                for (sql, parameters, update) in &prepared {
                    let mut query = sqlx::query(sql);
                    for value in parameters {
                        query = query.bind(value.as_deref());
                    }
                    let result = query
                        .execute(&mut *tx)
                        .await
                        .map(|result| result.rows_affected())
                        .map_err(Into::into);
                    if let Some(error) = table_draft_statement_error(result, *update) {
                        tx.rollback().await.context("回滚表格暂存提交失败")?;
                        return Err(error);
                    }
                }
                tx.commit().await.context(CommitOutcomeUnknown)?;
            }
            Driver::SqlServer(client) => {
                client
                    .simple_query("BEGIN TRANSACTION")
                    .await?
                    .into_results()
                    .await?;
                for (sql, parameters, update) in &prepared {
                    let mut query = tiberius::Query::new(sql);
                    for value in parameters {
                        query.bind(value.as_deref());
                    }
                    let result = query
                        .execute(client)
                        .await
                        .map(|result| result.total())
                        .map_err(Into::into);
                    if let Some(error) = table_draft_statement_error(result, *update) {
                        client
                            .simple_query("IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION")
                            .await?
                            .into_results()
                            .await?;
                        return Err(error);
                    }
                }
                client
                    .simple_query("COMMIT TRANSACTION")
                    .await
                    .context(CommitOutcomeUnknown)?
                    .into_results()
                    .await
                    .context(CommitOutcomeUnknown)?;
            }
        }
        Ok((changes.len(), rows.len()))
    }

    pub async fn insert_default_row(&mut self, database: &str, table: &Table) -> Result<()> {
        self.ensure_edit_transaction_safe().await?;
        if self.current_database().await?.as_deref() != Some(database) {
            return Err(
                EditRejected("当前数据库已改变，请选择此表所属数据库后再新增行".into()).into(),
            );
        }
        self.ensure_table_writable(table).await?;
        let engine = self.profile.engine;
        let name = format!(
            "{}.{}",
            engine.quote(&table.schema),
            engine.quote(&table.name)
        );
        let sql = match engine {
            Engine::MySql => format!("INSERT INTO {name} () VALUES ()"),
            Engine::Postgres | Engine::SqlServer => {
                format!("INSERT INTO {name} DEFAULT VALUES")
            }
        };
        match &mut self.driver {
            Driver::MySql(connection) => {
                sqlx::query(&sql)
                    .execute(connection)
                    .await
                    .map_err(|error| EditRejected(format!("新增默认行失败：{error}")))?;
            }
            Driver::Postgres(connection) => {
                sqlx::query(&sql)
                    .execute(connection)
                    .await
                    .map_err(|error| EditRejected(format!("新增默认行失败：{error}")))?;
            }
            Driver::SqlServer(client) => {
                let stream = client
                    .simple_query(&sql)
                    .await
                    .map_err(|error| EditRejected(format!("新增默认行失败：{error}")))?;
                stream
                    .into_results()
                    .await
                    .map_err(|error| EditRejected(format!("新增默认行失败：{error}")))?;
            }
        }
        Ok(())
    }
}
