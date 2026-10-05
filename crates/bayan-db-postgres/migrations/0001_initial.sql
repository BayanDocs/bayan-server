-- Initial schema of the BayanDocs server metadata database (PostgreSQL).
-- Keep in step with crates/bayan-db-sqlite/migrations; never edit a migration after it has been released, add a new one.
--
-- server_instance has exactly one row and records when this database was initialized. The readiness check reads it to prove the database is reachable and migrated. It contains no user or document data.
CREATE TABLE server_instance (
    id SMALLINT PRIMARY KEY CHECK (id = 1),
    initialized_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO server_instance (id) VALUES (1);
