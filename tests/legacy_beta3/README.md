# 1.0.0-beta.3 snapshot migration

`tests/fixtures/beta3_snapshots.json` holds the IndexedDB snapshots of two regtest wallets written
by the npm package `@utexo/rgb-lib-wasm@1.0.0-beta.3`, with what that version listed for them
(assets, balances, transfers, unspents). The unit tests in `src/wallet/legacy_snapshot.rs` migrate
them. The scripts here regenerate the fixture and run the migration end to end in Node.

1. Start the regtest services (bitcoind, esplora, rgb-proxy-server 0.3.0 for beta.3 on port 24500
   and 0.4.0 on port 24510), create a mining wallet and mine 111 blocks:

   ```sh
   docker compose -p migb3 -f compose.yaml up -d
   docker compose -p migb3 -f compose.yaml exec -T bitcoind bitcoin-cli -regtest -rpcuser=user -rpcpassword=pass createwallet miner
   docker compose -p migb3 -f compose.yaml exec -T bitcoind bitcoin-cli -regtest -rpcuser=user -rpcpassword=pass -rpcwallet=miner -generate 111
   npm install fake-indexeddb@6
   ```

2. Generate a fixture with beta.3 (wallet A issues a NIA, sends to B blinded and witness, B sends
   some back, a last send is broadcast but left unconfirmed):

   ```sh
   npm pack @utexo/rgb-lib-wasm@1.0.0-beta.3 && tar -xzf utexo-rgb-lib-wasm-1.0.0-beta.3.tgz
   node gen-beta3-fixture.mjs "$PWD/package" beta3-fixture.json
   ```

   `tests/fixtures/beta3_snapshots.json` keeps, for each wallet, `walletData`, the legacy key, the
   snapshot and the expected listings of the generated file.

3. Run the end to end test against the build of this branch (`bindings/wasm/build.sh`). It opens
   the beta.3 snapshots, checks the migrated wallets list what beta.3 listed, reopens them, settles
   the transfer left pending and spends the migrated assets to a new wallet. It needs the chain
   state of step 2, so it runs once per generated fixture:

   ```sh
   node migrate-e2e.mjs "$PWD/../../bindings/wasm/pkg"
   ```
