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
    let Ok(filter) = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&config.log_level))
    else {
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
    if velvt_service::ipc::transport::socket_already_in_use(&config.socket_path).await {
        tracing::error!(
            error_code = "duplicate_service_instance",
            "another velvt-service instance is already listening on this socket; exiting"
        );
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
    // Tracked before the plugin is consumed below: true only when an operator
    // explicitly configured a Tier 2 model and it failed to load — not when
    // Tier 2 was never configured at all (that is expected MVP-default Tier
    // 1/3 behavior, not a degradation worth alerting on).
    let tier2_unavailable = config.abstraction_model_path.is_some() && embedding_plugin.is_none();
    let Ok(abstraction_engine) =
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
        use velvt_service::auth::{
            AccountAuthService, AuthManager, AuthState, AuthStateMachine, HttpClient,
            KeychainTokenStore, ReqwestHttpClient, TokenStore,
        };
        use velvt_service::delivery::{
            CacheManager, FetchConfig, FetchScheduler, FetchService, Fetchable, PushAdapter,
            PushAdapterAlertSink,
        };
        use velvt_service::ipc::transport::{IpcTransport, TokioUnixTransport};
        use velvt_service::ipc::{MenuStatusProvider, R7Router, ReconnectTracker};
        use velvt_service::lifecycle::CancellationToken;
        use velvt_service::retention::{
            CacheRetentionTarget, RawEventRetentionTarget, RetentionScheduler,
            UploadBatchRetentionTarget,
        };
        use velvt_service::upload::{
            BatchAssembler, EventIngestor, HttpBatchUploader, SharedUploadBatcher, UploadBatcher,
            UploadCoordinator,
        };

        let token = CancellationToken::new();
        let abstraction_engine = Arc::new(abstraction_engine);

        let upload_batch_repo = persistence.upload_batch_repo();
        let history_cache_repo = persistence.history_cache_repo();
        let insight_cache_repo = persistence.insight_cache_repo();
        let raw_event_repo = persistence.raw_event_repo();

        let token_store = Arc::new(KeychainTokenStore::default());
        let raw_http = Arc::new(ReqwestHttpClient::new(config.upload_api_base_url.clone()));

        // Device registration requires a logged-in user's access token --
        // `/v1/devices` has no anonymous mode -- so it cannot happen here at
        // startup. It is instead triggered by `AccountAuthService` the first
        // time sign-up or login succeeds (see ensure_device_registered),
        // using the access token issued by that call. Here we only load
        // whatever was already persisted by a prior login.
        let device_id = match token_store.load_device_id() {
            Ok(device_id) => device_id,
            Err(error) => {
                tracing::error!(
                    error_code = "device_id_load_failed",
                    error = %error,
                    "failed to read stored device identifier"
                );
                None
            }
        };

        let auth_state = Arc::new(match &device_id {
            Some(device_id) => AuthStateMachine::from_token_store(&*token_store, device_id.clone())
                .unwrap_or_else(|_| AuthStateMachine::new(AuthState::Unauthenticated)),
            None => AuthStateMachine::new(AuthState::Unauthenticated),
        });
        let upload_host = reqwest::Url::parse(&config.upload_api_base_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_else(|| "invalid_upload_host".into());
        let authenticated_http = Arc::new(AuthManager::new(
            Arc::clone(&token_store),
            Arc::clone(&raw_http),
            Arc::clone(&auth_state),
            chrono::Duration::minutes(5),
        ));

        // Reconnect tracker — creates and manages the push queue per connection.
        let reconnect_tracker =
            ReconnectTracker::new(config.reconnect_window, config.push_queue_capacity);
        // Acquire the initial queue so PushAdapter can enqueue proactively.
        let initial_queue = reconnect_tracker.acquire();
        let push_adapter = PushAdapter::new(Arc::clone(&initial_queue));

        // An operator explicitly configured a Tier 2 model and it failed to
        // load: the user must not be silently left on the Tier 1/3 fallback
        // path indefinitely without notification (see README "On-Device
        // Classification").
        if tier2_unavailable {
            push_adapter
                .push_service_status(
                    velvt_shared_types::ServiceState::Degraded,
                    Some("tier2_classification_unavailable"),
                )
                .await;
        }

        // Watches device-bound auth state and forwards terminal transitions
        // to Swift as proactive IPC pushes, independent of any in-flight
        // request.
        {
            let mut auth_state_changes = auth_state.subscribe();
            let push_adapter_for_auth = Arc::clone(&push_adapter);
            let mut auth_shutdown = token.subscribe();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        changed = auth_state_changes.changed() => {
                            if changed.is_err() {
                                return;
                            }
                            let current = auth_state_changes.borrow().clone();
                            match current {
                                AuthState::DeviceRevoked => {
                                    push_adapter_for_auth
                                        .push_device_revoked(
                                            "This device was removed from your account.",
                                        )
                                        .await;
                                }
                                AuthState::NeedsReauth => {
                                    push_adapter_for_auth
                                        .push_needs_reauth("session_expired")
                                        .await;
                                }
                                _ => {}
                            }
                        }
                        _ = auth_shutdown.changed() => {
                            if *auth_shutdown.borrow() {
                                return;
                            }
                        }
                    }
                }
            });
        }

        // R6 — fetch service and scheduler.
        let fetch_config = FetchConfig {
            history_ttl: config.history_ttl,
            insight_ttl: config.insight_ttl,
            insight_negative_ttl: config.insight_negative_ttl,
            read_timeout: config.cache_read_timeout,
        };
        let fetch_service = Arc::new(
            FetchService::new(
                Arc::clone(&authenticated_http),
                Arc::clone(&history_cache_repo),
                Arc::clone(&insight_cache_repo),
                fetch_config,
            )
            .with_push_adapter(Arc::clone(&push_adapter)),
        );
        let cache_manager: Arc<dyn CacheManager> =
            Arc::clone(&fetch_service) as Arc<dyn CacheManager>;
        let fetch_scheduler = FetchScheduler::new(
            Arc::clone(&fetch_service) as Arc<dyn Fetchable>,
            7,
            config.fetch_interval,
            auth_state.subscribe(),
            token.subscribe(),
        );
        let fetch_task = tokio::spawn(async move { fetch_scheduler.run().await });

        let uploader = HttpBatchUploader::new(Arc::clone(&authenticated_http));
        let upload_coordinator = UploadCoordinator::new(
            Arc::clone(&upload_batch_repo),
            uploader,
            PushAdapterAlertSink::new(Arc::clone(&push_adapter)),
        )
        .with_host(upload_host.clone());
        let retry_scan_interval = config.upload_retry_scan_interval;
        let upload_shutdown = token.subscribe();
        let recovery_task = tokio::spawn(async move {
            upload_coordinator
                .run_retry_loop(
                    retry_scan_interval,
                    upload_shutdown,
                    "1",
                    env!("CARGO_PKG_VERSION"),
                    &["document:edit".into()],
                )
                .await;
        });

        // Live ingestion path: connects IPC RawEvent receipt to the
        // abstraction engine and upload pipeline. This uses its own
        // `UploadCoordinator` (sharing the same persistence, uploader
        // target, and alert sink as the retry-loop coordinator above) so
        // the router can hold it independently of the retry task; the
        // persisted per-host backoff state in SQLite is what actually gates
        // concurrent senders, so two in-memory backoff trackers do not risk
        // a correctness issue, only a minor heuristic duplication.
        // Fixed for this process's lifetime: if this is the very first login
        // (no device_id persisted yet), registration happens later via
        // AccountAuthService and events batched before the next restart are
        // tagged "unregistered-device". They still upload successfully; the
        // service is restarted alongside the menu bar app often enough
        // (login is a rare, early-session event) that this is acceptable.
        let device_id_for_batching = device_id
            .clone()
            .unwrap_or_else(|| "unregistered-device".into());
        let ingestion_coordinator = UploadCoordinator::new(
            Arc::clone(&upload_batch_repo),
            HttpBatchUploader::new(Arc::clone(&authenticated_http)),
            PushAdapterAlertSink::new(Arc::clone(&push_adapter)),
        )
        .with_host(upload_host);
        let upload_batcher = UploadBatcher::new(
            BatchAssembler::from_config(device_id_for_batching, &config),
            ingestion_coordinator,
        );
        let shared_batcher: Arc<dyn EventIngestor> =
            Arc::new(SharedUploadBatcher::new(upload_batcher));

        let account_service = Arc::new(AccountAuthService::new(
            Arc::clone(&raw_http) as Arc<dyn HttpClient>,
            Arc::clone(&authenticated_http) as Arc<dyn HttpClient>,
            Arc::clone(&token_store) as Arc<dyn TokenStore>,
            Arc::clone(&auth_state),
        ));

        // Periodic age-based flush: count-based flush happens inline on
        // ingest, but events that never reach the batch-size threshold must
        // still flush after `upload_flush_interval`.
        let flush_interval = config.upload_flush_interval;
        let flush_shutdown_token = token.subscribe();
        let flush_batcher = Arc::clone(&shared_batcher);
        let flush_task = tokio::spawn(async move {
            let mut shutdown = flush_shutdown_token;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(flush_interval) => {
                        if let Err(error) = flush_batcher.flush_due(chrono::Utc::now()).await {
                            tracing::error!(
                                error_code = "upload_flush_due_failed",
                                error = %error,
                                "periodic age-based flush failed"
                            );
                        }
                    }
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            return;
                        }
                    }
                }
            }
        });

        // R8 — retention scheduler.
        let raw_event_target = RawEventRetentionTarget::new(
            Arc::clone(&raw_event_repo),
            config.raw_event_ttl,
            config.retention_batch_size,
        );
        let upload_batch_target = UploadBatchRetentionTarget::new(
            Arc::clone(&upload_batch_repo),
            config.sent_batch_retention,
            config.rejected_batch_audit_period,
            config.retention_batch_size,
        );
        let cache_target = CacheRetentionTarget::new(
            history_cache_repo,
            insight_cache_repo,
            config.cache_expiry_grace,
            config.retention_batch_size,
        );
        let retention_scheduler =
            RetentionScheduler::new(config.raw_event_expiry_interval, token.subscribe())
                .add_target(raw_event_target)
                .add_target(upload_batch_target)
                .add_target(cache_target);
        let retention_task = tokio::spawn(async move { retention_scheduler.run().await });

        // R7 + R8 transport — shutdown-aware, reconnect-tracking.
        let transport = TokioUnixTransport::new_with_router(
            config.socket_path,
            config.ipc_max_errors,
            R7Router::new(
                cache_manager,
                Arc::clone(&abstraction_engine),
                Arc::clone(&raw_event_repo),
                Arc::clone(&shared_batcher),
                account_service,
            )
            .with_menu_status(Arc::new(MenuStatusProvider::new(
                Arc::clone(&raw_http) as Arc<dyn HttpClient>,
                Arc::clone(&token_store) as Arc<dyn TokenStore>,
                Arc::clone(&upload_batch_repo),
                Arc::clone(&raw_event_repo),
            ))),
        )
        .with_auth_state(auth_state.subscribe())
        .with_reconnect_tracker(reconnect_tracker, config.push_write_timeout)
        .with_shutdown(token.subscribe());
        let server_task = tokio::spawn(async move { transport.run().await });

        // Wait for SIGTERM or SIGINT.
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => {
                tracing::error!("failed to install SIGTERM handler");
                return;
            }
        };
        let reason = tokio::select! {
            _ = sigterm.recv() => "sigterm",
            result = tokio::signal::ctrl_c() => {
                if result.is_err() {
                    tracing::error!("failed to install SIGINT handler");
                    return;
                }
                "sigint"
            }
        };
        tracing::info!(reason, "shutdown signal received");

        // Shutdown sequence: notify clients → cancel tasks → wait → drop persistence.
        push_adapter.push_shutting_down(reason).await;
        token.cancel();

        if let Err(error) = shared_batcher.flush_shutdown().await {
            tracing::error!(
                error_code = "upload_flush_shutdown_failed",
                error = %error,
                "failed to flush in-flight batch during shutdown"
            );
        }

        let shutdown_deadline = config.shutdown_deadline;
        let _ = tokio::time::timeout(shutdown_deadline, async {
            let _ = fetch_task.await;
            let _ = recovery_task.await;
            let _ = server_task.await;
            let _ = retention_task.await;
            let _ = flush_task.await;
        })
        .await;

        // `persistence` is dropped here, which closes the SQLite connection.
        drop(persistence);
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
