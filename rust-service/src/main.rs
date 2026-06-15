//! Velvt local IPC service entry point.
//!
//! R1 owns transport, framing, version negotiation, and typed message
//! validation. It does not implement event processing or later service layers.

#[tokio::main]
async fn main() {
    use tracing_subscriber::EnvFilter;
    use velvt_service::abstraction::{AbstractionEngine, Taxonomy, API_EXPECTED_TAXONOMY_VERSION};
    use velvt_service::config::ServiceConfig;
    use velvt_service::persistence::SqlitePersistence;

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

    let Ok(persistence) = SqlitePersistence::open(&config.database_path) else {
        tracing::error!(
            error_code = "persistence_initialization_failed",
            "service startup halted"
        );
        return;
    };
    let Ok(taxonomy) = Taxonomy::from_path(&config.abstraction_taxonomy_path) else {
        tracing::error!(
            error_code = "abstraction_taxonomy_load_failed",
            "service startup halted"
        );
        return;
    };
    if taxonomy.version() != API_EXPECTED_TAXONOMY_VERSION {
        tracing::warn!(
            error_code = "taxonomy_version_mismatch",
            expected_version = API_EXPECTED_TAXONOMY_VERSION,
            configured_version = taxonomy.version(),
            "configured taxonomy version differs from API expected value"
        );
    }
    let embedding_plugin = load_embedding_plugin(&config, &taxonomy);
    let Ok(_abstraction_engine) =
        AbstractionEngine::builder(persistence.abstraction_mapping_store(), taxonomy)
            .register_builtin_plugins_with_embedding(embedding_plugin)
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
        use chrono::Duration;
        use std::sync::Arc;
        use velvt_service::auth::{
            AuthManager, AuthState, AuthStateMachine, KeychainTokenStore, ReqwestHttpClient,
        };
        use velvt_service::delivery::{
            CacheManager, FetchConfig, FetchScheduler, FetchService, Fetchable, PushAdapter,
            PushAdapterAlertSink, PushQueue,
        };
        use velvt_service::ipc::transport::{IpcTransport, TokioUnixTransport};
        use velvt_service::ipc::R7Router;
        use velvt_service::upload::{HttpBatchUploader, UploadCoordinator};

        let upload_batch_repo = persistence.upload_batch_repo();
        let history_cache_repo = persistence.history_cache_repo();
        let insight_cache_repo = persistence.insight_cache_repo();

        let auth_state = Arc::new(AuthStateMachine::new(AuthState::Unauthenticated));
        let upload_host = reqwest::Url::parse(&config.upload_api_base_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_else(|| "invalid_upload_host".into());
        let authenticated_http = Arc::new(AuthManager::new(
            Arc::new(KeychainTokenStore::default()),
            Arc::new(ReqwestHttpClient::new(config.upload_api_base_url.clone())),
            Arc::clone(&auth_state),
            Duration::minutes(5),
        ));

        // R7 — push queue and adapter.
        let push_queue = PushQueue::new(config.push_queue_capacity);
        let push_adapter = PushAdapter::new(Arc::clone(&push_queue));

        // R6 — fetch service and scheduler (push adapter injected for proactive delivery).
        let fetch_config = FetchConfig {
            history_ttl: config.history_ttl,
            insight_ttl: config.insight_ttl,
            insight_negative_ttl: config.insight_negative_ttl,
            read_timeout: config.cache_read_timeout,
        };
        let fetch_service = Arc::new(
            FetchService::new(
                Arc::clone(&authenticated_http),
                history_cache_repo,
                insight_cache_repo,
                fetch_config,
            )
            .with_push_adapter(Arc::clone(&push_adapter)),
        );
        let cache_manager: Arc<dyn CacheManager> =
            Arc::clone(&fetch_service) as Arc<dyn CacheManager>;
        let (fetch_shutdown_tx, fetch_shutdown_rx) = tokio::sync::watch::channel(false);
        let fetch_scheduler = FetchScheduler::new(
            Arc::clone(&fetch_service) as Arc<dyn Fetchable>,
            7,
            config.fetch_interval,
            auth_state.subscribe(),
            fetch_shutdown_rx,
        );
        let fetch_task = tokio::spawn(async move { fetch_scheduler.run().await });

        let uploader = HttpBatchUploader::new(authenticated_http);
        let upload_coordinator = UploadCoordinator::new(
            upload_batch_repo,
            uploader,
            PushAdapterAlertSink::new(push_adapter),
        )
        .with_host(upload_host);
        let (upload_shutdown, upload_shutdown_receiver) = tokio::sync::watch::channel(false);
        let retry_scan_interval = config.upload_retry_scan_interval;
        let recovery_task = tokio::spawn(async move {
            upload_coordinator
                .run_retry_loop(
                    retry_scan_interval,
                    upload_shutdown_receiver,
                    "1",
                    env!("CARGO_PKG_VERSION"),
                    &["document:edit".into()],
                )
                .await;
        });
        let transport = TokioUnixTransport::new_with_router(
            config.socket_path,
            config.ipc_max_errors,
            R7Router::new(cache_manager),
        )
        .with_auth_state(auth_state.subscribe())
        .with_push_queue(push_queue, config.push_write_timeout);
        let server_task = tokio::spawn(async move { transport.run().await });
        if tokio::signal::ctrl_c().await.is_err() {
            tracing::error!("failed to install shutdown signal");
        }
        fetch_shutdown_tx.send_replace(true);
        upload_shutdown.send_replace(true);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), fetch_task).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), recovery_task).await;
        server_task.abort();
    }

    #[cfg(not(unix))]
    tracing::error!("Unix domain socket transport is unavailable on this platform");
}

#[cfg(feature = "onnx")]
fn load_embedding_plugin(
    config: &velvt_service::config::ServiceConfig,
    taxonomy: &velvt_service::abstraction::Taxonomy,
) -> Option<velvt_service::abstraction::EmbeddingSimilarityPlugin> {
    use std::sync::Arc;
    use velvt_service::abstraction::{
        CategoryCentroids, EmbeddingMetrics, EmbeddingSimilarityPlugin, OrtEmbeddingModel,
    };

    let model_path = config.abstraction_model_path.as_deref()?;
    let Some(centroid_path) = config.abstraction_centroids_path.as_deref() else {
        tracing::warn!(
            error_code = "tier2_centroids_unavailable",
            "Tier 2 classification disabled"
        );
        return None;
    };
    let Ok(centroids) = CategoryCentroids::from_path(centroid_path) else {
        tracing::warn!(
            error_code = "tier2_centroids_unavailable",
            "Tier 2 classification disabled"
        );
        return None;
    };
    let Ok(model) = OrtEmbeddingModel::load(model_path) else {
        tracing::warn!(
            error_code = "tier2_model_unavailable",
            "Tier 2 classification disabled"
        );
        return None;
    };
    if centroids.taxonomy_version() != taxonomy.version()
        || centroids
            .categories()
            .any(|category| !taxonomy.contains_category(category))
    {
        tracing::warn!(
            error_code = "tier2_centroids_invalid",
            "Tier 2 classification disabled"
        );
        return None;
    }
    EmbeddingSimilarityPlugin::new(
        Arc::new(model),
        centroids.into_vectors(),
        taxonomy.version(),
        config.abstraction_similarity_threshold,
        config.abstraction_inference_timeout,
        Arc::new(EmbeddingMetrics::default()),
    )
    .map_err(|_| {
        tracing::warn!(
            error_code = "tier2_initialization_failed",
            "Tier 2 classification disabled"
        );
    })
    .ok()
}

#[cfg(not(feature = "onnx"))]
fn load_embedding_plugin(
    config: &velvt_service::config::ServiceConfig,
    _taxonomy: &velvt_service::abstraction::Taxonomy,
) -> Option<velvt_service::abstraction::EmbeddingSimilarityPlugin> {
    if config.abstraction_model_path.is_some() {
        let error_code = if config.abstraction_centroids_path.is_none() {
            "tier2_centroids_unavailable"
        } else {
            "tier2_model_unavailable"
        };
        tracing::warn!(error_code, "Tier 2 classification disabled");
    }
    None
}
