use chrono::{Duration, Utc};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;
use velvt_service::auth::{
    AuthError, AuthManager, AuthState, AuthStateMachine, ClientVersionReport, DeviceRegistrar,
    FakeTokenStore, HttpClient, HttpMethod, HttpRequest, HttpResponse, NoOpDeviceRegistrar,
    RedactedString, TokenPair, TokenStore,
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

/// An access token inside the refresh buffer has not expired: the cloud still
/// accepts it. A refresh that fails in transport must not stop it being used,
/// must not move the auth state Swift shows, and must not be tried again by
/// every request for the next minute, each waiting out its own timeout.
#[tokio::test]
async fn a_refresh_that_fails_in_transport_keeps_a_token_that_has_not_expired() {
    let store = Arc::new(FakeTokenStore::default());
    let current = token_pair(Duration::minutes(2), "current-access", "current-refresh");
    store.store_pair(current.clone()).unwrap();
    let http = Arc::new(FakeHttpClient::with_results(vec![
        Err(AuthError::Transport),
        Ok(response(200, None, None)),
        Ok(response(200, None, None)),
    ]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let states = state.subscribe();
    let manager = AuthManager::new(
        Arc::clone(&store),
        Arc::clone(&http),
        Arc::clone(&state),
        Duration::minutes(5),
    );

    for _ in 0..2 {
        let answered = manager
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await
            .unwrap();
        assert_eq!(answered.status, 200);
    }

    let paths = http
        .requests()
        .into_iter()
        .map(|request| request.path)
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec!["/v1/auth/refresh", "/v1/events", "/v1/events"],
        "the failed refresh was tried again within the minute"
    );
    assert!(http.requests()[1..]
        .iter()
        .all(|request| request.authorization.as_ref() == Some(current.access_token())));
    assert_eq!(store.load_tokens().unwrap(), Some(current));
    assert!(
        !states.has_changed().unwrap(),
        "a transport failure moved the auth state"
    );
}

/// An expired token cannot be used, and a refresh that failed in transport a
/// moment ago will not get through either: the request fails at once instead
/// of waiting out another timeout.
#[tokio::test]
async fn an_expired_token_fails_at_once_while_a_refresh_failure_is_remembered() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(
            Duration::seconds(-1),
            "old-access",
            "old-refresh",
        ))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_results(vec![Err(
        AuthError::Transport,
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

    for _ in 0..3 {
        assert!(matches!(
            manager
                .send_authenticated(HttpRequest::get("/v1/events"))
                .await,
            Err(AuthError::Transport)
        ));
    }

    assert_eq!(http.requests().len(), 1, "the refresh was tried again");
    assert!(matches!(state.current(), AuthState::Authenticated { .. }));
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
    )
    // The next cycle, without waiting out the minute a failure is remembered.
    .with_refresh_transport_retry_after(StdDuration::ZERO);

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

/// A rate limit is not a verdict on the session. A refresh it refused used
/// to end the session, and with it the person's sign-in and Keychain.
#[tokio::test]
async fn a_rate_limited_refresh_is_typed_and_keeps_the_session() {
    let store = Arc::new(FakeTokenStore::default());
    let expired = token_pair(Duration::seconds(-1), "access", "refresh");
    store.store_pair(expired.clone()).unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![response(
        429,
        Some("rate_limited"),
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
        Err(AuthError::RateLimited { .. })
    ));
    assert!(matches!(state.current(), AuthState::Authenticated { .. }));
    assert_eq!(store.load_tokens().unwrap(), Some(expired));
}

/// A refresh answered by an outage (here a Cloudflare 522 with no JSON) keeps
/// the session, and the token in hand, for the next attempt, which waits a
/// minute: a request straight after it neither refreshes again nor is sent
/// with the expired token.
#[tokio::test]
async fn a_refresh_answered_by_an_outage_keeps_the_session() {
    let store = Arc::new(FakeTokenStore::default());
    let expired = token_pair(Duration::seconds(-1), "access", "refresh");
    store.store_pair(expired.clone()).unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![
        response(522, None, None),
        response(200, None, Some(token_pair(Duration::hours(1), "a", "r"))),
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

    for attempt in ["first", "straight after"] {
        assert!(
            matches!(
                manager
                    .send_authenticated(HttpRequest::get("/v1/events"))
                    .await,
                Err(AuthError::Transport)
            ),
            "{attempt}"
        );
    }
    assert_eq!(
        http.requests().len(),
        1,
        "the refresh is not tried again within the minute"
    );
    assert!(matches!(state.current(), AuthState::Authenticated { .. }));
    assert_eq!(store.load_tokens().unwrap(), Some(expired));
}

/// A reissue answered by an outage backs off as a refresh does. Every request
/// answered `device_token_revoked` while the edge kept refusing used to POST
/// one more reissue.
#[tokio::test]
async fn a_reissue_answered_by_an_outage_is_not_tried_again_for_a_minute() {
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
        response(503, None, None),
        response(403, Some("device_token_revoked"), None),
        response(200, None, Some(token_pair(Duration::hours(1), "a", "r"))),
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

    for attempt in ["first", "straight after"] {
        assert!(
            matches!(
                manager
                    .send_authenticated(HttpRequest::get("/v1/events"))
                    .await,
                Err(AuthError::Transport)
            ),
            "{attempt}"
        );
    }
    assert_eq!(
        http.requests()
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>(),
        vec!["/v1/events", "/v1/auth/devices/reissue", "/v1/events"],
        "the second request is sent with the token in hand and does not reissue"
    );
    assert!(matches!(state.current(), AuthState::Authenticated { .. }));
    assert_eq!(store.load_tokens().unwrap(), Some(original));
}

/// The same for the user-token refresh a reissue needs first.
#[tokio::test]
async fn a_user_token_refresh_answered_by_an_outage_is_not_tried_again_for_a_minute() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(Duration::hours(1), "access", "refresh"))
        .unwrap();
    store
        .store_user_pair(token_pair(
            Duration::seconds(-1),
            "user-access",
            "user-refresh",
        ))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![
        response(403, Some("device_token_revoked"), None),
        response(522, None, None),
        response(403, Some("device_token_revoked"), None),
        response(200, None, Some(token_pair(Duration::hours(1), "a", "r"))),
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

    for attempt in ["first", "straight after"] {
        assert!(
            matches!(
                manager
                    .send_authenticated(HttpRequest::get("/v1/events"))
                    .await,
                Err(AuthError::Transport)
            ),
            "{attempt}"
        );
    }
    assert_eq!(
        http.requests()
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>(),
        vec!["/v1/events", "/v1/auth/refresh", "/v1/events"]
    );
    assert!(matches!(state.current(), AuthState::Authenticated { .. }));
}

/// Once the minute is up, the reissue is tried again.
#[tokio::test]
async fn a_reissue_is_tried_again_once_its_back_off_has_passed() {
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
    let reissued = token_pair(Duration::hours(1), "reissued-access", "reissued-refresh");
    let http = Arc::new(FakeHttpClient::with_responses(vec![
        response(403, Some("device_token_revoked"), None),
        response(503, None, None),
        response(403, Some("device_token_revoked"), None),
        response(200, None, Some(reissued.clone())),
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
    )
    .with_refresh_transport_retry_after(StdDuration::from_millis(20));

    assert!(matches!(
        manager
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await,
        Err(AuthError::Transport)
    ));
    tokio::time::sleep(StdDuration::from_millis(40)).await;
    let retried = manager
        .send_authenticated(HttpRequest::get("/v1/events"))
        .await
        .unwrap();

    assert_eq!(retried.status, 200);
    assert_eq!(store.load_tokens().unwrap(), Some(reissued));
}

/// A refresh the cloud refuses with a session code still ends the session.
#[tokio::test]
async fn a_refresh_refused_with_a_session_code_needs_reauthentication() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(Duration::seconds(-1), "access", "refresh"))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![response(
        401,
        Some("invalid_token"),
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

    // A reissue that failed on the server is not a verdict on the session:
    // the next request tries again.
    assert!(matches!(
        manager
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await,
        Err(AuthError::Transport)
    ));
    assert_eq!(http.requests().len(), 2);
    assert_eq!(store.load_tokens().unwrap(), Some(original));
    assert!(matches!(state.current(), AuthState::Authenticated { .. }));
}

/// Reissue answers a revoked device with `403 device_inactive`, which is the
/// same verdict as `device_revoked`.
#[tokio::test]
async fn device_inactive_during_token_reissue_is_terminal() {
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
        response(403, Some("device_inactive"), None),
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

    assert!(matches!(
        manager
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await,
        Err(AuthError::DeviceRevoked)
    ));
    assert_eq!(state.current(), AuthState::DeviceRevoked);
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
        Err(AuthError::RateLimited { .. })
    ));
    assert!(matches!(state.current(), AuthState::Authenticated { .. }));
}

/// critic-04. One 403 from Cloudflare (WAF, Bot Fight Mode, a challenge:
/// "error code: 1020", no JSON) signed the tester out, wiped the Keychain and
/// stopped the insight poll, with nothing to recover it. Only a code the cloud
/// attributes to the session may end it; anything else goes back to the caller
/// as the status it is, and the caller backs off.
#[tokio::test]
async fn a_403_without_a_session_code_keeps_the_session() {
    for code in [None, Some("policy_denied")] {
        let store = Arc::new(FakeTokenStore::default());
        let current = token_pair(Duration::hours(1), "access", "refresh");
        store.store_pair(current.clone()).unwrap();
        let http = Arc::new(FakeHttpClient::with_responses(vec![response(
            403, code, None,
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

        let answer = manager
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await;

        assert!(
            matches!(answer, Ok(ref response) if response.status == 403),
            "{code:?}: the caller gets the status, not a sign-out"
        );
        assert!(matches!(state.current(), AuthState::Authenticated { .. }));
        assert_eq!(store.load_tokens().unwrap(), Some(current));
    }
}

/// `401 account_deleted` is the cloud saying the account is gone: the session
/// ends as deleted, which the service answers by destroying what it queued
/// for that account (`ipc::end_deleted_account_session`).
#[tokio::test]
async fn account_deleted_ends_the_session_as_deleted() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(Duration::hours(1), "access", "refresh"))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![response(
        401,
        Some("account_deleted"),
        None,
    )]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        store,
        Arc::clone(&http),
        Arc::clone(&state),
        Duration::zero(),
    );

    assert!(matches!(
        manager
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await,
        Err(AuthError::AccountDeleted)
    ));
    assert_eq!(state.current(), AuthState::AccountDeleted);
    assert_eq!(
        http.requests().len(),
        1,
        "a deleted account is not one refresh away from working"
    );
    assert!(matches!(
        manager
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await,
        Err(AuthError::AccountDeleted)
    ));
    assert_eq!(http.requests().len(), 1, "and nothing more is sent for it");
}

/// The same verdict on a refresh.
#[tokio::test]
async fn account_deleted_on_refresh_ends_the_session_as_deleted() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(Duration::seconds(-1), "access", "refresh"))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![response(
        401,
        Some("account_deleted"),
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
        Err(AuthError::AccountDeleted)
    ));
    assert_eq!(state.current(), AuthState::AccountDeleted);
}

/// A restored session whose client named no account takes it from the
/// cloud's answer to the validation, so its queue is owned from then on.
#[tokio::test]
async fn validating_a_session_keeps_the_account_the_cloud_names() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(Duration::hours(1), "access", "refresh"))
        .unwrap();
    let http = Arc::new(FakeHttpClient::with_responses(vec![HttpResponse {
        user_id: Some("account-7".into()),
        ..response(200, None, None)
    }]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(Arc::clone(&store), http, state, Duration::zero());

    manager.validate_session().await.unwrap();

    assert_eq!(store.load_user_id().unwrap().as_deref(), Some("account-7"));
}

/// What `AccountAuthService::log_out` does to the local session, without
/// waiting on the refresh lock.
fn log_out_locally(store: &FakeTokenStore, state: &AuthStateMachine) {
    let _ = state.transition(AuthState::Unauthenticated);
    store.clear_tokens().unwrap();
    store.clear_user_tokens().unwrap();
    let _ = state.transition(AuthState::Unauthenticated);
}

async fn refresh_raced_by(
    expires_in: Duration,
    during_refresh: impl FnOnce(&FakeTokenStore, &AuthStateMachine),
) -> (
    Result<(), AuthError>,
    Arc<FakeTokenStore>,
    Arc<AuthStateMachine>,
    Arc<FakeHttpClient>,
) {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(expires_in, "old-access", "old-refresh"))
        .unwrap();
    let fresh = token_pair(Duration::hours(1), "refreshed-access", "refreshed-refresh");
    let http = Arc::new(
        FakeHttpClient::with_responses(vec![
            response(200, None, Some(fresh)),
            response(200, None, None),
        ])
        .with_refresh_delay(StdDuration::from_millis(300)),
    );
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = Arc::new(AuthManager::new(
        Arc::clone(&store),
        Arc::clone(&http),
        Arc::clone(&state),
        Duration::minutes(5),
    ));
    let validation = {
        let manager = Arc::clone(&manager);
        tokio::spawn(async move { manager.validate_session().await })
    };
    tokio::time::sleep(StdDuration::from_millis(100)).await;
    during_refresh(&store, &state);
    let result = validation.await.unwrap();
    (result, store, state, http)
}

#[tokio::test]
async fn sign_out_during_a_refresh_ahead_of_expiry_stays_signed_out() {
    let (result, store, state, http) =
        refresh_raced_by(Duration::minutes(2), log_out_locally).await;

    assert!(matches!(result, Err(AuthError::NeedsReauth)));
    assert_eq!(state.current(), AuthState::Unauthenticated);
    assert_eq!(store.load_tokens().unwrap(), None);
    assert_eq!(
        http.requests()
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>(),
        vec!["/v1/auth/refresh"]
    );
}

#[tokio::test]
async fn sign_out_during_a_refresh_of_an_expired_token_stays_signed_out() {
    let (result, store, state, _) = refresh_raced_by(Duration::seconds(-1), log_out_locally).await;

    assert!(matches!(result, Err(AuthError::NeedsReauth)));
    assert_eq!(state.current(), AuthState::Unauthenticated);
    assert_eq!(store.load_tokens().unwrap(), None);
}

#[tokio::test]
async fn login_during_a_refresh_keeps_its_own_tokens() {
    let login = token_pair(Duration::hours(1), "login-access", "login-refresh");
    let stored_login = login.clone();
    let (result, store, state, _) = refresh_raced_by(Duration::minutes(2), move |store, state| {
        log_out_locally(store, state);
        store.store_pair(stored_login).unwrap();
        state
            .transition(AuthState::Authenticated {
                device_id: "device-1".into(),
            })
            .unwrap();
    })
    .await;

    assert!(matches!(result, Err(AuthError::NeedsReauth)));
    assert_eq!(
        state.current(),
        AuthState::Authenticated {
            device_id: "device-1".into()
        }
    );
    assert_eq!(store.load_tokens().unwrap(), Some(login));
}

// ---------------------------------------------------------------------------
// The running version, once per launch (install-release-09, velvt-core #41)
// ---------------------------------------------------------------------------

fn signed_in_manager(
    http: Arc<FakeHttpClient>,
) -> (
    AuthManager<FakeTokenStore, FakeHttpClient>,
    Arc<AuthStateMachine>,
) {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(Duration::hours(1), "access", "refresh"))
        .unwrap();
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(store, http, Arc::clone(&state), Duration::zero());
    (manager, state)
}

/// velvt-core asks for `PATCH /v1/devices/{id}` with the running version
/// once per launch: registration records it once, so a device row named the
/// build the Mac first signed in with.
#[tokio::test]
async fn the_running_version_is_reported_once_per_launch() {
    let http = Arc::new(FakeHttpClient::with_responses(vec![response(
        200, None, None,
    )]));
    let (manager, state) = signed_in_manager(Arc::clone(&http));

    assert_eq!(
        manager.report_client_version().await,
        ClientVersionReport::Recorded
    );
    assert_eq!(
        manager.report_client_version().await,
        ClientVersionReport::AlreadyAnswered
    );

    let requests = http.requests();
    assert_eq!(requests.len(), 1, "one request per launch");
    assert_eq!(requests[0].method, HttpMethod::Patch);
    assert_eq!(requests[0].path, "/v1/devices/device-1");
    assert_eq!(
        requests[0].json_body,
        Some(serde_json::json!({
            "client_version": velvt_service::build_info::SERVICE_VERSION
        })),
        "the version registration and every batch already send, and nothing else"
    );
    assert_eq!(
        requests[0].authorization,
        Some(RedactedString::new("access"))
    );
    assert!(matches!(state.current(), AuthState::Authenticated { .. }));
}

/// A server without the route (404, 405), or one that refuses the body, has
/// answered: the session is untouched and nothing is sent again this launch.
#[tokio::test]
async fn a_server_that_does_not_take_the_version_is_answered_once_and_left_alone() {
    for (status, code) in [
        (404, None),
        (405, None),
        (404, Some("device_not_found")),
        (403, Some("device_forbidden")),
        (422, Some("validation_error")),
    ] {
        let http = Arc::new(FakeHttpClient::with_responses(vec![response(
            status, code, None,
        )]));
        let (manager, state) = signed_in_manager(Arc::clone(&http));

        assert_eq!(
            manager.report_client_version().await,
            ClientVersionReport::Declined { status },
            "{status} {code:?}"
        );
        assert_eq!(
            manager.report_client_version().await,
            ClientVersionReport::AlreadyAnswered,
            "{status} {code:?}"
        );
        assert_eq!(http.requests().len(), 1, "{status} {code:?}");
        assert_eq!(
            state.current(),
            AuthState::Authenticated {
                device_id: "device-1".into()
            },
            "{status} {code:?}: the session stands"
        );
    }
}

/// No answer is not an answer: the next restored session asks again, until
/// one comes.
#[tokio::test]
async fn a_report_that_gets_no_answer_is_sent_again_at_the_next_restore() {
    let mut rate_limited = response(429, Some("rate_limit_exceeded"), None);
    rate_limited.retry_after = Some("30".into());
    let http = Arc::new(FakeHttpClient::with_results(vec![
        Err(AuthError::Transport),
        Ok(response(503, None, None)),
        Ok(rate_limited),
        Ok(response(200, None, None)),
    ]));
    let (manager, state) = signed_in_manager(Arc::clone(&http));

    for _ in 0..3 {
        assert_eq!(
            manager.report_client_version().await,
            ClientVersionReport::Deferred
        );
    }
    assert_eq!(
        manager.report_client_version().await,
        ClientVersionReport::Recorded
    );
    assert_eq!(
        manager.report_client_version().await,
        ClientVersionReport::AlreadyAnswered
    );
    assert_eq!(http.requests().len(), 4);
    assert!(matches!(state.current(), AuthState::Authenticated { .. }));
}

#[tokio::test]
async fn nothing_is_reported_without_a_session() {
    let http = Arc::new(FakeHttpClient::default());
    let store = Arc::new(FakeTokenStore::default());
    let state = Arc::new(AuthStateMachine::new(AuthState::Unauthenticated));
    let manager = AuthManager::new(store, Arc::clone(&http), state, Duration::zero());

    assert_eq!(
        manager.report_client_version().await,
        ClientVersionReport::Deferred
    );
    assert!(http.requests().is_empty());
}

// ---------------------------------------------------------------------------
// One reissue for requests refused together (velvt-core #41, follow-up 3)
// ---------------------------------------------------------------------------

/// Refuses every `/v1/events` request sent with the revoked token, but only
/// once `refused_together` of them have arrived, and answers the reissue.
struct RevokedTokenCloud {
    requests: Mutex<Vec<HttpRequest>>,
    arrived: tokio::sync::Barrier,
}

impl HttpClient for RevokedTokenCloud {
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request.clone());
            if request.path == "/v1/auth/devices/reissue" {
                let reissues = self
                    .requests
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|request| request.path == "/v1/auth/devices/reissue")
                    .count();
                let access = format!("reissued-access-{reissues}");
                return Ok(response(
                    200,
                    None,
                    Some(token_pair(Duration::hours(1), &access, "reissued-refresh")),
                ));
            }
            if request.authorization == Some(RedactedString::new("revoked-access")) {
                self.arrived.wait().await;
                return Ok(response(401, Some("device_token_revoked"), None));
            }
            Ok(response(200, None, None))
        })
    }
}

/// Two requests refused as `device_token_revoked` at once each ran a reissue,
/// one after the other behind the refresh lock. velvt-core keeps every pair a
/// burst replaced working for its grace, so nothing broke, but the second
/// reissue was redundant. The one that waits now finds the tokens already
/// replaced since it was refused, and retries with them.
#[tokio::test]
async fn requests_refused_together_share_one_reissue() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(
            Duration::hours(1),
            "revoked-access",
            "revoked-refresh",
        ))
        .unwrap();
    store
        .store_user_pair(token_pair(
            Duration::hours(1),
            "user-access",
            "user-refresh",
        ))
        .unwrap();
    let cloud = Arc::new(RevokedTokenCloud {
        requests: Mutex::default(),
        arrived: tokio::sync::Barrier::new(2),
    });
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        Arc::clone(&store),
        Arc::clone(&cloud),
        Arc::clone(&state),
        Duration::zero(),
    );

    let (first, second) = tokio::join!(
        manager.send_authenticated(HttpRequest::get("/v1/events")),
        manager.send_authenticated(HttpRequest::get("/v1/history/daily?days=7")),
    );

    assert_eq!(first.unwrap().status, 200);
    assert_eq!(second.unwrap().status, 200);
    let requests = cloud.requests.lock().unwrap().clone();
    let reissues = requests
        .iter()
        .filter(|request| request.path == "/v1/auth/devices/reissue")
        .count();
    assert_eq!(reissues, 1, "one reissue answers both refusals");
    let reissued = Some(RedactedString::new("reissued-access-1"));
    let retried_with = requests
        .iter()
        .skip(3)
        .map(|request| request.authorization.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        retried_with,
        vec![reissued.clone(), reissued.clone()],
        "both retries carry the one reissued token"
    );
    assert_eq!(
        store
            .load_tokens()
            .unwrap()
            .map(|tokens| tokens.access_token().clone()),
        reissued
    );
    assert!(matches!(state.current(), AuthState::Authenticated { .. }));
}

/// A request refused with the very tokens a reissue just stored is not
/// handed them back: it reissues, as before.
#[tokio::test]
async fn a_refusal_of_the_stored_tokens_still_reissues() {
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
    let first = token_pair(Duration::hours(1), "first-reissue", "first-refresh");
    let second = token_pair(Duration::hours(1), "second-reissue", "second-refresh");
    let http = Arc::new(FakeHttpClient::with_responses(vec![
        response(401, Some("device_token_revoked"), None),
        response(200, None, Some(first)),
        response(200, None, None),
        response(401, Some("device_token_revoked"), None),
        response(200, None, Some(second.clone())),
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
    manager
        .send_authenticated(HttpRequest::get("/v1/events"))
        .await
        .unwrap();

    let paths = http
        .requests()
        .into_iter()
        .map(|request| request.path)
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec![
            "/v1/events",
            "/v1/auth/devices/reissue",
            "/v1/events",
            "/v1/events",
            "/v1/auth/devices/reissue",
            "/v1/events",
        ]
    );
    assert_eq!(store.load_tokens().unwrap(), Some(second));
}
