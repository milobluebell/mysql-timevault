# Local backup and restore test (2026-10-15)

## Scope

- Platform: Linux/aarch64 Docker on macOS; source and restore target: MySQL 8.4.11.
- Fixture: `tests/fixtures/relational_load.sql` on a disposable `app` database.
- Initial data: 10 InnoDB tables, 10 foreign keys, 63,400 rows. Point-in-time tests added two orders and two order items, so the final source has 63,404 rows.

## Results

| Check | Result |
| --- | --- |
| Latest full backup | 0.52 s wall time; 1,104,359-byte compressed SQL snapshot; about 5.3 MiB of copied binary log |
| Full restore to an independent empty instance | 0.80 s wall time; all 10 exact table counts and all 10 foreign keys match the source |
| Data comparison | SHA-256 of ordered, non-extended, data-only `mysqldump` output matches between source and restored instance |
| Point-in-time restore | Restored order/item 8001/24001; excluded later order/item 8002/24002 |
| Retention and metadata | Three successful backup versions retained; older versions marked `pruned`; 33 error-log lines and one slow query archived |

The latest backup ID is `20261015T051226Z-eff95a76`. The backup snapshot passed `gzip -t` and its checksum is recorded in its manifest.

## Limits

This is a small, single-host functional test. MySQL 5.7 and 8.0, large datasets, concurrent schema changes, long-running operation, host failure, and off-host recovery were not tested here. A production rollout still needs a restore drill against representative data and a separate copy of the backup storage.
