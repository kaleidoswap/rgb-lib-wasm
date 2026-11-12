// RefCell borrows held across .await are safe on wasm32 (single-threaded runtime).
#![allow(clippy::await_holding_refcell_ref)]

use serde::Serialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::panic;
use std::str::FromStr;
use wasm_bindgen::prelude::*;

use rgb_lib_wasm::bitcoin::{OutPoint, psbt::Psbt};
use rgb_lib_wasm::wallet::rust_only::{
    AssetColoringInfo, ColoringInfo, ExpectedTransfer, FetchedConsignment, PsbtOpExpiry,
};
use rgb_lib_wasm::wallet::{
    OnchainSwapCompletion, OnchainSwapOffer, OnchainSwapProposal, OnchainSwapRole, Online,
    OnlineOptions, Outpoint, Recipient, RefreshFilter, WalletData,
};
use rgb_lib_wasm::{Assignment, ContractId, FileContent, RgbTransfer, Wallet};

/// Serialize to JsValue using BigInt for u64/i64 values that exceed Number.MAX_SAFE_INTEGER.
fn to_js<T: Serialize>(val: &T) -> Result<JsValue, JsValue> {
    let serializer =
        serde_wasm_bindgen::Serializer::new().serialize_large_number_types_as_bigints(true);
    val.serialize(&serializer)
        .map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Parse an optional encryption preference: "Required", "Unsupported" or "Optional".
fn parse_encryption(
    encryption: Option<String>,
) -> Result<Option<rgb_lib_wasm::Encryption>, JsValue> {
    encryption
        .map(|e| {
            serde_wasm_bindgen::from_value(JsValue::from_str(&e))
                .map_err(|_| JsValue::from_str(&format!("Invalid encryption: {e}")))
        })
        .transpose()
}

/// Parse an optional Online object, `null` and `undefined` meaning none.
fn js_to_online_opt(value: JsValue) -> Result<Option<Online>, JsValue> {
    if value.is_null() || value.is_undefined() {
        return Ok(None);
    }
    serde_wasm_bindgen::from_value(value)
        .map(Some)
        .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))
}

/// Read a u64 from a JS number, BigInt or decimal string.
fn js_to_u64(value: JsValue) -> Result<u64, JsValue> {
    match value.as_string() {
        Some(s) => s.parse::<u64>().map_err(|e| e.to_string()),
        None => serde_wasm_bindgen::from_value::<u64>(value).map_err(|e| e.to_string()),
    }
    .map_err(|e| JsValue::from_str(&format!("Invalid expiration timestamp: {e}")))
}

fn js_err(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

fn online_from_js(online_js: JsValue) -> Result<Online, JsValue> {
    serde_wasm_bindgen::from_value(online_js)
        .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))
}

/// A u64 given as a decimal string, a BigInt or a safe-integer Number.
fn u64_from_js(value: JsValue, what: &str) -> Result<u64, JsValue> {
    let parsed = match value.as_string() {
        Some(s) => s.parse().ok(),
        None => serde_wasm_bindgen::from_value(value).ok(),
    };
    parsed.ok_or_else(|| JsValue::from_str(&format!("Invalid {what}")))
}

/// JS shape of [`AssetColoringInfo`]: `outputMap` maps a vout (object key) to an amount.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsAssetColoringInfo {
    output_map: HashMap<String, u64>,
    static_blinding: Option<u64>,
}

/// JS shape of [`ColoringInfo`]: `assetInfoMap` maps an asset ID to its coloring.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsColoringInfo {
    asset_info_map: HashMap<String, JsAssetColoringInfo>,
    static_blinding: Option<u64>,
    nonce: Option<u64>,
}

impl TryFrom<JsColoringInfo> for ColoringInfo {
    type Error = JsValue;

    fn try_from(info: JsColoringInfo) -> Result<Self, JsValue> {
        let mut asset_info_map = HashMap::new();
        for (asset_id, asset_info) in info.asset_info_map {
            let contract_id = ContractId::from_str(&asset_id)
                .map_err(|_| JsValue::from_str(&format!("Invalid asset ID: {asset_id}")))?;
            let mut output_map = HashMap::new();
            for (vout, amount) in asset_info.output_map {
                let vout: u32 = vout
                    .parse()
                    .map_err(|_| JsValue::from_str(&format!("Invalid vout: {vout}")))?;
                output_map.insert(vout, amount);
            }
            asset_info_map.insert(
                contract_id,
                AssetColoringInfo {
                    output_map,
                    static_blinding: asset_info.static_blinding,
                },
            );
        }
        Ok(ColoringInfo {
            asset_info_map,
            static_blinding: info.static_blinding,
            nonce: info.nonce,
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsAcceptedTransfer {
    /// Strict-encoded consignment, hex
    consignment: String,
    assignments: Vec<Assignment>,
}

fn consignment_from_hex(consignment_hex: &str) -> Result<RgbTransfer, JsValue> {
    let bytes =
        hex::decode(consignment_hex).map_err(|_| JsValue::from_str("Invalid consignment hex"))?;
    RgbTransfer::load(&bytes[..])
        .map_err(|e| JsValue::from_str(&format!("Invalid consignment: {e}")))
}

fn consignment_to_hex(consignment: &RgbTransfer) -> Result<String, JsValue> {
    let mut bytes = vec![];
    consignment.save(&mut bytes).map_err(js_err)?;
    Ok(hex::encode(bytes))
}

#[wasm_bindgen(js_namespace = console)]
extern "C" {
    #[wasm_bindgen(js_name = error)]
    fn console_error(s: &str);
}

#[wasm_bindgen(start)]
pub fn init() {
    let _ = instant::Instant::now();
    static PANIC_HOOK: std::sync::Once = std::sync::Once::new();
    PANIC_HOOK.call_once(|| {
        panic::set_hook(Box::new(move |info| {
            let location = info
                .location()
                .map(|l| {
                    format!(
                        "at {}:{}",
                        l.file().rsplit('/').next().unwrap_or("unknown"),
                        l.line()
                    )
                })
                .unwrap_or_default();
            let msg = format!("[rgb-lib WASM panic] {location}");
            console_error(&msg);
        }));
    });
}

#[wasm_bindgen(js_name = "generateKeys")]
pub fn generate_keys(network: &str) -> Result<JsValue, JsValue> {
    let bitcoin_network = network
        .parse::<rgb_lib_wasm::BitcoinNetwork>()
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let keys = rgb_lib_wasm::generate_keys(bitcoin_network);
    to_js(&keys)
}

#[wasm_bindgen(js_name = "restoreKeys")]
pub fn restore_keys(network: &str, mnemonic: &str) -> Result<JsValue, JsValue> {
    let bitcoin_network = network
        .parse::<rgb_lib_wasm::BitcoinNetwork>()
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let keys = rgb_lib_wasm::restore_keys(bitcoin_network, mnemonic.to_string())
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&keys)
}

#[wasm_bindgen]
pub struct WasmWallet {
    inner: RefCell<Wallet>,
}

#[wasm_bindgen]
impl WasmWallet {
    /// Create a new RGB wallet from a JSON-encoded WalletData.
    /// It does not load IndexedDB and cannot save over a stored snapshot; use `create()`.
    #[wasm_bindgen(constructor)]
    pub fn new(wallet_data_json: &str) -> Result<WasmWallet, JsValue> {
        let wd: WalletData = serde_json::from_str(wallet_data_json)
            .map_err(|e| JsValue::from_str(&format!("Invalid WalletData JSON: {e}")))?;
        let wallet = Wallet::new(wd).map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(WasmWallet {
            inner: RefCell::new(wallet),
        })
    }

    /// Create a new RGB wallet with IndexedDB state restoration.
    ///
    /// Like `new()`, but asynchronously checks IndexedDB for a previously saved
    /// snapshot and restores it, so wallet state survives page refreshes.
    #[wasm_bindgen(js_name = "create")]
    pub async fn create(wallet_data_json: &str) -> Result<WasmWallet, JsValue> {
        let wd: WalletData = serde_json::from_str(wallet_data_json)
            .map_err(|e| JsValue::from_str(&format!("Invalid WalletData JSON: {e}")))?;
        let wallet = Wallet::restore(wd)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(WasmWallet {
            inner: RefCell::new(wallet),
        })
    }

    /// Durably persist current wallet state to IndexedDB.
    #[wasm_bindgen(js_name = "flush")]
    pub async fn flush(&self) -> Result<(), JsValue> {
        self.inner
            .borrow()
            .flush()
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Return the error of the last failed IndexedDB save, until a save commits.
    #[wasm_bindgen(js_name = "lastSaveError")]
    pub fn last_save_error(&self) -> Option<String> {
        self.inner.borrow().last_save_error()
    }

    /// Return the WalletData as a JS object.
    /// The mnemonic is not returned: `mnemonic` is null.
    #[wasm_bindgen(js_name = "getWalletData")]
    pub fn get_wallet_data(&self) -> Result<JsValue, JsValue> {
        let mut wd = self.inner.borrow().get_wallet_data();
        wd.mnemonic = None;
        let data = to_js(&wd)?;
        js_sys::Reflect::set(&data, &JsValue::from_str("mnemonic"), &JsValue::NULL)?;
        Ok(data)
    }

    /// Issue a new NIA (Non-Inflatable Asset).
    ///
    /// `amounts_js` is a JS array of u64 values.
    #[wasm_bindgen(js_name = "issueAssetNia")]
    pub fn issue_asset_nia(
        &self,
        ticker: &str,
        name: &str,
        precision: u8,
        amounts_js: JsValue,
    ) -> Result<JsValue, JsValue> {
        let amounts: Vec<u64> = serde_wasm_bindgen::from_value(amounts_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid amounts array: {e}")))?;
        let asset = self
            .inner
            .borrow()
            .issue_asset_nia(ticker.to_string(), name.to_string(), precision, amounts)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&asset)
    }

    /// Issue a new IFA (Inflatable Fungible Asset).
    ///
    /// `amounts_js` is a JS array of u64 values.
    /// `inflation_amounts_js` is a JS array of u64 values for inflation allowances.
    /// `link_right` issues the link right, `linked_from_contract` is the contract linked from.
    #[wasm_bindgen(js_name = "issueAssetIfa")]
    #[allow(clippy::too_many_arguments)]
    pub fn issue_asset_ifa(
        &self,
        ticker: &str,
        name: &str,
        precision: u8,
        amounts_js: JsValue,
        inflation_amounts_js: JsValue,
        reject_list_url: Option<String>,
        link_right: bool,
        linked_from_contract: Option<String>,
    ) -> Result<JsValue, JsValue> {
        let amounts: Vec<u64> = serde_wasm_bindgen::from_value(amounts_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid amounts array: {e}")))?;
        let inflation_amounts: Vec<u64> = serde_wasm_bindgen::from_value(inflation_amounts_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid inflation_amounts array: {e}")))?;
        let asset = self
            .inner
            .borrow()
            .issue_asset_ifa(
                ticker.to_string(),
                name.to_string(),
                precision,
                amounts,
                inflation_amounts,
                reject_list_url,
                link_right,
                linked_from_contract,
            )
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&asset)
    }

    /// Issue a new BFA (Bridged Fungible Asset) with `mint_rights` mint rights and no units.
    ///
    /// `contract_address` is the bridge contract on the EVM chain `chain_id`.
    /// `reject_list_address` is an optional reject list contract on the same chain.
    /// `link_right` issues the link right, `linked_from_contract` is the contract linked from.
    #[wasm_bindgen(js_name = "issueAssetBfa")]
    #[allow(clippy::too_many_arguments)]
    pub fn issue_asset_bfa(
        &self,
        ticker: &str,
        name: &str,
        precision: u8,
        mint_rights: u8,
        chain_id: u64,
        contract_address: &str,
        reject_list_address: Option<String>,
        link_right: bool,
        linked_from_contract: Option<String>,
    ) -> Result<JsValue, JsValue> {
        let asset = self
            .inner
            .borrow()
            .issue_asset_bfa(
                ticker.to_string(),
                name.to_string(),
                precision,
                mint_rights,
                chain_id,
                contract_address.to_string(),
                reject_list_address,
                link_right,
                linked_from_contract,
            )
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&asset)
    }

    /// Return the BTC balance. Always skips sync on wasm32.
    #[wasm_bindgen(js_name = "getBtcBalance")]
    pub fn get_btc_balance(&self) -> Result<JsValue, JsValue> {
        let balance = self
            .inner
            .borrow_mut()
            .get_btc_balance(None, true)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&balance)
    }

    /// Return metadata for a specific asset (name, ticker, precision, supply, etc.).
    #[wasm_bindgen(js_name = "getAssetMetadata")]
    pub fn get_asset_metadata(&self, asset_id: &str) -> Result<JsValue, JsValue> {
        let metadata = self
            .inner
            .borrow()
            .get_asset_metadata(asset_id.to_string())
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&metadata)
    }

    /// Return the balance for a specific asset.
    #[wasm_bindgen(js_name = "getAssetBalance")]
    pub fn get_asset_balance(&self, asset_id: &str) -> Result<JsValue, JsValue> {
        let balance = self
            .inner
            .borrow()
            .get_asset_balance(asset_id.to_string())
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&balance)
    }

    /// List known RGB assets. Pass a JS array of schema strings to filter, or empty for all.
    #[wasm_bindgen(js_name = "listAssets")]
    pub fn list_assets(&self, filter_asset_schemas_js: JsValue) -> Result<JsValue, JsValue> {
        let schemas: Vec<rgb_lib_wasm::AssetSchema> =
            serde_wasm_bindgen::from_value(filter_asset_schemas_js)
                .map_err(|e| JsValue::from_str(&format!("Invalid schemas: {e}")))?;
        let assets = self
            .inner
            .borrow()
            .list_assets(schemas)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&assets)
    }

    /// List RGB transfers matching an asset filter ("Any", "NoAsset" or {"Id": "<asset_id>"}),
    /// optionally restricted to the transfers of an on-chain txid.
    #[wasm_bindgen(js_name = "listTransfers")]
    pub fn list_transfers(
        &self,
        filter_js: JsValue,
        txid: Option<String>,
    ) -> Result<JsValue, JsValue> {
        let filter: rgb_lib_wasm::wallet::AssetFilter =
            serde_wasm_bindgen::from_value(filter_js)
                .map_err(|e| JsValue::from_str(&format!("Invalid filter: {e}")))?;
        let transfers = self
            .inner
            .borrow()
            .list_transfers(filter, txid)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&transfers)
    }

    /// List unspent outputs. Always skips sync on wasm32.
    #[wasm_bindgen(js_name = "listUnspents")]
    pub fn list_unspents(&self, settled_only: bool) -> Result<JsValue, JsValue> {
        let unspents = self
            .inner
            .borrow_mut()
            .list_unspents(None, settled_only, true)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&unspents)
    }

    /// List Bitcoin transactions. Always skips sync on wasm32.
    #[wasm_bindgen(js_name = "listTransactions")]
    pub fn list_transactions(&self) -> Result<JsValue, JsValue> {
        let txs = self
            .inner
            .borrow_mut()
            .list_transactions(None, true)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&txs)
    }

    /// List begun vanilla operations that are not ended or aborted.
    #[wasm_bindgen(js_name = "listPendingVanillaTxs")]
    pub fn list_pending_vanilla_txs(&self) -> Result<JsValue, JsValue> {
        let txs = self
            .inner
            .borrow()
            .list_pending_vanilla_txs()
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&txs)
    }

    /// Fail a begun vanilla operation and release its inputs.
    #[wasm_bindgen(js_name = "abortPendingVanillaTx")]
    pub fn abort_pending_vanilla_tx(&self, txid: &str) -> Result<(), JsValue> {
        self.inner
            .borrow()
            .abort_pending_vanilla_tx(txid.to_string())
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Sign a PSBT (base64-encoded). Returns the signed PSBT string.
    #[wasm_bindgen(js_name = "signPsbt")]
    pub fn sign_psbt(&self, unsigned_psbt: &str) -> Result<String, JsValue> {
        self.inner
            .borrow()
            .sign_psbt(unsigned_psbt.to_string(), None)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Finalize a signed PSBT (base64-encoded). Returns the finalized PSBT string.
    #[wasm_bindgen(js_name = "finalizePsbt")]
    pub fn finalize_psbt(&self, signed_psbt: &str) -> Result<String, JsValue> {
        self.inner
            .borrow()
            .finalize_psbt(signed_psbt.to_string(), None)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Blind an UTXO to receive RGB assets. Returns ReceiveData as a JS object.
    ///
    /// `online_js` is the Online object returned by `goOnline` (the wallet reserves the invoice on
    /// its proxies).
    /// `assignment_js` is a JS object like `{ "Fungible": 100 }` or `"NonFungible"` or `"Any"`.
    /// `expiration_timestamp_js` is a UTC timestamp in seconds, as a number, BigInt or decimal string.
    /// `transport_endpoints_js` is a JS array of endpoint strings.
    /// `encryption` is "Required" (default), "Unsupported" or "Optional".
    #[wasm_bindgen(js_name = "blindReceive")]
    #[allow(clippy::too_many_arguments)]
    pub async fn blind_receive(
        &self,
        online_js: JsValue,
        asset_id: Option<String>,
        assignment_js: JsValue,
        expiration_timestamp_js: JsValue,
        transport_endpoints_js: JsValue,
        min_confirmations: u8,
        encryption: Option<String>,
    ) -> Result<JsValue, JsValue> {
        let online = js_to_online_opt(online_js)?;
        let expiration_timestamp = js_to_u64(expiration_timestamp_js)?;
        let assignment: rgb_lib_wasm::Assignment = serde_wasm_bindgen::from_value(assignment_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid assignment: {e}")))?;
        let transport_endpoints: Vec<String> =
            serde_wasm_bindgen::from_value(transport_endpoints_js)
                .map_err(|e| JsValue::from_str(&format!("Invalid transport endpoints: {e}")))?;
        let encryption = parse_encryption(encryption)?;
        let mut wallet = self.inner.borrow_mut();
        let data = wallet
            .blind_receive(
                online,
                asset_id,
                assignment,
                expiration_timestamp,
                transport_endpoints,
                min_confirmations,
                encryption,
            )
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&data)
    }

    /// Create an address to receive RGB assets via witness TX. Returns ReceiveData as a JS object.
    ///
    /// `online_js` is the Online object returned by `goOnline`.
    /// `expiration_timestamp_js` is a UTC timestamp in seconds, as a number, BigInt or decimal string.
    /// `encryption` is "Required" (default), "Unsupported" or "Optional".
    #[wasm_bindgen(js_name = "witnessReceive")]
    #[allow(clippy::too_many_arguments)]
    pub async fn witness_receive(
        &self,
        online_js: JsValue,
        asset_id: Option<String>,
        assignment_js: JsValue,
        expiration_timestamp_js: JsValue,
        transport_endpoints_js: JsValue,
        min_confirmations: u8,
        encryption: Option<String>,
    ) -> Result<JsValue, JsValue> {
        let online = js_to_online_opt(online_js)?;
        let expiration_timestamp = js_to_u64(expiration_timestamp_js)?;
        let assignment: rgb_lib_wasm::Assignment = serde_wasm_bindgen::from_value(assignment_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid assignment: {e}")))?;
        let transport_endpoints: Vec<String> =
            serde_wasm_bindgen::from_value(transport_endpoints_js)
                .map_err(|e| JsValue::from_str(&format!("Invalid transport endpoints: {e}")))?;
        let encryption = parse_encryption(encryption)?;
        let mut wallet = self.inner.borrow_mut();
        let data = wallet
            .witness_receive(
                online,
                asset_id,
                assignment,
                expiration_timestamp,
                transport_endpoints,
                min_confirmations,
                encryption,
            )
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&data)
    }

    /// Return a new Bitcoin address from the vanilla wallet.
    #[wasm_bindgen(js_name = "getAddress")]
    pub fn get_address(&self) -> Result<String, JsValue> {
        self.inner
            .borrow_mut()
            .get_address()
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Rotate the pinned address for the given keychain.
    /// keychain: 0 = External (colored), 1 = Internal (vanilla)
    #[wasm_bindgen(js_name = "rotateAddress")]
    pub fn rotate_address(&self, keychain: u32) -> Result<String, JsValue> {
        let kc = match keychain {
            0 => rgb_lib_wasm::bdk_wallet::KeychainKind::External,
            1 => rgb_lib_wasm::bdk_wallet::KeychainKind::Internal,
            _ => {
                return Err(JsValue::from_str(
                    "Invalid keychain: use 0 (External) or 1 (Internal)",
                ));
            }
        };
        self.inner
            .borrow_mut()
            .rotate_address(kc)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Delete failed transfers. Returns true if any were deleted.
    #[wasm_bindgen(js_name = "deleteTransfers")]
    pub fn delete_transfers(
        &self,
        batch_transfer_idx: Option<i32>,
        no_asset_only: bool,
    ) -> Result<bool, JsValue> {
        self.inner
            .borrow()
            .delete_transfers(batch_transfer_idx, no_asset_only)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Go online: connect to an indexer. Takes the online options as a JS object and returns
    /// Online data as a JS object.
    #[wasm_bindgen(js_name = "goOnline")]
    pub async fn go_online(&self, online_options: JsValue) -> Result<JsValue, JsValue> {
        let online_options: OnlineOptions = serde_wasm_bindgen::from_value(online_options)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let mut wallet = self.inner.borrow_mut();
        let online = wallet
            .go_online(online_options)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&online)
    }

    /// Sync the wallet with the indexer.
    ///
    /// Incremental: re-queries only already-revealed SPKs. Use `fullScan` to recover a thin
    /// restored state.
    pub async fn sync(&self, online_js: JsValue) -> Result<(), JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .sync(online)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Run a BIP44 stop-gap full scan against the indexer.
    ///
    /// Rebuilds a thin BDK state (no revealed SPKs) after a restore, recovering the BTC balance
    /// when an incremental `sync` cannot.
    #[wasm_bindgen(js_name = "fullScan")]
    pub async fn full_scan(&self, online_js: JsValue) -> Result<(), JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .full_scan(online)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Create UTXOs (begin): prepare a PSBT to create new UTXOs for RGB allocations.
    /// Returns the unsigned PSBT string.
    #[wasm_bindgen(js_name = "createUtxosBegin")]
    pub async fn create_utxos_begin(
        &self,
        online_js: JsValue,
        up_to: bool,
        num: Option<u8>,
        size: Option<u32>,
        fee_rate: u64,
        skip_sync: bool,
    ) -> Result<String, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .create_utxos_begin(online, up_to, num, size, fee_rate, skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Create UTXOs (end): broadcast a signed PSBT to create new UTXOs.
    /// Returns the number of created UTXOs.
    #[wasm_bindgen(js_name = "createUtxosEnd")]
    pub async fn create_utxos_end(
        &self,
        online_js: JsValue,
        signed_psbt: &str,
        skip_sync: bool,
    ) -> Result<u8, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .create_utxos_end(online, signed_psbt.to_string(), skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Send RGB assets (begin): prepare a PSBT. Returns a SendBeginResult JS object
    /// `{ psbt, batchTransferIdx, details }`.
    ///
    /// `recipient_map_js` is a JS object mapping asset IDs to arrays of Recipient objects.
    /// `expiration_timestamp_js` is a UTC timestamp in seconds, as a number, BigInt or decimal string.
    /// `sender_transport_endpoint` is the sender's own proxy (e.g. `rpc://127.0.0.1:3000/json-rpc`),
    /// where the recipients post their ACK.
    /// If `dry_run` is true, the wallet does not save the transfer as Initiated.
    #[wasm_bindgen(js_name = "sendBegin")]
    #[allow(clippy::too_many_arguments)]
    pub async fn send_begin(
        &self,
        online_js: JsValue,
        recipient_map_js: JsValue,
        donation: bool,
        fee_rate: u64,
        min_confirmations: u8,
        expiration_timestamp_js: JsValue,
        sender_transport_endpoint: Option<String>,
        lock_time: Option<u32>,
        dry_run: bool,
    ) -> Result<JsValue, JsValue> {
        let expiration_timestamp = js_to_u64(expiration_timestamp_js)?;
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let recipient_map: HashMap<String, Vec<Recipient>> =
            serde_wasm_bindgen::from_value(recipient_map_js)
                .map_err(|e| JsValue::from_str(&format!("Invalid recipient map: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .send_begin(
                online,
                recipient_map,
                donation,
                fee_rate,
                min_confirmations,
                expiration_timestamp,
                sender_transport_endpoint,
                lock_time,
                dry_run,
            )
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Send RGB assets (end): broadcast a signed PSBT. Returns an OperationResult JS object.
    #[wasm_bindgen(js_name = "sendEnd")]
    pub async fn send_end(
        &self,
        online_js: JsValue,
        signed_psbt: &str,
        skip_sync: bool,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .send_end(online, signed_psbt.to_string(), skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Send BTC (begin): prepare a PSBT. Returns the unsigned PSBT string.
    #[wasm_bindgen(js_name = "sendBtcBegin")]
    pub async fn send_btc_begin(
        &self,
        online_js: JsValue,
        address: &str,
        amount: u64,
        fee_rate: u64,
        skip_sync: bool,
    ) -> Result<String, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .send_btc_begin(online, address.to_string(), amount, fee_rate, skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Send BTC (end): broadcast a signed PSBT. Returns the txid string.
    #[wasm_bindgen(js_name = "sendBtcEnd")]
    pub async fn send_btc_end(
        &self,
        online_js: JsValue,
        signed_psbt: &str,
        skip_sync: bool,
    ) -> Result<String, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .send_btc_end(online, signed_psbt.to_string(), skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Refresh pending transfers. Returns a RefreshResult JS object.
    ///
    /// `filter_js` is a JS array of RefreshFilter objects (or empty array for all).
    pub async fn refresh(
        &self,
        online_js: JsValue,
        asset_id: Option<String>,
        filter_js: JsValue,
        skip_sync: bool,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let filter: Vec<RefreshFilter> = serde_wasm_bindgen::from_value(filter_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid filter: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .refresh(online, asset_id, filter, skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Fail pending transfers. Returns true if any transfers were failed.
    #[wasm_bindgen(js_name = "failTransfers")]
    pub async fn fail_transfers(
        &self,
        online_js: JsValue,
        batch_transfer_idx: Option<i32>,
        no_asset_only: bool,
        skip_sync: bool,
    ) -> Result<bool, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .fail_transfers(online, batch_transfer_idx, no_asset_only, skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Get fee estimation for a target number of blocks.
    #[wasm_bindgen(js_name = "getFeeEstimation")]
    pub async fn get_fee_estimation(
        &self,
        online_js: JsValue,
        blocks: u16,
    ) -> Result<f64, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let wallet = self.inner.borrow();
        wallet
            .get_fee_estimation(online, blocks)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Drain all wallet funds (begin): prepare a PSBT. Returns the unsigned PSBT string.
    #[wasm_bindgen(js_name = "drainToBegin")]
    pub async fn drain_to_begin(
        &self,
        online_js: JsValue,
        address: &str,
        destroy_assets: bool,
        fee_rate: u64,
    ) -> Result<String, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .drain_to_begin(online, address.to_string(), destroy_assets, fee_rate)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Drain all wallet funds (end): broadcast a signed PSBT. Returns the txid string.
    #[wasm_bindgen(js_name = "drainToEnd")]
    pub async fn drain_to_end(
        &self,
        online_js: JsValue,
        signed_psbt: &str,
    ) -> Result<String, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .drain_to_end(online, signed_psbt.to_string())
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Inflate an IFA asset (begin): prepare a PSBT. Returns the unsigned PSBT string.
    ///
    /// `inflation_amounts_js` is a JS array of u64 values.
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = "inflateBegin")]
    pub async fn inflate_begin(
        &self,
        online_js: JsValue,
        asset_id: &str,
        inflation_amounts_js: JsValue,
        fee_rate: u64,
        min_confirmations: u8,
    ) -> Result<String, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let inflation_amounts: Vec<u64> = serde_wasm_bindgen::from_value(inflation_amounts_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid inflation_amounts array: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .inflate_begin(
                online,
                asset_id.to_string(),
                inflation_amounts,
                fee_rate,
                min_confirmations,
            )
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Inflate an IFA asset (end): broadcast a signed PSBT. Returns an OperationResult JS object.
    #[wasm_bindgen(js_name = "inflateEnd")]
    pub async fn inflate_end(
        &self,
        online_js: JsValue,
        signed_psbt: &str,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .inflate_end(online, signed_psbt.to_string())
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Burn RGB assets: begin, sign with the wallet keys and end. Returns an OperationResult JS
    /// object.
    ///
    /// `burn_reason` is an optional hex-encoded 32-byte value.
    #[wasm_bindgen(js_name = "burn")]
    pub async fn burn(
        &self,
        online_js: JsValue,
        asset_id: &str,
        amount: u64,
        burn_reason: Option<String>,
        fee_rate: u64,
        min_confirmations: u8,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .burn(
                online,
                asset_id.to_string(),
                amount,
                burn_reason,
                fee_rate,
                min_confirmations,
            )
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Burn RGB assets (begin): prepare a PSBT. Returns a BurnBeginResult JS object.
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = "burnBegin")]
    pub async fn burn_begin(
        &self,
        online_js: JsValue,
        asset_id: &str,
        amount: u64,
        burn_reason: Option<String>,
        fee_rate: u64,
        min_confirmations: u8,
        dry_run: bool,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .burn_begin(
                online,
                asset_id.to_string(),
                amount,
                burn_reason,
                fee_rate,
                min_confirmations,
                dry_run,
            )
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Burn RGB assets (end): broadcast a signed PSBT. Returns an OperationResult JS object.
    #[wasm_bindgen(js_name = "burnEnd")]
    pub async fn burn_end(
        &self,
        online_js: JsValue,
        signed_psbt: &str,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .burn_end(online, signed_psbt.to_string())
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Link an IFA or BFA asset to another contract: begin, sign with the wallet keys and end.
    /// Returns an OperationResult JS object.
    #[wasm_bindgen(js_name = "link")]
    pub async fn link(
        &self,
        online_js: JsValue,
        asset_id: &str,
        linked_to_contract: &str,
        fee_rate: u64,
        min_confirmations: u8,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .link(
                online,
                asset_id.to_string(),
                linked_to_contract.to_string(),
                fee_rate,
                min_confirmations,
            )
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Link an IFA or BFA asset to another contract (begin): prepare a PSBT. Returns a
    /// LinkBeginResult JS object.
    #[wasm_bindgen(js_name = "linkBegin")]
    pub async fn link_begin(
        &self,
        online_js: JsValue,
        asset_id: &str,
        linked_to_contract: &str,
        fee_rate: u64,
        min_confirmations: u8,
        dry_run: bool,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .link_begin(
                online,
                asset_id.to_string(),
                linked_to_contract.to_string(),
                fee_rate,
                min_confirmations,
                dry_run,
            )
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Link an IFA or BFA asset to another contract (end): broadcast a signed PSBT. Returns an
    /// OperationResult JS object.
    #[wasm_bindgen(js_name = "linkEnd")]
    pub async fn link_end(
        &self,
        online_js: JsValue,
        signed_psbt: &str,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .link_end(online, signed_psbt.to_string())
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Begin a BFA mint. Returns a MintBeginResult object.
    #[wasm_bindgen(js_name = "mintBegin")]
    pub async fn mint_begin(
        &self,
        online_js: JsValue,
        asset_id: &str,
        recipient_js: JsValue,
        before_block: u64,
        fee_rate: u64,
        min_confirmations: u8,
        sender_transport_endpoint: &str,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let recipient: Recipient = serde_wasm_bindgen::from_value(recipient_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Recipient object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .mint_begin(
                online,
                asset_id.to_string(),
                recipient,
                before_block,
                fee_rate,
                min_confirmations,
                sender_transport_endpoint.to_string(),
            )
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Complete a BFA mint. Returns an OperationResult object.
    #[wasm_bindgen(js_name = "mintEnd")]
    pub async fn mint_end(
        &self,
        online_js: JsValue,
        signed_psbt: &str,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .mint_end(online, signed_psbt.to_string())
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Return the checked mint received on a recipient ID as a ReceivedMint object, or null.
    #[wasm_bindgen(js_name = "getReceivedMint")]
    pub fn get_received_mint(&self, recipient_id: &str) -> Result<JsValue, JsValue> {
        let received_mint = self
            .inner
            .borrow()
            .get_received_mint(recipient_id.to_string())
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        received_mint.map_or(Ok(JsValue::NULL), |m| to_js(&m))
    }

    /// List vanilla (non-colored) unspent outputs. Returns a JS array of LocalOutput objects.
    #[wasm_bindgen(js_name = "listUnspentsVanilla")]
    pub async fn list_unspents_vanilla(
        &self,
        online_js: JsValue,
        min_confirmations: u8,
        skip_sync: bool,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let unspents = wallet
            .list_unspents_vanilla(online, min_confirmations, skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&unspents)
    }

    /// Create an encrypted backup of the wallet state. Returns backup bytes as Uint8Array.
    pub fn backup(&self, password: &str) -> Result<Vec<u8>, JsValue> {
        self.inner
            .borrow()
            .backup(password)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Restore wallet state from an encrypted backup.
    #[wasm_bindgen(js_name = "restoreBackup")]
    pub fn restore_backup(&self, backup_bytes: &[u8], password: &str) -> Result<(), JsValue> {
        self.inner
            .borrow_mut()
            .restore_backup(backup_bytes, password)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Check if the wallet needs a backup. Returns true if modified since last backup.
    #[wasm_bindgen(js_name = "backupInfo")]
    pub fn backup_info(&self) -> Result<bool, JsValue> {
        self.inner
            .borrow()
            .backup_info()
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Configure VSS (cloud) backup for this wallet.
    ///
    /// `signing_key_hex` is the 32-byte secret key as a hex string (64 hex chars).
    ///
    /// **Security note:** The signing key crosses the JS/WASM boundary as a string. It will
    /// exist in V8's string pool and cannot be zeroed from Rust. Callers should avoid storing
    /// the key in JS longer than necessary (e.g., don't keep it in a global variable).
    #[wasm_bindgen(js_name = "configureVssBackup")]
    pub fn configure_vss_backup(
        &self,
        server_url: &str,
        store_id: &str,
        signing_key_hex: &str,
    ) -> Result<(), JsValue> {
        if signing_key_hex.len() != 64 {
            return Err(JsValue::from_str(&format!(
                "signing_key_hex must be exactly 64 hex chars (32 bytes), got {}",
                signing_key_hex.len()
            )));
        }
        let key_bytes = hex::decode(signing_key_hex)
            .map_err(|e| JsValue::from_str(&format!("Invalid signing key hex: {e}")))?;
        let signing_key =
            rgb_lib_wasm::bdk_wallet::bitcoin::secp256k1::SecretKey::from_slice(&key_bytes)
                .map_err(|e| JsValue::from_str(&format!("Invalid signing key: {e}")))?;
        let config = rgb_lib_wasm::wallet::vss::VssBackupConfig::new(
            server_url.to_string(),
            store_id.to_string(),
            signing_key,
        );
        self.inner.borrow_mut().configure_vss_backup(&config);
        Ok(())
    }

    /// Disable VSS (cloud) backup.
    #[wasm_bindgen(js_name = "disableVssBackup")]
    pub fn disable_vss_backup(&self) {
        self.inner.borrow_mut().disable_vss_backup();
    }

    /// Upload an encrypted backup to the configured VSS server. Returns the server version.
    #[wasm_bindgen(js_name = "vssBackup")]
    pub async fn vss_backup(&self) -> Result<JsValue, JsValue> {
        let wallet = self.inner.borrow();
        let version = wallet
            .vss_backup()
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(JsValue::from_f64(version as f64))
    }

    /// Upload a backup when the wallet has changes not yet backed up.
    /// Returns the server version, or null when no upload is needed.
    #[wasm_bindgen(js_name = "ensureInitialBackup")]
    pub async fn ensure_initial_backup(&self) -> Result<JsValue, JsValue> {
        let wallet = self.inner.borrow();
        let version = wallet
            .ensure_initial_backup()
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(version.map_or(JsValue::NULL, |v| JsValue::from_f64(v as f64)))
    }

    /// Download and restore wallet state from VSS server.
    #[wasm_bindgen(js_name = "vssRestoreBackup")]
    pub async fn vss_restore_backup(&self) -> Result<(), JsValue> {
        self.inner
            .borrow_mut()
            .vss_restore_backup()
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Query VSS backup status. Returns { backup_exists, server_version, backup_required }.
    #[wasm_bindgen(js_name = "vssBackupInfo")]
    pub async fn vss_backup_info(&self) -> Result<JsValue, JsValue> {
        let wallet = self.inner.borrow();
        let info = wallet
            .vss_backup_info()
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&info)
    }
}

#[wasm_bindgen]
impl WasmWallet {
    /// Accept a swap offer as the taker and return the request to forward to the maker. Inputs in
    /// `exclude_outpoints` (optional) are never selected.
    #[wasm_bindgen(js_name = "acceptSwapOffer")]
    pub async fn accept_swap_offer(
        &self,
        online_js: JsValue,
        offer_js: JsValue,
        min_confirmations: u8,
        skip_sync: bool,
        exclude_outpoints_js: JsValue,
    ) -> Result<JsValue, JsValue> {
        let online = online_from_js(online_js)?;
        let offer: OnchainSwapOffer = serde_wasm_bindgen::from_value(offer_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid swap offer: {e}")))?;
        let exclude_outpoints: Vec<Outpoint> = if exclude_outpoints_js.is_undefined() {
            vec![]
        } else {
            serde_wasm_bindgen::from_value(exclude_outpoints_js)
                .map_err(|e| JsValue::from_str(&format!("Invalid outpoints: {e}")))?
        };
        let mut wallet = self.inner.borrow_mut();
        let request = wallet
            .accept_swap_offer(
                online,
                offer,
                min_confirmations,
                skip_sync,
                exclude_outpoints,
            )
            .await
            .map_err(js_err)?;
        to_js(&request)
    }

    /// Confirmed BTC eligible for a swap, including empty unreserved RGB change.
    #[wasm_bindgen(js_name = "getSwapFundingBalance")]
    pub async fn get_swap_funding_balance(&self, online_js: JsValue) -> Result<JsValue, JsValue> {
        let online = online_from_js(online_js)?;
        let amount = self
            .inner
            .borrow()
            .get_swap_funding_balance(online)
            .await
            .map_err(js_err)?;
        to_js(&amount)
    }

    /// Complete a maker proposal as the taker and return the completion to forward to the maker.
    /// When the taker sends RGB, the completion carries its consignment (base64): no proxy is
    /// involved.
    #[wasm_bindgen(js_name = "completeSwapProposal")]
    pub async fn complete_swap_proposal(
        &self,
        online_js: JsValue,
        proposal_js: JsValue,
        min_confirmations: u8,
        skip_sync: bool,
    ) -> Result<JsValue, JsValue> {
        let online = online_from_js(online_js)?;
        let proposal: OnchainSwapProposal = serde_wasm_bindgen::from_value(proposal_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid swap proposal: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let completion = wallet
            .complete_swap_proposal(online, proposal, min_confirmations, skip_sync)
            .await
            .map_err(js_err)?;
        to_js(&completion)
    }

    /// Settle a swap once the maker has broadcast it, with the maker's completion or the one this
    /// wallet signed. `role` must be "Taker". Returns the assignments received, empty when the
    /// taker receives BTC.
    #[wasm_bindgen(js_name = "acceptSwapTransfers")]
    pub async fn accept_swap_transfers(
        &self,
        online_js: JsValue,
        completion_js: JsValue,
        role: &str,
        skip_sync: bool,
    ) -> Result<JsValue, JsValue> {
        let online = online_from_js(online_js)?;
        let completion: OnchainSwapCompletion = serde_wasm_bindgen::from_value(completion_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid swap completion: {e}")))?;
        let role = match role {
            "Maker" => OnchainSwapRole::Maker,
            "Taker" => OnchainSwapRole::Taker,
            other => return Err(JsValue::from_str(&format!("Invalid swap role: {other}"))),
        };
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .accept_swap_transfers(online, completion, role, skip_sync)
            .await
            .map_err(js_err)?;
        to_js(&result)
    }

    /// Cancel a swap whose transaction has not been broadcast.
    #[wasm_bindgen(js_name = "cancelSwap")]
    pub async fn cancel_swap(&self, online_js: JsValue, swap_id: String) -> Result<(), JsValue> {
        let online = online_from_js(online_js)?;
        let mut wallet = self.inner.borrow_mut();
        wallet.cancel_swap(online, swap_id).await.map_err(js_err)
    }

    /// Cancel every swap whose offer has expired and whose transaction has not been broadcast.
    /// Returns the cancelled swap IDs.
    #[wasm_bindgen(js_name = "cancelExpiredSwaps")]
    pub async fn cancel_expired_swaps(&self, online_js: JsValue) -> Result<JsValue, JsValue> {
        let online = online_from_js(online_js)?;
        let mut wallet = self.inner.borrow_mut();
        let swap_ids = wallet.cancel_expired_swaps(online).await.map_err(js_err)?;
        to_js(&swap_ids)
    }

    /// List the swaps this wallet has state for, including cancelled ones.
    #[wasm_bindgen(js_name = "listSwaps")]
    pub fn list_swaps(&self) -> Result<JsValue, JsValue> {
        let swaps = self.inner.borrow().list_swaps().map_err(js_err)?;
        to_js(&swaps)
    }
}

#[wasm_bindgen]
impl WasmWallet {
    /// Accept an HTLC lock consignment (base64) received out of band, whose witness output
    /// `txid:vout` must pay `witnessRecipientId` with at least `minConfirmations`, if it matches
    /// `expected` (`{ assetId, assetSchema: "Nia", assignment: { Fungible: n } }`). `blinding` is a
    /// decimal string, a BigInt or a safe Number. Returns `{ consignment: <hex>, assignments }`.
    /// Rejects with "Insufficient confirmations: ..." while the witness is too shallow.
    #[wasm_bindgen(js_name = "acceptHtlcConsignment")]
    #[allow(clippy::too_many_arguments)]
    pub async fn accept_htlc_consignment(
        &self,
        online_js: JsValue,
        consignment_base64: String,
        txid: String,
        vout: u32,
        witness_recipient_id: String,
        blinding_js: JsValue,
        min_confirmations: u8,
        expected_js: JsValue,
    ) -> Result<JsValue, JsValue> {
        let online = online_from_js(online_js)?;
        let blinding = u64_from_js(blinding_js, "blinding")?;
        let expected: ExpectedTransfer = serde_wasm_bindgen::from_value(expected_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid expected transfer: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let (consignment, assignments) = wallet
            .accept_htlc_consignment(
                online,
                &consignment_base64,
                txid,
                vout,
                witness_recipient_id,
                blinding,
                min_confirmations,
                expected,
            )
            .await
            .map_err(js_err)?;
        to_js(&JsAcceptedTransfer {
            consignment: consignment_to_hex(&consignment)?,
            assignments,
        })
    }

    /// The base64 consignment of an out-of-band send (a donation `send` whose recipients name no
    /// transport endpoint) of `assetId` committed by `txid`, to hand to the counterparty.
    #[wasm_bindgen(js_name = "getOutOfBandConsignment")]
    pub fn get_out_of_band_consignment(
        &self,
        txid: String,
        asset_id: String,
    ) -> Result<String, JsValue> {
        self.inner
            .borrow()
            .get_out_of_band_consignment(&txid, &asset_id)
            .map_err(js_err)
    }

    /// Accept a consignment (hex) whose witness output `txid:vout` must pay `witnessRecipientId`
    /// with at least `minConfirmations`, if it matches `expected`
    /// (`{ assetId, assetSchema: "Nia", assignment: { Fungible: n } }`). `blinding` is a decimal
    /// string, a BigInt or a safe Number. Returns `{ consignment: <hex>, assignments }`. Rejects
    /// with "Insufficient confirmations: ..." while the witness is too shallow.
    #[wasm_bindgen(js_name = "acceptTransferPinned")]
    #[allow(clippy::too_many_arguments)]
    pub async fn accept_transfer_pinned(
        &self,
        online_js: JsValue,
        consignment_hex: String,
        txid: String,
        vout: u32,
        witness_recipient_id: String,
        blinding_js: JsValue,
        min_confirmations: u8,
        expected_js: JsValue,
    ) -> Result<JsValue, JsValue> {
        let online = online_from_js(online_js)?;
        let fetched = FetchedConsignment {
            consignment: consignment_from_hex(&consignment_hex)?,
            txid,
            vout,
        };
        let blinding = u64_from_js(blinding_js, "blinding")?;
        let expected: ExpectedTransfer = serde_wasm_bindgen::from_value(expected_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid expected transfer: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let accepted = wallet
            .accept_transfer_pinned(
                online,
                fetched,
                witness_recipient_id,
                blinding,
                min_confirmations,
                expected,
            )
            .await
            .map_err(js_err)?;
        to_js(&JsAcceptedTransfer {
            consignment: consignment_to_hex(&accepted.consignment)?,
            assignments: accepted.assignments,
        })
    }

    /// Register the asset of an accepted consignment (hex, as returned by
    /// `acceptTransferPinned`) whose witness transaction is `txid`.
    #[wasm_bindgen(js_name = "saveNewAsset")]
    pub async fn save_new_asset(
        &self,
        online_js: JsValue,
        consignment_hex: String,
        txid: String,
    ) -> Result<(), JsValue> {
        let online = online_from_js(online_js)?;
        let consignment = consignment_from_hex(&consignment_hex)?;
        let wallet = self.inner.borrow();
        wallet
            .save_new_asset(online, consignment, txid)
            .await
            .map_err(js_err)
    }

    /// Color an externally built PSBT (base64) spending `inputOutpoints` (`[{ txid, vout }]`,
    /// possibly not this wallet's, e.g. an HTLC) and store the operation. `outputMap` keys in
    /// `coloringInfo` are final vouts: the PSBT must already carry its OP_RETURN (first next to
    /// a P2TR output). `expiry` is `"default"` (also when omitted), `"never"` or `{ at: <unix> }`.
    /// Returns `{ operationId, coloredPsbt, allocations }`.
    #[wasm_bindgen(js_name = "psbtOpPrepare")]
    pub async fn psbt_op_prepare(
        &self,
        psbt: String,
        coloring_info_js: JsValue,
        input_outpoints_js: JsValue,
        min_confirmations: u8,
        expiry_js: JsValue,
    ) -> Result<JsValue, JsValue> {
        let mut psbt = Psbt::from_str(&psbt).map_err(js_err)?;
        let coloring_info: JsColoringInfo = serde_wasm_bindgen::from_value(coloring_info_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid coloring info: {e}")))?;
        let input_outpoints: Vec<Outpoint> = serde_wasm_bindgen::from_value(input_outpoints_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid outpoints: {e}")))?;
        let input_outpoints = input_outpoints
            .into_iter()
            .map(|o| OutPoint::from_str(&o.to_string()).map_err(js_err))
            .collect::<Result<Vec<_>, _>>()?;
        let expiry: PsbtOpExpiry = if expiry_js.is_undefined() || expiry_js.is_null() {
            PsbtOpExpiry::Default
        } else {
            serde_wasm_bindgen::from_value(expiry_js)
                .map_err(|e| JsValue::from_str(&format!("Invalid expiry: {e}")))?
        };
        let wallet = self.inner.borrow();
        let result = wallet
            .psbt_op_prepare_with_expiry(
                &mut psbt,
                coloring_info.try_into()?,
                input_outpoints,
                min_confirmations,
                expiry,
            )
            .await
            .map_err(js_err)?;
        to_js(&result)
    }

    /// Record that the operation's transaction is about to be broadcast. Await it before
    /// broadcasting.
    #[wasm_bindgen(js_name = "psbtOpMarkBroadcast")]
    pub async fn psbt_op_mark_broadcast(&self, operation_id: String) -> Result<(), JsValue> {
        let wallet = self.inner.borrow();
        wallet
            .psbt_op_mark_broadcast(&operation_id)
            .await
            .map_err(js_err)
    }

    /// Consume a prepared operation into the RGB stash once the indexer sees its transaction.
    #[wasm_bindgen(js_name = "psbtOpApply")]
    pub async fn psbt_op_apply(
        &self,
        online_js: JsValue,
        operation_id: String,
    ) -> Result<(), JsValue> {
        let online = online_from_js(online_js)?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .psbt_op_apply(online, &operation_id)
            .await
            .map_err(js_err)
    }

    /// Abort a prepared operation whose transaction never reached the network.
    #[wasm_bindgen(js_name = "psbtOpAbort")]
    pub async fn psbt_op_abort(
        &self,
        online_js: JsValue,
        operation_id: String,
    ) -> Result<(), JsValue> {
        let online = online_from_js(online_js)?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .psbt_op_abort(online, &operation_id)
            .await
            .map_err(js_err)
    }

    /// Find a stored operation by its transaction ID. Returns
    /// `{ operationId, coloredPsbt, status, broadcast, txid, allocations }`.
    #[wasm_bindgen(js_name = "psbtOpByTxid")]
    pub fn psbt_op_by_txid(&self, txid: String) -> Result<JsValue, JsValue> {
        let op = self.inner.borrow().psbt_op_by_txid(&txid).map_err(js_err)?;
        to_js(&op)
    }

    /// List the stored operations spending `outpoint` (`{ txid, vout }`), oldest first.
    #[wasm_bindgen(js_name = "psbtOpsSpending")]
    pub fn psbt_ops_spending(&self, outpoint_js: JsValue) -> Result<JsValue, JsValue> {
        let outpoint: Outpoint = serde_wasm_bindgen::from_value(outpoint_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid outpoint: {e}")))?;
        let ops = self
            .inner
            .borrow()
            .psbt_ops_spending(outpoint)
            .map_err(js_err)?;
        to_js(&ops)
    }

    /// Hand an applied operation's consignment for `assetId` to the pending receive it pays. Not
    /// supported yet with RGB proxy protocol 1.0: always rejects.
    #[wasm_bindgen(js_name = "psbtOpProvideReceiveConsignment")]
    pub async fn psbt_op_provide_receive_consignment(
        &self,
        online_js: JsValue,
        operation_id: String,
        asset_id: String,
    ) -> Result<JsValue, JsValue> {
        let online = online_from_js(online_js)?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .psbt_op_provide_receive_consignment(online, &operation_id, &asset_id)
            .await
            .map_err(js_err)?;
        to_js(&result)
    }

    /// Reconcile an operation with its batch transfer and return its status
    /// (`"prepared"`, `"applied"`, `"failed"` or `"settled"`).
    #[wasm_bindgen(js_name = "psbtOpReconcile")]
    pub fn psbt_op_reconcile(&self, operation_id: String) -> Result<JsValue, JsValue> {
        let status = self
            .inner
            .borrow()
            .psbt_op_reconcile(&operation_id)
            .map_err(js_err)?;
        to_js(&status)
    }
}

/// An RGB invoice parsed from a string. Exposes structured invoice data to JavaScript.
#[wasm_bindgen]
pub struct WasmInvoice {
    inner: rgb_lib_wasm::wallet::Invoice,
}

#[wasm_bindgen]
impl WasmInvoice {
    /// Parse an RGB invoice string. Throws if the string is not a valid RGB invoice.
    #[wasm_bindgen(constructor)]
    pub fn new(invoice_string: &str) -> Result<WasmInvoice, JsValue> {
        let invoice = rgb_lib_wasm::wallet::Invoice::new(invoice_string.to_string())
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(WasmInvoice { inner: invoice })
    }

    /// Return the parsed invoice data as a JS object.
    #[wasm_bindgen(js_name = "invoiceData")]
    pub fn invoice_data(&self) -> Result<JsValue, JsValue> {
        // to_js keeps JS maps because refresh has integer keys. These keys are strings.
        let serializer = serde_wasm_bindgen::Serializer::new()
            .serialize_large_number_types_as_bigints(true)
            .serialize_maps_as_objects(true);
        self.inner
            .invoice_data()
            .serialize(&serializer)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Return the original invoice string.
    #[wasm_bindgen(js_name = "invoiceString")]
    pub fn invoice_string(&self) -> String {
        self.inner.invoice_string()
    }
}

/// The output script (hex) a witness `recipientId` pays to, or `null` for a blinded one.
#[wasm_bindgen(js_name = "scriptBufFromRecipientId")]
pub fn script_buf_from_recipient_id(recipient_id: &str) -> Result<Option<String>, JsValue> {
    rgb_lib_wasm::utils::script_buf_from_recipient_id(recipient_id.to_string())
        .map(|script| script.map(|s| s.to_hex_string()))
        .map_err(js_err)
}

/// Check whether the provided URL points to a valid RGB proxy server.
#[wasm_bindgen(js_name = "checkProxyUrl")]
pub async fn check_proxy_url(proxy_url: &str) -> Result<(), JsValue> {
    rgb_lib_wasm::wallet::rust_only::check_proxy_url(proxy_url)
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Validate an RGB consignment using witness data bundled in the consignment (offchain).
///
/// Works before the witness transaction is broadcast. Takes the raw consignment
/// bytes (strict-encoded, not base64), the witness transaction ID, and the Bitcoin network.
///
/// Returns a JS object: `{ valid: boolean, warnings?: string[], error?: string, details?: string }`
#[wasm_bindgen(js_name = "validateConsignmentOffchain")]
pub fn validate_consignment_offchain(
    consignment_bytes: &[u8],
    txid: &str,
    network: &str,
) -> Result<JsValue, JsValue> {
    let bitcoin_network = network
        .parse::<rgb_lib_wasm::BitcoinNetwork>()
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let result = rgb_lib_wasm::wallet::rust_only::validate_consignment_offchain(
        consignment_bytes,
        txid,
        bitcoin_network,
    )
    .map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&result)
}
