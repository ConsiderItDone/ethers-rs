Ethers uses a middleware-based architecture. You start the middleware stack with
a [`Provider`](ethers_providers::Provider), and wrap it with additional
middleware functionalities that you need.

## Available Middleware

- [`Signer`](./signer/struct.SignerMiddleware.html): Signs transactions locally,
  with a private key or a hardware wallet
- [`Nonce Manager`](./nonce_manager/struct.NonceManagerMiddleware.html): Manages
  nonces locally, allowing the rapid broadcast of transactions without having to
  wait for them to be submitted
- [`Gas Escalator`](./gas_escalator/struct.GasEscalatorMiddleware.html): Bumps
  transaction gas prices in the background
- [`Gas Oracle`](./gas_oracle/struct.GasOracleMiddleware.html): Allows getting
  your gas price estimates from places other than `eth_gasPrice`.
- [`Transformer`](./transformer/trait.Transformer.html): Allows intercepting and
  transforming a transaction to be broadcasted via a proxy wallet, e.g.
  [`DSProxy`](./transformer/struct.DsProxy.html).

