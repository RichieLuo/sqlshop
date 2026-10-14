# SQLShop local Tiberius patch

Base: crates.io `tiberius` 0.12.3, copied from Cargo's local registry source.
The upstream MIT and Apache 2.0 license files are retained unchanged.

## Added API

`Client::simple_query_with_events(sql)` returns `QueryEventStream`, a
`Stream<Item = tiberius::Result<QueryEvent>>`:

- `Metadata(ResultMetadata)` — emitted as soon as COLMETADATA is decoded,
  including result sets that contain no rows.
- `Row(Row)` — rows retain the original result index and column metadata.
- `Info { number, state, class, message, server, procedure, line }` — the actual
  TDS INFO token, including PRINT and informational RAISERROR messages.
- `Done { kind, rows_affected, more, is_error }` — one event for each completion
  token. `DoneKind::{Statement, Procedure, InProcedure}` distinguishes DONE,
  DONEPROC and DONEINPROC.

`rows_affected` is `Some(count)` only when DONE_COUNT is set. An absent count is
not zero. The protocol's count can describe SELECT rows as well as modified
rows. DONEPROC can summarize procedure work already reported by DONEINPROC;
consumers must not add every token's count together indiscriminately.

## Ordering and behavior

This method uses the original BatchRequest and TokenStream. It sends the SQL
batch once, without additional SQL, SQL splitting, callbacks or another socket.
It does not call `forward_to_metadata`: messages and completion tokens before
the first result remain visible. Each `try_next` returns exactly the next
metadata, row, INFO or completion event in wire order. Protocol tokens unrelated
to these events are processed by the original decoder.

The existing `simple_query`, `query`, `execute`, login, transaction descriptor
updates and token decode behavior are unchanged. In particular, TokenStream
keeps the first server error while draining and returns that error at EOF. The
new stream follows that policy, allowing consumers to keep earlier successful
results and informational messages. Transport errors are returned immediately.
Consumers must drain the stream before sending another request, just as with
the upstream query stream.

## Modified source

- `src/client.rs`: add the optional batch event stream API.
- `src/tds/stream/query.rs`: add the ordered public event stream and event types.
- `src/tds/codec/token/token_done.rs`: expose crate-private access to COUNT,
  MORE, ERROR and SRVERROR status flags for the new stream.
- `src/lib.rs`: re-export the public event API.

No database queries or tests were run for this patch. Compilation is checked
by the root SQLShop build with its configured dependency features; compilation does not prove compatibility
with every SQL Server response sequence or stored procedure.

Completion flags and token distinctions follow Microsoft's
[DONE](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-tds/3c06f110-98bd-4d5b-b836-b1ba66452cb7),
[DONEPROC](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-tds/65e24140-edea-46e5-b710-209af2016195)
and [DONEINPROC](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-tds/43e891c5-f7a1-432f-8f9f-233c4cd96afb)
definitions. `is_error` covers both DONE_ERROR and DONE_SRVERROR; a consumer
must not present a result completed with this flag as a successful result.

## ATTENTION cancellation

`Client::attention_handle()` installs a fresh per-execution handle; replacing it
disarms the previous generation. `AttentionHandle::request()` wakes the original
query stream. While it is polled, Connection sends an empty ATTENTION packet
through the existing Framed transport, including its existing TLS encryption,
and continues decoding/discarding buffered query responses until DONE_ATTN.
ENVCHANGE updates remain applied. An ordinary end-of-message before the
acknowledgement cannot end this drain. Only after acknowledgement can the query
return `Error::Cancelled` and the session be reused; an earlier actual server
error remains preserved instead of being overwritten.

`disarm()` clears unsent requests when that execution finishes. Once ATTENTION
is transmitted, disarming cannot bypass the required acknowledgement drain.
This implementation does not send SQL Server KILL, close the original session,
inject raw bytes into an encrypted socket, or retry interrupted SQL.

`Client::has_server_transaction()` reads the existing ENVCHANGE transaction
descriptor without SQL. This proves transaction presence, not committability;
SQLShop's explicit inspection operation uses XACT_STATE for the latter.

The token drain follows Microsoft's [out-of-band attention example](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-tds/f8aa004e-bf76-4f94-bd5b-39597a888146).
ATTENTION paths have compilation coverage only; no tests or live SQL Server
queries were run. A nonresponsive server requires the worker to preserve an
unknown outcome instead of treating a local timeout as acknowledgement.

## Wire database context (2026-10-14)

The existing decoded ENVCHANGE Database(new, old) now updates Context's current
database, including login and batch changes. `Client::observed_current_database`
returns this original protocol value without SELECT DB_NAME(). ENVCHANGE stays
processed while draining ATTENTION and while preserving server errors. The
existing transaction descriptor and optional server-message logging flag are
retained. This avoids overwriting @@ROWCOUNT merely to refresh the tab's target.
Default schema has no equivalent database ENVCHANGE token; SQLShop keeps its
binding and only refreshes it after an explicitly requested context inspection.

The TDS database token decoder reads its first string as the new database; the
observer follows the decoded field ordering, not the old Display formatter's
labels. No SQL is parsed, replayed, or inserted by this observer. Compilation
and static review do not establish coverage of every server/proxy sequence;
no tests or live database operations were run.

## Application-controlled server payload logging

SQLShop disables server payload logging per connection, while preserving
query events and response delivery. Server messages or trigger output can
contain values from account/password DDL, so disabling only the statement
logger is insufficient. The optional setter defaults to upstream logging
for other callers. PostgreSQL gates NOTICE logging; TDS gates ERROR, INFO,
metadata, rows and return values. SQLShop also disables SQLx regular/slow
statement logging. TDS AuthMethod Debug redacts Entra token contents.
No tests or live credential/account operations were executed for this change.
