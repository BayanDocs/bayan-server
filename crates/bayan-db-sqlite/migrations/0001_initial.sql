-- Initial schema of the BayanDocs server metadata database (SQLite).
-- Keep in step with crates/bayan-db-postgres/migrations; never edit a migration after it has been released, add a new one.
--
-- server_instance has exactly one row and records when this database was initialized. The readiness check reads it to prove the database is reachable and migrated. It contains no user or document data.
CREATE TABLE server_instance (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    initialized_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
) STRICT;

INSERT INTO server_instance (id) VALUES (1);
