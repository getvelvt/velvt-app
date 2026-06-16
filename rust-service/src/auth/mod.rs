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
    DeviceRegistrar, DeviceRegistrationError, HttpDeviceRegistrar, NoOpDeviceRegistrar,
};
pub use http::{HttpClient, HttpMethod, HttpRequest, HttpResponse, ReqwestHttpClient};
pub use manager::{AuthError, AuthManager};
pub use state::{AuthState, AuthStateMachine, AuthTransitionError};
pub use store::{FakeTokenStore, KeychainTokenStore, TokenStore, TokenStoreError};
pub use tokens::{RedactedString, TokenPair};
