use ethers_core::types::{
    transaction::{eip2718::TypedTransaction, eip2930::AccessListWithGasUsed},
    Address, BlockId, Bytes, Signature, U256,
};
use ethers_providers::{maybe, FromErr, Middleware};
use ethers_signers::Signer;

use async_trait::async_trait;
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct SignerMiddleware<M, S> {
    pub(crate) inner: M,
    pub(crate) signer: S,
    pub(crate) address: Address,
}

impl<M: Middleware, S: Signer> FromErr<M::Error> for SignerMiddlewareError<M, S> {
    fn from(src: M::Error) -> SignerMiddlewareError<M, S> {
        SignerMiddlewareError::MiddlewareError(src)
    }
}

#[derive(Error, Debug)]
/// Error thrown when the client interacts with the blockchain
pub enum SignerMiddlewareError<M: Middleware, S: Signer> {
    #[error("{0}")]
    /// Thrown when the internal call to the signer fails
    SignerError(S::Error),

    #[error("{0}")]
    /// Thrown when an internal middleware errors
    MiddlewareError(M::Error),

    /// Thrown if the `nonce` field is missing
    #[error("no nonce was specified")]
    NonceMissing,
    /// Thrown if the `gas_price` field is missing
    #[error("no gas price was specified")]
    GasPriceMissing,
    /// Thrown if the `gas` field is missing
    #[error("no gas was specified")]
    GasMissing,
    /// Thrown if a signature is requested from a different address
    #[error("specified from address is not signer")]
    WrongSigner,
    /// Thrown if the signer's chain_id is different than the chain_id of the transaction
    #[error("specified chain_id is different than the signer's chain_id")]
    DifferentChainID,
}

// Helper functions for locally signing transactions
impl<M, S> SignerMiddleware<M, S>
where
    M: Middleware,
    S: Signer,
{
    /// Creates a new client from the provider and signer.
    /// Sets the address of this middleware to the address of the signer.
    /// The chain_id of the signer will not be set to the chain id of the provider. If the signer
    /// passed here is initialized with a different chain id, then the client may throw errors, or
    /// methods like `sign_transaction` may error.
    /// To automatically set the signer's chain id, see `new_with_provider_chain`.
    ///
    /// [`Middleware`] ethers_providers::Middleware
    /// [`Signer`] ethers_signers::Signer
    pub fn new(inner: M, signer: S) -> Self {
        let address = signer.address();
        SignerMiddleware { inner, signer, address }
    }

    /// Signs and returns the RLP encoding of the signed transaction.
    /// If the transaction does not have a chain id set, it sets it to the signer's chain id.
    /// Returns an error if the transaction's existing chain id does not match the signer's chain
    /// id.
    async fn sign_transaction(
        &self,
        mut tx: TypedTransaction,
    ) -> Result<Bytes, SignerMiddlewareError<M, S>> {
        // compare chain_id and use signer's chain_id if the tranasaction's chain_id is None,
        // return an error if they are not consistent
        let chain_id = self.signer.chain_id();
        match tx.chain_id() {
            Some(id) if id.as_u64() != chain_id => {
                return Err(SignerMiddlewareError::DifferentChainID)
            }
            None => {
                tx.set_chain_id(chain_id);
            }
            _ => {}
        }

        let signature =
            self.signer.sign_transaction(&tx).await.map_err(SignerMiddlewareError::SignerError)?;

        // Return the raw rlp-encoded signed transaction
        Ok(tx.rlp_signed(&signature))
    }

    /// Returns the client's address
    pub fn address(&self) -> Address {
        self.address
    }

    /// Returns a reference to the client's signer
    pub fn signer(&self) -> &S {
        &self.signer
    }

    /// Builds a SignerMiddleware with the given Signer.
    #[must_use]
    pub fn with_signer(&self, signer: S) -> Self
    where
        S: Clone,
        M: Clone,
    {
        let mut this = self.clone();
        this.address = signer.address();
        this.signer = signer;
        this
    }

    /// Creates a new client from the provider and signer.
    /// Sets the address of this middleware to the address of the signer.
    /// Sets the chain id of the signer to the chain id of the inner [`Middleware`] passed in,
    /// using the [`Signer`]'s implementation of with_chain_id.
    ///
    /// [`Middleware`] ethers_providers::Middleware
    /// [`Signer`] ethers_signers::Signer
    pub async fn new_with_provider_chain(
        inner: M,
        signer: S,
    ) -> Result<Self, SignerMiddlewareError<M, S>> {
        let address = signer.address();
        let chain_id =
            inner.get_chainid().await.map_err(|e| SignerMiddlewareError::MiddlewareError(e))?;
        let signer = signer.with_chain_id(chain_id.as_u64());
        Ok(SignerMiddleware { inner, signer, address })
    }

    fn set_tx_from_if_none(&self, tx: &TypedTransaction) -> TypedTransaction {
        let mut tx = tx.clone();
        if tx.from().is_none() {
            tx.set_from(self.address);
        }
        tx
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<M, S> Middleware for SignerMiddleware<M, S>
where
    M: Middleware,
    S: Signer,
{
    type Error = SignerMiddlewareError<M, S>;
    type Provider = M::Provider;
    type Inner = M;

    fn inner(&self) -> &M {
        &self.inner
    }

    /// Returns the client's address
    fn default_sender(&self) -> Option<Address> {
        Some(self.address)
    }

    /// `SignerMiddleware` is instantiated with a signer.
    async fn is_signer(&self) -> bool {
        true
    }

    async fn sign_transaction(
        &self,
        tx: &TypedTransaction,
        _: Address,
    ) -> Result<Signature, Self::Error> {
        Ok(self.signer.sign_transaction(tx).await.map_err(SignerMiddlewareError::SignerError)?)
    }

    /// Helper for filling a transaction's nonce using the wallet
    async fn fill_transaction(
        &self,
        tx: &mut TypedTransaction,
        block: Option<BlockId>,
    ) -> Result<(), Self::Error> {
        // get the `from` field's nonce if it's set, else get the signer's nonce
        let from = if tx.from().is_some() && tx.from() != Some(&self.address()) {
            *tx.from().unwrap()
        } else {
            self.address
        };
        tx.set_from(from);

        // get the signer's chain_id if the transaction does not set it
        let chain_id = self.signer.chain_id();
        if tx.chain_id().is_none() {
            tx.set_chain_id(chain_id);
        }

        let nonce = maybe(tx.nonce().cloned(), self.get_transaction_count(from, block)).await?;
        tx.set_nonce(nonce);
        self.inner()
            .fill_transaction(tx, block)
            .await
            .map_err(SignerMiddlewareError::MiddlewareError)?;
        Ok(())
    }

    /// Signs a message with the internal signer, or if none is present it will make a call to
    /// the connected node's `eth_call` API.
    async fn sign<T: Into<Bytes> + Send + Sync>(
        &self,
        data: T,
        _: &Address,
    ) -> Result<Signature, Self::Error> {
        self.signer.sign_message(data.into()).await.map_err(SignerMiddlewareError::SignerError)
    }

    async fn estimate_gas(
        &self,
        tx: &TypedTransaction,
        block: Option<BlockId>,
    ) -> Result<U256, Self::Error> {
        let tx = self.set_tx_from_if_none(tx);
        self.inner.estimate_gas(&tx, block).await.map_err(SignerMiddlewareError::MiddlewareError)
    }

    async fn create_access_list(
        &self,
        tx: &TypedTransaction,
        block: Option<BlockId>,
    ) -> Result<AccessListWithGasUsed, Self::Error> {
        let tx = self.set_tx_from_if_none(tx);
        self.inner
            .create_access_list(&tx, block)
            .await
            .map_err(SignerMiddlewareError::MiddlewareError)
    }

    async fn call(
        &self,
        tx: &TypedTransaction,
        block: Option<BlockId>,
    ) -> Result<Bytes, Self::Error> {
        let tx = self.set_tx_from_if_none(tx);
        self.inner().call(&tx, block).await.map_err(SignerMiddlewareError::MiddlewareError)
    }
}

#[cfg(all(test, not(feature = "celo"), not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use ethers_core::{
        types::TransactionRequest,
        utils::{self, keccak256, Anvil},
    };
    use ethers_providers::Provider;
    use ethers_signers::LocalWallet;
    use std::convert::TryFrom;

    #[tokio::test]
    async fn signs_tx() {
        // retrieved test vector from:
        // https://web3js.readthedocs.io/en/v1.2.0/web3-eth-accounts.html#eth-accounts-signtransaction
        let tx = TransactionRequest {
            from: None,
            to: Some("F0109fC8DF283027b6285cc889F5aA624EaC1F55".parse::<Address>().unwrap().into()),
            value: Some(1_000_000_000.into()),
            gas: Some(2_000_000.into()),
            nonce: Some(0.into()),
            gas_price: Some(21_000_000_000u128.into()),
            data: None,
            chain_id: None,
        }
        .into();
        let chain_id = 1u64;

        // Signer middlewares now rely on a working provider which it can query the chain id from,
        // so we make sure Anvil is started with the chain id that the expected tx was signed
        // with
        let anvil = Anvil::new().args(vec!["--chain-id".to_string(), chain_id.to_string()]).spawn();
        let provider = Provider::try_from(anvil.endpoint()).unwrap();
        let key = "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318"
            .parse::<LocalWallet>()
            .unwrap()
            .with_chain_id(chain_id);
        let client = SignerMiddleware::new(provider, key);

        let tx = client.sign_transaction(tx).await.unwrap();

        assert_eq!(
            keccak256(&tx)[..],
            hex::decode("de8db924885b0803d2edc335f745b2b8750c8848744905684c20b987443a9593")
                .unwrap()
        );

        let expected_rlp = Bytes::from(hex::decode("f869808504e3b29200831e848094f0109fc8df283027b6285cc889f5aa624eac1f55843b9aca008025a0c9cf86333bcb065d140032ecaab5d9281bde80f21b9687b3e94161de42d51895a0727a108a0b8d101465414033c3f705a9c7b826e596766046ee1183dbc8aeaa68").unwrap());
        assert_eq!(tx, expected_rlp);
    }

    #[tokio::test]
    async fn signs_tx_none_chainid() {
        // retrieved test vector from:
        // https://web3js.readthedocs.io/en/v1.2.0/web3-eth-accounts.html#eth-accounts-signtransaction
        // the signature is different because we're testing signer middleware handling the None
        // case for a non-mainnet chain id
        let tx = TransactionRequest {
            from: None,
            to: Some("F0109fC8DF283027b6285cc889F5aA624EaC1F55".parse::<Address>().unwrap().into()),
            value: Some(1_000_000_000.into()),
            gas: Some(2_000_000.into()),
            nonce: Some(U256::zero()),
            gas_price: Some(21_000_000_000u128.into()),
            data: None,
            chain_id: None,
        }
        .into();
        let chain_id = 1337u64;

        // Signer middlewares now rely on a working provider which it can query the chain id from,
        // so we make sure Anvil is started with the chain id that the expected tx was signed
        // with
        let anvil = Anvil::new().args(vec!["--chain-id".to_string(), chain_id.to_string()]).spawn();
        let provider = Provider::try_from(anvil.endpoint()).unwrap();
        let key = "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318"
            .parse::<LocalWallet>()
            .unwrap()
            .with_chain_id(chain_id);
        let client = SignerMiddleware::new(provider, key);

        let tx = client.sign_transaction(tx).await.unwrap();

        let expected_rlp = Bytes::from(hex::decode("f86b808504e3b29200831e848094f0109fc8df283027b6285cc889f5aa624eac1f55843b9aca0080820a95a08290324bae25ca0490077e0d1f4098730333088f6a500793fa420243f35c6b23a06aca42876cd28fdf614a4641e64222fee586391bb3f4061ed5dfefac006be850").unwrap());
        assert_eq!(tx, expected_rlp);
    }

    #[tokio::test]
    async fn anvil_consistent_chainid() {
        let anvil = Anvil::new().spawn();
        let provider = Provider::try_from(anvil.endpoint()).unwrap();
        let chain_id = provider.get_chainid().await.unwrap();
        assert_eq!(chain_id, U256::from(31337));

        // Intentionally do not set the chain id here so we ensure that the signer pulls the
        // provider's chain id.
        let key = LocalWallet::new(&mut rand::thread_rng());

        // combine the provider and wallet and test that the chain id is the same for both the
        // signer returned by the middleware and through the middleware itself.
        let client = SignerMiddleware::new_with_provider_chain(provider, key).await.unwrap();
        let middleware_chainid = client.get_chainid().await.unwrap();
        assert_eq!(chain_id, middleware_chainid);

        let signer = client.signer();
        let signer_chainid = signer.chain_id();
        assert_eq!(chain_id.as_u64(), signer_chainid);
    }

    #[tokio::test]
    async fn anvil_consistent_chainid_not_default() {
        let anvil = Anvil::new().args(vec!["--chain-id", "13371337"]).spawn();
        let provider = Provider::try_from(anvil.endpoint()).unwrap();
        let chain_id = provider.get_chainid().await.unwrap();
        assert_eq!(chain_id, U256::from(13371337));

        // Intentionally do not set the chain id here so we ensure that the signer pulls the
        // provider's chain id.
        let key = LocalWallet::new(&mut rand::thread_rng());

        // combine the provider and wallet and test that the chain id is the same for both the
        // signer returned by the middleware and through the middleware itself.
        let client = SignerMiddleware::new_with_provider_chain(provider, key).await.unwrap();
        let middleware_chainid = client.get_chainid().await.unwrap();
        assert_eq!(chain_id, middleware_chainid);

        let signer = client.signer();
        let signer_chainid = signer.chain_id();
        assert_eq!(chain_id.as_u64(), signer_chainid);
    }
}
