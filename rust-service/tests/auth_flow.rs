use chrono::{Duration, Utc};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;
use velvt_service::auth::{
    AuthError, AuthManager, AuthState, AuthStateMachine, DeviceRegistrar, FakeTokenStore,
    HttpClient, HttpMethod, HttpRequest, HttpResponse, NoOpDeviceRegistrar, RedactedString,
    TokenPair, TokenStore,
};

#[derive(Clone, Default)]
struct FakeHttpClient {
    requests: Arc<Mutex<Vec<HttpRequest>>>,
    responses: Arc<Mutex<VecDeque<Result<HttpResponse, AuthError>>>>,
    refresh_delay: Option<StdDuration>,
}

impl FakeHttpClient {
    fn with_responses(responses: Vec<HttpResponse>) -> Self {
        Self {
            requests: Arc::default(),
            responses: Arc::new(Mutex::new(responses.into_iter().map(Ok).collect())),
            refresh_delay: None,
        }
    }

    fn with_results(responses: Vec<Result<HttpResponse, AuthError>>) -> Self {
        Self {
            requests: Arc::default(),
            responses: Arc::new(Mutex::new(responses.into())),
            refresh_delay: None,
        }
    }

    fn with_refresh_delay(mut self, delay: StdDuration) -> Self {
        self.refresh_delay = Some(delay);
        self
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl HttpClient for FakeHttpClient {
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let is_refresh = request.path == "/v1/auth/refresh";
            self.requests.lock().unwrap().push(request);
            if is_refresh {
                if let Some(delay) = self.refresh_delay {
                    tokio::time::sleep(delay).await;
                }
            }
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(AuthError::Transport))
        })
    }
}

fn token_pair(expires_in: Duration, access: &str, refresh: &str) -> TokenPair {
    TokenPair::new(
        RedactedString::new(access),
        RedactedString::new(refresh),
        Utc::now() + expires_in,
    )
}

fn response(status: u16, code: Option<&str>, body: Option<TokenPair>) -> HttpResponse {
    HttpResponse {
        status,
        error_code: code.map(str::to_owned),
        tokens: body,
        retry_after: None,
        message: None,
        raw_body: None,
        user_id: None,
        device_id: None,
    }
}

#[test]
fn redacted_string_never_formats_its_secret() {
    let secret = RedactedString::new("top-secret");

    assert_eq!(format!("{secret:?}"), "[redacted]");
    assert_eq!(format!("{secret}"), "[redacted]");
}

#[test]
fn token_pair_debug_output_redacts_both_tokens() {
    let tokens = token_pair(Duration::hours(1), "private-access", "private-refresh");
    let output = format!("{tokens:?}");

    assert!(!output.contains("private-access"));
    assert!(!output.contains("private-refresh"));
    assert_eq!(output.matches("[redacted]").count(), 2);
}

#[tokio::test]
async fn no_op_device_registrar_compiles_and_succeeds() {
    NoOpDeviceRegistrar.register().await.unwrap();
}

#[test]
fn fake_token_store_round_trips_without_keychain() {
    let store = FakeTokenStore::default();
    let tokens = token_pair(Duration::hours(1), "access", "refresh");

    store
        .store_tokens(
            tokens.access_token().clone(),
            tokens.refresh_token().clone(),
            tokens.expires_at(),
        )
        .unwrap();

    assert_eq!(store.load_tokens().unwrap(), Some(tokens));
    store.clear_tokens().unwrap();
    assert_eq!(store.load_tokens().unwrap(), None);
}

#[test]
fn missing_tokens_initialize_auth_state_as_unauthenticated() {
    let store = FakeTokenStore::default();

    let state = AuthStateMachine::from_token_store(&store, "device-1").unwrap();

    assert_eq!(state.current(), AuthState::Unauthenticated);
}

#[tokio::test]
async fn expired_token_is_refreshed_and_atomically_replaced_before_request() {
    let store = Arc::new(FakeTokenStore::default());
    let old = token_pair(Duration::seconds(-1), "old-access", "old-refresh");
    let fresh = token_pair(Duration::hours(1), "new-access", "new-refresh");
    store.store_pair(old).unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![
        response(200, None, Some(fresh.clone())),
        response(200, None, None),
    ]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        Arc::clone(&store),
        Arc::clone(&http),
        Arc::clone(&state),
        Duration::minutes(5),
    );

    manager
        .send_authenticated(HttpRequest::get("/v1/events"))
        .await
        .unwrap();

    assert_eq!(http.requests()[0].method, HttpMethod::Post);
    assert_eq!(http.requests()[0].path, "/v1/auth/refresh");
    assert_eq!(store.load_tokens().unwrap(), Some(fresh));
    assert_eq!(
        state.current(),
        AuthState::Authenticated {
            device_id: "device-1".into()
        }
    );
}

#[tokio::test]
async fn restored_session_validation_checks_backend_with_device_token() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(
            Duration::hours(1),
            "device-access",
            "device-refresh",
        ))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![response(
        200, None, None,
    )]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        Arc::clone(&store),
        Arc::clone(&http),
        Arc::clone(&state),
        Duration::zero(),
    );

    manager.validate_session().await.unwrap();

    let requests = http.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, HttpMethod::Get);
    assert_eq!(requests[0].path, "/v1/auth/session");
    assert!(requests[0].authorization.is_some());
    assert_eq!(
        state.current(),
        AuthState::Authenticated {
            device_id: "device-1".into()
        }
    );
}

#[tokio::test]
async fn restored_session_validation_reissues_revoked_device_token_with_user_token() {
    let store = Arc::new(FakeTokenStore::default());
    let fresh_device = token_pair(
        Duration::hours(2),
        "fresh-device-access",
        "fresh-device-refresh",
    );
    store
        .store_pair(token_pair(
            Duration::hours(1),
            "old-device-access",
            "old-device-refresh",
        ))
        .unwrap();
    store
        .store_user_pair(token_pair(
            Duration::hours(1),
            "user-access",
            "user-refresh",
        ))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![
        response(401, Some("device_token_revoked"), None),
        response(200, None, Some(fresh_device.clone())),
        response(200, None, None),
    ]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        Arc::clone(&store),
        Arc::clone(&http),
        Arc::clone(&state),
        Duration::zero(),
    );

    manager.validate_session().await.unwrap();

    let requests = http.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].path, "/v1/auth/session");
    assert_eq!(requests[1].path, "/v1/auth/devices/reissue");
    assert_eq!(requests[2].path, "/v1/auth/session");
    assert_eq!(store.load_tokens().unwrap(), Some(fresh_device));
    assert_eq!(
        state.current(),
        AuthState::Authenticated {
            device_id: "device-1".into()
        }
    );
}

#[tokio::test]
async fn refresh_transport_failure_preserves_tokens_and_retries_next_cycle() {
    let store = Arc::new(FakeTokenStore::default());
    let old = token_pair(Duration::seconds(-1), "old-access", "old-refresh");
    let fresh = token_pair(Duration::hours(1), "new-access", "new-refresh");
    store.store_pair(old.clone()).unwrap();
    let http = Arc::new(FakeHttpClient::with_results(vec![
        Err(AuthError::Transport),
        Ok(response(200, None, Some(fresh.clone()))),
        Ok(response(200, None, None)),
    ]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        Arc::clone(&store),
        Arc::clone(&http),
        Arc::clone(&state),
        Duration::zero(),
    );

    assert!(matches!(
        manager
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await,
        Err(AuthError::Transport)
    ));
    assert_eq!(store.load_tokens().unwrap(), Some(old));
    assert!(matches!(state.current(), AuthState::Authenticated { .. }));

    manager
        .send_authenticated(HttpRequest::get("/v1/events"))
        .await
        .unwrap();

    assert_eq!(
        http.requests()
            .iter()
            .filter(|request| request.path == "/v1/auth/refresh")
            .count(),
        2
    );
    assert_eq!(store.load_tokens().unwrap(), Some(fresh));
}

#[tokio::test]
async fn concurrent_expired_token_checks_send_one_refresh_request() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(Duration::seconds(-1), "access", "refresh"))
        .unwrap();
    let fresh = token_pair(Duration::hours(1), "new-access", "new-refresh");
    let http = Arc::new(
        FakeHttpClient::with_responses(vec![
            response(200, None, Some(fresh)),
            response(200, None, None),
            response(200, None, None),
        ])
        .with_refresh_delay(StdDuration::from_millis(25)),
    );
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = Arc::new(AuthManager::new(
        Arc::clone(&store),
        Arc::clone(&http),
        state,
        Duration::zero(),
    ));

    let first = {
        let manager = Arc::clone(&manager);
        tokio::spawn(async move {
            manager
                .send_authenticated(HttpRequest::get("/v1/events/first"))
                .await
        })
    };
    let second = {
        let manager = Arc::clone(&manager);
        tokio::spawn(async move {
            manager
                .send_authenticated(HttpRequest::get("/v1/events/second"))
                .await
        })
    };

    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();

    assert_eq!(
        http.requests()
            .iter()
            .filter(|request| request.path == "/v1/auth/refresh")
            .count(),
        1
    );
}

#[tokio::test]
async fn invalid_credentials_transition_to_needs_reauth_after_one_refresh_cycle() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(Duration::seconds(-1), "access", "refresh"))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![response(
        401,
        Some("invalid_credentials"),
        None,
    )]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        Arc::clone(&store),
        Arc::clone(&http),
        Arc::clone(&state),
        Duration::zero(),
    );

    assert!(matches!(
        manager
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await,
        Err(AuthError::NeedsReauth)
    ));
    assert_eq!(http.requests().len(), 1);
    assert_eq!(state.current(), AuthState::NeedsReauth);
}

#[tokio::test]
async fn invalid_credentials_refreshes_and_retries_original_request_once() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(Duration::hours(1), "access", "refresh"))
        .unwrap();
    let fresh = token_pair(Duration::hours(2), "new-access", "new-refresh");
    let http = Arc::new(FakeHttpClient::with_responses(vec![
        response(401, Some("invalid_credentials"), None),
        response(200, None, Some(fresh.clone())),
        response(200, None, None),
    ]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        Arc::clone(&store),
        Arc::clone(&http),
        Arc::clone(&state),
        Duration::zero(),
    );

    manager
        .send_authenticated(HttpRequest::get("/v1/events"))
        .await
        .unwrap();

    assert_eq!(http.requests().len(), 3);
    assert_eq!(http.requests()[1].path, "/v1/auth/refresh");
    assert_eq!(http.requests()[2].path, "/v1/events");
    assert_eq!(store.load_tokens().unwrap(), Some(fresh));
}

#[tokio::test]
async fn device_revoked_from_refresh_is_terminal() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(Duration::seconds(-1), "access", "refresh"))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![response(
        403,
        Some("device_revoked"),
        None,
    )]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        Arc::clone(&store),
        http,
        Arc::clone(&state),
        Duration::zero(),
    );

    assert!(matches!(
        manager
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await,
        Err(AuthError::DeviceRevoked)
    ));
    assert_eq!(state.current(), AuthState::DeviceRevoked);
}

#[tokio::test]
async fn device_token_revoked_from_refresh_requires_reauthentication() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(Duration::seconds(-1), "access", "refresh"))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![response(
        403,
        Some("device_token_revoked"),
        None,
    )]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        Arc::clone(&store),
        Arc::clone(&http),
        Arc::clone(&state),
        Duration::zero(),
    );

    assert!(matches!(
        manager
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await,
        Err(AuthError::NeedsReauth)
    ));
    assert_eq!(http.requests().len(), 1);
    assert_eq!(state.current(), AuthState::NeedsReauth);
}

#[tokio::test]
async fn device_token_revoked_from_refresh_reissues_with_stored_user_token() {
    let store = Arc::new(FakeTokenStore::default());
    let fresh = token_pair(Duration::hours(2), "fresh-access", "fresh-refresh");
    store
        .store_pair(token_pair(Duration::seconds(-1), "access", "refresh"))
        .unwrap();
    store
        .store_user_pair(token_pair(
            Duration::hours(1),
            "user-access",
            "user-refresh",
        ))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![
        response(403, Some("device_token_revoked"), None),
        response(200, None, Some(fresh.clone())),
        response(200, None, None),
    ]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        Arc::clone(&store),
        Arc::clone(&http),
        Arc::clone(&state),
        Duration::zero(),
    );

    manager
        .send_authenticated(HttpRequest::get("/v1/events"))
        .await
        .unwrap();

    let requests = http.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].path, "/v1/auth/refresh");
    assert_eq!(requests[1].path, "/v1/auth/devices/reissue");
    assert!(requests[1].authorization.is_some());
    assert_eq!(requests[2].path, "/v1/events");
    assert_eq!(store.load_tokens().unwrap(), Some(fresh));
    assert_eq!(
        state.current(),
        AuthState::Authenticated {
            device_id: "device-1".into()
        }
    );
}

#[tokio::test]
async fn malformed_refresh_success_transitions_to_needs_reauth() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(Duration::seconds(-1), "access", "refresh"))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![response(
        200, None, None,
    )]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        Arc::clone(&store),
        http,
        Arc::clone(&state),
        Duration::zero(),
    );

    assert!(matches!(
        manager
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await,
        Err(AuthError::InvalidResponse)
    ));
    assert_eq!(state.current(), AuthState::NeedsReauth);
}

#[tokio::test]
async fn refresh_rate_limit_is_typed_and_transitions_to_needs_reauth() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(Duration::seconds(-1), "access", "refresh"))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![response(
        429,
        Some("rate_limited"),
        None,
    )]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(store, http, Arc::clone(&state), Duration::zero());

    assert!(matches!(
        manager
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await,
        Err(AuthError::RateLimited)
    ));
    assert_eq!(state.current(), AuthState::NeedsReauth);
}

#[tokio::test]
async fn device_token_revoked_reissues_tokens_and_retries_original_request() {
    let store = Arc::new(FakeTokenStore::default());
    let fresh = token_pair(Duration::hours(2), "fresh-access", "fresh-refresh");
    store
        .store_pair(token_pair(Duration::hours(1), "access", "refresh"))
        .unwrap();
    store
        .store_user_pair(token_pair(
            Duration::hours(1),
            "user-access",
            "user-refresh",
        ))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![
        response(403, Some("device_token_revoked"), None),
        response(200, None, Some(fresh.clone())),
        response(200, None, None),
    ]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        Arc::clone(&store),
        Arc::clone(&http),
        Arc::clone(&state),
        Duration::zero(),
    );

    manager
        .send_authenticated(HttpRequest::get("/v1/events"))
        .await
        .unwrap();
    let requests = http.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].path, "/v1/events");
    assert_eq!(requests[1].path, "/v1/auth/devices/reissue");
    assert_eq!(requests[2].path, "/v1/events");
    assert_eq!(store.load_tokens().unwrap(), Some(fresh));
    assert_eq!(
        state.current(),
        AuthState::Authenticated {
            device_id: "device-1".into()
        }
    );
}

#[tokio::test]
async fn device_token_revoked_reissue_uses_stored_user_access_token() {
    let store = Arc::new(FakeTokenStore::default());
    let fresh = token_pair(Duration::hours(2), "fresh-access", "fresh-refresh");
    store
        .store_pair(token_pair(Duration::hours(1), "access", "refresh"))
        .unwrap();
    store
        .store_user_pair(token_pair(
            Duration::hours(1),
            "user-access",
            "user-refresh",
        ))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![
        response(403, Some("device_token_revoked"), None),
        response(200, None, Some(fresh)),
        response(200, None, None),
    ]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        store,
        Arc::clone(&http),
        Arc::clone(&state),
        Duration::zero(),
    );

    manager
        .send_authenticated(HttpRequest::get("/v1/events"))
        .await
        .unwrap();
    let requests = http.requests();
    assert_eq!(requests[1].path, "/v1/auth/devices/reissue");
    assert!(requests[1].authorization.is_some());
    assert!(requests[1].refresh_token.is_none());
    assert_eq!(
        requests[1].json_body.as_ref().unwrap()["device_id"],
        "device-1"
    );
}

#[tokio::test]
async fn expired_user_token_is_refreshed_before_device_token_reissue() {
    let store = Arc::new(FakeTokenStore::default());
    let fresh_user = token_pair(
        Duration::hours(1),
        "fresh-user-access",
        "fresh-user-refresh",
    );
    let fresh_device = token_pair(
        Duration::hours(2),
        "fresh-device-access",
        "fresh-device-refresh",
    );
    store
        .store_pair(token_pair(
            Duration::hours(1),
            "device-access",
            "device-refresh",
        ))
        .unwrap();
    store
        .store_user_pair(token_pair(
            Duration::seconds(-1),
            "old-user-access",
            "old-user-refresh",
        ))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![
        response(403, Some("device_token_revoked"), None),
        response(200, None, Some(fresh_user.clone())),
        response(200, None, Some(fresh_device.clone())),
        response(200, None, None),
    ]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        Arc::clone(&store),
        Arc::clone(&http),
        Arc::clone(&state),
        Duration::zero(),
    );

    manager
        .send_authenticated(HttpRequest::get("/v1/events"))
        .await
        .unwrap();

    let requests = http.requests();
    assert_eq!(requests[1].path, "/v1/auth/refresh");
    assert!(requests[1].refresh_token.is_some());
    assert_eq!(requests[2].path, "/v1/auth/devices/reissue");
    assert!(requests[2].authorization.is_some());
    assert_eq!(store.load_user_tokens().unwrap(), Some(fresh_user));
    assert_eq!(store.load_tokens().unwrap(), Some(fresh_device));
}

#[tokio::test]
async fn failed_device_token_reissue_preserves_existing_credentials() {
    let store = Arc::new(FakeTokenStore::default());
    let original = token_pair(Duration::hours(1), "access", "refresh");
    store.store_pair(original.clone()).unwrap();
    store
        .store_user_pair(token_pair(
            Duration::hours(1),
            "user-access",
            "user-refresh",
        ))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![
        response(403, Some("device_token_revoked"), None),
        response(500, Some("server_error"), None),
    ]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        Arc::clone(&store),
        Arc::clone(&http),
        Arc::clone(&state),
        Duration::zero(),
    );

    assert!(matches!(
        manager
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await,
        Err(AuthError::NeedsReauth)
    ));
    assert_eq!(http.requests().len(), 2);
    assert_eq!(store.load_tokens().unwrap(), Some(original));
    assert_eq!(state.current(), AuthState::NeedsReauth);
}

#[tokio::test]
async fn device_revoked_during_token_reissue_is_terminal() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(Duration::hours(1), "access", "refresh"))
        .unwrap();
    store
        .store_user_pair(token_pair(
            Duration::hours(1),
            "user-access",
            "user-refresh",
        ))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![
        response(403, Some("device_token_revoked"), None),
        response(403, Some("device_revoked"), None),
    ]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        Arc::clone(&store),
        Arc::clone(&http),
        Arc::clone(&state),
        Duration::zero(),
    );

    assert!(matches!(
        manager
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await,
        Err(AuthError::DeviceRevoked)
    ));
    assert_eq!(http.requests().len(), 2);
    assert!(matches!(state.current(), AuthState::DeviceRevoked));
}

#[tokio::test]
async fn device_revoked_state_blocks_all_later_authenticated_requests() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(Duration::hours(1), "access", "refresh"))
        .unwrap();
    let http = Arc::new(FakeHttpClient::default());
    let state = Arc::new(AuthStateMachine::new(AuthState::DeviceRevoked));
    let manager = AuthManager::new(store, Arc::clone(&http), state, Duration::zero());

    assert!(matches!(
        manager
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await,
        Err(AuthError::DeviceRevoked)
    ));
    assert!(http.requests().is_empty());
}

#[tokio::test]
async fn rate_limit_is_returned_without_changing_authenticated_state() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(Duration::hours(1), "access", "refresh"))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![response(
        429,
        Some("rate_limited"),
        None,
    )]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(store, http, Arc::clone(&state), Duration::zero());

    assert!(matches!(
        manager
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await,
        Err(AuthError::RateLimited)
    ));
    assert!(matches!(state.current(), AuthState::Authenticated { .. }));
}

#[tokio::test]
async fn forbidden_device_request_requires_reauthentication() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(Duration::hours(1), "access", "refresh"))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![response(
        403,
        Some("policy_denied"),
        None,
    )]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(store, http, Arc::clone(&state), Duration::zero());

    assert!(matches!(
        manager
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await,
        Err(AuthError::NeedsReauth)
    ));
    assert_eq!(state.current(), AuthState::NeedsReauth);
}
