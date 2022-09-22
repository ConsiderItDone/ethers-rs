mod geometric;
use ethers_core::types::transaction::eip2718::TypedTransaction;
pub use geometric::GeometricGasPrice;

mod linear;
pub use linear::LinearGasPrice;

use async_trait::async_trait;
use ethers_core::types::{BlockId, TransactionRequest, TxHash, U256};
use ethers_providers::{FromErr, Middleware};
use futures_util::lock::Mutex;
use instant::Instant;
use std::{pin::Pin, sync::Arc};
use thiserror::Error;

#[cfg(not(target_arch = "wasm32"))]
use tokio::spawn;

#[cfg(target_arch = "wasm32")]
type WatcherFuture<'a> = Pin<Box<dyn futures_util::stream::Stream<Item = ()> + 'a>>;
#[cfg(not(target_arch = "wasm32"))]
type WatcherFuture<'a> = Pin<Box<dyn futures_util::stream::Stream<Item = ()> + Send + 'a>>;

/// Trait for fetching updated gas prices after a transaction has been first
/// broadcast
pub trait GasEscalator: Send + Sync + std::fmt::Debug {
    /// Given the initial gas price and the time elapsed since the transaction's
    /// first broadcast, it returns the new gas price
    fn get_gas_price(&self, initial_price: U256, time_elapsed: u64) -> U256;
}

#[derive(Debug, Clone)]
/// The frequency at which transactions will be bumped
pub enum Frequency {
    /// On a per block basis using the eth_newBlock filter
    PerBlock,
    /// On a duration basis (in milliseconds)
    Duration(u64),
}

#[derive(Debug)]
pub struct GasEscalatorMiddleware<M, E> {
    pub(crate) inner: Arc<M>,
    pub(crate) escalator: E,
    /// The transactions which are currently being monitored for escalation
    #[allow(clippy::type_complexity)]
    pub txs: Arc<Mutex<Vec<(TxHash, TransactionRequest, Instant, Option<BlockId>)>>>,
    frequency: Frequency,
}

impl<M, E: Clone> Clone for GasEscalatorMiddleware<M, E> {
    fn clone(&self) -> Self {
        GasEscalatorMiddleware {
            inner: self.inner.clone(),
            escalator: self.escalator.clone(),
            txs: self.txs.clone(),
            frequency: self.frequency.clone(),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<M, E> Middleware for GasEscalatorMiddleware<M, E>
where
    M: Middleware,
    E: GasEscalator,
{
    type Error = GasEscalatorError<M>;
    type Provider = M::Provider;
    type Inner = M;

    fn inner(&self) -> &M {
        &self.inner
    }
}

// Boilerplate
impl<M: Middleware> FromErr<M::Error> for GasEscalatorError<M> {
    fn from(src: M::Error) -> GasEscalatorError<M> {
        GasEscalatorError::MiddlewareError(src)
    }
}

#[derive(Error, Debug)]
/// Error thrown when the GasEscalator interacts with the blockchain
pub enum GasEscalatorError<M: Middleware> {
    #[error("{0}")]
    /// Thrown when an internal middleware errors
    MiddlewareError(M::Error),

    #[error("Gas escalation is only supported for EIP2930 or Legacy transactions")]
    UnsupportedTxType,
}
