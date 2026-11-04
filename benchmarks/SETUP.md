# Setup and validation record

Date: 2026-11-05 (Japan time). These preparation runs are **not** included in the
performance charts.

1. A 1,000-record, 100-operation, one-repetition smoke run passed all six cases
   and full-content/reopen checks for catdb embedded, SQLite, catdb TCP,
   PostgreSQL and MySQL. Oracle initialization failed: the Free Lite image has no
   `USERS` tablespace. A dedicated disposable tablespace was added. An unnecessary
   attempt to grant access to `v_$version` also failed, leaving an already-created
   user; repeated initialization then reported ORA-01920. Initialization errors
   are now kept separate from connection-readiness retries.
2. An Oracle-only retry still contained that unnecessary view grant and failed
   with ORA-00942. The grant was removed; the public version view is readable
   without it.
3. The next Oracle-only retry reached bulk loading but failed with ORA-08177 when
   SERIALIZABLE first touched a new, deferred-segment table. Only unmeasured bulk
   loading was changed to READ COMMITTED. Measured operations remain SERIALIZABLE.
   This restriction is described in Oracle's
   [Database Development Guide](https://docs.oracle.com/en/database/oracle/oracle-database/26/adfns/database-development-guide.pdf).
4. The corrected Oracle-only smoke run passed all six cases, exact-value checks,
   missing-key/empty-range/tail checks and full comparison after server restart.

All fixtures were synthetic and used uniquely named containers/volumes. Each
attempt removed its own fixtures. No production library, SQLite source or backup
was used. No failed preparation result was converted into a performance number.
