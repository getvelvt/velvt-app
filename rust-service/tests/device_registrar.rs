use chrono::{Duration, Utc};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use velvt_service::auth::{
    AuthError, DeviceRegistrar, DeviceRegistrationError, DeviceRegistrationPayload, FakeTokenStore,
    HttpClient, HttpDeviceRegistrar, HttpMethod, HttpRequest, HttpResponse, NoOpDeviceRegistrar,
    RedactedString, TokenPair, TokenStore,
};

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

#[derive(Default)]
struct FakeHttpClient {
    requests: Mutex<Vec<HttpRequest>>,
}

impl HttpClient for FakeHttpClient {
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request);
            Ok(HttpResponse {
                status: 201,
                error_code: None,
                tokens: Some(TokenPair::new(
                    RedactedString::new("device-access"),
                    RedactedString::new("device-refresh"),
                    Utc::now() + Duration::hours(1),
                )),
                retry_after: None,
                message: None,
                raw_body: None,
                user_id: None,
                device_id: Some("device-id".into()),
            })
        })
    }
}

#[tokio::test]
async fn http_device_registrar_sends_required_registration_body() {
    let http = Arc::new(FakeHttpClient::default());
    let store = Arc::new(FakeTokenStore::default());
    let registrar = HttpDeviceRegistrar::new(
        Arc::clone(&http),
        Arc::clone(&store),
        DeviceRegistrationPayload::new(
            "1.0.0",
            vec!["document:edit".into()],
            serde_json::json!({"ipc_protocol": 11}),
        ),
    );

    registrar.register().await.unwrap();

    let requests = http.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, HttpMethod::Post);
    assert_eq!(request.path, "/v1/devices");
    assert_eq!(
        request.json_body.as_ref().unwrap(),
        &serde_json::json!({
            "client_version": "1.0.0",
            "supported_abstraction_types": ["document:edit"],
            "capabilities": {"ipc_protocol": 11}
        })
    );
    assert_eq!(
        store.load_device_id().unwrap().as_deref(),
        Some("device-id")
    );
    assert!(store.load_tokens().unwrap().is_some());
}
