# Query protocol event patches

SQLShop uses the following local crates through `[patch.crates-io]` to retain
empty result metadata and server messages without changing or replaying SQL.

| Crate | Upstream version | Original crates.io package checksum |
| --- | --- | --- |
| sqlx-mysql | 0.8.6 | aa003f0038df784eb8fecbbac13affe3da23b45194bd57dba231c8f48199c526 |
| sqlx-postgres | 0.8.6 | db58fcd5a53cf07c184b154801ff91347e4c30d17a3562a635ff028ad5deda46 |
| tiberius | 0.12.3 | a1446cb4198848d1562301a3340424b4f425ef79f35ef9ee034769a9dd92c10d |

The source packages were copied from the local Cargo registry. These checksums
identify the original upstream packages, not the modified local directories.
Each crate retains its original MIT and Apache 2.0 licenses and contains a
`SQLSHOP_PATCH.md` describing the modified source and behavioral limits.

Keep the patches when upgrading dependencies, or port their APIs to the new
versions. Removing them loses the metadata/messages integration. Root Cargo
check, Clippy and build cover the configured features; no database tests were
run for these added events in this batch. Refer to the rebuild plan for the
remaining runtime verification scope.
