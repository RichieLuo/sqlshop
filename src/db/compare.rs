use super::Session;
use crate::{
    compare::{COMPARE_ROW_LIMIT, CompareEndpoint, TableSnapshot},
    db::TableBrowseOptions,
};
use anyhow::Result;

impl Session {
    pub async fn compare_snapshot(&mut self, endpoint: &CompareEndpoint) -> Result<TableSnapshot> {
        endpoint.validate()?;
        anyhow::ensure!(self.profile.id == endpoint.profile.id, "比较会话连接不一致");
        anyhow::ensure!(
            self.current_database().await? == endpoint.target.database,
            "比较会话数据库不一致"
        );
        let structure = self.table_structure(&endpoint.table).await?;
        let page = self
            .table_page(
                &endpoint.table,
                &TableBrowseOptions {
                    page_size: COMPARE_ROW_LIMIT,
                    ..Default::default()
                },
            )
            .await?;
        let complete = !page.has_more && !page.data.output.sets.iter().any(|s| s.truncated);
        Ok(TableSnapshot {
            endpoint: endpoint.clone(),
            data: page.data,
            structure,
            complete,
        })
    }
}

impl Session {
    pub async fn comparison_plain_index_column(
        &mut self,
        table: &crate::model::Table,
        index_name: &str,
    ) -> Result<Option<String>> {
        let super::Driver::Postgres(connection) = &mut self.driver else {
            anyhow::bail!("此目录查询只用于PostgreSQL");
        };
        Ok(sqlx::query_scalar::<_, String>(
            r#"SELECT a.attname::text FROM pg_index i
            JOIN pg_class c ON c.oid=i.indrelid JOIN pg_namespace n ON n.oid=c.relnamespace
            JOIN pg_class ic ON ic.oid=i.indexrelid JOIN pg_am am ON am.oid=ic.relam
            JOIN pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=i.indkey[0]
            JOIN pg_opclass oc ON oc.oid=i.indclass[0]
            WHERE n.nspname=$1 AND c.relname=$2 AND ic.relname=$3
            AND i.indnatts=1 AND i.indexprs IS NULL AND i.indpred IS NULL
            AND NOT i.indisunique AND NOT i.indisprimary AND NOT i.indisexclusion AND i.indisvalid AND i.indisready
            AND am.amname='btree' AND i.indoption[0]=0 AND oc.opcdefault
            AND i.indcollation[0]=a.attcollation AND ic.reltablespace=0
            AND COALESCE(array_length(ic.reloptions,1),0)=0"#,
        )
        .bind(&table.schema)
        .bind(&table.name)
        .bind(index_name)
        .fetch_optional(connection)
        .await?)
    }
}
