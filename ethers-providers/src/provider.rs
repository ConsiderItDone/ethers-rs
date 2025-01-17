use crate::{
    call_raw::CallBuilder, ens, erc, FromErr, Http as HttpProvider, JsonRpcClient, LogQuery,
    MockProvider, SyncingStatus,
};

#[cfg(feature = "celo")]
use crate::CeloMiddleware;
use crate::Middleware;

use ethers_core::{
    abi::{self, Detokenize, ParamType},
    types::{
        transaction::{eip2718::TypedTransaction, eip2930::AccessListWithGasUsed},
        Address, Block, BlockId, BlockNumber, BlockTrace, Bytes, EIP1186ProofResponse, FeeHistory,
        Filter, FilterBlockOption, GethDebugTracingOptions, GethTrace, Log, NameOrAddress,
        Selector, Signature, Trace, TraceFilter, TraceType, Transaction, TransactionReceipt,
        TransactionRequest, TxHash, TxpoolContent, TxpoolInspect, TxpoolStatus, H256, U256, U64,
    },
    utils,
};
use hex::FromHex;
use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;
use url::{ParseError, Url};

use ethers_core::types::Chain;
use std::{
    collections::VecDeque, convert::TryFrom, fmt::Debug, str::FromStr, sync::Arc, time::Duration,
};
use tracing::trace;

#[derive(Copy, Clone, Debug)]
pub enum NodeClient {
    Geth,
    Erigon,
    OpenEthereum,
    Nethermind,
    Besu,
}

impl FromStr for NodeClient {
    type Err = ProviderError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.split('/').next().unwrap().to_lowercase().as_str() {
            "geth" => Ok(NodeClient::Geth),
            "erigon" => Ok(NodeClient::Erigon),
            "openethereum" => Ok(NodeClient::OpenEthereum),
            "nethermind" => Ok(NodeClient::Nethermind),
            "besu" => Ok(NodeClient::Besu),
            _ => Err(ProviderError::UnsupportedNodeClient),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Provider<P> {
    inner: P,
    ens: Option<Address>,
    interval: Option<Duration>,
    from: Option<Address>,
    /// Node client hasn't been checked yet = `None`
    /// Unsupported node client = `Some(None)`
    /// Supported node client = `Some(Some(NodeClient))`
    _node_client: Option<NodeClient>,
}

impl<P> AsRef<P> for Provider<P> {
    fn as_ref(&self) -> &P {
        &self.inner
    }
}

impl FromErr<ProviderError> for ProviderError {
    fn from(src: ProviderError) -> Self {
        src
    }
}

#[derive(Debug, Error)]
/// An error thrown when making a call to the provider
pub enum ProviderError {
    /// An internal error in the JSON RPC Client
    #[error(transparent)]
    JsonRpcClientError(#[from] Box<dyn std::error::Error + Send + Sync>),

    /// An error during ENS name resolution
    #[error("ens name not found: {0}")]
    EnsError(String),

    /// Invalid reverse ENS name
    #[error("reverse ens name not pointing to itself: {0}")]
    EnsNotOwned(String),

    #[error(transparent)]
    SerdeJson(#[from] serde_json::Error),

    #[error(transparent)]
    HexError(#[from] hex::FromHexError),

    #[error(transparent)]
    HTTPError(#[from] reqwest::Error),

    #[error("custom error: {0}")]
    CustomError(String),

    #[error("unsupported RPC")]
    UnsupportedRPC,

    #[error("unsupported node client")]
    UnsupportedNodeClient,

    #[error("Attempted to sign a transaction with no available signer. Hint: did you mean to use a SignerMiddleware?")]
    SignerUnavailable,
}

/// Types of filters supported by the JSON-RPC.
#[derive(Clone, Debug)]
pub enum FilterKind<'a> {
    /// `eth_newBlockFilter`
    Logs(&'a Filter),

    /// `eth_newBlockFilter` filter
    NewBlocks,
}

// JSON RPC bindings
impl<P: JsonRpcClient> Provider<P> {
    /// Instantiate a new provider with a backend.
    pub fn new(provider: P) -> Self {
        Self { inner: provider, ens: None, interval: None, from: None, _node_client: None }
    }

    /// Returns the type of node we're connected to, while also caching the value for use
    /// in other node-specific API calls, such as the get_block_receipts call.
    pub fn node_client(&self) -> Result<NodeClient, ProviderError> {
        let mut node_client = self._node_client;

        if let Some(node_client) = node_client {
            Ok(node_client)
        } else {
            let client_version = self.client_version()?;
            let client_version = match client_version.parse::<NodeClient>() {
                Ok(res) => res,
                Err(_) => return Err(ProviderError::UnsupportedNodeClient),
            };
            node_client = Some(client_version);
            Ok(client_version)
        }
    }

    #[must_use]
    pub fn with_sender(mut self, address: impl Into<Address>) -> Self {
        self.from = Some(address.into());
        self
    }

    pub fn request<T, R>(&self, method: &str, params: T) -> Result<R, ProviderError>
    where
        T: Debug + Serialize + Send + Sync,
        R: Serialize + DeserializeOwned + Debug,
    {
        let res = {
            let res: R = self.inner.request(method, params).map_err(Into::into)?;
            Ok::<_, ProviderError>(res)
        }?;
        Ok(res)
    }

    fn get_block_gen<Tx: Default + Serialize + DeserializeOwned + Debug>(
        &self,
        id: BlockId,
        include_txs: bool,
    ) -> Result<Option<Block<Tx>>, ProviderError> {
        let include_txs = utils::serialize(&include_txs);

        Ok(match id {
            BlockId::Hash(hash) => {
                let hash = utils::serialize(&hash);
                self.request("eth_getBlockByHash", [hash, include_txs])?
            }
            BlockId::Number(num) => {
                let num = utils::serialize(&num);
                self.request("eth_getBlockByNumber", [num, include_txs])?
            }
        })
    }

    /// Analogous to [`Middleware::call`], but returns a [`CallBuilder`] that can either be
    /// ``d or used to override the parameters sent to `eth_call`.
    ///
    /// See the [`call_raw::spoof`] for functions to construct state override parameters.
    ///
    /// Note: this method _does not_ send a transaction from your account
    ///
    /// [`call_raw::spoof`]: crate::call_raw::spoof
    pub fn call_raw<'a>(&'a self, tx: &'a TypedTransaction) -> CallBuilder<'a, P> {
        CallBuilder::new(self, tx)
    }
}

#[cfg(feature = "celo")]
impl<P: JsonRpcClient> CeloMiddleware for Provider<P> {
    fn get_validators_bls_public_keys<T: Into<BlockId> + Send + Sync>(
        &self,
        block_id: T,
    ) -> Result<Vec<String>, ProviderError> {
        let block_id = utils::serialize(&block_id.into());
        self.request("istanbul_getValidatorsBLSPublicKeys", [block_id])
    }
}

impl<P: JsonRpcClient> Middleware for Provider<P> {
    type Error = ProviderError;
    type Provider = P;
    type Inner = Self;

    fn inner(&self) -> &Self::Inner {
        unreachable!("There is no inner provider here")
    }

    fn provider(&self) -> &Provider<Self::Provider> {
        self
    }

    fn default_sender(&self) -> Option<Address> {
        self.from
    }

    ////// Blockchain Status
    //
    // Functions for querying the state of the blockchain

    /// Returns the current client version using the `web3_clientVersion` RPC.
    fn client_version(&self) -> Result<String, Self::Error> {
        self.request("web3_clientVersion", ())
    }

    fn fill_transaction(
        &self,
        tx: &mut TypedTransaction,
        block: Option<BlockId>,
    ) -> Result<(), Self::Error> {
        if let Some(default_sender) = self.default_sender() {
            if tx.from().is_none() {
                tx.set_from(default_sender);
            }
        }

        // TODO: Join the name resolution and gas price future

        // set the ENS name
        if let Some(NameOrAddress::Name(ref ens_name)) = tx.to() {
            let addr = self.resolve_name(ens_name)?;
            tx.set_to(addr);
        }

        // fill gas price
        match tx {
            TypedTransaction::Eip2930(_) | TypedTransaction::Legacy(_) => {
                let gas_price = match tx.gas_price() {
                    Some(item) => item,
                    None => self.get_gas_price()?,
                };

                tx.set_gas_price(gas_price);
            }
            TypedTransaction::Eip1559(ref mut inner) => {
                if inner.max_fee_per_gas.is_none() || inner.max_priority_fee_per_gas.is_none() {
                    let (max_fee_per_gas, max_priority_fee_per_gas) =
                        self.estimate_eip1559_fees(None)?;
                    inner.max_fee_per_gas = Some(max_fee_per_gas);
                    inner.max_priority_fee_per_gas = Some(max_priority_fee_per_gas);
                };
            }
        }

        // Set gas to estimated value only if it was not set by the caller,
        // even if the access list has been populated and saves gas
        if tx.gas().is_none() {
            let gas_estimate = self.estimate_gas(tx, block)?;
            tx.set_gas(gas_estimate);
        }

        Ok(())
    }

    /// Gets the latest block number via the `eth_BlockNumber` API
    fn get_block_number(&self) -> Result<U64, ProviderError> {
        self.request("eth_blockNumber", ())
    }

    /// Gets the block at `block_hash_or_number` (transaction hashes only)
    fn get_block<T: Into<BlockId> + Send + Sync>(
        &self,
        block_hash_or_number: T,
    ) -> Result<Option<Block<TxHash>>, Self::Error> {
        self.get_block_gen(block_hash_or_number.into(), false)
    }

    /// Gets the block at `block_hash_or_number` (full transactions included)
    fn get_block_with_txs<T: Into<BlockId> + Send + Sync>(
        &self,
        block_hash_or_number: T,
    ) -> Result<Option<Block<Transaction>>, ProviderError> {
        self.get_block_gen(block_hash_or_number.into(), true)
    }

    /// Gets the block uncle count at `block_hash_or_number`
    fn get_uncle_count<T: Into<BlockId> + Send + Sync>(
        &self,
        block_hash_or_number: T,
    ) -> Result<U256, Self::Error> {
        let id = block_hash_or_number.into();
        Ok(match id {
            BlockId::Hash(hash) => {
                let hash = utils::serialize(&hash);
                self.request("eth_getUncleCountByBlockHash", [hash])?
            }
            BlockId::Number(num) => {
                let num = utils::serialize(&num);
                self.request("eth_getUncleCountByBlockNumber", [num])?
            }
        })
    }

    /// Gets the block uncle at `block_hash_or_number` and `idx`
    fn get_uncle<T: Into<BlockId> + Send + Sync>(
        &self,
        block_hash_or_number: T,
        idx: U64,
    ) -> Result<Option<Block<H256>>, ProviderError> {
        let blk_id = block_hash_or_number.into();
        let idx = utils::serialize(&idx);
        Ok(match blk_id {
            BlockId::Hash(hash) => {
                let hash = utils::serialize(&hash);
                self.request("eth_getUncleByBlockHashAndIndex", [hash, idx])?
            }
            BlockId::Number(num) => {
                let num = utils::serialize(&num);
                self.request("eth_getUncleByBlockNumberAndIndex", [num, idx])?
            }
        })
    }

    /// Gets the transaction with `transaction_hash`
    fn get_transaction<T: Send + Sync + Into<TxHash>>(
        &self,
        transaction_hash: T,
    ) -> Result<Option<Transaction>, ProviderError> {
        let hash = transaction_hash.into();
        self.request("eth_getTransactionByHash", [hash])
    }

    /// Gets the transaction receipt with `transaction_hash`
    fn get_transaction_receipt<T: Send + Sync + Into<TxHash>>(
        &self,
        transaction_hash: T,
    ) -> Result<Option<TransactionReceipt>, ProviderError> {
        let hash = transaction_hash.into();
        self.request("eth_getTransactionReceipt", [hash])
    }

    /// Returns all receipts for a block.
    ///
    /// Note that this uses the `eth_getBlockReceipts` RPC, which is
    /// non-standard and currently supported by Erigon.
    fn get_block_receipts<T: Into<BlockNumber> + Send + Sync>(
        &self,
        block: T,
    ) -> Result<Vec<TransactionReceipt>, Self::Error> {
        self.request("eth_getBlockReceipts", [block.into()])
    }

    /// Returns all receipts for that block. Must be done on a parity node.
    fn parity_block_receipts<T: Into<BlockNumber> + Send + Sync>(
        &self,
        block: T,
    ) -> Result<Vec<TransactionReceipt>, Self::Error> {
        self.request("parity_getBlockReceipts", vec![block.into()])
    }

    /// Gets the current gas price as estimated by the node
    fn get_gas_price(&self) -> Result<U256, ProviderError> {
        self.request("eth_gasPrice", ())
    }

    /// Gets a heuristic recommendation of max fee per gas and max priority fee per gas for
    /// EIP-1559 compatible transactions.
    fn estimate_eip1559_fees(
        &self,
        estimator: Option<fn(U256, Vec<Vec<U256>>) -> (U256, U256)>,
    ) -> Result<(U256, U256), Self::Error> {
        let base_fee_per_gas = self
            .get_block(BlockNumber::Latest)?
            .ok_or_else(|| ProviderError::CustomError("Latest block not found".into()))?
            .base_fee_per_gas
            .ok_or_else(|| ProviderError::CustomError("EIP-1559 not activated".into()))?;

        let fee_history = self.fee_history(
            utils::EIP1559_FEE_ESTIMATION_PAST_BLOCKS,
            BlockNumber::Latest,
            &[utils::EIP1559_FEE_ESTIMATION_REWARD_PERCENTILE],
        )?;

        // use the provided fee estimator function, or fallback to the default implementation.
        let (max_fee_per_gas, max_priority_fee_per_gas) = if let Some(es) = estimator {
            es(base_fee_per_gas, fee_history.reward)
        } else {
            utils::eip1559_default_estimator(base_fee_per_gas, fee_history.reward)
        };

        Ok((max_fee_per_gas, max_priority_fee_per_gas))
    }

    /// Gets the accounts on the node
    fn get_accounts(&self) -> Result<Vec<Address>, ProviderError> {
        self.request("eth_accounts", ())
    }

    /// Returns the nonce of the address
    fn get_transaction_count<T: Into<NameOrAddress> + Send + Sync>(
        &self,
        from: T,
        block: Option<BlockId>,
    ) -> Result<U256, ProviderError> {
        let from = match from.into() {
            NameOrAddress::Name(ens_name) => self.resolve_name(&ens_name)?,
            NameOrAddress::Address(addr) => addr,
        };

        let from = utils::serialize(&from);
        let block = utils::serialize(&block.unwrap_or_else(|| BlockNumber::Latest.into()));
        self.request("eth_getTransactionCount", [from, block])
    }

    /// Returns the account's balance
    fn get_balance<T: Into<NameOrAddress> + Send + Sync>(
        &self,
        from: T,
        block: Option<BlockId>,
    ) -> Result<U256, ProviderError> {
        let from = match from.into() {
            NameOrAddress::Name(ens_name) => self.resolve_name(&ens_name)?,
            NameOrAddress::Address(addr) => addr,
        };

        let from = utils::serialize(&from);
        let block = utils::serialize(&block.unwrap_or_else(|| BlockNumber::Latest.into()));
        self.request("eth_getBalance", [from, block])
    }

    /// Returns the currently configured chain id, a value used in replay-protected
    /// transaction signing as introduced by EIP-155.
    fn get_chainid(&self) -> Result<U256, ProviderError> {
        self.request("eth_chainId", ())
    }

    /// Return current client syncing status. If IsFalse sync is over.
    fn syncing(&self) -> Result<SyncingStatus, Self::Error> {
        self.request("eth_syncing", ())
    }

    /// Returns the network version.
    fn get_net_version(&self) -> Result<String, ProviderError> {
        self.request("net_version", ())
    }

    ////// Contract Execution
    //
    // These are relatively low-level calls. The Contracts API should usually be used instead.

    /// Sends the read-only (constant) transaction to a single Ethereum node and return the result
    /// (as bytes) of executing it. This is free, since it does not change any state on the
    /// blockchain.
    fn call(&self, tx: &TypedTransaction, block: Option<BlockId>) -> Result<Bytes, ProviderError> {
        let tx = utils::serialize(tx);
        let block = utils::serialize(&block.unwrap_or_else(|| BlockNumber::Latest.into()));
        self.request("eth_call", [tx, block])
    }

    /// Sends a transaction to a single Ethereum node and return the estimated amount of gas
    /// required (as a U256) to send it This is free, but only an estimate. Providing too little
    /// gas will result in a transaction being rejected (while still consuming all provided
    /// gas).
    fn estimate_gas(
        &self,
        tx: &TypedTransaction,
        block: Option<BlockId>,
    ) -> Result<U256, ProviderError> {
        let tx = utils::serialize(tx);
        // Some nodes (e.g. old Optimism clients) don't support a block ID being passed as a param,
        // so refrain from defaulting to BlockNumber::Latest.
        let params = if let Some(block_id) = block {
            vec![tx, utils::serialize(&block_id)]
        } else {
            vec![tx]
        };
        self.request("eth_estimateGas", params)
    }

    fn create_access_list(
        &self,
        tx: &TypedTransaction,
        block: Option<BlockId>,
    ) -> Result<AccessListWithGasUsed, ProviderError> {
        let tx = utils::serialize(tx);
        let block = utils::serialize(&block.unwrap_or_else(|| BlockNumber::Latest.into()));
        self.request("eth_createAccessList", [tx, block])
    }
    /// The JSON-RPC provider is at the bottom-most position in the middleware stack. Here we check
    /// if it has the key for the sender address unlocked, as well as supports the `eth_sign` call.
    fn is_signer(&self) -> bool {
        match self.from {
            Some(sender) => self.sign(vec![], &sender).is_ok(),
            None => false,
        }
    }

    /// Signs data using a specific account. This account needs to be unlocked.
    fn sign<T: Into<Bytes> + Send + Sync>(
        &self,
        data: T,
        from: &Address,
    ) -> Result<Signature, ProviderError> {
        let data = utils::serialize(&data.into());
        let from = utils::serialize(from);

        // get the response from `eth_sign` call and trim the 0x-prefix if present.
        let sig: String = self.request("eth_sign", [from, data])?;
        let sig = sig.strip_prefix("0x").unwrap_or(&sig);

        // decode the signature.
        let sig = hex::decode(sig)?;
        Ok(Signature::try_from(sig.as_slice())
            .map_err(|e| ProviderError::CustomError(e.to_string()))?)
    }

    /// Sign a transaction via RPC call
    fn sign_transaction(
        &self,
        _tx: &TypedTransaction,
        _from: Address,
    ) -> Result<Signature, Self::Error> {
        Err(ProviderError::SignerUnavailable).map_err(FromErr::from)
    }

    ////// Contract state

    /// Returns an array (possibly empty) of logs that match the filter
    fn get_logs(&self, filter: &Filter) -> Result<Vec<Log>, ProviderError> {
        self.request("eth_getLogs", [filter])
    }

    fn get_logs_paginated<'a>(&'a self, filter: &Filter, page_size: u64) -> LogQuery<'a, P> {
        LogQuery::new(self, filter).with_page_size(page_size)
    }

    /// Creates a filter object, based on filter options, to notify when the state changes (logs).
    /// To check if the state has changed, call `get_filter_changes` with the filter id.
    fn new_filter(&self, filter: FilterKind<'_>) -> Result<U256, ProviderError> {
        let (method, args) = match filter {
            FilterKind::NewBlocks => ("eth_newBlockFilter", vec![]),
            FilterKind::Logs(filter) => ("eth_newFilter", vec![utils::serialize(&filter)]),
        };

        self.request(method, args)
    }

    /// Uninstalls a filter
    fn uninstall_filter<T: Into<U256> + Send + Sync>(&self, id: T) -> Result<bool, ProviderError> {
        let id = utils::serialize(&id.into());
        self.request("eth_uninstallFilter", [id])
    }

    /// Polling method for a filter, which returns an array of logs which occurred since last poll.
    ///
    /// This method must be called with one of the following return types, depending on the filter
    /// type:
    /// - `eth_newBlockFilter`: [`H256`], returns block hashes
    /// - `eth_newPendingTransactionFilter`: [`H256`], returns transaction hashes
    /// - `eth_newFilter`: [`Log`], returns raw logs
    ///
    /// If one of these types is not used, decoding will fail and the method will
    /// return an error.
    ///
    /// [`H256`]: ethers_core::types::H256
    /// [`Log`]: ethers_core::types::Log
    fn get_filter_changes<T, R>(&self, id: T) -> Result<Vec<R>, ProviderError>
    where
        T: Into<U256> + Send + Sync,
        R: Serialize + DeserializeOwned + Send + Sync + Debug,
    {
        let id = utils::serialize(&id.into());
        self.request("eth_getFilterChanges", [id])
    }

    /// Get the storage of an address for a particular slot location
    fn get_storage_at<T: Into<NameOrAddress> + Send + Sync>(
        &self,
        from: T,
        location: H256,
        block: Option<BlockId>,
    ) -> Result<H256, ProviderError> {
        let from = match from.into() {
            NameOrAddress::Name(ens_name) => self.resolve_name(&ens_name)?,
            NameOrAddress::Address(addr) => addr,
        };

        // position is a QUANTITY according to the [spec](https://eth.wiki/json-rpc/API#eth_getstorageat): integer of the position in the storage, converting this to a U256
        // will make sure the number is formatted correctly as [quantity](https://eips.ethereum.org/EIPS/eip-1474#quantity)
        let position = U256::from_big_endian(location.as_bytes());
        let position = utils::serialize(&position);
        let from = utils::serialize(&from);
        let block = utils::serialize(&block.unwrap_or_else(|| BlockNumber::Latest.into()));

        // get the hex encoded value.
        let value: String = self.request("eth_getStorageAt", [from, position, block])?;
        // get rid of the 0x prefix and left pad it with zeroes.
        let value = format!("{:0>64}", value.replace("0x", ""));
        Ok(H256::from_slice(&Vec::from_hex(value)?))
    }

    /// Returns the deployed code at a given address
    fn get_code<T: Into<NameOrAddress> + Send + Sync>(
        &self,
        at: T,
        block: Option<BlockId>,
    ) -> Result<Bytes, ProviderError> {
        let at = match at.into() {
            NameOrAddress::Name(ens_name) => self.resolve_name(&ens_name)?,
            NameOrAddress::Address(addr) => addr,
        };

        let at = utils::serialize(&at);
        let block = utils::serialize(&block.unwrap_or_else(|| BlockNumber::Latest.into()));
        self.request("eth_getCode", [at, block])
    }

    /// Returns the EIP-1186 proof response
    /// <https://github.com/ethereum/EIPs/issues/1186>
    fn get_proof<T: Into<NameOrAddress> + Send + Sync>(
        &self,
        from: T,
        locations: Vec<H256>,
        block: Option<BlockId>,
    ) -> Result<EIP1186ProofResponse, ProviderError> {
        let from = match from.into() {
            NameOrAddress::Name(ens_name) => self.resolve_name(&ens_name)?,
            NameOrAddress::Address(addr) => addr,
        };

        let from = utils::serialize(&from);
        let locations = locations.iter().map(|location| utils::serialize(&location)).collect();
        let block = utils::serialize(&block.unwrap_or_else(|| BlockNumber::Latest.into()));

        self.request("eth_getProof", [from, locations, block])
    }

    ////// Ethereum Naming Service
    // The Ethereum Naming Service (ENS) allows easy to remember and use names to
    // be assigned to Ethereum addresses. Any provider operation which takes an address
    // may also take an ENS name.
    //
    // ENS also provides the ability for a reverse lookup, which determines the name for an address
    // if it has been configured.

    /// Returns the address that the `ens_name` resolves to (or None if not configured).
    ///
    /// # Panics
    ///
    /// If the bytes returned from the ENS registrar/resolver cannot be interpreted as
    /// an address. This should theoretically never happen.
    fn resolve_name(&self, ens_name: &str) -> Result<Address, ProviderError> {
        self.query_resolver(ParamType::Address, ens_name, ens::ADDR_SELECTOR)
    }

    /// Returns the ENS name the `address` resolves to (or None if not configured).
    /// # Panics
    ///
    /// If the bytes returned from the ENS registrar/resolver cannot be interpreted as
    /// a string. This should theoretically never happen.
    fn lookup_address(&self, address: Address) -> Result<String, ProviderError> {
        let ens_name = ens::reverse_address(address);
        let domain: String =
            self.query_resolver(ParamType::String, &ens_name, ens::NAME_SELECTOR)?;
        let reverse_address = self.resolve_name(&domain)?;
        if address != reverse_address {
            Err(ProviderError::EnsNotOwned(domain))
        } else {
            Ok(domain)
        }
    }

    /// Returns the avatar HTTP link of the avatar that the `ens_name` resolves to (or None
    /// if not configured)
    ///
    /// # Example
    ///
    /// # Panics
    ///
    /// If the bytes returned from the ENS registrar/resolver cannot be interpreted as
    /// a string. This should theoretically never happen.
    fn resolve_avatar(&self, ens_name: &str) -> Result<Url, ProviderError> {
        let field = self.resolve_field(ens_name, "avatar")?;
        let owner = self.resolve_name(ens_name)?;
        let url = Url::from_str(&field).map_err(|e| ProviderError::CustomError(e.to_string()))?;
        match url.scheme() {
            "https" | "data" => Ok(url),
            "ipfs" => erc::http_link_ipfs(url).map_err(ProviderError::CustomError),
            "eip155" => {
                let token =
                    erc::ERCNFT::from_str(url.path()).map_err(ProviderError::CustomError)?;
                match token.type_ {
                    erc::ERCNFTType::ERC721 => {
                        let tx = TransactionRequest {
                            data: Some(
                                [&erc::ERC721_OWNER_SELECTOR[..], &token.id].concat().into(),
                            ),
                            to: Some(NameOrAddress::Address(token.contract)),
                            ..Default::default()
                        };
                        let data = self.call(&tx.into(), None)?;
                        if decode_bytes::<Address>(ParamType::Address, data) != owner {
                            return Err(ProviderError::CustomError("Incorrect owner.".to_string()))
                        }
                    }
                    erc::ERCNFTType::ERC1155 => {
                        let tx = TransactionRequest {
                            data: Some(
                                [
                                    &erc::ERC1155_BALANCE_SELECTOR[..],
                                    &[0x0; 12],
                                    &owner.0,
                                    &token.id,
                                ]
                                .concat()
                                .into(),
                            ),
                            to: Some(NameOrAddress::Address(token.contract)),
                            ..Default::default()
                        };
                        let data = self.call(&tx.into(), None)?;
                        if decode_bytes::<u64>(ParamType::Uint(64), data) == 0 {
                            return Err(ProviderError::CustomError("Incorrect balance.".to_string()))
                        }
                    }
                }

                let image_url = self.resolve_nft(token)?;
                match image_url.scheme() {
                    "https" | "data" => Ok(image_url),
                    "ipfs" => erc::http_link_ipfs(image_url).map_err(ProviderError::CustomError),
                    _ => Err(ProviderError::CustomError(
                        "Unsupported scheme for the image".to_string(),
                    )),
                }
            }
            _ => Err(ProviderError::CustomError("Unsupported scheme".to_string())),
        }
    }

    /// Returns the URL (not necesserily HTTP) of the image behind a token.
    ///
    /// # Example
    ///
    /// # Panics
    ///
    /// If the bytes returned from the ENS registrar/resolver cannot be interpreted as
    /// a string. This should theoretically never happen.
    fn resolve_nft(&self, token: erc::ERCNFT) -> Result<Url, ProviderError> {
        let selector = token.type_.resolution_selector();
        let tx = TransactionRequest {
            data: Some([&selector[..], &token.id].concat().into()),
            to: Some(NameOrAddress::Address(token.contract)),
            ..Default::default()
        };
        let data = self.call(&tx.into(), None)?;
        let mut metadata_url = Url::parse(&decode_bytes::<String>(ParamType::String, data))
            .map_err(|e| ProviderError::CustomError(format!("Invalid metadata url: {}", e)))?;

        if token.type_ == erc::ERCNFTType::ERC1155 {
            metadata_url.set_path(&metadata_url.path().replace("%7Bid%7D", &hex::encode(token.id)));
        }
        if metadata_url.scheme() == "ipfs" {
            metadata_url = erc::http_link_ipfs(metadata_url).map_err(ProviderError::CustomError)?;
        }
        let metadata: erc::Metadata = reqwest::blocking::get(metadata_url)?.json()?;
        Url::parse(&metadata.image).map_err(|e| ProviderError::CustomError(e.to_string()))
    }

    /// Fetch a field for the `ens_name` (no None if not configured).
    ///
    /// # Panics
    ///
    /// If the bytes returned from the ENS registrar/resolver cannot be interpreted as
    /// a string. This should theoretically never happen.
    fn resolve_field(&self, ens_name: &str, field: &str) -> Result<String, ProviderError> {
        let field: String = self.query_resolver_parameters(
            ParamType::String,
            ens_name,
            ens::FIELD_SELECTOR,
            Some(&ens::parameterhash(field)),
        )?;
        Ok(field)
    }

    /// Returns the details of all transactions currently pending for inclusion in the next
    /// block(s), as well as the ones that are being scheduled for future execution only.
    /// Ref: [Here](https://geth.ethereum.org/docs/rpc/ns-txpool#txpool_content)
    fn txpool_content(&self) -> Result<TxpoolContent, ProviderError> {
        self.request("txpool_content", ())
    }

    /// Returns a summary of all the transactions currently pending for inclusion in the next
    /// block(s), as well as the ones that are being scheduled for future execution only.
    /// Ref: [Here](https://geth.ethereum.org/docs/rpc/ns-txpool#txpool_inspect)
    fn txpool_inspect(&self) -> Result<TxpoolInspect, ProviderError> {
        self.request("txpool_inspect", ())
    }

    /// Returns the number of transactions currently pending for inclusion in the next block(s), as
    /// well as the ones that are being scheduled for future execution only.
    /// Ref: [Here](https://geth.ethereum.org/docs/rpc/ns-txpool#txpool_status)
    fn txpool_status(&self) -> Result<TxpoolStatus, ProviderError> {
        self.request("txpool_status", ())
    }

    /// Executes the given call and returns a number of possible traces for it
    fn debug_trace_transaction(
        &self,
        tx_hash: TxHash,
        trace_options: GethDebugTracingOptions,
    ) -> Result<GethTrace, ProviderError> {
        let tx_hash = utils::serialize(&tx_hash);
        let trace_options = utils::serialize(&trace_options);
        self.request("debug_traceTransaction", [tx_hash, trace_options])
    }

    /// Executes the given call and returns a number of possible traces for it
    fn trace_call<T: Into<TypedTransaction> + Send + Sync>(
        &self,
        req: T,
        trace_type: Vec<TraceType>,
        block: Option<BlockNumber>,
    ) -> Result<BlockTrace, ProviderError> {
        let req = req.into();
        let req = utils::serialize(&req);
        let block = utils::serialize(&block.unwrap_or(BlockNumber::Latest));
        let trace_type = utils::serialize(&trace_type);
        self.request("trace_call", [req, trace_type, block])
    }

    /// Executes given calls and returns a number of possible traces for each call
    fn trace_call_many<T: Into<TypedTransaction> + Send + Sync>(
        &self,
        req: Vec<(T, Vec<TraceType>)>,
        block: Option<BlockNumber>,
    ) -> Result<Vec<BlockTrace>, ProviderError> {
        let req: Vec<(TypedTransaction, Vec<TraceType>)> =
            req.into_iter().map(|(tx, trace_type)| (tx.into(), trace_type)).collect();
        let req = utils::serialize(&req);
        let block = utils::serialize(&block.unwrap_or(BlockNumber::Latest));
        self.request("trace_callMany", [req, block])
    }

    /// Traces a call to `eth_sendRawTransaction` without making the call, returning the traces
    fn trace_raw_transaction(
        &self,
        data: Bytes,
        trace_type: Vec<TraceType>,
    ) -> Result<BlockTrace, ProviderError> {
        let data = utils::serialize(&data);
        let trace_type = utils::serialize(&trace_type);
        self.request("trace_rawTransaction", [data, trace_type])
    }

    /// Replays a transaction, returning the traces
    fn trace_replay_transaction(
        &self,
        hash: H256,
        trace_type: Vec<TraceType>,
    ) -> Result<BlockTrace, ProviderError> {
        let hash = utils::serialize(&hash);
        let trace_type = utils::serialize(&trace_type);
        self.request("trace_replayTransaction", [hash, trace_type])
    }

    /// Replays all transactions in a block returning the requested traces for each transaction
    fn trace_replay_block_transactions(
        &self,
        block: BlockNumber,
        trace_type: Vec<TraceType>,
    ) -> Result<Vec<BlockTrace>, ProviderError> {
        let block = utils::serialize(&block);
        let trace_type = utils::serialize(&trace_type);
        self.request("trace_replayBlockTransactions", [block, trace_type])
    }

    /// Returns traces created at given block
    fn trace_block(&self, block: BlockNumber) -> Result<Vec<Trace>, ProviderError> {
        let block = utils::serialize(&block);
        self.request("trace_block", [block])
    }

    /// Return traces matching the given filter
    fn trace_filter(&self, filter: TraceFilter) -> Result<Vec<Trace>, ProviderError> {
        let filter = utils::serialize(&filter);
        self.request("trace_filter", vec![filter])
    }

    /// Returns trace at the given position
    fn trace_get<T: Into<U64> + Send + Sync>(
        &self,
        hash: H256,
        index: Vec<T>,
    ) -> Result<Trace, ProviderError> {
        let hash = utils::serialize(&hash);
        let index: Vec<U64> = index.into_iter().map(|i| i.into()).collect();
        let index = utils::serialize(&index);
        self.request("trace_get", vec![hash, index])
    }

    /// Returns all traces of a given transaction
    fn trace_transaction(&self, hash: H256) -> Result<Vec<Trace>, ProviderError> {
        let hash = utils::serialize(&hash);
        self.request("trace_transaction", vec![hash])
    }

    fn fee_history<T: Into<U256> + Send + Sync>(
        &self,
        block_count: T,
        last_block: BlockNumber,
        reward_percentiles: &[f64],
    ) -> Result<FeeHistory, Self::Error> {
        let block_count = block_count.into();
        let last_block = utils::serialize(&last_block);
        let reward_percentiles = utils::serialize(&reward_percentiles);

        // The blockCount param is expected to be an unsigned integer up to geth v1.10.6.
        // Geth v1.10.7 onwards, this has been updated to a hex encoded form. Failure to
        // decode the param from client side would fallback to the old API spec.
        match self.request::<_, FeeHistory>(
            "eth_feeHistory",
            [utils::serialize(&block_count), last_block.clone(), reward_percentiles.clone()],
        ) {
            success @ Ok(_) => success,
            err @ Err(_) => {
                let fallback = self.request::<_, FeeHistory>(
                    "eth_feeHistory",
                    [utils::serialize(&block_count.as_u64()), last_block, reward_percentiles],
                );

                if fallback.is_err() {
                    // if the older fallback also resulted in an error, we return the error from the
                    // initial attempt
                    return err
                }
                fallback
            }
        }
    }
}

impl<P: JsonRpcClient> Provider<P> {
    fn query_resolver<T: Detokenize>(
        &self,
        param: ParamType,
        ens_name: &str,
        selector: Selector,
    ) -> Result<T, ProviderError> {
        self.query_resolver_parameters(param, ens_name, selector, None)
    }

    fn query_resolver_parameters<T: Detokenize>(
        &self,
        param: ParamType,
        ens_name: &str,
        selector: Selector,
        parameters: Option<&[u8]>,
    ) -> Result<T, ProviderError> {
        // Get the ENS address, prioritize the local override variable
        let ens_addr = self.ens.unwrap_or(ens::ENS_ADDRESS);

        // first get the resolver responsible for this name
        // the call will return a Bytes array which we convert to an address
        let data = self.call(&ens::get_resolver(ens_addr, ens_name).into(), None)?;

        // otherwise, decode_bytes panics
        if data.0.is_empty() {
            return Err(ProviderError::EnsError(ens_name.to_string()))
        }

        let resolver_address: Address = decode_bytes(ParamType::Address, data);
        if resolver_address == Address::zero() {
            return Err(ProviderError::EnsError(ens_name.to_string()))
        }

        if let ParamType::Address = param {
            // Reverse resolver reverts when calling `supportsInterface(bytes4)`
            self.validate_resolver(resolver_address, selector, ens_name)?;
        }

        // resolve
        let data = self
            .call(&ens::resolve(resolver_address, selector, ens_name, parameters).into(), None)?;

        Ok(decode_bytes(param, data))
    }

    /// Validates that the resolver supports `selector`.
    fn validate_resolver(
        &self,
        resolver_address: Address,
        selector: Selector,
        ens_name: &str,
    ) -> Result<(), ProviderError> {
        let data = self.call(&ens::supports_interface(resolver_address, selector).into(), None)?;

        if data.is_empty() {
            return Err(ProviderError::EnsError(format!(
                "`{}` resolver ({:?}) is invalid.",
                ens_name, resolver_address
            )))
        }

        let supports_selector = abi::decode(&[ParamType::Bool], data.as_ref())
            .map(|token| token[0].clone().into_bool().unwrap_or_default())
            .unwrap_or_default();

        if !supports_selector {
            return Err(ProviderError::EnsError(format!(
                "`{}` resolver ({:?}) does not support selector {}.",
                ens_name,
                resolver_address,
                hex::encode(selector)
            )))
        }

        Ok(())
    }

    #[cfg(test)]
    /// Anvil and Ganache-only function for mining empty blocks
    pub fn mine(&self, num_blocks: usize) -> Result<(), ProviderError> {
        for _ in 0..num_blocks {
            self.inner.request::<_, U256>("evm_mine", None::<()>).map_err(Into::into)?;
        }
        Ok(())
    }

    /// Sets the ENS Address (default: mainnet)
    #[must_use]
    pub fn ens<T: Into<Address>>(mut self, ens: T) -> Self {
        self.ens = Some(ens.into());
        self
    }

    /// Sets the default polling interval for event filters and pending transactions
    /// (default: 7 seconds)
    pub fn set_interval<T: Into<Duration>>(&mut self, interval: T) -> &mut Self {
        self.interval = Some(interval.into());
        self
    }

    /// Sets the default polling interval for event filters and pending transactions
    /// (default: 7 seconds)
    #[must_use]
    pub fn interval<T: Into<Duration>>(mut self, interval: T) -> Self {
        self.set_interval(interval);
        self
    }
}

impl Provider<HttpProvider> {
    /// The Url to which requests are made
    pub fn url(&self) -> &Url {
        self.inner.url()
    }

    /// Mutable access to the Url to which requests are made
    pub fn url_mut(&mut self) -> &mut Url {
        self.inner.url_mut()
    }
}

impl Provider<MockProvider> {
    /// Returns a `Provider` instantiated with an internal "mock" transport.
    ///
    /// # Example
    ///
    /// ```
    /// # fn foo() -> Result<(), Box<dyn std::error::Error>> {
    /// use ethers_core::types::U64;
    /// use ethers_providers::{Middleware, Provider};
    /// // Instantiate the provider
    /// let (provider, mock) = Provider::mocked();
    /// // Push the mock response
    /// mock.push(U64::from(12))?;
    /// // Make the call
    /// let blk = provider.get_block_number().unwrap();
    /// // The response matches
    /// assert_eq!(blk.as_u64(), 12);
    /// // and the request as well!
    /// mock.assert_request("eth_blockNumber", ()).unwrap();
    /// # Ok(())
    /// # }
    /// ```
    pub fn mocked() -> (Self, MockProvider) {
        let mock = MockProvider::new();
        let mock_clone = mock.clone();
        (Self::new(mock), mock_clone)
    }
}

/// infallible conversion of Bytes to Address/String
///
/// # Panics
///
/// If the provided bytes were not an interpretation of an address
fn decode_bytes<T: Detokenize>(param: ParamType, bytes: Bytes) -> T {
    let tokens = abi::decode(&[param], bytes.as_ref())
        .expect("could not abi-decode bytes to address tokens");
    T::from_tokens(tokens).expect("could not parse tokens as address")
}

impl TryFrom<&str> for Provider<HttpProvider> {
    type Error = ParseError;

    fn try_from(src: &str) -> Result<Self, Self::Error> {
        Ok(Provider::new(HttpProvider::new(Url::parse(src)?)))
    }
}

impl TryFrom<String> for Provider<HttpProvider> {
    type Error = ParseError;

    fn try_from(src: String) -> Result<Self, Self::Error> {
        Provider::try_from(src.as_str())
    }
}

impl<'a> TryFrom<&'a String> for Provider<HttpProvider> {
    type Error = ParseError;

    fn try_from(src: &'a String) -> Result<Self, Self::Error> {
        Provider::try_from(src.as_str())
    }
}

/// Returns true if the endpoint is local
///
/// # Example
///
/// ```
/// use ethers_providers::is_local_endpoint;
/// assert!(is_local_endpoint("http://localhost:8545"));
/// assert!(is_local_endpoint("http://127.0.0.1:8545"));
/// ```
#[inline]
pub fn is_local_endpoint(url: &str) -> bool {
    url.contains("127.0.0.1") || url.contains("localhost")
}

/// A middleware supporting development-specific JSON RPC methods
///
/// # Example
///
///```
/// use ethers_providers::{Provider, Http, Middleware, DevRpcMiddleware};
/// use ethers_core::types::TransactionRequest;
/// use ethers_core::utils::Anvil;
/// use std::convert::TryFrom;
#[cfg(feature = "dev-rpc")]
pub mod dev_rpc {
    use crate::{FromErr, Middleware, ProviderError};
    use ethers_core::types::U256;
    use thiserror::Error;

    use std::fmt::Debug;

    #[derive(Clone, Debug)]
    pub struct DevRpcMiddleware<M>(M);

    #[derive(Error, Debug)]
    pub enum DevRpcMiddlewareError<M: Middleware> {
        #[error("{0}")]
        MiddlewareError(M::Error),

        #[error("{0}")]
        ProviderError(ProviderError),

        #[error("Could not revert to snapshot")]
        NoSnapshot,
    }

    impl<M: Middleware> Middleware for DevRpcMiddleware<M> {
        type Error = DevRpcMiddlewareError<M>;
        type Provider = M::Provider;
        type Inner = M;

        fn inner(&self) -> &M {
            &self.0
        }
    }

    impl<M: Middleware> FromErr<M::Error> for DevRpcMiddlewareError<M> {
        fn from(src: M::Error) -> DevRpcMiddlewareError<M> {
            DevRpcMiddlewareError::MiddlewareError(src)
        }
    }

    impl<M> From<ProviderError> for DevRpcMiddlewareError<M>
    where
        M: Middleware,
    {
        fn from(src: ProviderError) -> Self {
            Self::ProviderError(src)
        }
    }

    impl<M: Middleware> DevRpcMiddleware<M> {
        pub fn new(inner: M) -> Self {
            Self(inner)
        }

        // Ganache, Hardhat and Anvil increment snapshot ID even if no state has changed
        pub fn snapshot(&self) -> Result<U256, DevRpcMiddlewareError<M>> {
            self.provider().request::<(), U256>("evm_snapshot", ()).map_err(From::from)
        }

        pub fn revert_to_snapshot(&self, id: U256) -> Result<(), DevRpcMiddlewareError<M>> {
            let ok = self
                .provider()
                .request::<[U256; 1], bool>("evm_revert", [id])
                .map_err(DevRpcMiddlewareError::ProviderError)?;
            if ok {
                Ok(())
            } else {
                Err(DevRpcMiddlewareError::NoSnapshot)
            }
        }
    }
    #[cfg(test)]
    // Celo blocks can not get parsed when used with Ganache
    #[cfg(not(feature = "celo"))]
    mod tests {
        use super::*;
        use crate::{Http, Provider};
        use ethers_core::utils::Anvil;
        use std::convert::TryFrom;

        #[test]
        fn test_snapshot() {
            let anvil = Anvil::new().spawn();
            let provider = Provider::<Http>::try_from(anvil.endpoint()).unwrap();
            let client = DevRpcMiddleware::new(provider);

            // snapshot initial state
            let block0 = client.get_block_number().unwrap();
            let time0 = client.get_block(block0).unwrap().unwrap().timestamp;
            let snap_id0 = client.snapshot().unwrap();

            // mine a new block
            client.provider().mine(1).unwrap();

            // snapshot state
            let block1 = client.get_block_number().unwrap();
            let time1 = client.get_block(block1).unwrap().unwrap().timestamp;
            let snap_id1 = client.snapshot().unwrap();

            // mine some blocks
            client.provider().mine(5).unwrap();

            // snapshot state
            let block2 = client.get_block_number().unwrap();
            let time2 = client.get_block(block2).unwrap().unwrap().timestamp;
            let snap_id2 = client.snapshot().unwrap();

            // mine some blocks
            client.provider().mine(5).unwrap();

            // revert_to_snapshot should reset state to snap id
            client.revert_to_snapshot(snap_id2).unwrap();
            let block = client.get_block_number().unwrap();
            let time = client.get_block(block).unwrap().unwrap().timestamp;
            assert_eq!(block, block2);
            assert_eq!(time, time2);

            client.revert_to_snapshot(snap_id1).unwrap();
            let block = client.get_block_number().unwrap();
            let time = client.get_block(block).unwrap().unwrap().timestamp;
            assert_eq!(block, block1);
            assert_eq!(time, time1);

            // revert_to_snapshot should throw given non-existent or
            // previously used snapshot
            let result = client.revert_to_snapshot(snap_id1);
            assert!(result.is_err());

            client.revert_to_snapshot(snap_id0).unwrap();
            let block = client.get_block_number().unwrap();
            let time = client.get_block(block).unwrap().unwrap().timestamp;
            assert_eq!(block, block0);
            assert_eq!(time, time0);
        }
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::*;
    use crate::Http;
    use ethers_core::{
        types::{
            transaction::eip2930::AccessList, Eip1559TransactionRequest, TransactionRequest, H256,
        },
        utils::Anvil,
    };

    #[test]
    fn convert_h256_u256_quantity() {
        let hash: H256 = H256::zero();
        let quantity = U256::from_big_endian(hash.as_bytes());
        assert_eq!(format!("{quantity:#x}"), "0x0");
        assert_eq!(utils::serialize(&quantity).to_string(), "\"0x0\"");

        let address: Address = "0x295a70b2de5e3953354a6a8344e616ed314d7251".parse().unwrap();
        let block = BlockNumber::Latest;
        let params =
            [utils::serialize(&address), utils::serialize(&quantity), utils::serialize(&block)];

        let params = serde_json::to_string(&params).unwrap();
        assert_eq!(params, r#"["0x295a70b2de5e3953354a6a8344e616ed314d7251","0x0","latest"]"#);
    }

    #[test]
    // Test vector from: https://docs.ethers.io/ethers.js/v5-beta/api-providers.html#id2
    fn mainnet_resolve_name() {
        let provider = crate::test_provider::MAINNET.provider();

        let addr = provider.resolve_name("registrar.firefly.eth").unwrap();
        assert_eq!(addr, "6fC21092DA55B392b045eD78F4732bff3C580e2c".parse().unwrap());

        // registrar not found
        provider.resolve_name("asdfasdffads").unwrap_err();

        // name not found
        provider.resolve_name("asdfasdf.registrar.firefly.eth").unwrap_err();
    }

    #[test]
    // Test vector from: https://docs.ethers.io/ethers.js/v5-beta/api-providers.html#id2
    fn mainnet_lookup_address() {
        let provider = crate::MAINNET.provider();

        let name = provider
            .lookup_address("6fC21092DA55B392b045eD78F4732bff3C580e2c".parse().unwrap())
            .unwrap();

        assert_eq!(name, "registrar.firefly.eth");

        provider
            .lookup_address("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".parse().unwrap())
            .unwrap_err();
    }

    #[test]
    #[ignore]
    fn mainnet_resolve_avatar() {
        let provider = crate::MAINNET.provider();

        for (ens_name, res) in &[
            // HTTPS
            ("alisha.eth", "https://ipfs.io/ipfs/QmeQm91kAdPGnUKsE74WvkqYKUeHvc2oHd2FW11V3TrqkQ"),
            // ERC-1155
            ("nick.eth", "https://img.seadn.io/files/3ae7be6c41ad4767bf3ecbc0493b4bfb.png"),
            // HTTPS
            ("parishilton.eth", "https://i.imgur.com/YW3Hzph.jpg"),
            // ERC-721 with IPFS link
            ("ikehaya-nft.eth", "https://ipfs.io/ipfs/QmdKkwCE8uVhgYd7tWBfhtHdQZDnbNukWJ8bvQmR6nZKsk"),
            // ERC-1155 with IPFS link
            ("vitalik.eth", "https://ipfs.io/ipfs/QmSP4nq9fnN9dAiCj42ug9Wa79rqmQerZXZch82VqpiH7U/image.gif"),
            // IPFS
            ("cdixon.eth", "https://ipfs.io/ipfs/QmYA6ZpEARgHvRHZQdFPynMMX8NtdL2JCadvyuyG2oA88u"),
            ("0age.eth", "data:image/svg+xml;base64,PD94bWwgdmVyc2lvbj0iMS4wIiBlbmNvZGluZz0iVVRGLTgiPz48c3ZnIHN0eWxlPSJiYWNrZ3JvdW5kLWNvbG9yOmJsYWNrIiB2aWV3Qm94PSIwIDAgNTAwIDUwMCIgeG1sbnM9Imh0dHA6Ly93d3cudzMub3JnLzIwMDAvc3ZnIj48cmVjdCB4PSIxNTUiIHk9IjYwIiB3aWR0aD0iMTkwIiBoZWlnaHQ9IjM5MCIgZmlsbD0iIzY5ZmYzNyIvPjwvc3ZnPg==")
        ] {
        println!("Resolving: {}", ens_name);
        assert_eq!(provider.resolve_avatar(ens_name).unwrap(), Url::parse(res).unwrap());
    }
    }

    #[test]
    #[cfg_attr(feature = "celo", ignore)]
    fn test_is_signer() {
        use ethers_core::utils::Anvil;
        use std::str::FromStr;

        let anvil = Anvil::new().spawn();
        let provider =
            Provider::<Http>::try_from(anvil.endpoint()).unwrap().with_sender(anvil.addresses()[0]);
        assert!(provider.is_signer());

        let provider = Provider::<Http>::try_from(anvil.endpoint()).unwrap();
        assert!(!provider.is_signer());

        let sender = Address::from_str("635B4764D1939DfAcD3a8014726159abC277BecC")
            .expect("should be able to parse hex address");
        let provider = Provider::<Http>::try_from(
            "https://ropsten.infura.io/v3/fd8b88b56aa84f6da87b60f5441d6778",
        )
        .unwrap()
        .with_sender(sender);
        assert!(!provider.is_signer());
    }

    #[test]
    fn parity_block_receipts() {
        let url = match std::env::var("PARITY") {
            Ok(inner) => inner,
            _ => return,
        };
        let provider = Provider::<Http>::try_from(url.as_str()).unwrap();
        let receipts = provider.parity_block_receipts(10657200).unwrap();
        assert!(!receipts.is_empty());
    }

    #[test]
    #[cfg_attr(feature = "celo", ignore)]
    fn fee_history() {
        let provider = Provider::<Http>::try_from(
            "https://goerli.infura.io/v3/fd8b88b56aa84f6da87b60f5441d6778",
        )
        .unwrap();

        let history = provider.fee_history(10u64, BlockNumber::Latest, &[10.0, 40.0]).unwrap();
        dbg!(&history);
    }

    #[test]
    fn test_fill_transaction_1559() {
        let (mut provider, mock) = Provider::mocked();
        provider.from = Some("0x6fC21092DA55B392b045eD78F4732bff3C580e2c".parse().unwrap());

        let gas = U256::from(21000_usize);
        let max_fee = U256::from(25_usize);
        let prio_fee = U256::from(25_usize);
        let access_list: AccessList = vec![Default::default()].into();

        // --- leaves a filled 1559 transaction unchanged, making no requests
        let from: Address = "0x0000000000000000000000000000000000000001".parse().unwrap();
        let to: Address = "0x0000000000000000000000000000000000000002".parse().unwrap();
        let mut tx = Eip1559TransactionRequest::new()
            .from(from)
            .to(to)
            .gas(gas)
            .max_fee_per_gas(max_fee)
            .max_priority_fee_per_gas(prio_fee)
            .access_list(access_list.clone())
            .into();
        provider.fill_transaction(&mut tx, None).unwrap();

        assert_eq!(tx.from(), Some(&from));
        assert_eq!(tx.to(), Some(&to.into()));
        assert_eq!(tx.gas(), Some(&gas));
        assert_eq!(tx.gas_price(), Some(max_fee));
        assert_eq!(tx.access_list(), Some(&access_list));

        // --- fills a 1559 transaction, leaving the existing gas limit unchanged,
        // without generating an access-list
        let mut tx = Eip1559TransactionRequest::new()
            .gas(gas)
            .max_fee_per_gas(max_fee)
            .max_priority_fee_per_gas(prio_fee)
            .into();

        provider.fill_transaction(&mut tx, None).unwrap();

        assert_eq!(tx.from(), provider.from.as_ref());
        assert!(tx.to().is_none());
        assert_eq!(tx.gas(), Some(&gas));
        assert_eq!(tx.access_list(), Some(&Default::default()));

        // --- fills a 1559 transaction, using estimated gas
        let mut tx = Eip1559TransactionRequest::new()
            .max_fee_per_gas(max_fee)
            .max_priority_fee_per_gas(prio_fee)
            .into();

        mock.push(gas).unwrap();

        provider.fill_transaction(&mut tx, None).unwrap();

        assert_eq!(tx.from(), provider.from.as_ref());
        assert!(tx.to().is_none());
        assert_eq!(tx.gas(), Some(&gas));
        assert_eq!(tx.access_list(), Some(&Default::default()));

        // --- propogates estimate_gas() error
        let mut tx = Eip1559TransactionRequest::new()
            .max_fee_per_gas(max_fee)
            .max_priority_fee_per_gas(prio_fee)
            .into();

        // bad mock value causes error response for eth_estimateGas
        mock.push(b'b').unwrap();

        let res = provider.fill_transaction(&mut tx, None);

        assert!(matches!(res, Err(ProviderError::JsonRpcClientError(_))));
    }

    #[test]
    fn test_fill_transaction_legacy() {
        let (mut provider, mock) = Provider::mocked();
        provider.from = Some("0x6fC21092DA55B392b045eD78F4732bff3C580e2c".parse().unwrap());

        let gas = U256::from(21000_usize);
        let gas_price = U256::from(50_usize);

        // --- leaves a filled legacy transaction unchanged, making no requests
        let from: Address = "0x0000000000000000000000000000000000000001".parse().unwrap();
        let to: Address = "0x0000000000000000000000000000000000000002".parse().unwrap();
        let mut tx =
            TransactionRequest::new().from(from).to(to).gas(gas).gas_price(gas_price).into();
        provider.fill_transaction(&mut tx, None).unwrap();

        assert_eq!(tx.from(), Some(&from));
        assert_eq!(tx.to(), Some(&to.into()));
        assert_eq!(tx.gas(), Some(&gas));
        assert_eq!(tx.gas_price(), Some(gas_price));
        assert!(tx.access_list().is_none());

        // --- fills an empty legacy transaction
        let mut tx = TransactionRequest::new().into();
        mock.push(gas).unwrap();
        mock.push(gas_price).unwrap();
        provider.fill_transaction(&mut tx, None).unwrap();

        assert_eq!(tx.from(), provider.from.as_ref());
        assert!(tx.to().is_none());
        assert_eq!(tx.gas(), Some(&gas));
        assert_eq!(tx.gas_price(), Some(gas_price));
        assert!(tx.access_list().is_none());
    }

    #[test]
    fn mainnet_lookup_address_invalid_resolver() {
        let provider = crate::MAINNET.provider();

        let err = provider
            .lookup_address("0x30c9223d9e3d23e0af1073a38e0834b055bf68ed".parse().unwrap())
            .unwrap_err();

        assert_eq!(
            &err.to_string(),
            "ens name not found: `ox63616e.eth` resolver (0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2) is invalid."
        );
    }
}
