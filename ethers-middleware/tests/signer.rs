#![allow(unused)]
use ethers_providers::{Http, JsonRpcClient, Middleware, Provider, RINKEBY};

use ethers_core::{
    types::{BlockNumber, TransactionRequest},
    utils::parse_units,
};
use ethers_middleware::signer::SignerMiddleware;
use ethers_signers::{coins_bip39::English, LocalWallet, MnemonicBuilder, Signer};
use once_cell::sync::Lazy;
use std::{convert::TryFrom, iter::Cycle, sync::atomic::AtomicU8, time::Duration};

static WALLETS: Lazy<TestWallets> = Lazy::new(|| {
    TestWallets {
        mnemonic: MnemonicBuilder::default()
            // Please don't drain this :)
            .phrase("impose air often almost medal sudden finish quote dwarf devote theme layer"),
        next: Default::default(),
    }
});

#[cfg(not(feature = "celo"))]
use ethers_core::types::{Address, Eip1559TransactionRequest};

#[tokio::test]
#[cfg(feature = "celo")]
async fn deploy_and_call_contract() {
    use ethers_contract::ContractFactory;
    use ethers_core::{
        abi::Abi,
        types::{BlockNumber, Bytes, H256, U256},
    };
    use ethers_solc::Solc;
    use std::sync::Arc;

    // compiles the given contract and returns the ABI and Bytecode
    fn compile_contract(path: &str, name: &str) -> (Abi, Bytes) {
        let path = format!("./tests/solidity-contracts/{}", path);
        let compiled = Solc::default().compile_source(&path).unwrap();
        let contract = compiled.get(&path, name).expect("could not find contract");
        let (abi, bin, _) = contract.into_parts_or_default();
        (abi, bin)
    }

    let (abi, bytecode) = compile_contract("SimpleStorage.sol", "SimpleStorage");

    // Celo testnet
    let provider = Provider::<Http>::try_from("https://alfajores-forno.celo-testnet.org")
        .unwrap()
        .interval(Duration::from_millis(6000));
    let chain_id = provider.get_chainid().await.unwrap().as_u64();

    // Funded with https://celo.org/developers/faucet
    let wallet = "58ea5643a78c36926ad5128a6b0d8dfcc7fc705788a993b1c724be3469bc9697"
        .parse::<LocalWallet>()
        .unwrap()
        .with_chain_id(chain_id);
    let client = SignerMiddleware::new_with_provider_chain(provider, wallet).await.unwrap();
    let client = Arc::new(client);

    let factory = ContractFactory::new(abi, bytecode, client);
    let deployer = factory.deploy(()).unwrap().legacy();
    let contract = deployer.block(BlockNumber::Pending).send().await.unwrap();

    let value: U256 = contract.method("value", ()).unwrap().call().await.unwrap();
    assert_eq!(value, 0.into());

    // make a state mutating transaction
    // gas estimation costs are sometimes under-reported on celo,
    // so we manually set it to avoid failures
    let call = contract.method::<_, H256>("setValue", U256::from(1)).unwrap().gas(100000);
    let pending_tx = call.send().await.unwrap();
    let _receipt = pending_tx.await.unwrap();

    let value: U256 = contract.method("value", ()).unwrap().call().await.unwrap();
    assert_eq!(value, 1.into());
}

#[derive(Debug, Default)]
struct TestWallets {
    mnemonic: MnemonicBuilder<English>,
    next: AtomicU8,
}

impl TestWallets {
    /// Helper for funding the wallets with an instantiated provider
    #[allow(unused)]

    pub fn next(&self) -> LocalWallet {
        let idx = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // println!("Got wallet {:?}", wallet.address());
        self.get(idx)
    }

    pub fn get<T: Into<u32>>(&self, idx: T) -> LocalWallet {
        self.mnemonic
            .clone()
            .index(idx)
            .expect("index not found")
            .build()
            .expect("cannot build wallet")
    }
}
