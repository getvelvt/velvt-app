use super::BatchPayload;
use crate::auth::{HttpClient, HttpRequest};
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadOutcome {
    Accepted,
    Duplicate,
    RawFieldRejected { message: String },
    RateLimited { retry_after: Option<String> },
    Retryable { code: String },
}

impl UploadOutcome {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Accepted | Self::Duplicate)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BatchUploadError {
    #[error("batch upload transport unavailable")]
    Transport,
}

pub trait BatchUploader: Send + Sync {
    fn upload<'a>(
        &'a self,
        batch: &'a BatchPayload,
    ) -> Pin<Box<dyn Future<Output = Result<UploadOutcome, BatchUploadError>> + Send + 'a>>;
}

#[derive(Clone, Default)]
pub struct FakeBatchUploader {
    outcomes: Arc<Mutex<VecDeque<UploadOutcome>>>,
    uploads: Arc<Mutex<Vec<BatchPayload>>>,
}

impl FakeBatchUploader {
    pub fn with_outcomes(outcomes: Vec<UploadOutcome>) -> Self {
        Self {
            outcomes: Arc::new(Mutex::new(outcomes.into())),
            uploads: Arc::default(),
        }
    }

    pub fn upload_count(&self) -> usize {
        self.uploads.lock().unwrap().len()
    }
}

impl BatchUploader for FakeBatchUploader {
    fn upload<'a>(
        &'a self,
        batch: &'a BatchPayload,
    ) -> Pin<Box<dyn Future<Output = Result<UploadOutcome, BatchUploadError>> + Send + 'a>> {
        Box::pin(async move {
            self.uploads.lock().unwrap().push(batch.clone());
            self.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(BatchUploadError::Transport)
        })
    }
}

pub struct HttpBatchUploader<H> {
    http: Arc<H>,
}

impl<H> HttpBatchUploader<H> {
    pub fn new(http: Arc<H>) -> Self {
        Self { http }
    }
}

impl<H> BatchUploader for HttpBatchUploader<H>
where
    H: HttpClient,
{
    fn upload<'a>(
        &'a self,
        batch: &'a BatchPayload,
    ) -> Pin<Box<dyn Future<Output = Result<UploadOutcome, BatchUploadError>> + Send + 'a>> {
        Box::pin(async move {
            let mut request = HttpRequest::post("/v1/events/batches");
            request.json_body =
                Some(serde_json::to_value(batch).map_err(|_| BatchUploadError::Transport)?);
            let response = self
                .http
                .send(request)
                .await
                .map_err(|_| BatchUploadError::Transport)?;
            Ok(if (200..300).contains(&response.status) {
                if response
                    .raw_body
                    .as_ref()
                    .and_then(|body| body.get("status"))
                    .and_then(serde_json::Value::as_str)
                    == Some("duplicate")
                {
                    UploadOutcome::Duplicate
                } else {
                    UploadOutcome::Accepted
                }
            } else {
                match (response.status, response.error_code.as_deref()) {
                    (409, Some("duplicate_batch" | "duplicate")) => UploadOutcome::Duplicate,
                    (422, Some("raw_field_rejected")) => UploadOutcome::RawFieldRejected {
                        message: response
                            .message
                            .unwrap_or_else(|| "raw_field_rejected".into()),
                    },
                    (429, _) => UploadOutcome::RateLimited {
                        retry_after: response.retry_after,
                    },
                    (status, _) => UploadOutcome::Retryable {
                        code: format!("http_{status}"),
                    },
                }
            })
        })
    }
}
