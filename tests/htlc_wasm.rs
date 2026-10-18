//! Claim an RGB allocation locked in an output the browser wallet doesn't hold, as a reverse-swap
//! claim of an HTLC lock does, with the PSBT operation API.
//!
//! The lock is a P2WSH whose witness script is `OP_TRUE`, so the claim needs no HTLC signature;
//! everything RGB-side matches an HTLC lock: the allocation sits on a foreign witness output and
//! its consignment is on the proxy under that output's recipient ID.
//!
//! Needs an esplora, a proxy and the regtest helper (see DEV-ENVIRONMENT.md):
//!
//! ```text
//! wasm-pack test --headless --firefox -- --test htlc_wasm
//! ```

use std::collections::HashMap;
use std::str::FromStr;

use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

mod utils;

use rgb_lib_wasm::bitcoin::{
    Amount, OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid, Witness, absolute::LockTime,
    consensus::encode::serialize_hex, psbt::Psbt, transaction::Version,
};
use rgb_lib_wasm::utils::{recipient_id_from_script_buf, script_buf_from_recipient_id};
use rgb_lib_wasm::wallet::rust_only::{
    AssetColoringInfo, ColoringInfo, ExpectedTransfer, PsbtBroadcastState, PsbtOpExpiry,
    PsbtOperationStatus,
};
use rgb_lib_wasm::wallet::{DatabaseType, Online, Recipient, Wallet, WalletData, WitnessData};
use rgb_lib_wasm::{
    AssetSchema, Assignment, BitcoinNetwork, ContractId, Error, TransferStatus, generate_keys,
};
use utils::*;

const AMOUNT: u64 = 1_000;
const LOCK_SAT: u64 = 10_000;
const CLAIM_FEE_SAT: u64 = 1_000;
const BLINDING: u64 = 4_242;

fn test_wallet_data() -> WalletData {
    let keys = generate_keys(BitcoinNetwork::Regtest);
    WalletData {
        data_dir: "/tmp/rgb_htlc".to_string(),
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

fn transport() -> String {
    format!("rpc://{}", PROXY_URL.trim_start_matches("http://"))
}

/// `<nonce> OP_DROP OP_TRUE`: anyone can spend it, and the nonce gives each run its own recipient
/// ID on the proxy.
fn lock_witness_script(nonce: u64) -> ScriptBuf {
    let mut bytes = vec![0x08];
    bytes.extend(nonce.to_le_bytes());
    bytes.extend([0x75, 0x51]);
    ScriptBuf::from_bytes(bytes)
}

fn lock_script(nonce: u64) -> ScriptBuf {
    ScriptBuf::new_p2wsh(&lock_witness_script(nonce).wscript_hash())
}

async fn online_wallet(wallet_data: WalletData) -> (Wallet, Online) {
    let mut wallet = Wallet::new(wallet_data).unwrap();
    let online = wallet
        .go_online(false, ESPLORA_URL.to_string())
        .await
        .unwrap();
    let address = wallet.get_address().unwrap();
    fund_address(&address, "1").await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();
    (wallet, online)
}

/// Issue `AMOUNT` of an asset and lock it on the `OP_TRUE` P2WSH, returning the asset ID and the
/// lock's recipient ID.
async fn lock_asset(nonce: u64) -> (String, String) {
    let (mut wallet, online) = online_wallet(test_wallet_data()).await;
    let unsigned = wallet
        .create_utxos_begin(online.clone(), true, Some(3), Some(30_000), 1, true)
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
        .issue_asset_nia("HTLC".to_string(), "Htlc".to_string(), 0, vec![AMOUNT])
        .unwrap();

    let lock_recipient_id =
        recipient_id_from_script_buf(lock_script(nonce), BitcoinNetwork::Regtest);
    let recipient = Recipient {
        recipient_id: lock_recipient_id.clone(),
        witness_data: Some(WitnessData {
            amount_sat: LOCK_SAT,
            blinding: Some(BLINDING),
        }),
        assignment: Assignment::Fungible(AMOUNT),
        transport_endpoints: vec![transport()],
    };
    let unsigned = wallet
        .send_begin(
            online.clone(),
            HashMap::from([(asset.asset_id.clone(), vec![recipient])]),
            // broadcast right away, as a lock is: nobody ACKs it
            true,
            1,
            1,
            None,
        )
        .await
        .unwrap();
    let signed = wallet.sign_psbt(unsigned, None).unwrap();
    wallet.send_end(online, signed, false).await.unwrap();
    (asset.asset_id, lock_recipient_id)
}

async fn broadcast(tx: &Transaction) {
    let res = reqwest::Client::new()
        .post(format!("{ESPLORA_URL}/tx"))
        .body(serialize_hex(tx))
        .send()
        .await
        .unwrap();
    let status = res.status();
    let text = res.text().await.unwrap();
    assert!(status.is_success(), "broadcast failed: {text}");
}

#[wasm_bindgen_test]
async fn test_claim_a_foreign_htlc_lock() {
    let nonce = (js_sys::Math::random() * u32::MAX as f64) as u64 + js_sys::Date::now() as u64;
    let (asset_id, lock_recipient_id) = lock_asset(nonce).await;
    mine_blocks(1).await;
    wait_for_esplora_sync().await;

    let wallet_data = test_wallet_data();
    let (mut wallet, online) = online_wallet(wallet_data.clone()).await;
    let expected = ExpectedTransfer {
        asset_id: asset_id.clone(),
        asset_schema: AssetSchema::Nia,
        assignment: Assignment::Fungible(AMOUNT),
    };

    // the claimer waits for its confirmations, and rejects a lock that pays a different amount
    let shallow = wallet
        .fetch_and_accept_transfer_by_recipient_id(
            online.clone(),
            lock_recipient_id.clone(),
            lock_recipient_id.clone(),
            &transport(),
            BLINDING,
            10,
            expected.clone(),
        )
        .await;
    assert!(
        matches!(
            shallow,
            Err(Error::InsufficientConfirmations { needed: 10, got }) if (1..10).contains(&got)
        ),
        "{shallow:?}"
    );
    let wrong = wallet
        .fetch_and_accept_transfer_by_recipient_id(
            online.clone(),
            lock_recipient_id.clone(),
            lock_recipient_id.clone(),
            &transport(),
            BLINDING,
            1,
            ExpectedTransfer {
                assignment: Assignment::Fungible(AMOUNT + 1),
                ..expected.clone()
            },
        )
        .await;
    assert!(
        matches!(wrong, Err(Error::UnexpectedTransfer { .. })),
        "{wrong:?}"
    );
    let fetched = wallet
        .fetch_consignment_by_recipient_id_unchecked(lock_recipient_id.clone(), &transport())
        .await
        .unwrap();
    let lock_outpoint = OutPoint::new(Txid::from_str(&fetched.txid).unwrap(), fetched.vout);
    let (consignment, assignments) = wallet
        .fetch_and_accept_transfer_by_recipient_id(
            online.clone(),
            lock_recipient_id.clone(),
            lock_recipient_id,
            &transport(),
            BLINDING,
            1,
            expected,
        )
        .await
        .unwrap();
    assert_eq!(assignments, vec![Assignment::Fungible(AMOUNT)]);
    wallet
        .save_new_asset(online.clone(), consignment, fetched.txid.clone())
        .await
        .unwrap();

    // claim to a witness receive of this wallet, OP_RETURN first
    let receive = wallet
        .witness_receive(None, Assignment::Any, None, vec![transport()], 1)
        .unwrap();
    let destination = script_buf_from_recipient_id(receive.recipient_id)
        .unwrap()
        .unwrap();
    let mut psbt = Psbt::from_unsigned_tx(Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: lock_outpoint,
            ..Default::default()
        }],
        output: vec![
            TxOut {
                value: Amount::ZERO,
                script_pubkey: ScriptBuf::new_op_return([]),
            },
            TxOut {
                value: Amount::from_sat(LOCK_SAT - CLAIM_FEE_SAT),
                script_pubkey: destination,
            },
        ],
    })
    .unwrap();
    psbt.inputs[0].witness_utxo = Some(TxOut {
        value: Amount::from_sat(LOCK_SAT),
        script_pubkey: lock_script(nonce),
    });
    let contract_id = ContractId::from_str(&asset_id).unwrap();
    let prepared = wallet
        .psbt_op_prepare_with_expiry(
            &mut psbt,
            ColoringInfo {
                asset_info_map: HashMap::from([(
                    contract_id,
                    AssetColoringInfo {
                        output_map: HashMap::from([(1, AMOUNT)]),
                        static_blinding: Some(BLINDING + 1),
                    },
                )]),
                static_blinding: Some(BLINDING + 1),
                nonce: None,
            },
            vec![lock_outpoint],
            1,
            PsbtOpExpiry::Never,
        )
        .await
        .unwrap();
    let op = prepared.operation_id.clone();
    assert_eq!(prepared.allocations.len(), 1);
    assert_eq!(prepared.allocations[0].vout, Some(1));
    assert_eq!(
        wallet.psbt_ops_spending(lock_outpoint.into()).unwrap()[0].operation_id,
        op
    );

    // the operation survives a reload from IndexedDB
    let restored = Wallet::restore(wallet_data).await.unwrap();
    let txid = psbt.unsigned_tx.compute_txid().to_string();
    assert_eq!(restored.psbt_op_by_txid(&txid).unwrap().operation_id, op);
    drop(restored);

    // nothing broadcast yet: apply is refused
    let early = wallet.psbt_op_apply(online.clone(), &op).await;
    assert!(
        matches!(early, Err(Error::InvalidPsbtOperationStatus { .. })),
        "{early:?}"
    );

    psbt.inputs[0].final_script_witness = Some(Witness::from_slice(&[
        lock_witness_script(nonce).into_bytes()
    ]));
    let tx = psbt.extract_tx_unchecked_fee_rate();
    assert_eq!(tx.compute_txid().to_string(), txid);
    wallet.psbt_op_mark_broadcast(&op).await.unwrap();
    broadcast(&tx).await;

    // seen by the indexer: abort is refused, apply goes through
    let abort = wallet.psbt_op_abort(online.clone(), &op).await;
    assert!(
        matches!(abort, Err(Error::InvalidPsbtOperationStatus { .. })),
        "{abort:?}"
    );
    wallet.psbt_op_apply(online.clone(), &op).await.unwrap();
    let applied = wallet.psbt_op_by_txid(&txid).unwrap();
    assert_eq!(applied.status, PsbtOperationStatus::Applied);
    assert_eq!(applied.broadcast, PsbtBroadcastState::Observed);

    let provided = wallet
        .psbt_op_provide_receive_consignment(online.clone(), &op, &asset_id)
        .await
        .unwrap();
    assert_eq!(
        provided.values().next().unwrap().updated_status,
        Some(TransferStatus::WaitingConfirmations)
    );

    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    // the indexer may report the new tip before it has indexed the block's transactions
    let mut settled = 0;
    for _ in 0..20 {
        let refreshed = wallet
            .refresh(online.clone(), None, vec![], false)
            .await
            .unwrap();
        assert!(
            refreshed.values().all(|r| r.failure.is_none()),
            "{refreshed:?}"
        );
        settled += refreshed
            .values()
            .filter(|r| r.updated_status == Some(TransferStatus::Settled))
            .count();
        if settled == 2 {
            break;
        }
        sleep_ms(1_000).await;
    }
    // the receive the claim paid and the claim's own batch transfer
    assert_eq!(settled, 2);
    assert_eq!(
        wallet.psbt_op_reconcile(&op).unwrap(),
        PsbtOperationStatus::Settled
    );
    let balance = wallet.get_asset_balance(asset_id).unwrap();
    assert_eq!((balance.settled, balance.spendable), (AMOUNT, AMOUNT));
}
