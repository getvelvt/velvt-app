use std::sync::Arc;

use velvt_shared_types::{CacheEmpty, ClientMessage, ServerMessage};

use crate::delivery::{shaper, CacheManager};

use super::IpcError;

/// Routes validated post-handshake messages independently of their transport.
#[allow(async_fn_in_trait)]
pub trait MessageRouter {
    fn route(&self, message: ClientMessage) -> impl std::future::Future<Output = Result<Option<ServerMessage>, IpcError>> + Send;
}

/// Minimal R1 router used until business handlers are introduced.
#[derive(Debug, Clone, Copy)]
pub struct DefaultRouter;

impl MessageRouter for DefaultRouter {
    async fn route(&self, message: ClientMessage) -> Result<Option<ServerMessage>, IpcError> {
        match message {
            ClientMessage::ClientHello(_) => Err(IpcError::MalformedMessage),
            _ => Ok(None),
        }
    }
}

/// R7 router: handles on-demand insight and history requests from Swift.
///
/// On a cache miss or validation failure the router returns `CacheEmpty` so
/// Swift can display a loading state rather than crashing.  Cache errors are
/// logged but never surfaced to the transport layer.
#[derive(Clone)]
pub struct R7Router {
    cache: Arc<dyn CacheManager>,
}

impl R7Router {
    pub fn new(cache: Arc<dyn CacheManager>) -> Self {
        Self { cache }
    }
}

impl MessageRouter for R7Router {
    async fn route(&self, message: ClientMessage) -> Result<Option<ServerMessage>, IpcError> {
        match message {
            ClientMessage::ClientHello(_) => Err(IpcError::MalformedMessage),

            ClientMessage::RequestLatestInsight(req) => {
                let result = self.cache.daily_insight(req.date).await;
                let response = match result {
                    Ok(Some(insight)) => match shaper::shape_insight(insight) {
                        Ok(validated) => ServerMessage::InsightPayload(validated.into_inner()),
                        Err(err) => {
                            tracing::warn!(
                                message_type = "insight_payload",
                                error_code = "outbound_validation_failed",
                                error = %err,
                                "shaped insight failed validation; sending cache_empty"
                            );
                            cache_empty("insight_payload")
                        }
                    },
                    Ok(None) => cache_empty("insight_payload"),
                    Err(err) => {
                        tracing::warn!(
                            date = %req.date,
                            error_code = "cache_read_failed",
                            error = %err,
                            "failed to read insight from cache"
                        );
                        cache_empty("insight_payload")
                    }
                };
                Ok(Some(response))
            }

            ClientMessage::RequestLatestHistory(req) => {
                let result = self.cache.daily_history(req.days).await;
                let response = match result {
                    Ok(history) => match shaper::shape_history(history) {
                        Ok(validated) => ServerMessage::HistoryPayload(validated.into_inner()),
                        Err(err) => {
                            tracing::warn!(
                                message_type = "history_payload",
                                error_code = "outbound_validation_failed",
                                error = %err,
                                "shaped history failed validation; sending cache_empty"
                            );
                            cache_empty("history_payload")
                        }
                    },
                    Err(err) => {
                        tracing::warn!(
                            days = req.days,
                            error_code = "cache_read_failed",
                            error = %err,
                            "failed to read history from cache"
                        );
                        cache_empty("history_payload")
                    }
                };
                Ok(Some(response))
            }

            _ => Ok(None),
        }
    }
}

fn cache_empty(payload_type: &'static str) -> ServerMessage {
    ServerMessage::CacheEmpty(CacheEmpty {
        payload_type: payload_type.to_owned(),
    })
}
