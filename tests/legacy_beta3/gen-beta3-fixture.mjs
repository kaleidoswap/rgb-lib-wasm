// Build a real rgb-lib-wasm 1.0.0-beta.3 IndexedDB state on regtest and dump it.
// Wallet A issues a NIA and sends to B (blinded, then witness); B sends some back;
// a last blinded send to B is broadcast but left unconfirmed (B: WaitingConfirmations).
import 'fake-indexeddb/auto'
import { readFileSync, writeFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'

const HERE = new URL('.', import.meta.url).pathname
const PKG = process.argv[2] // the unpacked @utexo/rgb-lib-wasm@1.0.0-beta.3 package
const m = await import(PKG + '/rgb_lib_wasm_bindings.js')
await m.default({ module_or_path: readFileSync(PKG + '/rgb_lib_wasm_bindings_bg.wasm') })
try { m.init() } catch {}

const INDEXER = 'http://127.0.0.1:24502'
const PROXY = 'rpc://127.0.0.1:24500/json-rpc'
const btc = (...a) => execFileSync('docker', ['compose', '-p', 'migb3', '-f', HERE + 'compose.yaml', 'exec', '-T', 'bitcoind', 'bitcoin-cli', '-regtest', '-rpcuser=user', '-rpcpassword=pass', ...a]).toString().trim()
const minerAddr = btc('-rpcwallet=miner', 'getnewaddress')
const sleep = (ms) => new Promise(r => setTimeout(r, ms))
const tip = async () => Number(await (await fetch(INDEXER + '/blocks/tip/height')).text())
async function mine(n = 1) {
  const target = Number(btc('getblockcount')) + n
  btc('generatetoaddress', String(n), minerAddr)
  while (await tip() < target) await sleep(300)
}
const log = (...a) => console.log(...a)
const j = (x) => JSON.stringify(x, (_, v) => typeof v === 'bigint' ? v.toString() : v)

const out = { network: 'Regtest', wallets: {}, expected: {} }
async function wallet(name) {
  const keys = m.generateKeys('Regtest')
  const wd = {
    dataDir: 'migtest', bitcoinNetwork: 'Regtest', databaseType: 'Sqlite', maxAllocationsPerUtxo: 5,
    accountXpubVanilla: keys.accountXpubVanilla, accountXpubColored: keys.accountXpubColored,
    mnemonic: keys.mnemonic, masterFingerprint: keys.masterFingerprint, vanillaKeychain: null,
    supportedSchemas: ['Nia', 'Ifa'], reuseAddresses: false,
  }
  const w = await m.WasmWallet.create(JSON.stringify(wd))
  const online = await w.goOnline(false, INDEXER)
  out.wallets[name] = { walletData: wd }
  return { w, online, wd }
}
async function fund(x, sats) {
  btc('-rpcwallet=miner', 'sendtoaddress', x.w.getAddress(), (sats / 1e8).toFixed(8))
  await mine(1)
  await x.w.sync(x.online)
}
async function createUtxos(x, num) {
  const psbt = await x.w.createUtxosBegin(x.online, true, num, 5000, 2n, false)
  await x.w.createUtxosEnd(x.online, x.w.signPsbt(psbt), false)
}
async function send(from, assetId, recv, amount, witness) {
  const recipient = {
    recipientId: recv.recipientId,
    witnessData: witness ? { amountSat: '1000', blinding: null } : null,
    assignment: { Fungible: amount },
    transportEndpoints: [PROXY],
  }
  const psbt = await from.w.sendBegin(from.online, { [assetId]: [recipient] }, false, 2n, 1)
  return from.w.sendEnd(from.online, from.w.signPsbt(psbt), false)
}
const refresh = async (x) => x.w.refresh(x.online, null, [], false)
async function settle(...ws) {
  for (const x of ws) await refresh(x)
  for (const x of ws) await refresh(x)
  await mine(1)
  for (const x of ws) await refresh(x)
  for (const x of ws) await refresh(x)
}

const A = await wallet('A'), B = await wallet('B')
await fund(A, 1_000_000); await fund(B, 1_000_000)
await createUtxos(A, 5); await createUtxos(B, 5); await mine(1)
await A.w.sync(A.online); await B.w.sync(B.online)
const nia = A.w.issueAssetNia('MIG', 'Migration test', 0, [1000, 600])
log('issued', nia.assetId)

const r1 = B.w.blindReceive(null, { Fungible: 100 }, null, [PROXY], 1)
log('send blinded', j(await send(A, nia.assetId, r1, 100, false)))
await settle(B, A)
const r2 = B.w.witnessReceive(nia.assetId, { Fungible: 50 }, null, [PROXY], 1)
log('send witness', j(await send(A, nia.assetId, r2, 50, true)))
await settle(B, A)
const r3 = A.w.blindReceive(nia.assetId, { Fungible: 30 }, null, [PROXY], 1)
log('send back', j(await send(B, nia.assetId, r3, 30, false)))
await settle(A, B)
// left unconfirmed: B accepts the consignment, A broadcasts, no block is mined
const r4 = B.w.blindReceive(nia.assetId, { Fungible: 20 }, null, [PROXY], 1)
log('send pending', j(await send(A, nia.assetId, r4, 20, false)))
await refresh(B); await refresh(A); await refresh(B)

for (const [name, x] of [['A', A], ['B', B]]) {
  await x.w.flush()
  out.expected[name] = {
    assets: x.w.listAssets([]),
    balance: x.w.getAssetBalance(nia.assetId),
    transfers: x.w.listTransfers(nia.assetId),
    unspents: x.w.listUnspents(false),
  }
  log(name, 'balance', j(out.expected[name].balance), 'transfers',
    out.expected[name].transfers.map(t => t.kind + ':' + t.status).join(','))
}
out.assetId = nia.assetId

// dump every IndexedDB snapshot as stored (key -> JSON string)
const req = indexedDB.open('rgb_lib_wallet')
const db = await new Promise((res, rej) => { req.onsuccess = () => res(req.result); req.onerror = rej })
const tx = db.transaction('snapshots', 'readonly').objectStore('snapshots')
const keys = await new Promise(res => { const r = tx.getAllKeys(); r.onsuccess = () => res(r.result) })
const vals = await new Promise(res => { const r = tx.getAll(); r.onsuccess = () => res(r.result) })
out.idb = Object.fromEntries(keys.map((k, i) => [k, vals[i]]))
log('idb keys', keys)
writeFileSync(process.argv[3] ?? 'beta3-fixture.json', j(out))
log('FIXTURE OK')
process.exit(0)
