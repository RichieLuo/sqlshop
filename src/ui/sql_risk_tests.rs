use super::sql_needs_confirmation;

#[test]
fn comments_and_literals_do_not_trigger() {
    for sql in [
        "-- DELETE\nSELECT 'DROP; INSERT'",
        "/* UPDATE */ SELECT 'DELETE'",
        "# TRUNCATE\n SELECT \"DROP\"",
        "SELECT $$DELETE; DROP$$",
        "SELECT 'it''s UPDATE'",
        "SELECT 1; /* DROP */",
        "SELECT 1 /* DROP; */",
        "SELECT 'DROP;'",
    ] {
        assert!(!sql_needs_confirmation(sql), "{sql}");
    }
}

#[test]
fn writes_ctes_batches_and_unknown_statements_confirm() {
    for sql in [
        "INSERT INTO t VALUES (1)",
        "UPDATE t SET x = 1",
        "DELETE FROM t",
        "CREATE TABLE t (x int)",
        "DROP TABLE t",
        "ALTER TABLE t ADD x int",
        "TRUNCATE t",
        "REPLACE INTO t VALUES (1)",
        "MERGE INTO t USING s ON 1=1",
        "GRANT SELECT ON t TO u",
        "REVOKE SELECT ON t FROM u",
        "WITH c AS (DELETE FROM t RETURNING *) SELECT * FROM c",
        "SELECT 1; DELETE FROM t",
        "SELECT * INTO backup FROM users",
        "SELECT 'unterminated",
        "/* unclosed",
    ] {
        assert!(sql_needs_confirmation(sql), "{sql}");
    }
}

#[test]
fn simple_reads_do_not_confirm() {
    for sql in [
        "SELECT 1",
        "  SELECT name FROM users WHERE name = 'DELETE'",
        "SHOW TABLES",
        "DESCRIBE users",
        "SELECT 1; -- trailing comment",
    ] {
        assert!(!sql_needs_confirmation(sql), "{sql}");
    }
}
