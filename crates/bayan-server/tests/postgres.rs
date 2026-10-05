//! The server with PostgreSQL. Needs a server, so it is ignored in normal test runs; `cargo xtask test-postgres` runs it with `BAYAN_TEST_POSTGRES_URL` set (CI does this against a PostgreSQL service container).

mod common;

use bayan_server::config::DatabaseConfig;
use common::{Server, TempDir, config};

#[tokio::test]
#[ignore = "needs PostgreSQL; run `cargo xtask test-postgres` with BAYAN_TEST_POSTGRES_URL set"]
async fn starts_with_postgres_when_configured() {
    let url = std::env::var("BAYAN_TEST_POSTGRES_URL")
        .expect("BAYAN_TEST_POSTGRES_URL must name an empty PostgreSQL database");
    let data = TempDir::new("postgres");
    let config = config(data.path(), &[("BAYAN_DATABASE_URL", &url)]);
    assert!(matches!(config.database, DatabaseConfig::Postgres { .. }));

    let server = Server::start(config.clone()).await;
    let ready = common::get(server.addr, "/readyz").await;
    assert_eq!(ready.status, 200);
    assert_eq!(ready.body_text(), "ready\n");
    server.stop().await;
    assert!(
        std::fs::read_dir(data.path())
            .expect("data directory exists")
            .next()
            .is_none(),
        "no SQLite database is created when PostgreSQL is configured"
    );

    // A second start finds the schema already migrated.
    let server = Server::start(config).await;
    assert_eq!(common::get(server.addr, "/readyz").await.status, 200);
    server.stop().await;
}
