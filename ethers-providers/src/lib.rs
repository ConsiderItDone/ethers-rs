#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(unsafe_code)]
#![deny(rustdoc::broken_intra_doc_links)]
#![allow(clippy::type_complexity)]
#![doc = include_str!("../README.md")]
mod transports;
pub use transports::*;

mod provider;
pub use provider::{is_local_endpoint, FilterKind, Provider, ProviderError};

// ENS support
pub mod ens;

mod log_query;
pub use log_query::{LogQuery, LogQueryError};

pub mod call_raw;
pub mod erc;

use auto_impl::auto_impl;
use ethers_core::types::transaction::{eip2718::TypedTransaction, eip2930::AccessListWithGasUsed};
use serde::{de::DeserializeOwned, Serialize};
use std::{error::Error, fmt::Debug, pin::Pin};
use url::Url;

// feature-enabled support for dev-rpc methods
#[cfg(feature = "dev-rpc")]
pub use provider::dev_rpc::DevRpcMiddleware;

/// A simple gas escalation policy
pub type EscalationPolicy = Box<dyn Fn(U256, usize) -> U256 + Send + Sync>;

// Helper type alias
#[cfg(target_arch = "wasm32")]
pub(crate) type PinBoxFut<T> = Pin<T>;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type PinBoxFut<T> = Pin<T>;

#[auto_impl(&, Box, Arc)]
/// Trait which must be implemented by data transports to be used with the Ethereum
/// JSON-RPC provider.
pub trait JsonRpcClient: Debug + Send + Sync {
    /// A JSON-RPC Error
    type Error: Error + Into<ProviderError>;

    /// Sends a request with the provided JSON-RPC and parameters serialized as JSON
    fn request<T, R>(&self, method: &str, params: T) -> Result<R, Self::Error>
    where
        T: Debug + Serialize + Send + Sync,
        R: DeserializeOwned;
}

use ethers_core::types::*;
pub trait FromErr<T> {
    fn from(src: T) -> Self;
}

/// A middleware allows customizing requests send and received from an ethereum node.
///
/// Writing a middleware is as simple as:
/// 1. implementing the [`inner`](crate::Middleware::inner) method to point to the next layer in the
/// "middleware onion", 2. implementing the [`FromErr`](crate::FromErr) trait on your middleware's
/// error type 3. implementing any of the methods you want to override
#[auto_impl(&, Box, Arc)]
pub trait Middleware: Sync + Send + Debug {
    type Error: Sync + Send + Error + FromErr<<Self::Inner as Middleware>::Error>;
    type Provider: JsonRpcClient;
    type Inner: Middleware<Provider = Self::Provider>;

    /// The next middleware in the stack
    fn inner(&self) -> &Self::Inner;

    /// The HTTP or Websocket provider.
    fn provider(&self) -> &Provider<Self::Provider> {
        self.inner().provider()
    }

    fn default_sender(&self) -> Option<Address> {
        self.inner().default_sender()
    }

    fn client_version(&self) -> Result<String, Self::Error> {
        self.inner().client_version().map_err(FromErr::from)
    }

    /// Fill necessary details of a transaction for dispatch
    ///
    /// This function is defined on providers to behave as follows:
    /// 1. populate the `from` field with the default sender
    /// 2. resolve any ENS names in the tx `to` field
    /// 3. Estimate gas usage
    /// 4. Poll and set legacy or 1559 gas prices
    /// 5. Set the chain_id with the provider's, if not already set
    ///
    /// It does NOT set the nonce by default.
    ///
    /// Middleware are encouraged to override any values _before_ delegating
    /// to the inner implementation AND/OR modify the values provided by the
    /// default implementation _after_ delegating.
    ///
    /// E.g. a middleware wanting to double gas prices should consider doing so
    /// _after_ delegating and allowing the default implementation to poll gas.
    fn fill_transaction(
        &self,
        tx: &mut TypedTransaction,
        block: Option<BlockId>,
    ) -> Result<(), Self::Error> {
        self.inner().fill_transaction(tx, block).map_err(FromErr::from)
    }

    fn get_block_number(&self) -> Result<U64, Self::Error> {
        self.inner().get_block_number().map_err(FromErr::from)
    }

    fn resolve_name(&self, ens_name: &str) -> Result<Address, Self::Error> {
        self.inner().resolve_name(ens_name).map_err(FromErr::from)
    }

    fn lookup_address(&self, address: Address) -> Result<String, Self::Error> {
        self.inner().lookup_address(address).map_err(FromErr::from)
    }

    fn resolve_avatar(&self, ens_name: &str) -> Result<Url, Self::Error> {
        self.inner().resolve_avatar(ens_name).map_err(FromErr::from)
    }

    fn resolve_nft(&self, token: erc::ERCNFT) -> Result<Url, Self::Error> {
        self.inner().resolve_nft(token).map_err(FromErr::from)
    }

    fn resolve_field(&self, ens_name: &str, field: &str) -> Result<String, Self::Error> {
        self.inner().resolve_field(ens_name, field).map_err(FromErr::from)
    }

    fn get_block<T: Into<BlockId> + Send + Sync>(
        &self,
        block_hash_or_number: T,
    ) -> Result<Option<Block<TxHash>>, Self::Error> {
        self.inner().get_block(block_hash_or_number).map_err(FromErr::from)
    }

    fn get_block_with_txs<T: Into<BlockId> + Send + Sync>(
        &self,
        block_hash_or_number: T,
    ) -> Result<Option<Block<Transaction>>, Self::Error> {
        self.inner().get_block_with_txs(block_hash_or_number).map_err(FromErr::from)
    }

    fn get_uncle_count<T: Into<BlockId> + Send + Sync>(
        &self,
        block_hash_or_number: T,
    ) -> Result<U256, Self::Error> {
        self.inner().get_uncle_count(block_hash_or_number).map_err(FromErr::from)
    }

    fn get_uncle<T: Into<BlockId> + Send + Sync>(
        &self,
        block_hash_or_number: T,
        idx: U64,
    ) -> Result<Option<Block<H256>>, Self::Error> {
        self.inner().get_uncle(block_hash_or_number, idx).map_err(FromErr::from)
    }

    fn get_transaction_count<T: Into<NameOrAddress> + Send + Sync>(
        &self,
        from: T,
        block: Option<BlockId>,
    ) -> Result<U256, Self::Error> {
        self.inner().get_transaction_count(from, block).map_err(FromErr::from)
    }

    fn estimate_gas(
        &self,
        tx: &TypedTransaction,
        block: Option<BlockId>,
    ) -> Result<U256, Self::Error> {
        self.inner().estimate_gas(tx, block).map_err(FromErr::from)
    }

    fn call(&self, tx: &TypedTransaction, block: Option<BlockId>) -> Result<Bytes, Self::Error> {
        self.inner().call(tx, block).map_err(FromErr::from)
    }

    fn syncing(&self) -> Result<SyncingStatus, Self::Error> {
        self.inner().syncing().map_err(FromErr::from)
    }

    fn get_chainid(&self) -> Result<U256, Self::Error> {
        self.inner().get_chainid().map_err(FromErr::from)
    }

    fn get_net_version(&self) -> Result<String, Self::Error> {
        self.inner().get_net_version().map_err(FromErr::from)
    }

    fn get_balance<T: Into<NameOrAddress> + Send + Sync>(
        &self,
        from: T,
        block: Option<BlockId>,
    ) -> Result<U256, Self::Error> {
        self.inner().get_balance(from, block).map_err(FromErr::from)
    }

    fn get_transaction<T: Send + Sync + Into<TxHash>>(
        &self,
        transaction_hash: T,
    ) -> Result<Option<Transaction>, Self::Error> {
        self.inner().get_transaction(transaction_hash).map_err(FromErr::from)
    }

    fn get_transaction_receipt<T: Send + Sync + Into<TxHash>>(
        &self,
        transaction_hash: T,
    ) -> Result<Option<TransactionReceipt>, Self::Error> {
        self.inner().get_transaction_receipt(transaction_hash).map_err(FromErr::from)
    }

    fn get_block_receipts<T: Into<BlockNumber> + Send + Sync>(
        &self,
        block: T,
    ) -> Result<Vec<TransactionReceipt>, Self::Error> {
        self.inner().get_block_receipts(block).map_err(FromErr::from)
    }

    fn get_gas_price(&self) -> Result<U256, Self::Error> {
        self.inner().get_gas_price().map_err(FromErr::from)
    }

    fn estimate_eip1559_fees(
        &self,
        estimator: Option<fn(U256, Vec<Vec<U256>>) -> (U256, U256)>,
    ) -> Result<(U256, U256), Self::Error> {
        self.inner().estimate_eip1559_fees(estimator).map_err(FromErr::from)
    }

    fn get_accounts(&self) -> Result<Vec<Address>, Self::Error> {
        self.inner().get_accounts().map_err(FromErr::from)
    }

    /// This returns true if either the middleware stack contains a `SignerMiddleware`, or the
    /// JSON-RPC provider has an unlocked key that can sign using the `eth_sign` call. If none of
    /// the above conditions are met, then the middleware stack is not capable of signing data.
    fn is_signer(&self) -> bool {
        self.inner().is_signer()
    }

    fn sign<T: Into<Bytes> + Send + Sync>(
        &self,
        data: T,
        from: &Address,
    ) -> Result<Signature, Self::Error> {
        self.inner().sign(data, from).map_err(FromErr::from)
    }

    /// Sign a transaction via RPC call
    fn sign_transaction(
        &self,
        tx: &TypedTransaction,
        from: Address,
    ) -> Result<Signature, Self::Error> {
        self.inner().sign_transaction(tx, from).map_err(FromErr::from)
    }

    ////// Contract state

    fn get_logs(&self, filter: &Filter) -> Result<Vec<Log>, Self::Error> {
        self.inner().get_logs(filter).map_err(FromErr::from)
    }

    /// Returns a stream of logs are loaded in pages of given page size
    fn get_logs_paginated<'a>(
        &'a self,
        filter: &Filter,
        page_size: u64,
    ) -> LogQuery<'a, Self::Provider> {
        self.inner().get_logs_paginated(filter, page_size)
    }

    fn new_filter(&self, filter: FilterKind<'_>) -> Result<U256, Self::Error> {
        self.inner().new_filter(filter).map_err(FromErr::from)
    }

    fn uninstall_filter<T: Into<U256> + Send + Sync>(&self, id: T) -> Result<bool, Self::Error> {
        self.inner().uninstall_filter(id).map_err(FromErr::from)
    }

    fn get_filter_changes<T, R>(&self, id: T) -> Result<Vec<R>, Self::Error>
    where
        T: Into<U256> + Send + Sync,
        R: Serialize + DeserializeOwned + Send + Sync + Debug,
    {
        self.inner().get_filter_changes(id).map_err(FromErr::from)
    }

    fn get_code<T: Into<NameOrAddress> + Send + Sync>(
        &self,
        at: T,
        block: Option<BlockId>,
    ) -> Result<Bytes, Self::Error> {
        self.inner().get_code(at, block).map_err(FromErr::from)
    }

    fn get_storage_at<T: Into<NameOrAddress> + Send + Sync>(
        &self,
        from: T,
        location: H256,
        block: Option<BlockId>,
    ) -> Result<H256, Self::Error> {
        self.inner().get_storage_at(from, location, block).map_err(FromErr::from)
    }

    fn get_proof<T: Into<NameOrAddress> + Send + Sync>(
        &self,
        from: T,
        locations: Vec<H256>,
        block: Option<BlockId>,
    ) -> Result<EIP1186ProofResponse, Self::Error> {
        self.inner().get_proof(from, locations, block).map_err(FromErr::from)
    }

    // Mempool inspection for Geth's API

    fn txpool_content(&self) -> Result<TxpoolContent, Self::Error> {
        self.inner().txpool_content().map_err(FromErr::from)
    }

    fn txpool_inspect(&self) -> Result<TxpoolInspect, Self::Error> {
        self.inner().txpool_inspect().map_err(FromErr::from)
    }

    fn txpool_status(&self) -> Result<TxpoolStatus, Self::Error> {
        self.inner().txpool_status().map_err(FromErr::from)
    }

    // Geth `trace` support
    /// After replaying any previous transactions in the same block,
    /// Replays a transaction, returning the traces configured with passed options
    fn debug_trace_transaction(
        &self,
        tx_hash: TxHash,
        trace_options: GethDebugTracingOptions,
    ) -> Result<GethTrace, ProviderError> {
        self.inner().debug_trace_transaction(tx_hash, trace_options).map_err(FromErr::from)
    }

    // Parity `trace` support

    /// Executes the given call and returns a number of possible traces for it
    fn trace_call<T: Into<TypedTransaction> + Send + Sync>(
        &self,
        req: T,
        trace_type: Vec<TraceType>,
        block: Option<BlockNumber>,
    ) -> Result<BlockTrace, Self::Error> {
        self.inner().trace_call(req, trace_type, block).map_err(FromErr::from)
    }

    fn trace_call_many<T: Into<TypedTransaction> + Send + Sync>(
        &self,
        req: Vec<(T, Vec<TraceType>)>,
        block: Option<BlockNumber>,
    ) -> Result<Vec<BlockTrace>, Self::Error> {
        self.inner().trace_call_many(req, block).map_err(FromErr::from)
    }

    /// Traces a call to `eth_sendRawTransaction` without making the call, returning the traces
    fn trace_raw_transaction(
        &self,
        data: Bytes,
        trace_type: Vec<TraceType>,
    ) -> Result<BlockTrace, Self::Error> {
        self.inner().trace_raw_transaction(data, trace_type).map_err(FromErr::from)
    }

    /// Replays a transaction, returning the traces
    fn trace_replay_transaction(
        &self,
        hash: H256,
        trace_type: Vec<TraceType>,
    ) -> Result<BlockTrace, Self::Error> {
        self.inner().trace_replay_transaction(hash, trace_type).map_err(FromErr::from)
    }

    /// Replays all transactions in a block returning the requested traces for each transaction
    fn trace_replay_block_transactions(
        &self,
        block: BlockNumber,
        trace_type: Vec<TraceType>,
    ) -> Result<Vec<BlockTrace>, Self::Error> {
        self.inner().trace_replay_block_transactions(block, trace_type).map_err(FromErr::from)
    }

    /// Returns traces created at given block
    fn trace_block(&self, block: BlockNumber) -> Result<Vec<Trace>, Self::Error> {
        self.inner().trace_block(block).map_err(FromErr::from)
    }

    /// Return traces matching the given filter
    fn trace_filter(&self, filter: TraceFilter) -> Result<Vec<Trace>, Self::Error> {
        self.inner().trace_filter(filter).map_err(FromErr::from)
    }

    /// Returns trace at the given position
    fn trace_get<T: Into<U64> + Send + Sync>(
        &self,
        hash: H256,
        index: Vec<T>,
    ) -> Result<Trace, Self::Error> {
        self.inner().trace_get(hash, index).map_err(FromErr::from)
    }

    /// Returns all traces of a given transaction
    fn trace_transaction(&self, hash: H256) -> Result<Vec<Trace>, Self::Error> {
        self.inner().trace_transaction(hash).map_err(FromErr::from)
    }

    // Parity namespace

    /// Returns all receipts for that block. Must be done on a parity node.
    fn parity_block_receipts<T: Into<BlockNumber> + Send + Sync>(
        &self,
        block: T,
    ) -> Result<Vec<TransactionReceipt>, Self::Error> {
        self.inner().parity_block_receipts(block).map_err(FromErr::from)
    }

    fn fee_history<T: Into<U256> + serde::Serialize + Send + Sync>(
        &self,
        block_count: T,
        last_block: BlockNumber,
        reward_percentiles: &[f64],
    ) -> Result<FeeHistory, Self::Error> {
        self.inner().fee_history(block_count, last_block, reward_percentiles).map_err(FromErr::from)
    }

    fn create_access_list(
        &self,
        tx: &TypedTransaction,
        block: Option<BlockId>,
    ) -> Result<AccessListWithGasUsed, Self::Error> {
        self.inner().create_access_list(tx, block).map_err(FromErr::from)
    }
}

#[cfg(feature = "celo")]
pub trait CeloMiddleware: Middleware {
    fn get_validators_bls_public_keys<T: Into<BlockId> + Send + Sync>(
        &self,
        block_id: T,
    ) -> Result<Vec<String>, ProviderError> {
        self.provider().get_validators_bls_public_keys(block_id).map_err(FromErr::from)
    }
}

pub use test_provider::{GOERLI, MAINNET, RINKEBY, ROPSTEN};

/// Pre-instantiated Infura HTTP clients which rotate through multiple API keys
/// to prevent rate limits
pub mod test_provider {
    use super::*;
    use crate::Http;
    use once_cell::sync::Lazy;
    use std::{convert::TryFrom, iter::Cycle, slice::Iter, sync::Mutex};

    // List of infura keys to rotate through so we don't get rate limited
    const INFURA_KEYS: &[&str] = &[
        "6770454bc6ea42c58aac12978531b93f",
        "7a8769b798b642f6933f2ed52042bd70",
        "631fd9a6539644088297dc605d35fff3",
        "16a8be88795540b9b3903d8de0f7baa5",
        "f4a0bdad42674adab5fc0ac077ffab2b",
        "5c812e02193c4ba793f8c214317582bd",
    ];

    pub static RINKEBY: Lazy<TestProvider> =
        Lazy::new(|| TestProvider::new(INFURA_KEYS, "rinkeby"));
    pub static MAINNET: Lazy<TestProvider> =
        Lazy::new(|| TestProvider::new(INFURA_KEYS, "mainnet"));
    pub static GOERLI: Lazy<TestProvider> = Lazy::new(|| TestProvider::new(INFURA_KEYS, "goerli"));
    pub static ROPSTEN: Lazy<TestProvider> =
        Lazy::new(|| TestProvider::new(INFURA_KEYS, "ropsten"));

    #[derive(Debug)]
    pub struct TestProvider {
        network: String,
        keys: Mutex<Cycle<Iter<'static, &'static str>>>,
    }

    impl TestProvider {
        pub fn new(keys: &'static [&'static str], network: &str) -> Self {
            Self { keys: Mutex::new(keys.iter().cycle()), network: network.to_owned() }
        }

        pub fn url(&self) -> String {
            format!(
                "https://{}.infura.io/v3/{}",
                self.network,
                self.keys.lock().unwrap().next().unwrap()
            )
        }

        pub fn provider(&self) -> Provider<Http> {
            Provider::try_from(self.url().as_str()).unwrap()
        }
    }
}
