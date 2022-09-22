mod factory;
use factory::{CreatedFilter, DsProxyFactory, ADDRESS_BOOK};

use super::{Transformer, TransformerError};
use ethers_contract::{builders::ContractCall, BaseContract, ContractError};
use ethers_core::{
    abi::parse_abi,
    types::{transaction::eip2718::TypedTransaction, *},
    utils::id,
};
use ethers_providers::Middleware;
use std::sync::Arc;

/// The function signature of DsProxy's execute function, to execute data on a target address.
const DS_PROXY_EXECUTE_TARGET: &str =
    "function execute(address target, bytes memory data) public payable returns (bytes memory response)";
/// The function signature of DsProxy's execute function, to deploy bytecode and execute data on it.
const DS_PROXY_EXECUTE_CODE: &str =
    "function execute(bytes memory code, bytes memory data) public payable returns (address target, bytes memory response)";

#[derive(Debug, Clone)]
pub struct DsProxy {
    address: Address,
    contract: BaseContract,
}

impl DsProxy {
    /// Create a new instance of DsProxy by providing the address of the DsProxy contract that has
    /// already been deployed to the Ethereum network.
    pub fn new(address: Address) -> Self {
        let contract = parse_abi(&[DS_PROXY_EXECUTE_TARGET, DS_PROXY_EXECUTE_CODE])
            .expect("could not parse ABI")
            .into();

        Self { address, contract }
    }

    /// The address of the DsProxy instance.
    pub fn address(&self) -> Address {
        self.address
    }
}

impl DsProxy {
    /// Execute a tx through the DsProxy instance. The target can either be a deployed smart
    /// contract's address, or bytecode of a compiled smart contract. Depending on the target, the
    /// appropriate `execute` method is called, that is, either
    /// [execute(address,bytes)](https://github.com/dapphub/ds-proxy/blob/master/src/proxy.sol#L53-L58)
    /// or [execute(bytes,bytes)](https://github.com/dapphub/ds-proxy/blob/master/src/proxy.sol#L39-L42).
    pub fn execute<M: Middleware, C: Into<Arc<M>>, T: Into<AddressOrBytes>>(
        &self,
        client: C,
        target: T,
        data: Bytes,
    ) -> Result<ContractCall<M, Bytes>, ContractError<M>> {
        // construct the full contract using DsProxy's address and the injected client.
        let ds_proxy = self.contract.clone().into_contract(self.address, client.into());

        match target.into() {
            // handle the case when the target is an address to a deployed contract.
            AddressOrBytes::Address(addr) => {
                let selector = id("execute(address,bytes)");
                let args = (addr, data);
                Ok(ds_proxy.method_hash(selector, args)?)
            }
            // handle the case when the target is actually bytecode of a contract to be deployed
            // and executed on.
            AddressOrBytes::Bytes(code) => {
                let selector = id("execute(bytes,bytes)");
                let args = (code, data);
                Ok(ds_proxy.method_hash(selector, args)?)
            }
        }
    }
}

impl Transformer for DsProxy {
    fn transform(&self, tx: &mut TypedTransaction) -> Result<(), TransformerError> {
        // the target address cannot be None.
        let target =
            *tx.to_addr().ok_or_else(|| TransformerError::MissingField("to".to_string()))?;

        // fetch the data field.
        let data = tx.data().cloned().unwrap_or_else(|| vec![].into());

        // encode data as the ABI encoded data for DSProxy's execute method.
        let selector = id("execute(address,bytes)");
        let encoded_data = self.contract.encode_with_selector(selector, (target, data))?;

        // update appropriate fields of the proxy tx.
        tx.set_data(encoded_data);
        tx.set_to(self.address);

        Ok(())
    }
}
