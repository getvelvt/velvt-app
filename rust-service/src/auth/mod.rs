//! Authentication, credential storage, and device-management seams.

mod account;
mod device;
mod http;
mod manager;
mod state;
mod store;
mod tokens;

pub use account::AccountAuthService;
pub use device::{
    DeviceRegistrar, DeviceRegistrationError, DeviceRegistrationPayload, HttpDeviceRegistrar,
    NoOpDeviceRegistrar,
};
pub use http::{HttpClient, HttpMethod, HttpRequest, HttpResponse, ReqwestHttpClient};
pub use manager::{AuthError, AuthManager, SessionValidator};
pub use state::{AuthState, AuthStateMachine, AuthTransitionError};
pub use store::{FakeTokenStore, TokenStore, TokenStoreError, VolatileTokenStore};
pub use tokens::{RedactedString, TokenPair};
