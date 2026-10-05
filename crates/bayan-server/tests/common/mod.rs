//! Helpers shared by the integration tests: temporary directories, a running server, and a minimal raw HTTP/1.1 client (raw so tests can send arbitrary methods, paths and headers).

#![allow(
    dead_code,
    reason = "each test binary uses a different subset of these helpers"
)]

pub mod content_free;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use bayan_server::config::Config;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// A directory under the system temporary directory, removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(label: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "bayan-server-test-{}-{label}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temporary directory");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A configuration from `BAYAN_*` variables, with the data directory in `data_dir`.
pub fn config(data_dir: &Path, extra: &[(&str, &str)]) -> Config {
    let mut vars = BTreeMap::new();
    vars.insert(
        "BAYAN_DATA_DIR".to_owned(),
        data_dir.to_str().expect("UTF-8 temporary path").to_owned(),
    );
    for (name, value) in extra {
        vars.insert((*name).to_owned(), (*value).to_owned());
    }
    Config::from_vars(&vars).expect("valid test configuration")
}

/// A server running in the background on an ephemeral loopback port.
pub struct Server {
    pub addr: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), bayan_server::RunError>>>,
}

impl Server {
    pub async fn start(config: Config) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("local address");
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            bayan_server::run(&config, listener, async {
                let _ = stopped.await;
            })
            .await
        });
        let server = Self {
            addr,
            stop: Some(stop),
            task: Some(task),
        };
        server.wait_until_ready().await;
        server
    }

    async fn wait_until_ready(&self) {
        for _ in 0..200 {
            if let Ok(response) = try_request(
                self.addr,
                b"GET /readyz HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n",
            )
            .await
                && response.status == 200
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the server did not become ready");
    }

    /// Sends the shutdown signal and waits for the server to stop.
    pub async fn stop(mut self) {
        let _ = self.stop.take().expect("not yet stopped").send(());
        let task = self.task.take().expect("not yet stopped");
        let result = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("the server stops within 10 seconds");
        result
            .expect("the server task does not panic")
            .expect("the server stops without error");
    }
}

/// A parsed HTTP response.
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Sends raw request bytes and reads the whole response (requests should say `Connection: close`).
pub async fn request(addr: SocketAddr, raw: &[u8]) -> Response {
    try_request(addr, raw)
        .await
        .expect("a complete HTTP response")
}

pub async fn get(addr: SocketAddr, path: &str) -> Response {
    request(
        addr,
        format!("GET {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await
}

async fn try_request(addr: SocketAddr, raw: &[u8]) -> Result<Response, String> {
    let exchange = async {
        let mut stream = TcpStream::connect(addr)
            .await
            .map_err(|error| error.to_string())?;
        // The server may answer and close before reading everything (for example 413); still read its answer.
        let _ = stream.write_all(raw).await;
        let mut bytes = Vec::new();
        stream
            .read_to_end(&mut bytes)
            .await
            .map_err(|error| error.to_string())?;
        parse(&bytes)
    };
    tokio::time::timeout(Duration::from_secs(10), exchange)
        .await
        .map_err(|_| "timed out".to_owned())?
}

fn parse(bytes: &[u8]) -> Result<Response, String> {
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("no end of headers")?;
    let head = std::str::from_utf8(&bytes[..split]).map_err(|_| "headers are not UTF-8")?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or("no status line")?;
    let status = status_line
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or("bad status line")?;
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    Ok(Response {
        status,
        headers,
        body: bytes[split + 4..].to_vec(),
    })
}

/// Serves `routes`, wrapped in the production middleware, with the production connection loop and no database. Send on the returned channel to begin shutdown; the task ends when `serve` returns.
pub async fn serve_routes(
    routes: Router,
    config: &Config,
) -> (SocketAddr, oneshot::Sender<()>, JoinHandle<()>) {
    let app = bayan_server::http::with_middleware(routes, config);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral port");
    let addr = listener.local_addr().expect("local address");
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(bayan_server::http::serve(
        listener,
        app,
        config.header_read_timeout,
        config.shutdown_grace,
        async move {
            let _ = stopped.await;
        },
    ));
    (addr, stop, task)
}

/// Reads from `stream` until the server closes it (end of stream or reset), ignoring anything it sends first. Returns how long that took, or `None` if it is still open after `limit`.
pub async fn closed_within(stream: &mut TcpStream, limit: Duration) -> Option<Duration> {
    let started = Instant::now();
    let mut buffer = [0_u8; 1024];
    let closed = async {
        loop {
            match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    };
    tokio::time::timeout(limit, closed)
        .await
        .ok()
        .map(|()| started.elapsed())
}

/// Reads one response with a body of `body_len` bytes from a kept-alive connection, without waiting for the connection to close.
pub async fn read_one_response(stream: &mut TcpStream, body_len: usize) -> Response {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    let read = async {
        loop {
            if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
                && bytes.len() >= end + 4 + body_len
            {
                return;
            }
            let n = stream.read(&mut buffer).await.expect("read a response");
            assert!(
                n > 0,
                "the connection closed before the response was complete"
            );
            bytes.extend_from_slice(&buffer[..n]);
        }
    };
    tokio::time::timeout(Duration::from_secs(10), read)
        .await
        .expect("a response within 10 seconds");
    parse(&bytes).expect("a valid HTTP response")
}
