use super::{EditRejected, Session, single_statement_keyword};
use crate::{
    diagnostics::DiagnosticKind,
    model::{Engine, QueryMessage, QueryOutput},
};
use anyhow::Result;

impl Session {
    /// Reads the server's visible diagnostic catalogs. Permission/extension
    /// failures remain errors; they are never converted into empty statistics.
    pub async fn diagnostics(&mut self, kind: DiagnosticKind) -> Result<QueryOutput> {
        let sql = match (self.profile.engine, kind) {
            (Engine::MySql, DiagnosticKind::Activity) => MYSQL_ACTIVITY.to_owned(),
            (Engine::MySql, DiagnosticKind::Locks) => {
                let probe = self.query_internal("SELECT CAST(COUNT(*) AS CHAR) FROM information_schema.TABLES WHERE TABLE_SCHEMA='performance_schema' AND TABLE_NAME='data_locks'").await?;
                if first_cell(&probe) == Some("1") {
                    MYSQL_LOCKS_8
                } else {
                    MYSQL_LOCKS_57
                }
                .to_owned()
            }
            (Engine::MySql, DiagnosticKind::SlowQueries) => MYSQL_STATISTICS.to_owned(),
            (Engine::Postgres, DiagnosticKind::Activity) => PG_ACTIVITY.to_owned(),
            (Engine::Postgres, DiagnosticKind::Locks) => PG_LOCKS.to_owned(),
            (Engine::Postgres, DiagnosticKind::SlowQueries) => {
                let extension = self.query_internal("SELECT n.nspname::text FROM pg_extension e JOIN pg_namespace n ON n.oid=e.extnamespace WHERE e.extname='pg_stat_statements'").await?;
                let schema = first_cell(&extension).ok_or_else(|| EditRejected("当前数据库未安装 pg_stat_statements；查询统计不可用。请由管理员配置扩展后刷新。".into()))?;
                let attributes = self.parameter_query("SELECT a.attname::text FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname='pg_stat_statements' AND a.attnum>0 AND NOT a.attisdropped", &[schema]).await?;
                let names: Vec<&str> = attributes
                    .sets
                    .iter()
                    .flat_map(|set| &set.rows)
                    .filter_map(|row| row.first().and_then(Option::as_deref))
                    .collect();
                let total_time = if names.contains(&"total_exec_time") {
                    "total_exec_time"
                } else if names.contains(&"total_time") {
                    "total_time"
                } else {
                    return Err(EditRejected(
                        "pg_stat_statements 未提供受支持的执行时间字段。".into(),
                    )
                    .into());
                };
                format!(
                    "SELECT userid::text AS user_id,queryid::text AS query_id,calls::text, {total_time}::text AS total_execution_ms,({total_time}/NULLIF(calls,0))::text AS mean_execution_ms,rows::text,query::text FROM {}.pg_stat_statements WHERE dbid=(SELECT oid FROM pg_database WHERE datname=current_database()) ORDER BY {total_time} DESC LIMIT 1001",
                    Engine::Postgres.quote(schema)
                )
            }
            (Engine::SqlServer, DiagnosticKind::Activity) => MSSQL_ACTIVITY.to_owned(),
            (Engine::SqlServer, DiagnosticKind::Locks) => MSSQL_LOCKS.to_owned(),
            (Engine::SqlServer, DiagnosticKind::SlowQueries) => MSSQL_STATISTICS.to_owned(),
        };
        let mut output = self.query_internal(&sql).await?;
        output.messages.push(QueryMessage {
            severity: "目录说明".into(),
            text: match kind {
                DiagnosticKind::Activity => "显示服务器向当前账号公开的会话快照；其他账号的 SQL 与状态可能不可见。",
                DiagnosticKind::Locks => "显示服务器向当前账号公开的锁与等待快照；锁状态可能在刷新之间变化。",
                DiagnosticKind::SlowQueries => "显示服务器累计查询统计，按累计执行时间排序；统计会受服务器重启、缓存逐出或管理员重置影响，不代表完整慢查询日志。",
            }.into(),
            ..Default::default()
        });
        Ok(output)
    }

    /// Estimates a single SELECT/CTE plan. This never enables ANALYZE or runs
    /// the query with result collection. Callers use an expendable diagnostic
    /// connection, especially for SQL Server's connection-scoped SHOWPLAN flag.
    pub async fn explain_query(&mut self, sql: &str) -> Result<QueryOutput> {
        validate_plan_sql(sql, self.profile.engine)?;
        match self.profile.engine {
            Engine::MySql => {
                self.query_internal(&format!("EXPLAIN FORMAT=JSON {sql}"))
                    .await
            }
            Engine::Postgres => {
                self.query_internal(&format!(
                    "EXPLAIN (FORMAT JSON, VERBOSE TRUE, COSTS TRUE) {sql}"
                ))
                .await
            }
            Engine::SqlServer => {
                // Each SET is the sole statement of its own batch. Consume the
                // entire response before advancing; query_internal drains it.
                if let Err(error) = self.query_internal("SET SHOWPLAN_XML ON").await {
                    return match self.query_internal("SET SHOWPLAN_XML OFF").await {
                        Ok(_) => Err(error),
                        Err(off_error) => {
                            Err(off_error.context("关闭 SHOWPLAN_XML 失败；此诊断连接不可复用"))
                        }
                    };
                }
                let plan = self.query_internal(sql).await;
                let off = self.query_internal("SET SHOWPLAN_XML OFF").await;
                match off {
                    Ok(_) => plan,
                    Err(error) => Err(error.context("关闭 SHOWPLAN_XML 失败；此诊断连接不可复用")),
                }
            }
        }
    }
}

fn first_cell(output: &QueryOutput) -> Option<&str> {
    output.sets.first()?.rows.first()?.first()?.as_deref()
}

fn validate_plan_sql(sql: &str, engine: Engine) -> Result<()> {
    let rejected = || {
        EditRejected("执行计划仅接受一个 SELECT 或只读 WITH 查询；请移除批次指令、写入语句与 SELECT INTO。此操作不启用 ANALYZE。".into())
    };
    if !single_statement_keyword(sql, engine).is_some_and(|word| {
        word.eq_ignore_ascii_case("SELECT") || word.eq_ignore_ascii_case("WITH")
    }) {
        return Err(rejected().into());
    }
    // Conservative lexical gate after the editor lexer validates quotes,
    // comments and single-statement boundaries. Reserved write/control words
    // outside quotes are refused, including writable CTEs and SELECT INTO.
    let mut chars = sql.char_indices().peekable();
    let mut depth = 0usize;
    let mut main_select = false;
    let mut set_select = false;
    while let Some((index, ch)) = chars.next() {
        if ch == '\''
            || ch == '"'
            || (engine == Engine::MySql && ch == '`')
            || (engine == Engine::SqlServer && ch == '[')
        {
            let end = if ch == '[' { ']' } else { ch };
            let backslash_escape = engine == Engine::Postgres
                && ch == '\''
                && index > 0
                && matches!(sql.as_bytes()[index - 1], b'e' | b'E');
            while let Some((_, next)) = chars.next() {
                if next == end {
                    if chars.peek().is_some_and(|(_, next)| *next == end) {
                        chars.next();
                    } else {
                        break;
                    }
                } else if next == '\\' && backslash_escape {
                    chars.next();
                }
            }
        } else if (sql[index..].starts_with("--")
            && (engine != Engine::MySql
                || sql
                    .as_bytes()
                    .get(index + 2)
                    .is_none_or(|byte| byte.is_ascii_whitespace() || *byte < 32)))
            || (engine == Engine::MySql && ch == '#')
        {
            for (_, next) in chars.by_ref() {
                if next == '\n' || next == '\r' {
                    break;
                }
            }
        } else if sql[index..].starts_with("/*") {
            chars.next();
            let mut comments = 1usize;
            while let Some((_, next)) = chars.next() {
                if next == '/'
                    && engine != Engine::MySql
                    && chars.peek().is_some_and(|(_, next)| *next == '*')
                {
                    chars.next();
                    comments += 1;
                } else if next == '*' && chars.peek().is_some_and(|(_, next)| *next == '/') {
                    chars.next();
                    comments -= 1;
                    if comments == 0 {
                        break;
                    }
                }
            }
        } else if ch == '$' {
            return Err(rejected().into()); // unbound parameters/dollar bodies are outside this plan UI's scope
        } else if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth = depth.saturating_sub(1);
        } else if ch.is_alphabetic() || ch == '_' {
            let mut end = index + ch.len_utf8();
            while let Some((next_index, next)) = chars.peek().copied() {
                if next.is_alphanumeric() || next == '_' {
                    chars.next();
                    end = next_index + next.len_utf8();
                } else {
                    break;
                }
            }
            let word = sql[index..end].to_ascii_uppercase();
            if [
                "INSERT",
                "UPDATE",
                "DELETE",
                "MERGE",
                "INTO",
                "CREATE",
                "ALTER",
                "DROP",
                "TRUNCATE",
                "EXEC",
                "EXECUTE",
                "CALL",
                "DO",
                "COPY",
                "GRANT",
                "REVOKE",
                "SET",
                "KILL",
                "LOCK",
                "ANALYZE",
                "PRINT",
                "RAISERROR",
                "THROW",
                "DECLARE",
                "BEGIN",
                "WAITFOR",
                "BACKUP",
                "RESTORE",
                "DBCC",
                "USE",
                "RECONFIGURE",
                "SHUTDOWN",
                "LOAD",
                "REPLACE",
                "HANDLER",
                "PREPARE",
                "DEALLOCATE",
                "EXPLAIN",
                "PROCEDURE",
            ]
            .contains(&word.as_str())
            {
                return Err(rejected().into());
            }
            if depth == 0 {
                if word == "SELECT" {
                    if main_select && !set_select {
                        return Err(rejected().into());
                    }
                    main_select = true;
                    set_select = false;
                } else if ["UNION", "INTERSECT", "EXCEPT"].contains(&word.as_str()) {
                    set_select = true;
                } else if !["ALL", "DISTINCT"].contains(&word.as_str()) {
                    set_select = false;
                }
            }
        }
    }
    if !main_select {
        return Err(rejected().into());
    }
    Ok(())
}

const MYSQL_ACTIVITY: &str = r#"SELECT CAST(ID AS CHAR) AS session_id,CAST(USER AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS user_name,
    CAST(HOST AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS host,CAST(DB AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS database_name,
    CAST(COMMAND AS CHAR) AS command,CAST(TIME AS CHAR) AS elapsed_seconds,CAST(STATE AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS state,
    CAST(INFO AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS query_text FROM information_schema.PROCESSLIST
    WHERE DB=DATABASE() OR ID=CONNECTION_ID() ORDER BY TIME DESC,ID LIMIT 1001"#;

const MYSQL_LOCKS_8: &str = r#"SELECT CAST(t.PROCESSLIST_ID AS CHAR) AS session_id,CAST(l.ENGINE AS CHAR) AS engine,
    CAST(l.OBJECT_SCHEMA AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS schema_name,CAST(l.OBJECT_NAME AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS object_name,
    CAST(l.INDEX_NAME AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS index_name,CAST(l.LOCK_TYPE AS CHAR) AS lock_type,
    CAST(l.LOCK_MODE AS CHAR) AS lock_mode,CAST(l.LOCK_STATUS AS CHAR) AS lock_status,
    CAST(l.ENGINE_TRANSACTION_ID AS CHAR) AS transaction_id,CAST(w.BLOCKING_ENGINE_TRANSACTION_ID AS CHAR) AS blocking_transaction_id
    FROM performance_schema.data_locks l LEFT JOIN performance_schema.threads t ON t.THREAD_ID=l.THREAD_ID
    LEFT JOIN performance_schema.data_lock_waits w ON w.REQUESTING_ENGINE_LOCK_ID=l.ENGINE_LOCK_ID AND w.ENGINE=l.ENGINE
    WHERE l.OBJECT_SCHEMA=DATABASE() ORDER BY l.LOCK_STATUS,l.OBJECT_NAME LIMIT 1001"#;

const MYSQL_LOCKS_57: &str = r#"SELECT CAST(l.lock_trx_id AS CHAR) AS transaction_id,CAST(l.lock_mode AS CHAR) AS lock_mode,
    CAST(l.lock_type AS CHAR) AS lock_type,CAST(l.lock_table AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS object_name,
    CAST(l.lock_index AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS index_name,CAST(w.blocking_trx_id AS CHAR) AS blocking_transaction_id
    FROM information_schema.INNODB_LOCKS l LEFT JOIN information_schema.INNODB_LOCK_WAITS w ON w.requested_lock_id=l.lock_id
    ORDER BY l.lock_table,l.lock_id LIMIT 1001"#;

const MYSQL_STATISTICS: &str = r#"SELECT CAST(SCHEMA_NAME AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS database_name,
    CAST(DIGEST AS CHAR) AS query_digest,CAST(COUNT_STAR AS CHAR) AS calls,CAST(SUM_TIMER_WAIT/1000000000 AS CHAR) AS total_execution_ms,
    CAST(AVG_TIMER_WAIT/1000000000 AS CHAR) AS mean_execution_ms,CAST(SUM_ROWS_SENT AS CHAR) AS rows_sent,
    CAST(FIRST_SEEN AS CHAR) AS first_seen,CAST(LAST_SEEN AS CHAR) AS last_seen,
    CAST(DIGEST_TEXT AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_unicode_ci AS normalized_query
    FROM performance_schema.events_statements_summary_by_digest WHERE SCHEMA_NAME=DATABASE() ORDER BY SUM_TIMER_WAIT DESC LIMIT 1001"#;

const PG_ACTIVITY: &str = r#"SELECT pid::text AS session_id,usename::text AS user_name,application_name::text,client_addr::text,
    state::text,wait_event_type::text,wait_event::text,query_start::text,xact_start::text,
    EXTRACT(EPOCH FROM (clock_timestamp()-query_start))::text AS elapsed_seconds,query::text
    FROM pg_stat_activity WHERE datname=current_database() ORDER BY query_start NULLS LAST,pid LIMIT 1001"#;

const PG_LOCKS: &str = r#"SELECT l.pid::text AS session_id,l.locktype::text,n.nspname::text AS schema_name,c.relname::text AS object_name,
    l.mode::text,l.granted::text,l.relation::text AS relation_oid,l.transactionid::text,l.virtualxid::text,
    array_to_string(pg_blocking_pids(l.pid),',')::text AS blocking_session_ids,a.wait_event_type::text,a.wait_event::text
    FROM pg_locks l LEFT JOIN pg_class c ON c.oid=l.relation LEFT JOIN pg_namespace n ON n.oid=c.relnamespace
    LEFT JOIN pg_stat_activity a ON a.pid=l.pid WHERE l.database=(SELECT oid FROM pg_database WHERE datname=current_database())
    OR a.datname=current_database() ORDER BY l.granted,l.pid LIMIT 1001"#;

const MSSQL_ACTIVITY: &str = r#"SELECT TOP (1001) CONVERT(nvarchar(20),s.session_id) AS session_id,s.login_name,s.host_name,s.program_name,
    s.status AS session_status,r.status AS request_status,r.command,CONVERT(nvarchar(30),r.total_elapsed_time) AS elapsed_ms,
    r.wait_type,CONVERT(nvarchar(20),r.blocking_session_id) AS blocking_session_id,CONVERT(nvarchar(20),r.database_id) AS database_id,t.text AS query_text
    FROM sys.dm_exec_sessions s LEFT JOIN sys.dm_exec_requests r ON r.session_id=s.session_id OUTER APPLY sys.dm_exec_sql_text(r.sql_handle) t
    WHERE s.is_user_process=1 AND (r.database_id=DB_ID() OR s.database_id=DB_ID()) ORDER BY r.total_elapsed_time DESC,s.session_id"#;

const MSSQL_LOCKS: &str = r#"SELECT TOP (1001) CONVERT(nvarchar(20),l.request_session_id) AS session_id,l.resource_type,l.resource_subtype,
    CONVERT(nvarchar(30),l.resource_associated_entity_id) AS entity_id,l.resource_description,l.request_mode,l.request_status,
    l.request_owner_type,CONVERT(nvarchar(30),l.request_owner_id) AS owner_id,CONVERT(nvarchar(20),w.blocking_session_id) AS blocking_session_id,w.wait_type
    FROM sys.dm_tran_locks l LEFT JOIN sys.dm_os_waiting_tasks w ON w.resource_address=l.lock_owner_address
    WHERE l.resource_database_id=DB_ID() ORDER BY l.request_status,l.request_session_id"#;

const MSSQL_STATISTICS: &str = r#"SELECT TOP (1001) CONVERT(nvarchar(30),q.execution_count) AS calls,
    CONVERT(nvarchar(100),q.total_elapsed_time/1000.0) AS total_execution_ms,
    CONVERT(nvarchar(100),q.total_elapsed_time/NULLIF(q.execution_count,0)/1000.0) AS mean_execution_ms,
    CONVERT(nvarchar(100),q.total_worker_time/1000.0) AS total_cpu_ms,CONVERT(nvarchar(100),q.total_logical_reads) AS logical_reads,
    CONVERT(nvarchar(33),q.creation_time,126) AS statistics_since,CONVERT(nvarchar(33),q.last_execution_time,126) AS last_execution_at,
    SUBSTRING(t.text,q.statement_start_offset/2+1,(CASE WHEN q.statement_end_offset=-1 THEN DATALENGTH(t.text) ELSE q.statement_end_offset END-q.statement_start_offset)/2+1) AS query_text
    FROM sys.dm_exec_query_stats q CROSS APPLY sys.dm_exec_sql_text(q.sql_handle) t
    WHERE t.dbid=DB_ID() ORDER BY q.total_elapsed_time DESC"#;
