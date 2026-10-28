//! The two database backends must stay in step (finding M11 of the architecture review of 2026-10-27). Every schema change is written twice, once for SQLite (`crates/bayan-db-sqlite/migrations`) and once for PostgreSQL (`crates/bayan-db-postgres/migrations`); these tests fail when one side changes alone.
//!
//! - `both_backends_have_the_same_migrations` compares the migrations' versions, descriptions and kinds. It needs no database.
//! - `both_backends_create_the_same_schema` migrates a fresh SQLite database and a fresh PostgreSQL schema and compares what they contain: the same tables; the same columns, with the same nullability and corresponding types (see [`TYPE_MAPPING`]); the same primary keys; and the same indexes. It needs PostgreSQL, so it is ignored in normal test runs; `cargo xtask test-postgres` runs it with `BAYAN_TEST_POSTGRES_URL` set (CI does this against a PostgreSQL service container).
//!
//! Column defaults, `CHECK` constraints and foreign keys are not compared, nor are an index's `WHERE` condition (only whether it has one), its sort order and its collations: like defaults and `CHECK` expressions, they are written in each database's own dialect.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::TempDir;
use sqlx::AssertSqlSafe;

/// How the two databases' column types correspond: a PostgreSQL type, as PostgreSQL's `format_type` writes it, and the SQLite type of the same column.
///
/// SQLite has few types (a STRICT table allows only INT or INTEGER, REAL, TEXT, BLOB and ANY) and PostgreSQL many, so several PostgreSQL types share one SQLite type. SQLite types are compared as declared, in any letter case: write `INTEGER`, not `INT`, which SQLite treats differently in a primary key. A PostgreSQL type missing here fails the test; add it, with the SQLite type that stores its values, in the pull request whose migration first uses it.
const TYPE_MAPPING: [(&str, &str); 8] = [
    // SQLite's INTEGER holds any 64-bit signed integer, so every PostgreSQL integer type fits.
    ("smallint", "INTEGER"),
    ("integer", "INTEGER"),
    ("bigint", "INTEGER"),
    // SQLite has no boolean type; sqlx stores a `bool` as 0 or 1.
    ("boolean", "INTEGER"),
    ("double precision", "REAL"),
    ("text", "TEXT"),
    // A point in time; in SQLite, text in ISO 8601 format in UTC, such as `2026-10-28T09:30:00Z`.
    ("timestamp with time zone", "TEXT"),
    ("bytea", "BLOB"),
];

/// The table where sqlx records which migrations it has applied. It is sqlx's, not the server's, and its columns differ between the backends, so it is left out of the comparison.
const MIGRATIONS_TABLE: &str = "_sqlx_migrations";

/// What an expression in an index is compared as: both databases describe such a key only as an expression, in their own syntax.
const EXPRESSION: &str = "(expression)";

/// A column, as the test compares it.
#[derive(Debug, PartialEq, Eq)]
struct Column {
    /// The SQLite type in upper case; for a PostgreSQL column, the SQLite type that [`TYPE_MAPPING`] gives for its type.
    sqlite_type: String,
    /// Whether the column refuses NULL. PostgreSQL makes every primary-key column NOT NULL by itself; SQLite does not for an `INTEGER PRIMARY KEY` or in a table that is not STRICT, so write NOT NULL on every SQLite primary-key column.
    not_null: bool,
}

/// An index, as the test compares it. The primary key's index is compared as [`Table::primary_key`] instead, because SQLite has none for an `INTEGER PRIMARY KEY`.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Index {
    /// The name given in `CREATE INDEX`; `None` for the index of a `UNIQUE` constraint, which each database names itself.
    name: Option<String>,
    /// The indexed columns in order, an expression as [`EXPRESSION`].
    columns: Vec<String>,
    unique: bool,
    /// Whether the index has a `WHERE` clause, which limits it to the rows the clause selects. The condition itself is not compared.
    partial: bool,
}

/// A table, as the test compares it.
#[derive(Debug, Default, PartialEq, Eq)]
struct Table {
    /// Every column, by name.
    columns: BTreeMap<String, Column>,
    /// The primary key's columns in key order, empty if the table has none.
    primary_key: Vec<String>,
    indexes: BTreeSet<Index>,
}

/// Every table the migrations create, by name.
type Schema = BTreeMap<String, Table>;

#[test]
fn both_backends_have_the_same_migrations() {
    let describe = |migrator: &'static sqlx::migrate::Migrator| {
        migrator
            .iter()
            .map(|migration| {
                (
                    migration.version,
                    migration.description.to_string(),
                    migration.migration_type,
                )
            })
            .collect::<Vec<_>>()
    };
    let sqlite = describe(&bayan_db_sqlite::MIGRATOR);
    assert!(!sqlite.is_empty(), "the SQLite backend has migrations");
    assert_eq!(
        sqlite,
        describe(&bayan_db_postgres::MIGRATOR),
        "migrations (version, description, kind): SQLite left, PostgreSQL right"
    );
}

#[tokio::test]
#[ignore = "needs PostgreSQL; run `cargo xtask test-postgres` with BAYAN_TEST_POSTGRES_URL set"]
async fn both_backends_create_the_same_schema() {
    let url = std::env::var("BAYAN_TEST_POSTGRES_URL")
        .expect("BAYAN_TEST_POSTGRES_URL must name a PostgreSQL database owned by its user");

    let data = TempDir::new("migrations");
    let sqlite_pool = bayan_db_sqlite::connect(&data.path().join("schema.sqlite3"), 1)
        .await
        .expect("create a SQLite database");
    bayan_db_sqlite::migrate(&sqlite_pool)
        .await
        .expect("migrate the SQLite database");
    let sqlite = sqlite_schema(&sqlite_pool).await;
    sqlite_pool.close().await;

    // A schema of its own stands in for a fresh database and leaves the database's other contents alone. It equals a fresh database only while no migration creates an extension or a role, or names a schema, and no `sqlx.toml` sets `create-schemas` or a schema for the migrations table, because all of these reach beyond the schema; a migration that needs any of them must switch this test to a fresh database. The schema's name holds only the prefix and digits, so it needs no quoting.
    let schema = format!("bayan_test_migrations_{}", std::process::id());
    let admin = bayan_db_postgres::connect(&url, 1)
        .await
        .expect("connect to PostgreSQL");
    execute(&admin, &format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await;
    execute(&admin, &format!("CREATE SCHEMA {schema}")).await;
    // Every connection of this pool finds and creates tables in that schema only. Migrating and describing it run in a task of their own, so that the schema is dropped even when they fail; the failure is reported once the schema is gone.
    let separator = if url.contains('?') { '&' } else { '?' };
    let schema_url = format!("{url}{separator}options[search_path]={schema}");
    let postgres = tokio::spawn(async move {
        let pool = bayan_db_postgres::connect(&schema_url, 1)
            .await
            .expect("connect to PostgreSQL with the test schema");
        bayan_db_postgres::migrate(&pool)
            .await
            .expect("migrate the PostgreSQL schema");
        let described = postgres_schema(&pool).await;
        pool.close().await;
        described
    })
    .await;
    execute(&admin, &format!("DROP SCHEMA {schema} CASCADE")).await;
    admin.close().await;
    let postgres =
        postgres.unwrap_or_else(|failure| std::panic::resume_unwind(failure.into_panic()));

    assert!(!sqlite.is_empty(), "the migrations create tables");
    assert_eq!(
        sqlite.keys().collect::<Vec<_>>(),
        postgres.keys().collect::<Vec<_>>(),
        "tables: SQLite left, PostgreSQL right"
    );
    for (name, sqlite_table) in &sqlite {
        let postgres_table = &postgres[name];
        assert_eq!(
            sqlite_table.columns, postgres_table.columns,
            "columns of {name}: SQLite left, PostgreSQL right"
        );
        assert_eq!(
            sqlite_table.primary_key, postgres_table.primary_key,
            "primary key of {name}: SQLite left, PostgreSQL right"
        );
        assert_eq!(
            sqlite_table.indexes, postgres_table.indexes,
            "indexes of {name}: SQLite left, PostgreSQL right"
        );
    }
}

/// Describes every table of the SQLite database, from SQLite's table-valued pragma functions.
async fn sqlite_schema(pool: &bayan_db_sqlite::Pool) -> Schema {
    let names: Vec<String> = sqlx::query_scalar(
        r"SELECT name FROM pragma_table_list
          WHERE schema = 'main' AND type = 'table' AND name NOT LIKE 'sqlite\_%' ESCAPE '\' AND name <> ?1",
    )
    .bind(MIGRATIONS_TABLE)
    .fetch_all(pool)
    .await
    .expect("list the SQLite tables");
    let mut schema = Schema::new();
    for name in names {
        let mut table = Table::default();
        // Hidden columns (value 1) exist only in virtual tables; generated columns (2 and 3) are compared like the others.
        let columns: Vec<(String, String, bool, i64)> = sqlx::query_as(
            r#"SELECT name, type, "notnull", pk FROM pragma_table_xinfo(?1) WHERE hidden <> 1 ORDER BY cid"#,
        )
        .bind(&name)
        .fetch_all(pool)
        .await
        .expect("list the columns of a SQLite table");
        let mut key = Vec::new();
        for (column, declared_type, not_null, key_position) in columns {
            if key_position > 0 {
                key.push((key_position, column.clone()));
            }
            table.columns.insert(
                column,
                Column {
                    sqlite_type: declared_type.to_ascii_uppercase(),
                    not_null,
                },
            );
        }
        key.sort();
        table.primary_key = key.into_iter().map(|(_, column)| column).collect();
        // Origin `c` is an index made with CREATE INDEX, `u` the index of a UNIQUE constraint, and `pk` the primary key's (compared above).
        let indexes: Vec<(String, bool, String, bool)> = sqlx::query_as(
            r#"SELECT name, "unique", origin, partial FROM pragma_index_list(?1) WHERE origin <> 'pk'"#,
        )
        .bind(&name)
        .fetch_all(pool)
        .await
        .expect("list the indexes of a SQLite table");
        for (index, unique, origin, partial) in indexes {
            // Key columns only; a column without a name is an expression.
            let columns: Vec<Option<String>> = sqlx::query_scalar(
                "SELECT name FROM pragma_index_xinfo(?1) WHERE key = 1 ORDER BY seqno",
            )
            .bind(&index)
            .fetch_all(pool)
            .await
            .expect("list the columns of a SQLite index");
            table.indexes.insert(Index {
                name: (origin == "c").then_some(index),
                columns: columns
                    .into_iter()
                    .map(|column| column.unwrap_or_else(|| EXPRESSION.to_owned()))
                    .collect(),
                unique,
                partial,
            });
        }
        schema.insert(name, table);
    }
    schema
}

/// Describes every table in the current schema of the PostgreSQL database, from PostgreSQL's system catalogs.
async fn postgres_schema(pool: &bayan_db_postgres::Pool) -> Schema {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_catalog.pg_class c
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = current_schema() AND c.relkind IN ('r', 'p') AND c.relname::text <> $1",
    )
    .bind(MIGRATIONS_TABLE)
    .fetch_all(pool)
    .await
    .expect("list the PostgreSQL tables");
    let mut schema = Schema::new();
    for name in names {
        let mut table = Table::default();
        let columns: Vec<(String, String, bool)> = sqlx::query_as(
            "SELECT a.attname::text, pg_catalog.format_type(a.atttypid, a.atttypmod), a.attnotnull
             FROM pg_catalog.pg_attribute a
             JOIN pg_catalog.pg_class c ON c.oid = a.attrelid
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
             WHERE n.nspname = current_schema() AND c.relname::text = $1 AND a.attnum > 0 AND NOT a.attisdropped",
        )
        .bind(&name)
        .fetch_all(pool)
        .await
        .expect("list the columns of a PostgreSQL table");
        for (column, postgres_type, not_null) in columns {
            let Some((_, sqlite_type)) = TYPE_MAPPING
                .iter()
                .find(|(mapped, _)| *mapped == postgres_type)
            else {
                panic!(
                    "column {name}.{column} has the PostgreSQL type {postgres_type}, which TYPE_MAPPING does not list; add it with the SQLite type that stores its values"
                );
            };
            table.columns.insert(
                column,
                Column {
                    sqlite_type: (*sqlite_type).to_owned(),
                    not_null,
                },
            );
        }
        // For each index: whether it is the primary key's, whether it backs a primary-key, UNIQUE or exclusion constraint (which PostgreSQL names itself), and its key columns in order, an expression as `EXPRESSION` (key number 0, which no column has), without the extra columns of an INCLUDE clause.
        let indexes: Vec<(String, bool, bool, bool, bool, Vec<String>)> = sqlx::query_as(
            "SELECT ic.relname::text, i.indisprimary, i.indisunique, i.indpred IS NOT NULL,
                 EXISTS (SELECT FROM pg_catalog.pg_constraint con
                         WHERE con.conindid = i.indexrelid AND con.conrelid = i.indrelid AND con.contype IN ('p', 'u', 'x')),
                 ARRAY(SELECT coalesce(a.attname::text, $2)
                       FROM unnest(i.indkey::int2[]) WITH ORDINALITY AS k(attnum, ord)
                       LEFT JOIN pg_catalog.pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = k.attnum
                       WHERE k.ord <= i.indnkeyatts
                       ORDER BY k.ord)
             FROM pg_catalog.pg_index i
             JOIN pg_catalog.pg_class t ON t.oid = i.indrelid
             JOIN pg_catalog.pg_namespace n ON n.oid = t.relnamespace
             JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid
             WHERE n.nspname = current_schema() AND t.relname::text = $1",
        )
        .bind(&name)
        .bind(EXPRESSION)
        .fetch_all(pool)
        .await
        .expect("list the indexes of a PostgreSQL table");
        for (index, primary, unique, partial, backs_constraint, columns) in indexes {
            if primary {
                table.primary_key = columns;
            } else {
                table.indexes.insert(Index {
                    name: (!backs_constraint).then_some(index),
                    columns,
                    unique,
                    partial,
                });
            }
        }
        schema.insert(name, table);
    }
    schema
}

/// Runs one SQL statement that returns no rows.
async fn execute(pool: &bayan_db_postgres::Pool, statement: &str) {
    sqlx::query(AssertSqlSafe(statement))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("{statement}: {error}"));
}
