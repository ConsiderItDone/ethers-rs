use crate::{
    base::{encode_function_data, AbiError, BaseContract},
    call::ContractCall,
    event::{EthEvent, Event},
    EthLogDecode,
};

use ethers_core::{
    abi::{Abi, Detokenize, Error, EventExt, Function, Tokenize},
    types::{Address, Filter, Selector, ValueOrArray},
};

#[cfg(not(feature = "legacy"))]
use ethers_core::types::Eip1559TransactionRequest;
#[cfg(feature = "legacy")]
use ethers_core::types::TransactionRequest;

use ethers_providers::Middleware;

use std::{fmt::Debug, marker::PhantomData, sync::Arc};

#[derive(Debug)]
pub struct Contract<M> {
    base_contract: BaseContract,
    client: Arc<M>,
    address: Address,
}

impl<M> Clone for Contract<M> {
    fn clone(&self) -> Self {
        Contract {
            base_contract: self.base_contract.clone(),
            client: self.client.clone(),
            address: self.address,
        }
    }
}

impl<M: Middleware> Contract<M> {
    /// Creates a new contract from the provided client, abi and address
    pub fn new(address: Address, abi: impl Into<BaseContract>, client: impl Into<Arc<M>>) -> Self {
        Self { base_contract: abi.into(), client: client.into(), address }
    }

    /// Returns an [`Event`](crate::builders::Event) builder for the provided event.
    pub fn event<D: EthEvent>(&self) -> Event<M, D> {
        self.event_with_filter(Filter::new().event(&D::abi_signature()))
    }

    /// Returns an [`Event`](crate::builders::Event) builder with the provided filter.
    pub fn event_with_filter<D: EthLogDecode>(&self, filter: Filter) -> Event<M, D> {
        Event {
            provider: &self.client,
            filter: filter.address(ValueOrArray::Value(self.address)),
            datatype: PhantomData,
        }
    }

    /// Returns an [`Event`](crate::builders::Event) builder with the provided name.
    pub fn event_for_name<D: EthLogDecode>(&self, name: &str) -> Result<Event<M, D>, Error> {
        // get the event's full name
        let event = self.base_contract.abi.event(name)?;
        Ok(self.event_with_filter(Filter::new().event(&event.abi_signature())))
    }

    /// Returns a transaction builder for the provided function name. If there are
    /// multiple functions with the same name due to overloading, consider using
    /// the `method_hash` method instead, since this will use the first match.
    pub fn method<T: Tokenize, D: Detokenize>(
        &self,
        name: &str,
        args: T,
    ) -> Result<ContractCall<M, D>, AbiError> {
        // get the function
        let function = self.base_contract.abi.function(name)?;
        self.method_func(function, args)
    }

    /// Returns a transaction builder for the selected function signature. This should be
    /// preferred if there are overloaded functions in your smart contract
    pub fn method_hash<T: Tokenize, D: Detokenize>(
        &self,
        signature: Selector,
        args: T,
    ) -> Result<ContractCall<M, D>, AbiError> {
        let function = self
            .base_contract
            .methods
            .get(&signature)
            .map(|(name, index)| &self.base_contract.abi.functions[name][*index])
            .ok_or_else(|| Error::InvalidName(hex::encode(signature)))?;
        self.method_func(function, args)
    }

    fn method_func<T: Tokenize, D: Detokenize>(
        &self,
        function: &Function,
        args: T,
    ) -> Result<ContractCall<M, D>, AbiError> {
        let data = encode_function_data(function, args)?;

        #[cfg(feature = "legacy")]
        let tx = TransactionRequest {
            to: Some(self.address.into()),
            data: Some(data),
            ..Default::default()
        };
        #[cfg(not(feature = "legacy"))]
        let tx = Eip1559TransactionRequest {
            to: Some(self.address.into()),
            data: Some(data),
            ..Default::default()
        };

        let tx = tx.into();

        Ok(ContractCall {
            tx,
            client: Arc::clone(&self.client), // cheap clone behind the Arc
            block: None,
            function: function.to_owned(),
            datatype: PhantomData,
        })
    }

    /// Returns a new contract instance at `address`.
    ///
    /// Clones `self` internally
    #[must_use]
    pub fn at<T: Into<Address>>(&self, address: T) -> Self
    where
        M: Clone,
    {
        let mut this = self.clone();
        this.address = address.into();
        this
    }

    /// Returns a new contract instance using the provided client
    ///
    /// Clones `self` internally
    #[must_use]
    pub fn connect<N>(&self, client: Arc<N>) -> Contract<N>
    where
        N: Clone,
    {
        Contract { base_contract: self.base_contract.clone(), client, address: self.address }
    }

    /// Returns the contract's address
    pub fn address(&self) -> Address {
        self.address
    }

    /// Returns a reference to the contract's ABI
    pub fn abi(&self) -> &Abi {
        &self.base_contract.abi
    }

    /// Returns a reference to the contract's client
    pub fn client(&self) -> &M {
        &self.client
    }
}

impl<M: Middleware> std::ops::Deref for Contract<M> {
    type Target = BaseContract;
    fn deref(&self) -> &Self::Target {
        &self.base_contract
    }
}
