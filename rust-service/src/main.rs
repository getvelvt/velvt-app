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
        use std::sync::Arc;
        use velvt_service::auth::{AuthState, AuthStateMachine};
        use velvt_service::ipc::transport::{IpcTransport, TokioUnixTransport};

        let auth_state = Arc::new(AuthStateMachine::new(AuthState::Unauthenticated));
        let transport = TokioUnixTransport::new(config.socket_path, config.ipc_max_errors)
            .with_auth_state(auth_state.subscribe());
        let server_task = tokio::spawn(async move { transport.run().await });
        if tokio::signal::ctrl_c().await.is_err() {
            tracing::error!("failed to install shutdown signal");
        }
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
