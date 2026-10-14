# SQLShop PostgreSQL protocol observer

Upstream: `sqlx-postgres` 0.8.6, copied from the published Cargo registry
package. `Cargo.toml`, `Cargo.toml.orig`, `.cargo_vcs_info.json`, and both
upstream license files are retained. Upstream repository:
https://github.com/launchbadge/sqlx

SQLShop adds `PgQueryEvent` and
`PgConnection::set_query_event_handler(Option<Arc<dyn Fn(PgQueryEvent) + Send + Sync>>)`,
an optional synchronous observer for:

- `Metadata(Vec<PgColumn>)`: emitted after a simple-query `RowDescription` is
  decoded and converted by the existing driver. This includes result sets
  containing zero rows. Extended-query prepared metadata is unchanged.
- `Notice(PgDatabaseError)`: the original owned `NoticeResponse`, moved into the
  existing public PostgreSQL error wrapper after upstream logging. Severity,
  SQLSTATE, primary message, detail, hint, cursor position, and other server
  fields remain accessible through its existing methods.

The handler defaults to `None`. This patch does not split SQL, prepare or
re-execute SQL, add SQL commands, change protocol completion, retry queries, or
change transaction handling. Notices retain the upstream logging and continue
being consumed at the same point in `PgStream::recv`. Metadata is observed
before the existing `DataRow` / `CommandComplete` processing, so consumers can
associate it with results while polling the existing `fetch_many` stream.

Handlers must not block; consumers should enqueue the owned events and drain
them while processing query stream items. Consumers should install and remove
the handler around the operation they intend to observe.

## Native query control

`PgConnection::cancel_key()` exposes the original backend PID/secret exclusively
for in-memory cancellation; neither value is logged or persisted by SQLShop.
`cancel_request_if_active(options, key, active)` opens a separate transport using
the driver's existing TLS handshake, checks the execution generation after TLS,
sends the native 16-byte CancelRequest, and waits for server EOF. It sends no
StartupMessage or authentication SQL. The original query response, not the
control connection, proves whether cancellation succeeded. The worker must keep
the original session occupied until this control operation finishes.

`observed_transaction_status()` drains pending ReadyForQuery after query errors
and returns its actual I/T/E status. It sends no probing SQL, commits, rollback,
or retries. See PostgreSQL's [message flow](https://www.postgresql.org/docs/17/protocol-flow.html),
including cancellation and SSLRequest on the cancellation transport.

These optional query-control APIs were compiled with SQLShop's configured
features. No new tests or live database operations were run; protocol
cancellation, proxies and server TLS combinations still require integration
verification.

## Application-controlled server payload logging

SQLShop disables server payload logging per connection, while preserving
query events and response delivery. Server messages or trigger output can
contain values from account/password DDL, so disabling only the statement
logger is insufficient. The optional setter defaults to upstream logging
for other callers. PostgreSQL gates NOTICE logging; TDS gates ERROR, INFO,
metadata, rows and return values. SQLShop also disables SQLx regular/slow
statement logging. TDS AuthMethod Debug redacts Entra token contents.
No tests or live credential/account operations were executed for this change.
