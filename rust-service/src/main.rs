//! Velvt local IPC service entry point.
//!
//! R1 owns transport, framing, version negotiation, and typed message
//! validation. It does not implement event processing or later service layers.

#[tokio::main]
async fn main() {
    use std::sync::Arc;
    use tracing_subscriber::EnvFilter;
    use velvt_service::abstraction::{AbstractionEngine, InMemoryMappingStore, Taxonomy};
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

    let Ok(taxonomy) = Taxonomy::from_path(&config.abstraction_taxonomy_path) else {
        tracing::error!(
            error_code = "abstraction_taxonomy_load_failed",
            "service startup halted"
        );
        return;
    };
    let Ok(_abstraction_engine) =
        AbstractionEngine::builder(Arc::new(InMemoryMappingStore::default()), taxonomy)
            .register_builtin_plugins()
            .build()
    else {
        tracing::error!(
            error_code = "abstraction_engine_initialization_failed",
            "service startup halted"
        );
        return;
    };

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
