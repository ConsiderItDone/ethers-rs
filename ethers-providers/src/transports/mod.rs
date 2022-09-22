mod common;
pub use common::Authorization;

#[cfg(all(target_family = "unix", feature = "ipc"))]
mod ipc;
#[cfg(all(target_family = "unix", feature = "ipc"))]
pub use ipc::{Ipc, IpcError};

mod http;
pub use self::http::{ClientError as HttpClientError, Provider as Http};

mod rw;
pub use rw::{RwClient, RwClientError};

#[cfg(not(target_arch = "wasm32"))]
mod retry;
#[cfg(not(target_arch = "wasm32"))]
pub use retry::*;

mod mock;
pub use mock::{MockError, MockProvider};
