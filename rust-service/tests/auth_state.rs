use velvt_service::auth::{AuthState, AuthStateMachine, AuthTransitionError};

fn states() -> Vec<AuthState> {
    vec![
        AuthState::Unauthenticated,
        AuthState::Authenticated {
            device_id: "device-1".into(),
        },
        AuthState::NeedsReauth,
        AuthState::DeviceRevoked,
        AuthState::RefreshInFlight,
    ]
}

fn transition_is_valid(from: &AuthState, to: &AuthState) -> bool {
    matches!(
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
    ) || from == to
}

#[test]
fn state_machine_accepts_only_declared_transitions() {
    for from in states() {
        for to in states() {
            let machine = AuthStateMachine::new(from.clone());
            let result = machine.transition(to.clone());

            if transition_is_valid(&from, &to) {
                assert_eq!(result.unwrap(), to);
                assert_eq!(machine.current(), to);
            } else {
                assert!(matches!(result, Err(AuthTransitionError::Invalid)));
                assert_eq!(machine.current(), from);
            }
        }
    }
}

#[test]
fn device_revoked_is_terminal() {
    let machine = AuthStateMachine::new(AuthState::DeviceRevoked);

    assert!(machine
        .transition(AuthState::Authenticated {
            device_id: "device-2".into()
        })
        .is_err());
    assert_eq!(machine.current(), AuthState::DeviceRevoked);
}
