//! Velvt local processing service entry point.
//!
//! The service owns local processing and the final privacy boundary. It does
//! not render UI or request macOS permissions.

#[tokio::main]
async fn main() {
    use tracing_subscriber::EnvFilter;
    use velvt_service::config::ServiceConfig;

    let Ok(config) = ServiceConfig::load() else {
        return;
    };
    let Ok(filter) = EnvFilter::try_new(&config.log_level) else {
        return;
    };
    if tracing_subscriber::fmt()
        .with_env_filter(filter)
        .try_init()
        .is_err()
    {
        return;
    }

    #[cfg(unix)]
    {
        use velvt_service::ipc::transport::{IpcTransport, TokioUnixTransport};

        let transport = TokioUnixTransport::new(config.socket_path, config.ipc_max_errors);
        let server_task = tokio::spawn(async move { transport.run().await });
        if tokio::signal::ctrl_c().await.is_err() {
            tracing::error!("failed to install shutdown signal");
        }
        server_task.abort();
    }

    #[cfg(not(unix))]
    tracing::error!("Unix domain socket transport is unavailable on this platform");
}
