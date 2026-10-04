//! The HTTP/1.1 connection loop.
//!
//! `axum::serve` gives hyper no timer, which silently turns off hyper's header-read timeout: a client could then hold a connection open forever by sending nothing, or half a request, and enough such connections exhaust the server's file descriptors. This loop serves every connection with hyper's HTTP/1 implementation, a Tokio timer and a header-read timeout. The timeout runs whenever the server waits for a request: on a new connection, while headers are arriving, and on an idle keep-alive connection between requests. Graceful shutdown closes idle connections at once, lets requests in progress finish, and gives up after the grace period.

use std::future::Future;
use std::io;
use std::time::Duration;

use axum::Router;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;

/// How long to wait before accepting again after an error that is not about a single connection, such as running out of file descriptors.
const ACCEPT_ERROR_PAUSE: Duration = Duration::from_secs(1);

/// Serves `app` on `listener` until `shutdown` completes.
///
/// Each connection gets `header_read_timeout` to send a complete request head; otherwise it is closed. After `shutdown` completes, the listener is closed (new connections are refused), idle connections are closed, and requests in progress get up to `shutdown_grace` to finish.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    header_read_timeout: Duration,
    shutdown_grace: Duration,
    shutdown: impl Future<Output = ()>,
) {
    let mut http = http1::Builder::new();
    http.timer(TokioTimer::new())
        .header_read_timeout(header_read_timeout);
    let service = TowerToHyperService::new(app);
    let graceful = GracefulShutdown::new();
    let mut shutdown = std::pin::pin!(shutdown);

    loop {
        let stream = tokio::select! {
            accepted = listener.accept() => match accepted {
                // The peer address is deliberately unused: it is never logged or stored.
                Ok((stream, _peer)) => stream,
                Err(error) => {
                    if !is_connection_error(&error) {
                        tracing::error!(error = %error, "cannot accept connections; retrying");
                        tokio::select! {
                            () = tokio::time::sleep(ACCEPT_ERROR_PAUSE) => {}
                            () = &mut shutdown => break,
                        }
                    }
                    continue;
                }
            },
            () = &mut shutdown => break,
        };
        let connection = http.serve_connection(TokioIo::new(stream), service.clone());
        let connection = graceful.watch(connection);
        tokio::spawn(async move {
            // An error ends only this connection (the client disconnected, timed out or sent a malformed request); hyper has already answered the client where HTTP allows it, and there is nothing content-free worth logging.
            let _ = connection.await;
        });
    }

    // Closing the listener makes new connection attempts fail instead of waiting in the backlog.
    drop(listener);
    tracing::info!(grace_secs = shutdown_grace.as_secs(), "shutting down");
    if tokio::time::timeout(shutdown_grace, graceful.shutdown())
        .await
        .is_err()
    {
        tracing::warn!("requests still running after the shutdown grace period; stopping anyway");
    }
}

/// Errors about one connection that went away before it was accepted; they need no pause.
fn is_connection_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
    )
}
