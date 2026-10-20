//! Take an on-chain swap from the browser wallet, against a native rgb-lib maker.
//!
//! The maker is the `swap-sim` binary of rgb-p2p-swaps (`cargo run --features sim --bin swap-sim`),
//! which exposes rgb-lib's maker steps over HTTP. Run with the `camel_case` feature, the wire format
//! the simulator speaks:
//!
//! ```text
//! RGB_TEST_SWAP_SIM_URL=http://127.0.0.1:4300 \
//!   wasm-pack test --headless --firefox --release --features camel_case -- --test swap_wasm
//! ```

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::json;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

mod utils;

use rgb_lib_wasm::wallet::DatabaseType;
use rgb_lib_wasm::wallet::{
    OnchainSwapCompletion, OnchainSwapOffer, OnchainSwapProposal, OnchainSwapRole,
    OnchainSwapStage, Recipient, Wallet, WalletData, WitnessData,
};
use rgb_lib_wasm::{AssetSchema, Assignment, BitcoinNetwork, TransferStatus, generate_keys};
use utils::*;

const SIM_URL: &str = match option_env!("RGB_TEST_SWAP_SIM_URL") {
    Some(url) => url,
    None => "http://127.0.0.1:4300",
};

const ASSET_AMOUNT: u64 = 1_000;
const PRICE_SAT: u64 = 100_000;
const NETWORK_FEE_SAT: u64 = 2_000;

fn test_wallet_data() -> WalletData {
    let keys = generate_keys(BitcoinNetwork::Regtest);
    WalletData {
        data_dir: "/tmp/rgb_swap".to_string(),
        bitcoin_network: BitcoinNetwork::Regtest,
        database_type: DatabaseType::Sqlite,
        max_allocations_per_utxo: 5,
        account_xpub_vanilla: keys.account_xpub_vanilla,
        account_xpub_colored: keys.account_xpub_colored,
        mnemonic: Some(keys.mnemonic),
        master_fingerprint: keys.master_fingerprint,
        vanilla_keychain: None,
        supported_schemas: vec![AssetSchema::Nia],
        reuse_addresses: false,
    }
}

async fn sim_post<B: Serialize, T: for<'de> Deserialize<'de>>(path: &str, body: &B) -> T {
    let res = reqwest::Client::new()
        .post(format!("{SIM_URL}{path}"))
        .json(body)
        .send()
        .await
        .unwrap_or_else(|e| panic!("swap-sim {path}: {e}"));
    let status = res.status();
    let text = res.text().await.unwrap();
    assert!(status.is_success(), "swap-sim {path} failed: {text}");
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("swap-sim {path}: bad body ({e}): {text}"))
}

async fn sim_get<T: for<'de> Deserialize<'de>>(path: &str) -> T {
    reqwest::Client::new()
        .get(format!("{SIM_URL}{path}"))
        .send()
        .await
        .unwrap_or_else(|e| panic!("swap-sim {path}: {e}"))
        .json()
        .await
        .unwrap()
}

/// A funded maker that issued an asset, and a funded taker wallet that is online.
async fn setup() -> (Wallet, rgb_lib_wasm::wallet::Online, String) {
    let maker_address: String = sim_get("/address").await;
    fund_address(&maker_address, "1").await;
    wait_for_esplora_sync().await;
    let asset_id: String = sim_post(
        "/issue",
        &json!({ "ticker": "SWAP", "supply": ASSET_AMOUNT }),
    )
    .await;

    let mut wallet = Wallet::new(test_wallet_data()).unwrap();
    let online = wallet
        .go_online(false, ESPLORA_URL.to_string())
        .await
        .unwrap();
    let address = wallet.get_address().unwrap();
    fund_address(&address, "1").await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();
    (wallet, online, asset_id)
}

async fn make_offer(asset_id: &str, expiry_in: u64) -> OnchainSwapOffer {
    let expiry = (js_sys::Date::now() / 1000.0) as u64 + expiry_in;
    sim_post(
        "/offer",
        &json!({
            "assetId": asset_id,
            "amount": ASSET_AMOUNT,
            "priceSat": PRICE_SAT,
            "networkFeeSat": NETWORK_FEE_SAT,
            "expiry": expiry,
        }),
    )
    .await
}

/// A funded taker wallet that holds `ASSET_AMOUNT` of a freshly issued asset, on colored UTXOs of
/// `utxo_size` sats (rgb-lib's default when `None`).
async fn setup_holder(utxo_size: Option<u32>) -> (Wallet, rgb_lib_wasm::wallet::Online, String) {
    let mut wallet = Wallet::new(test_wallet_data()).unwrap();
    let online = wallet
        .go_online(false, ESPLORA_URL.to_string())
        .await
        .unwrap();
    let address = wallet.get_address().unwrap();
    fund_address(&address, "1").await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();
    let unsigned = wallet
        .create_utxos_begin(online.clone(), true, Some(3), utxo_size, 1, true)
        .await
        .unwrap();
    let signed = wallet.sign_psbt(unsigned, None).unwrap();
    wallet
        .create_utxos_end(online.clone(), signed, true)
        .await
        .unwrap();
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();
    let asset = wallet
        .issue_asset_nia(
            "SELL".to_string(),
            "Sell".to_string(),
            0,
            vec![ASSET_AMOUNT],
        )
        .unwrap();
    (wallet, online, asset.asset_id)
}

async fn make_buy_offer(asset_id: &str, amount: u64, expiry_in: u64) -> OnchainSwapOffer {
    let expiry = (js_sys::Date::now() / 1000.0) as u64 + expiry_in;
    sim_post(
        "/offer",
        &json!({
            "assetId": asset_id,
            "amount": amount,
            "priceSat": PRICE_SAT,
            "networkFeeSat": NETWORK_FEE_SAT,
            "expiry": expiry,
            "side": "buy",
        }),
    )
    .await
}

#[wasm_bindgen_test]
async fn test_taker_sells_rgb_to_a_native_maker() {
    let maker_address: String = sim_get("/address").await;
    fund_address(&maker_address, "1").await;
    wait_for_esplora_sync().await;
    let (mut wallet, online, asset_id) = setup_holder(Some(10_000)).await;
    let btc_before = wallet
        .get_btc_balance(Some(online.clone()), true)
        .unwrap()
        .vanilla
        .spendable;

    // the maker buys 400 of the 1000 units: the rest comes back to the seller as change
    let sold = 400u64;
    let offer = make_buy_offer(&asset_id, sold, 3600).await;
    let request = wallet
        .accept_swap_offer(online.clone(), offer, 0, false, vec![])
        .await
        .unwrap();
    let proposal: OnchainSwapProposal = sim_post("/request", &json!({ "request": request })).await;
    let completion = wallet
        .complete_swap_proposal(online.clone(), proposal, 0, false)
        .await
        .unwrap();
    // the RGB leg is colored and its consignment is on the proxy. The batch is only prepared:
    // the settled balance is untouched and the pending view already shows the change
    assert_eq!(completion.consignments.len(), 1);
    assert!(completion.finalized_psbt.is_none());
    let pending = wallet.get_asset_balance(asset_id.clone()).unwrap();
    assert_eq!(
        (pending.settled, pending.future),
        (ASSET_AMOUNT, ASSET_AMOUNT - sold)
    );

    // the maker validates the consignment, signs last and broadcasts
    let done: OnchainSwapCompletion =
        sim_post("/completion", &json!({ "completion": completion })).await;
    assert!(done.finalized_psbt.is_some());
    wait_for_esplora_sync().await;

    let received = wallet
        .accept_swap_transfers(online.clone(), done.clone(), OnchainSwapRole::Taker, false)
        .await
        .unwrap();
    assert!(received.assignments.is_empty());
    let settled: usize = sim_post("/settle", &json!({ "completion": done })).await;
    assert!(settled > 0);

    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();
    // what is left is exactly the change; the BTC price landed in the wallet
    assert_eq!(
        wallet.get_asset_balance(asset_id.clone()).unwrap().future,
        ASSET_AMOUNT - sold
    );
    let btc_after = wallet
        .get_btc_balance(Some(online.clone()), true)
        .unwrap()
        .vanilla
        .spendable;
    assert!(
        btc_after >= btc_before + PRICE_SAT - 2 * NETWORK_FEE_SAT,
        "{btc_before} -> {btc_after}"
    );
    let maker: serde_json::Value = sim_get(&format!("/balance?assetId={asset_id}")).await;
    assert_eq!(maker["assetFuture"], sold);
    assert_eq!(
        wallet.list_swaps().unwrap()[0].stage,
        OnchainSwapStage::Accepted
    );
}

#[wasm_bindgen_test]
async fn test_taker_abandons_a_sale_and_keeps_its_asset() {
    let (mut wallet, online, asset_id) = setup_holder(Some(10_000)).await;
    let offer = make_buy_offer(&asset_id, ASSET_AMOUNT, 3600).await;
    let swap_id = offer.swap_id.clone();
    let request = wallet
        .accept_swap_offer(online.clone(), offer, 0, false, vec![])
        .await
        .unwrap();
    let proposal: OnchainSwapProposal = sim_post("/request", &json!({ "request": request })).await;
    wallet
        .complete_swap_proposal(online.clone(), proposal, 0, false)
        .await
        .unwrap();
    // never broadcast: cancelling puts everything back
    wallet.cancel_swap(online.clone(), swap_id).await.unwrap();
    assert_eq!(
        wallet.list_swaps().unwrap()[0].stage,
        OnchainSwapStage::Cancelled
    );
    assert_eq!(
        wallet.get_asset_balance(asset_id).unwrap().spendable,
        ASSET_AMOUNT
    );
}

#[wasm_bindgen_test]
async fn test_taker_buys_rgb_from_a_native_maker() {
    let (mut wallet, online, asset_id) = setup().await;
    let offer = make_offer(&asset_id, 3600).await;

    // taker: accept the offer, the maker answers with the PSBT
    let request = wallet
        .accept_swap_offer(online.clone(), offer, 0, false, vec![])
        .await
        .unwrap();
    assert_eq!(request.taker_inputs.len(), 1);
    let proposal: OnchainSwapProposal = sim_post("/request", &json!({ "request": request })).await;

    // taker: check and sign. The taker's completion can never be broadcast on its own
    let completion = wallet
        .complete_swap_proposal(online.clone(), proposal, 0, false)
        .await
        .unwrap();
    assert!(completion.finalized_psbt.is_none());

    // maker: signs last and broadcasts
    let done: OnchainSwapCompletion =
        sim_post("/completion", &json!({ "completion": completion })).await;
    assert!(done.finalized_psbt.is_some());
    wait_for_esplora_sync().await;

    let received = wallet
        .accept_swap_transfers(online.clone(), done, OnchainSwapRole::Taker, false)
        .await
        .unwrap();
    assert!(!received.assignments.is_empty());

    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();
    assert_eq!(
        wallet.get_asset_balance(asset_id).unwrap().future,
        ASSET_AMOUNT
    );

    let swaps = wallet.list_swaps().unwrap();
    assert_eq!(swaps.len(), 1);
    assert_eq!(swaps[0].stage, OnchainSwapStage::Accepted);
}

#[wasm_bindgen_test]
async fn test_taker_spends_an_asset_it_bought() {
    let (mut wallet, online, asset_id) = setup().await;
    let offer = make_offer(&asset_id, 3600).await;
    let request = wallet
        .accept_swap_offer(online.clone(), offer, 0, false, vec![])
        .await
        .unwrap();
    let proposal: OnchainSwapProposal = sim_post("/request", &json!({ "request": request })).await;
    let completion = wallet
        .complete_swap_proposal(online.clone(), proposal, 0, false)
        .await
        .unwrap();
    let done: OnchainSwapCompletion =
        sim_post("/completion", &json!({ "completion": completion })).await;
    wait_for_esplora_sync().await;
    wallet
        .accept_swap_transfers(online.clone(), done, OnchainSwapRole::Taker, false)
        .await
        .unwrap();
    // before confirmation the bought output is not spendable yet
    assert_eq!(
        wallet
            .get_asset_balance(asset_id.clone())
            .unwrap()
            .spendable,
        0
    );

    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();
    // the indexer can see the new tip a moment before the transaction in it
    let mut settled = false;
    for _ in 0..20 {
        let refreshed = wallet
            .refresh(online.clone(), None, vec![], false)
            .await
            .unwrap();
        assert!(
            refreshed.values().all(|r| r.failure.is_none()),
            "{refreshed:?}"
        );
        if refreshed
            .values()
            .any(|r| r.updated_status == Some(TransferStatus::Settled))
        {
            settled = true;
            break;
        }
        sleep_ms(500).await;
    }
    assert!(settled, "the swap receive never settled");
    let balance = wallet.get_asset_balance(asset_id.clone()).unwrap();
    assert_eq!(
        (balance.settled, balance.spendable),
        (ASSET_AMOUNT, ASSET_AMOUNT)
    );

    // the bought asset leaves in an ordinary RGB send, funded by another colored UTXO
    let unsigned = wallet
        .create_utxos_begin(online.clone(), false, Some(1), Some(10_000), 1, true)
        .await
        .unwrap();
    let signed = wallet.sign_psbt(unsigned, None).unwrap();
    wallet
        .create_utxos_end(online.clone(), signed, true)
        .await
        .unwrap();
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();
    let mut receiver = Wallet::new(test_wallet_data()).unwrap();
    let receiver_online = receiver
        .go_online(false, ESPLORA_URL.to_string())
        .await
        .unwrap();
    let transport = format!("rpc://{}", PROXY_URL.trim_start_matches("http://"));
    let receive = receiver
        .witness_receive(
            None,
            Assignment::Fungible(ASSET_AMOUNT),
            None,
            vec![transport.clone()],
            1,
        )
        .unwrap();
    let recipient = Recipient {
        recipient_id: receive.recipient_id,
        witness_data: Some(WitnessData {
            amount_sat: 2_000,
            blinding: None,
        }),
        assignment: Assignment::Fungible(ASSET_AMOUNT),
        transport_endpoints: vec![transport],
    };
    let unsigned = wallet
        .send_begin(
            online.clone(),
            HashMap::from([(asset_id.clone(), vec![recipient])]),
            true,
            1,
            1,
            None,
        )
        .await
        .unwrap();
    let signed = wallet.sign_psbt(unsigned, None).unwrap();
    wallet
        .send_end(online.clone(), signed, false)
        .await
        .unwrap();

    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    receiver.sync(receiver_online.clone()).await.unwrap();
    for _ in 0..20 {
        let refreshed = receiver
            .refresh(receiver_online.clone(), None, vec![], false)
            .await
            .unwrap();
        assert!(
            refreshed.values().all(|r| r.failure.is_none()),
            "{refreshed:?}"
        );
        if receiver
            .get_asset_balance(asset_id.clone())
            .unwrap()
            .settled
            > 0
        {
            break;
        }
        sleep_ms(500).await;
    }
    assert_eq!(
        receiver
            .get_asset_balance(asset_id.clone())
            .unwrap()
            .settled,
        ASSET_AMOUNT
    );
    wallet.sync(online.clone()).await.unwrap();
    wallet
        .refresh(online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_eq!(wallet.get_asset_balance(asset_id).unwrap().future, 0);
}

#[wasm_bindgen_test]
async fn test_taker_refuses_a_tampered_proposal() {
    use bdk_wallet::bitcoin::{Amount, psbt::Psbt};
    use std::str::FromStr;

    let (mut wallet, online, asset_id) = setup().await;
    let offer = make_offer(&asset_id, 3600).await;
    let request = wallet
        .accept_swap_offer(online.clone(), offer, 0, false, vec![])
        .await
        .unwrap();
    let proposal: OnchainSwapProposal = sim_post("/request", &json!({ "request": request })).await;

    // a maker that pays itself a little more than agreed: the PSBT no longer matches the terms
    let mut tampered = proposal.clone();
    let mut psbt = Psbt::from_str(&tampered.psbt).unwrap();
    psbt.unsigned_tx.output[1].value += Amount::from_sat(1);
    tampered.psbt = psbt.to_string();
    tampered.txid = psbt.unsigned_tx.compute_txid().to_string();
    let err = wallet
        .complete_swap_proposal(online.clone(), tampered, 0, false)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("mismatch"), "{err}");

    // and a request that differs from the one the taker sent is refused as well
    let mut other = proposal.clone();
    other.request.offer.network_fee_sat += 1;
    let err = wallet
        .complete_swap_proposal(online.clone(), other, 0, false)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("does not match local state"),
        "{err}"
    );

    // the honest proposal still goes through
    wallet
        .complete_swap_proposal(online.clone(), proposal, 0, false)
        .await
        .unwrap();
}

#[wasm_bindgen_test]
async fn test_taker_refuses_a_maker_that_declares_the_takers_coins_as_its_own() {
    let (mut wallet, online, asset_id) = setup().await;
    let offer = make_offer(&asset_id, 3600).await;
    let request = wallet
        .accept_swap_offer(online.clone(), offer, 0, false, vec![])
        .await
        .unwrap();
    let proposal: OnchainSwapProposal = sim_post("/request", &json!({ "request": request })).await;

    // the very same coin declared on the maker's side: the taker would sign it for the maker
    let mut same_coin = proposal.clone();
    same_coin.maker_inputs[0] = same_coin.request.taker_inputs[0].clone();
    let err = wallet
        .complete_swap_proposal(online.clone(), same_coin, 0, false)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("belongs to this wallet"), "{err}");

    // another coin of the taker's: recognised by its script, whatever outpoint it is given
    let mut other_coin = proposal.clone();
    other_coin.maker_inputs[0].script_pubkey_hex =
        proposal.request.taker_inputs[0].script_pubkey_hex.clone();
    other_coin.maker_inputs[0].outpoint.vout += 7;
    let err = wallet
        .complete_swap_proposal(online.clone(), other_coin, 0, false)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("belongs to this wallet"), "{err}");

    // the honest proposal still goes through
    wallet
        .complete_swap_proposal(online.clone(), proposal, 0, false)
        .await
        .unwrap();
}

#[wasm_bindgen_test]
async fn test_taker_cancels_a_swap_it_has_not_completed() {
    let (mut wallet, online, asset_id) = setup().await;
    let offer = make_offer(&asset_id, 3600).await;
    let swap_id = offer.swap_id.clone();
    wallet
        .accept_swap_offer(online.clone(), offer.clone(), 0, false, vec![])
        .await
        .unwrap();
    // the same offer cannot be accepted twice
    assert!(
        wallet
            .accept_swap_offer(online.clone(), offer, 0, false, vec![])
            .await
            .is_err()
    );
    wallet.cancel_swap(online.clone(), swap_id).await.unwrap();
    assert_eq!(
        wallet.list_swaps().unwrap()[0].stage,
        OnchainSwapStage::Cancelled
    );
}

#[wasm_bindgen_test]
async fn test_taker_refuses_an_expired_offer() {
    let (mut wallet, online, asset_id) = setup().await;
    let offer = make_offer(&asset_id, 0).await;
    let err = wallet
        .accept_swap_offer(online.clone(), offer, 0, false, vec![])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("expired"), "{err}");
}

#[wasm_bindgen_test]
async fn test_taker_sells_from_default_size_utxos() {
    let maker_address: String = sim_get("/address").await;
    fund_address(&maker_address, "1").await;
    wait_for_esplora_sync().await;
    // 1,000-sat colored UTXOs cannot pay the fees: plain BTC tops them up
    let (mut wallet, online, asset_id) = setup_holder(None).await;
    let sold = 400u64;
    let offer = make_buy_offer(&asset_id, sold, 3600).await;
    let request = wallet
        .accept_swap_offer(online.clone(), offer, 0, false, vec![])
        .await
        .unwrap();
    let colored: Vec<(String, u32)> = wallet
        .list_unspents(None, false, true)
        .unwrap()
        .into_iter()
        .filter(|u| u.utxo.colorable)
        .map(|u| (u.utxo.outpoint.txid, u.utxo.outpoint.vout))
        .collect();
    assert!(
        request
            .taker_inputs
            .iter()
            .any(|i| !colored.contains(&(i.outpoint.txid.clone(), i.outpoint.vout))),
        "no plain BTC input in {:?}",
        request.taker_inputs
    );
    let proposal: OnchainSwapProposal = sim_post("/request", &json!({ "request": request })).await;
    let completion = wallet
        .complete_swap_proposal(online.clone(), proposal, 0, false)
        .await
        .unwrap();
    let done: OnchainSwapCompletion =
        sim_post("/completion", &json!({ "completion": completion })).await;
    wait_for_esplora_sync().await;
    wallet
        .accept_swap_transfers(online.clone(), done.clone(), OnchainSwapRole::Taker, false)
        .await
        .unwrap();
    let settled: usize = sim_post("/settle", &json!({ "completion": done })).await;
    assert!(settled > 0);
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();
    // the RGB change stays with the seller
    assert_eq!(
        wallet.get_asset_balance(asset_id.clone()).unwrap().future,
        ASSET_AMOUNT - sold
    );
    let maker: serde_json::Value = sim_get(&format!("/balance?assetId={asset_id}")).await;
    assert_eq!(maker["assetFuture"], sold);
}

#[wasm_bindgen_test]
async fn test_taker_refuses_a_spent_input() {
    let (mut wallet, online, asset_id) = setup().await;
    let offer = make_offer(&asset_id, 3600).await;
    let request = wallet
        .accept_swap_offer(online.clone(), offer, 0, false, vec![])
        .await
        .unwrap();
    let proposal: OnchainSwapProposal = sim_post("/request", &json!({ "request": request })).await;

    // the taker's only BTC coin is spent elsewhere before it signs
    let elsewhere: String = sim_get("/address").await;
    let unsigned = wallet
        .send_btc_begin(online.clone(), elsewhere, 10_000, 1, false)
        .await
        .unwrap();
    let signed = wallet.sign_psbt(unsigned, None).unwrap();
    wallet
        .send_btc_end(online.clone(), signed, false)
        .await
        .unwrap();
    mine_blocks(1).await;
    wait_for_esplora_sync().await;

    let err = wallet
        .complete_swap_proposal(online.clone(), proposal, 0, false)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("already spent"), "{err}");
}

#[wasm_bindgen_test]
async fn test_taker_refuses_a_maker_input_that_differs_from_the_chain() {
    use bdk_wallet::bitcoin::{ScriptBuf, psbt::Psbt};
    use std::str::FromStr;

    let (mut wallet, online, asset_id) = setup().await;
    let offer = make_offer(&asset_id, 3600).await;
    let request = wallet
        .accept_swap_offer(online.clone(), offer, 0, false, vec![])
        .await
        .unwrap();
    let proposal: OnchainSwapProposal = sim_post("/request", &json!({ "request": request })).await;

    // a maker input declared, consistently in the PSBT, with a script it does not have on chain
    let mut lying = proposal.clone();
    let mut psbt = Psbt::from_str(&lying.psbt).unwrap();
    let outpoint = &lying.maker_inputs[0].outpoint;
    let idx = psbt
        .unsigned_tx
        .input
        .iter()
        .position(|i| {
            i.previous_output.txid.to_string() == outpoint.txid
                && i.previous_output.vout == outpoint.vout
        })
        .unwrap();
    let script = ScriptBuf::from_hex(&lying.maker_change_script_pubkey_hex).unwrap();
    psbt.inputs[idx]
        .witness_utxo
        .as_mut()
        .unwrap()
        .script_pubkey = script;
    lying.maker_inputs[0].script_pubkey_hex = lying.maker_change_script_pubkey_hex.clone();
    lying.psbt = psbt.to_string();
    let err = wallet
        .complete_swap_proposal(online.clone(), lying, 0, false)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("does not match the chain"),
        "{err}"
    );

    // the honest proposal still goes through
    wallet
        .complete_swap_proposal(online.clone(), proposal, 0, false)
        .await
        .unwrap();
}

#[wasm_bindgen_test]
async fn test_expired_swaps_are_cancelled_and_release_their_asset() {
    let (mut wallet, online, asset_id) = setup_holder(Some(10_000)).await;
    let offer = make_buy_offer(&asset_id, ASSET_AMOUNT, 60).await;
    let expiry = offer.expiration_timestamp.unwrap();
    let request = wallet
        .accept_swap_offer(online.clone(), offer, 0, false, vec![])
        .await
        .unwrap();
    let proposal: OnchainSwapProposal = sim_post("/request", &json!({ "request": request })).await;
    wallet
        .complete_swap_proposal(online.clone(), proposal, 0, false)
        .await
        .unwrap();
    // nothing to sweep yet, and the staged sale holds the asset
    assert!(
        wallet
            .cancel_expired_swaps(online.clone())
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        wallet
            .get_asset_balance(asset_id.clone())
            .unwrap()
            .spendable,
        0
    );

    while (js_sys::Date::now() / 1000.0) as u64 <= expiry {
        sleep_ms(1_000).await;
    }
    let cancelled = wallet.cancel_expired_swaps(online.clone()).await.unwrap();
    assert_eq!(cancelled.len(), 1);
    assert_eq!(
        wallet.list_swaps().unwrap()[0].stage,
        OnchainSwapStage::Cancelled
    );
    assert_eq!(
        wallet.get_asset_balance(asset_id).unwrap().spendable,
        ASSET_AMOUNT
    );
}
