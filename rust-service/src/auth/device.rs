use super::{HttpClient, HttpRequest, TokenStore};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub trait DeviceRegistrar: Send + Sync {
    fn register(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<(), DeviceRegistrationError>> + Send + '_>>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoOpDeviceRegistrar;

impl DeviceRegistrar for NoOpDeviceRegistrar {
    fn register(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<(), DeviceRegistrationError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
}

/// Registers this device with the cloud via `POST /v1/devices` and persists
/// the returned `device_id` and device-bound tokens via [`TokenStore`].
///
/// This is the seam connecting Swift onboarding (S5) to the Rust auth layer
/// (R4): without a registered device, [`super::AuthManager`] never has
/// tokens to attach to outbound requests, so uploads and fetches can never
/// authenticate.
pub struct HttpDeviceRegistrar<H, S> {
    http: Arc<H>,
    store: Arc<S>,
}

impl<H, S> HttpDeviceRegistrar<H, S> {
    pub fn new(http: Arc<H>, store: Arc<S>) -> Self {
        Self { http, store }
    }
}

impl<H, S> DeviceRegistrar for HttpDeviceRegistrar<H, S>
where
    H: HttpClient,
    S: TokenStore,
{
    fn register(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<(), DeviceRegistrationError>> + Send + '_>> {
        Box::pin(async move {
            let response = self
                .http
                .send(HttpRequest::post("/v1/devices"))
                .await
                .map_err(|_| DeviceRegistrationError::Unavailable)?;
            if !(200..300).contains(&response.status) {
                return Err(DeviceRegistrationError::Rejected);
            }
            let device_id = response
                .device_id
                .ok_or(DeviceRegistrationError::InvalidResponse)?;
            let tokens = response
                .tokens
                .ok_or(DeviceRegistrationError::InvalidResponse)?;
            self.store
                .store_pair(tokens)
                .map_err(|_| DeviceRegistrationError::Unavailable)?;
            self.store
                .store_device_id(&device_id)
                .map_err(|_| DeviceRegistrationError::Unavailable)?;
            Ok(())
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeviceRegistrationError {
    #[error("device registration unavailable")]
    Unavailable,
    #[error("device registration rejected by server")]
    Rejected,
    #[error("device registration response was invalid")]
    InvalidResponse,
}
