use std::sync::{Mutex, MutexGuard};
use tokio::sync::watch;

use super::{TokenStore, TokenStoreError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthState {
    Unauthenticated,
    Authenticated { device_id: String },
    NeedsReauth,
    DeviceRevoked,
    RefreshInFlight,
}

pub struct AuthStateMachine {
    state: Mutex<AuthState>,
    changes: watch::Sender<AuthState>,
}

impl AuthStateMachine {
    pub fn from_token_store(
        store: &dyn TokenStore,
        device_id: impl Into<String>,
    ) -> Result<Self, TokenStoreError> {
        let state = if store.load_tokens()?.is_some() {
            AuthState::Authenticated {
                device_id: device_id.into(),
            }
        } else {
            AuthState::Unauthenticated
        };
        Ok(Self::new(state))
    }

    pub fn new(initial: AuthState) -> Self {
        let (changes, _) = watch::channel(initial.clone());
        Self {
            state: Mutex::new(initial),
            changes,
        }
    }

    pub fn current(&self) -> AuthState {
        self.lock().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<AuthState> {
        self.changes.subscribe()
    }

    pub fn transition(&self, next: AuthState) -> Result<AuthState, AuthTransitionError> {
        let mut current = self.lock();
        if !valid_transition(&current, &next) {
            return Err(AuthTransitionError::Invalid);
        }
        *current = next.clone();
        self.changes.send_replace(next.clone());
        Ok(next)
    }

    fn lock(&self) -> MutexGuard<'_, AuthState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn valid_transition(from: &AuthState, to: &AuthState) -> bool {
    from == to
        || matches!(
            (from, to),
            (AuthState::Unauthenticated, AuthState::Authenticated { .. })
                | (AuthState::Authenticated { .. }, AuthState::RefreshInFlight)
                | (AuthState::Authenticated { .. }, AuthState::NeedsReauth)
                | (AuthState::Authenticated { .. }, AuthState::DeviceRevoked)
                | (AuthState::RefreshInFlight, AuthState::Authenticated { .. })
                | (AuthState::RefreshInFlight, AuthState::NeedsReauth)
                | (AuthState::RefreshInFlight, AuthState::DeviceRevoked)
                | (AuthState::NeedsReauth, AuthState::Authenticated { .. })
                | (AuthState::NeedsReauth, AuthState::Unauthenticated)
        )
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthTransitionError {
    #[error("invalid authentication state transition")]
    Invalid,
}
