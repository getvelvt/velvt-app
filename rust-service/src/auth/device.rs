use std::future::Future;
use std::pin::Pin;

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

#[derive(Debug, thiserror::Error)]
pub enum DeviceRegistrationError {
    #[error("device registration unavailable")]
    Unavailable,
}
