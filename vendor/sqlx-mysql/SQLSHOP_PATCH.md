# SQLShop MySQL query event patch

Upstream: `sqlx-mysql` 0.8.6, copied from the Cargo registry package
`index.crates.io-1949cf8c6b5b557f/sqlx-mysql-0.8.6`.
The original `LICENSE-MIT`, `LICENSE-APACHE`, normalized `Cargo.toml`,
`Cargo.toml.orig` and all upstream source files are retained.

SQLShop additions:

- `src/connection/mod.rs`: public `MySqlQueryEvent`, an optional synchronous
  callback on `MySqlConnectionInner`, and `MySqlConnection::set_query_event_handler`.
- `src/connection/establish.rs`: initialize the callback to `None`.
- `src/lib.rs`: re-export `MySqlQueryEvent`.
- `src/connection/executor.rs`: emit `Metadata` after the existing executor has
  read output column definitions, before it reads result rows; emit `Completed`
  from the existing decoded OK or final result EOF packet, before the existing
  `Either::Left(MySqlQueryResult)` yield.

Both raw `COM_QUERY` execution (including multiple statements) and prepared
`COM_STMT_EXECUTE` execution receive metadata events. Only execution output is
reported: `COM_STMT_PREPARE` and `describe` metadata do not produce events.
An empty result set therefore has columns even when no `MySqlRow` is yielded.
The initial metadata EOF does not produce `Completed`; the final result EOF does.

Warning counts are the existing `OkPacket::warnings` or `EofPacket::warnings`
decoded from the server response. `info` is always empty because the upstream
OK decoder does not retain the packet's optional info text. No warning details
or server info text are invented.

The patch sends no additional SQL or protocol messages, does not retry or split
SQL, and does not change protocol decoding, query errors, row yields, affected
row accounting, connection readiness or transaction status processing. Clients
must continue to use the existing `MySqlQueryResult` for affected row totals;
event `rows_affected` is observational and must not be added a second time.

Callbacks persist until cleared or replaced, run inline during stream polling,
and must return promptly without panicking. A client may queue events in its
callback and drain them after every stream poll, including failed polls, to
preserve event order before processing that poll's row or completion.

No new tests or live database operations are included in this patch.

## Observed session status and cancellation identity

The stream now preserves `connection_id` from the original server handshake
through TLS upgrades. `server_connection_id()` exposes that immutable id for
SQLShop's separately authenticated KILL QUERY control connection. The driver
does not itself open this connection, execute KILL or disconnect the original
query session. SQLShop applies the same database TLS and SSH configuration and
checks the active execution generation after connecting/authenticating.

The stream also preserves status from OK/final EOF packets.
`observed_in_transaction()` drains existing pending responses and reads
SERVER_STATUS_IN_TRANS without SQL. ERR packets contain no transaction status;
they invalidate this observation instead of guessing whether a deadlock rolled
back the transaction. Explicit SQLShop state inspection may issue COM_PING to
obtain a fresh status-bearing OK packet. It is an explicit protocol command and
is not automatically appended to user SQL.

Protocol references: [KILL QUERY](https://dev.mysql.com/doc/refman/8.4/en/kill.html),
[OK packet](https://dev.mysql.com/doc/dev/mysql-server/latest/page_protocol_basic_ok_packet.html),
[COM_PING](https://dev.mysql.com/doc/dev/mysql-server/latest/page_protocol_com_ping.html).
The original error and row behavior remains unchanged. New session-control
paths have compilation coverage only; no live database verification was run.

## Wire database context (2026-10-14)

The handshake advertises CLIENT_SESSION_TRACK and intersects it with the actual
server capabilities. Existing raw/prepared queries, metadata observers and
warnings retain their original stream. OK decoding now accepts the negotiated
capabilities, retains actual info text, and bounds-checks the length-encoded
session-state fields. SCHEMA_CHANGED updates the observed database; only the
two observer control variables are retained from SYSTEM_VARIABLES. Other
tracked variable values are discarded and never included in Debug.

Extended 0xfe OK replacements may exceed nine bytes when they contain tracked
state. The result terminator check distinguishes them from text rows whose
0xfe length prefix indicates a field of at least 16 MiB. Both ordinary query
polling and readiness drains decode the same state.

`enable_session_context_tracking()` is opt-in, called by SQLShop only at a query
document's connection startup. It sets session_track_schema=ON and
session_track_system_variables='*' before the first user statement. SQLShop
performs one initial explicit database read and confirms it. Normal management
and table connections do not force these settings. No probing SELECT is
automatically executed after user SQL, preserving ROW_COUNT and diagnostics.

`observed_current_database()` drains pending packets only and returns the last
confirmed database, or None when absent/unobserved. An ERR packet invalidates
database confirmation, instead of inheriting an earlier successful command's
context. Disabled trackers, unsupported capability, explicit USE without a
schema acknowledgement, tracker reconfiguration or indirect CALL/EXECUTE are
conservatively unobserved until a new schema event or an explicit successful
inspection. `confirm_current_database()` accepts that explicit inspection's
value. Token recognition can conservatively mark a query with USE in a comment
or quoted value as unobserved; it never executes or rewrites SQL.

The control settings themselves are not immutable server policy: an application
must never claim that protocol observation protects against a malicious server
or arbitrary native UDFs changing internal connection state. Unknown context
must remain labelled unknown and must not trigger an automatic SQL probe.

References: [OK packet and session-state layout](https://dev.mysql.com/doc/dev/mysql-server/latest/page_protocol_basic_ok_packet.html),
[upstream session tracker implementation](https://github.com/mysql/mysql-server/blob/8.4/sql/session_tracker.cc).
No tests or live database operations were added/run for this patch.
