# Clients for interacting with Ethereum nodes

This crate provides asynchronous
[Ethereum JSON-RPC](https://github.com/ethereum/wiki/wiki/JSON-RPC) compliant
clients.

For more documentation on the available calls, refer to the
[`Provider`](./struct.Provider.html) struct.

# Examples


# Ethereum Name Service

The provider may also be used to resolve
[Ethereum Name Service](https://ens.domains) (ENS) names to addresses (and vice
versa). The default ENS address is
[mainnet](https://etherscan.io/address/0x00000000000C2E074eC69A0dFb2997BA6C7d2e1e)
and can be overriden by calling the [`ens`](./struct.Provider.html#method.ens)
method on the provider.

