use super::*;

impl Session {
    pub fn raw_session_requires_confirmation(&self) -> bool {
        self.raw_query_used
    }

    pub async fn query(&mut self, sql: &str) -> Result<QueryOutput> {
        // SQLx hides MySQL server transaction status. Never implicitly commit
        // a possible transaction after arbitrary SQL has run on this connection.
        self.raw_query_used = true;
        // User SQL can change relation shapes, search_path, or issue DEALLOCATE
        // ALL. Drop driver-owned plans before that SQL runs, so subsequent
        // context probes cannot reference plans it invalidated. This closes
        // protocol statements only; it does not end the user's transaction.
        self.prepare_user_query().await?;
        self.query_internal(sql).await
    }
}
