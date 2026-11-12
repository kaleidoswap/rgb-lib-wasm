// Open the real 1.0.0-beta.3 IndexedDB state with the migrating beta.4 build, then settle the
// transfer left pending under beta.3 and spend the migrated assets to a fresh beta.4 wallet.
import 'fake-indexeddb/auto'
import { readFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import assert from 'node:assert/strict'

const HERE = new URL('.', import.meta.url).pathname
const PKG = process.argv[2]
const m = await import(PKG + '/rgb_lib_wasm_bindings.js')
await m.default({ module_or_path: readFileSync(PKG + '/rgb_lib_wasm_bindings_bg.wasm') })
try { m.init() } catch {}

const INDEXER = 'http://127.0.0.1:24502'
const PROXY = 'rpc://127.0.0.1:24510/json-rpc'
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
const norm = (x) => JSON.parse(j(x))

const fixture = JSON.parse(readFileSync(HERE + '../fixtures/beta3_snapshots.json'))
const assetId = fixture.assetId

// put the beta.3 snapshots where beta.3 left them
const req = indexedDB.open('rgb_lib_wallet', 1)
req.onupgradeneeded = () => req.result.createObjectStore('snapshots')
const idb = await new Promise((res, rej) => { req.onsuccess = () => res(req.result); req.onerror = rej })
const put = idb.transaction('snapshots', 'readwrite').objectStore('snapshots')
for (const w of Object.values(fixture.wallets)) put.put(w.snapshot, w.legacyKey)
await new Promise(res => { put.transaction.oncomplete = res })
idb.close()
const idbKeys = async () => {
  const r = indexedDB.open('rgb_lib_wallet')
  const db = await new Promise(res => { r.onsuccess = () => res(r.result) })
  const keys = await new Promise(res => { const q = db.transaction('snapshots').objectStore('snapshots').getAllKeys(); q.onsuccess = () => res(q.result) })
  db.close()
  return keys
}

const onlineOptions = { indexerConfig: { Esplora: { url: INDEXER } }, skipConsistencyCheck: false, proxyAuthKeys: {}, evmChains: {} }
async function open(name) {
  const wd = fixture.wallets[name].walletData
  const w = await m.WasmWallet.create(JSON.stringify(wd))
  return { name, w, wd }
}
function sameAsBeta3(x) {
  const e = fixture.wallets[x.name].expected
  assert.deepEqual(norm(x.w.getAssetBalance(assetId)), norm(e.balance), x.name + ' balance')
  const ts = norm(x.w.listTransfers({ id: assetId }))
  const pick = (t) => ({ idx: t.idx, status: t.status, kind: t.kind, txid: t.txid, recipientId: t.recipientId, requested: t.requestedAssignment, assignments: t.assignments })
  assert.deepEqual(ts.map(pick), norm(e.transfers).map(pick), x.name + ' transfers')
  const nia = norm(x.w.listAssets([])).nia
  assert.equal(nia.length, 1); assert.equal(nia[0].assetId, assetId); assert.equal(nia[0].ticker, 'MIG')
}

// 1. open: the beta.3 snapshot is migrated and saved under the new key, the old one is kept
let A = await open('A'), B = await open('B')
for (const x of [A, B]) {
  assert.deepEqual(norm(x.w.legacyMigration()), { status: 'migrated' }, x.name)
  sameAsBeta3(x)
  log(x.name, 'migrated', j(x.w.getAssetBalance(assetId)))
}
const keys = await idbKeys()
log('idb keys', keys)
for (const w of Object.values(fixture.wallets)) {
  assert.ok(keys.includes(w.legacyKey)); assert.ok(keys.includes(w.legacyKey + '/Regtest'))
}

// 2. reopen: the migrated snapshot is loaded, no migration runs again
A.w.free(); B.w.free()
A = await open('A'); B = await open('B')
for (const x of [A, B]) { assert.equal(x.w.legacyMigration(), null); sameAsBeta3(x) }
log('reopened without migrating again')

// 3. online: BTC state comes back from a full scan, the pending beta.3 transfer settles
for (const x of [A, B]) { x.online = await x.w.goOnline(onlineOptions); await x.w.fullScan(x.online) }
log('btc after full scan', j(A.w.getBtcBalance()), j(B.w.getBtcBalance()))
await mine(1)
const refresh = async (x) => x.w.refresh(x.online, null, [], false)
for (let i = 0; i < 2; i++) for (const x of [B, A]) await refresh(x)
for (const x of [A, B]) {
  const ts = norm(x.w.listTransfers({ id: assetId }))
  log(x.name, ts.map(t => t.kind + ':' + t.status).join(','), j(x.w.getAssetBalance(assetId)))
  assert.ok(ts.every(t => t.status === 'Settled'), x.name + ' all settled')
}
assert.equal(norm(B.w.getAssetBalance(assetId)).settled, '140')
assert.equal(norm(A.w.getAssetBalance(assetId)).settled, '1460')

// 4. spend the migrated assets to a fresh beta.4 wallet, from both migrated wallets
const keysC = m.generateKeys('Regtest')
const wdC = { ...A.wd, dataDir: 'migtest-c', mnemonic: keysC.mnemonic, masterFingerprint: keysC.masterFingerprint, accountXpubVanilla: keysC.accountXpubVanilla, accountXpubColored: keysC.accountXpubColored }
const C = { name: 'C', w: await m.WasmWallet.create(JSON.stringify(wdC)) }
C.online = await C.w.goOnline(onlineOptions)
btc('-rpcwallet=miner', 'sendtoaddress', C.w.getAddress(), '0.01'); await mine(1); await C.w.sync(C.online)
const psbt = await C.w.createUtxosBegin(C.online, true, 4, 5000, 2n, false)
await C.w.createUtxosEnd(C.online, C.w.signPsbt(psbt), false); await mine(1); await C.w.sync(C.online)

async function sendTo(from, amount) {
  const rcv = await C.w.blindReceive(C.online, null, { Fungible: amount }, Math.floor(Date.now() / 1000) + 86400, [PROXY], 1, null)
  const inv = new m.WasmInvoice(rcv.invoice).invoiceData()
  const recipient = { recipientId: inv.recipientId, witnessData: null, assignment: inv.assignment, transportEndpoints: inv.transportEndpoints, publicKey: inv.publicKey, encryption: inv.encryption, expirationTimestamp: inv.expirationTimestamp }
  const begun = await from.w.sendBegin(from.online, { [assetId]: [recipient] }, false, 2n, 1, Math.floor(Date.now() / 1000) + 3600, PROXY, null, false)
  const res = await from.w.sendEnd(from.online, from.w.signPsbt(begun.psbt), false)
  log(from.name, 'sent', amount, 'txid', res.txid)
  for (let i = 0; i < 3; i++) { await refresh(C); await refresh(from) }
  await mine(1)
  for (let i = 0; i < 2; i++) { await refresh(C); await refresh(from) }
}
await sendTo(A, 400)
await sendTo(B, 100)
log('C', j(C.w.getAssetBalance(assetId)), 'A', j(A.w.getAssetBalance(assetId)), 'B', j(B.w.getAssetBalance(assetId)))
assert.equal(norm(C.w.getAssetBalance(assetId)).settled, '500')
assert.equal(norm(A.w.getAssetBalance(assetId)).settled, '1060')
assert.equal(norm(B.w.getAssetBalance(assetId)).settled, '40')
log('MIGRATION E2E OK')
process.exit(0)
