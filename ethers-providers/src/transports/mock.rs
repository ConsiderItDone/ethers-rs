use crate::{JsonRpcClient, ProviderError};

use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::{
    borrow::Borrow,
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use thiserror::Error;

/// Helper type that can be used to pass through the `params` value.
/// This is necessary because the wrapper provider is supposed to skip the `params` if it's of
/// size 0, see `crate::transports::common::Request`
#[derive(Debug)]
enum MockParams {
    Value(Value),
    Zst,
}

#[derive(Clone, Debug)]
/// Mock transport used in test environments.
pub struct MockProvider {
    requests: Arc<Mutex<VecDeque<(String, MockParams)>>>,
    responses: Arc<Mutex<VecDeque<Value>>>,
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl JsonRpcClient for MockProvider {
    type Error = MockError;

    /// Pushes the `(method, params)` to the back of the `requests` queue,
    /// pops the responses from the back of the `responses` queue
    fn request<T: Serialize + Send + Sync, R: DeserializeOwned>(
        &self,
        method: &str,
        params: T,
    ) -> Result<R, MockError> {
        let params = if std::mem::size_of::<T>() == 0 {
            MockParams::Zst
        } else {
            MockParams::Value(serde_json::to_value(params)?)
        };
        self.requests.lock().unwrap().push_back((method.to_owned(), params));
        let mut data = self.responses.lock().unwrap();
        let element = data.pop_back().ok_or(MockError::EmptyResponses)?;
        let res: R = serde_json::from_value(element)?;

        Ok(res)
    }
}

impl MockProvider {
    /// Checks that the provided request was submitted by the client
    pub fn assert_request<T: Serialize + Send + Sync>(
        &self,
        method: &str,
        data: T,
    ) -> Result<(), MockError> {
        let (m, inp) = self.requests.lock().unwrap().pop_front().ok_or(MockError::EmptyRequests)?;
        assert_eq!(m, method);
        assert!(!matches!(inp, MockParams::Value(serde_json::Value::Null)));
        if std::mem::size_of::<T>() == 0 {
            assert!(matches!(inp, MockParams::Zst));
        } else if let MockParams::Value(inp) = inp {
            assert_eq!(serde_json::to_value(data).expect("could not serialize data"), inp);
        } else {
            unreachable!("Zero sized types must be denoted with MockParams::Zst")
        }

        Ok(())
    }

    /// Instantiates a mock transport
    pub fn new() -> Self {
        Self {
            requests: Arc::new(Mutex::new(VecDeque::new())),
            responses: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Pushes the data to the responses
    pub fn push<T: Serialize + Send + Sync, K: Borrow<T>>(&self, data: K) -> Result<(), MockError> {
        let value = serde_json::to_value(data.borrow())?;
        self.responses.lock().unwrap().push_back(value);
        Ok(())
    }
}

#[derive(Error, Debug)]
/// Errors for the `MockProvider`
pub enum MockError {
    #[error(transparent)]
    SerdeJson(#[from] serde_json::Error),

    #[error("empty responses array, please push some requests")]
    EmptyRequests,

    #[error("empty responses array, please push some responses")]
    EmptyResponses,
}

impl From<MockError> for ProviderError {
    fn from(src: MockError) -> Self {
        ProviderError::JsonRpcClientError(Box::new(src))
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::*;
    use crate::Middleware;
    use ethers_core::types::U64;

    #[test]
    fn pushes_request_and_response() {
        let mock = MockProvider::new();
        mock.push(U64::from(12)).unwrap();
        let block: U64 = mock.request("eth_blockNumber", ()).unwrap();
        mock.assert_request("eth_blockNumber", ()).unwrap();
        assert_eq!(block.as_u64(), 12);
    }

    #[test]
    fn empty_responses() {
        let mock = MockProvider::new();
        // tries to get a response without pushing a response
        let err = mock.request::<_, ()>("eth_blockNumber", ()).unwrap_err();
        match err {
            MockError::EmptyResponses => {}
            _ => panic!("expected empty responses"),
        };
    }

    #[test]
    fn empty_requests() {
        let mock = MockProvider::new();
        // tries to assert a request without making one
        let err = mock.assert_request("eth_blockNumber", ()).unwrap_err();
        match err {
            MockError::EmptyRequests => {}
            _ => panic!("expected empty request"),
        };
    }

    #[test]
    fn composes_with_provider() {
        let (provider, mock) = crate::Provider::mocked();

        mock.push(U64::from(12)).unwrap();
        let block = provider.get_block_number().unwrap();
        assert_eq!(block.as_u64(), 12);
    }
}
