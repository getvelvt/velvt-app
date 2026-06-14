use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use velvt_service::auth::{DeviceRegistrar, DeviceRegistrationError, NoOpDeviceRegistrar};

#[derive(Clone, Default)]
struct FakeDeviceRegistrar {
    calls: Arc<Mutex<usize>>,
}

impl FakeDeviceRegistrar {
    fn calls(&self) -> usize {
        *self.calls.lock().unwrap()
    }
}

impl DeviceRegistrar for FakeDeviceRegistrar {
    fn register(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<(), DeviceRegistrationError>> + Send + '_>> {
        Box::pin(async move {
            *self.calls.lock().unwrap() += 1;
            Ok(())
        })
    }
}

async fn wiring_site(registrar: &dyn DeviceRegistrar) {
    registrar.register().await.unwrap();
}

#[tokio::test]
async fn registrar_implementations_swap_only_at_wiring_site() {
    wiring_site(&NoOpDeviceRegistrar).await;

    let fake = FakeDeviceRegistrar::default();
    wiring_site(&fake).await;

    assert_eq!(fake.calls(), 1);
}
