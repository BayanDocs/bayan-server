//! Proves that sensitive request data never reaches the logs in the JSON format (SRV-001 AC-3). See `common/content_free.rs`.

mod common;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sensitive_request_data_is_never_logged_as_json() {
    common::content_free::run(bayan_server::config::LogFormat::Json).await;
}
