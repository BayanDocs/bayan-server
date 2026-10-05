//! Connection handling: the header-read timeout closes connections that never send a complete request head (SRV-001 review, fix 1), and graceful shutdown lets requests in progress finish without waiting for idle or stuck connections.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::routing::get;
use common::{Server, TempDir, closed_within, config};
use tokio::io::AsyncWriteExt as _;
use tokio::net::TcpStream;
use tokio::sync::Notify;

/// Closed by the timer, not at once: at least the 1-second timeout (with a little scheduling slack).
const AT_LEAST_THE_TIMEOUT: Duration = Duration::from_millis(900);
const WELL_WITHIN_THE_LIMIT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn half_sent_requests_are_closed_after_the_header_read_timeout() {
    let data = TempDir::new("half-sent");
    let server = Server::start(config(
        data.path(),
        &[("BAYAN_HEADER_READ_TIMEOUT_SECS", "1")],
    ))
    .await;

    let mut stream = TcpStream::connect(server.addr).await.expect("connect");
    stream
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: test\r\n")
        .await
        .expect("send half a request");
    let closed = closed_within(&mut stream, WELL_WITHIN_THE_LIMIT)
        .await
        .expect("the server closes the connection");
    assert!(closed >= AT_LEAST_THE_TIMEOUT, "closed after {closed:?}");

    assert_eq!(common::get(server.addr, "/healthz").await.status, 200);
    server.stop().await;
}

#[tokio::test]
async fn silent_connections_are_closed_after_the_header_read_timeout() {
    let data = TempDir::new("silent");
    let server = Server::start(config(
        data.path(),
        &[("BAYAN_HEADER_READ_TIMEOUT_SECS", "1")],
    ))
    .await;

    let mut stream = TcpStream::connect(server.addr).await.expect("connect");
    let closed = closed_within(&mut stream, WELL_WITHIN_THE_LIMIT)
        .await
        .expect("the server closes the connection");
    assert!(closed >= AT_LEAST_THE_TIMEOUT, "closed after {closed:?}");

    assert_eq!(common::get(server.addr, "/healthz").await.status, 200);
    server.stop().await;
}

#[tokio::test]
async fn idle_keep_alive_connections_are_closed_after_the_header_read_timeout() {
    let data = TempDir::new("keep-alive");
    let server = Server::start(config(
        data.path(),
        &[("BAYAN_HEADER_READ_TIMEOUT_SECS", "1")],
    ))
    .await;

    let mut stream = TcpStream::connect(server.addr).await.expect("connect");
    stream
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .expect("send a request without closing");
    let response = common::read_one_response(&mut stream, 3).await;
    assert_eq!(
        (response.status, response.body_text().as_str()),
        (200, "ok\n")
    );
    let closed = closed_within(&mut stream, WELL_WITHIN_THE_LIMIT)
        .await
        .expect("the server closes the idle connection");
    assert!(closed >= AT_LEAST_THE_TIMEOUT, "closed after {closed:?}");

    server.stop().await;
}

/// A route that signals when it starts and then takes `duration` to answer.
fn slow_route(started: &Arc<Notify>, duration: Duration) -> Router {
    let started = Arc::clone(started);
    Router::new().route(
        "/slow",
        get(move || async move {
            started.notify_one();
            tokio::time::sleep(duration).await;
            "finished\n"
        }),
    )
}

#[tokio::test]
async fn in_flight_requests_finish_during_shutdown() {
    let data = TempDir::new("in-flight");
    let config = config(data.path(), &[("BAYAN_SHUTDOWN_GRACE_SECS", "10")]);
    let started = Arc::new(Notify::new());
    let (addr, stop, server) =
        common::serve_routes(slow_route(&started, Duration::from_millis(500)), &config).await;

    let request = tokio::spawn(async move { common::get(addr, "/slow").await });
    started.notified().await;
    // Shutdown begins while the request is running.
    let _ = stop.send(());
    let response = request.await.expect("the request task does not panic");
    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "finished\n");
    tokio::time::timeout(WELL_WITHIN_THE_LIMIT, server)
        .await
        .expect("the server stops once the request has finished")
        .expect("the server task does not panic");
    assert!(
        TcpStream::connect(addr).await.is_err(),
        "new connections are refused after shutdown"
    );
}

#[tokio::test]
async fn idle_connections_do_not_delay_shutdown() {
    let data = TempDir::new("idle-shutdown");
    // Long timeouts, so only graceful shutdown can close these connections quickly.
    let config = config(
        data.path(),
        &[
            ("BAYAN_HEADER_READ_TIMEOUT_SECS", "60"),
            ("BAYAN_SHUTDOWN_GRACE_SECS", "30"),
        ],
    );
    let routes = Router::new().route("/ok", get(|| async { "ok\n" }));
    let (addr, stop, server) = common::serve_routes(routes, &config).await;

    let mut silent = TcpStream::connect(addr).await.expect("connect");
    let mut kept_alive = TcpStream::connect(addr).await.expect("connect");
    kept_alive
        .write_all(b"GET /ok HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .expect("send a request without closing");
    assert_eq!(
        common::read_one_response(&mut kept_alive, 3).await.status,
        200
    );

    let begin = Instant::now();
    let _ = stop.send(());
    tokio::time::timeout(WELL_WITHIN_THE_LIMIT, server)
        .await
        .expect("idle connections do not hold up shutdown")
        .expect("the server task does not panic");
    assert!(begin.elapsed() < WELL_WITHIN_THE_LIMIT);
    assert!(
        closed_within(&mut silent, WELL_WITHIN_THE_LIMIT)
            .await
            .is_some()
    );
    assert!(
        closed_within(&mut kept_alive, WELL_WITHIN_THE_LIMIT)
            .await
            .is_some()
    );
}

#[tokio::test]
async fn a_stuck_request_cannot_hold_shutdown_past_the_grace_period() {
    let data = TempDir::new("stuck-shutdown");
    let config = config(
        data.path(),
        &[
            ("BAYAN_REQUEST_TIMEOUT_SECS", "120"),
            ("BAYAN_SHUTDOWN_GRACE_SECS", "1"),
        ],
    );
    let started = Arc::new(Notify::new());
    let (addr, stop, server) =
        common::serve_routes(slow_route(&started, Duration::from_secs(60)), &config).await;

    let request = tokio::spawn(async move { common::get(addr, "/slow").await });
    started.notified().await;
    let begin = Instant::now();
    let _ = stop.send(());
    tokio::time::timeout(WELL_WITHIN_THE_LIMIT, server)
        .await
        .expect("the server stops after the grace period")
        .expect("the server task does not panic");
    let stopped = begin.elapsed();
    assert!(stopped >= AT_LEAST_THE_TIMEOUT, "stopped after {stopped:?}");
    request.abort();
}
