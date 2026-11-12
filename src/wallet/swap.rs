//! On-chain swap taker: the wallet state and checks that do not depend on the RGB proxy.
//!
//! Offer acceptance, proposal completion and transfer acceptance exchange consignments and asset
//! history through the proxy. They still speak RGB proxy protocol 0.2 and return
//! [`Error::NotSupportedYet`] until they are ported to protocol 1.0.

use super::*;

impl Indexer {
    /// The output `outpoint` points to, if it exists and is unspent (mempool included).
    pub(crate) async fn get_unspent_txout(
        &self,
        outpoint: &BdkOutPoint,
    ) -> Result<Option<TxOut>, Error> {
        Ok(match self {
            Indexer::EsploraAsync(client) => {
                // the outspend endpoint reports an unknown outpoint as unspent, so fetch the
                // funding tx first
                let Some(tx) = client
                    .get_tx(&outpoint.txid)
                    .await
                    .map_err(IndexerError::from)?
                else {
                    return Ok(None);
                };
                let Some(txout) = tx.output.get(outpoint.vout as usize).cloned() else {
                    return Ok(None);
                };
                match client
                    .get_output_status(&outpoint.txid, outpoint.vout as u64)
                    .await
                    .map_err(IndexerError::from)?
                {
                    Some(status) if !status.spent => Some(txout),
                    _ => None,
                }
            }
        })
    }
}

/// This wallet takes swaps between RGB and BTC; RGB-for-RGB swaps are not supported.
pub(crate) fn swap_ensure_supported(direction: SwapDirection) -> Result<(), Error> {
    if direction == SwapDirection::RgbForRgb {
        return Err(swap_invalid(
            "this wallet does not support RGB-for-RGB swaps",
        ));
    }
    Ok(())
}

/// Amounts per contract carried by the given wallet inputs.
pub(crate) fn swap_input_rgb_amounts(
    wallet: &Wallet,
    inputs: &HashSet<BdkOutPoint>,
) -> Result<BTreeMap<ContractId, u64>, Error> {
    let runtime = wallet.rgb_runtime()?;
    let mut amounts = BTreeMap::new();
    for contract_id in runtime.contracts_assigning(inputs.iter().copied())? {
        let mut total = 0u64;
        for (_, states) in runtime.contract_assignments_for(contract_id, inputs.iter().copied())? {
            for (opout, state) in states {
                if let AllocatedState::Amount(amt) = state
                    && opout.ty == OS_ASSET
                {
                    total = total
                        .checked_add(amt.as_u64())
                        .ok_or_else(|| swap_invalid("swap input amounts overflow"))?;
                }
            }
        }
        amounts.insert(contract_id, total);
    }
    Ok(amounts)
}

/// The assignments received for an RGB leg must add up to the agreed amount.
pub(crate) fn swap_validate_assignments_for_leg(
    assignments: &[Assignment],
    leg: &OnchainSwapLeg,
) -> Result<(), Error> {
    if !matches!(leg.kind, OnchainSwapLegKind::Rgb) {
        return Ok(());
    }
    let total = assignments.iter().try_fold(0u64, |acc, assignment| {
        acc.checked_add(assignment.main_amount())
            .ok_or_else(|| swap_invalid("swap assignment amounts overflow"))
    })?;
    if total != leg.amount {
        return Err(swap_invalid(format!(
            "RGB swap assignment amount mismatch: expected {}, got {total}",
            leg.amount
        )));
    }
    Ok(())
}

/// The consignment's witness transaction must pay `recipient_id`'s script at `vout`.
pub(crate) fn swap_validate_witness_output(
    consignment: &RgbTransfer,
    witness_id: RgbTxid,
    recipient_id: &str,
    vout: u32,
) -> Result<(), Error> {
    let Some(anchored_bundle) = consignment
        .bundles
        .iter()
        .find(|bundle| bundle.witness_id() == witness_id)
    else {
        return Err(swap_invalid(
            "RGB swap consignment does not include the expected witness transaction",
        ));
    };
    let output = anchored_bundle
        .tx
        .output
        .get(vout as usize)
        .ok_or_else(|| swap_invalid("RGB swap consignment missing expected output"))?;
    let script_pubkey = script_buf_from_recipient_id(recipient_id.to_string())?
        .ok_or_else(|| swap_invalid("RGB swap recipient ID is not witness-based"))?;
    if output.script_pubkey != script_pubkey {
        return Err(swap_invalid(
            "RGB swap consignment output pays an unexpected script",
        ));
    }
    Ok(())
}

fn not_supported_yet(operation: &str) -> Error {
    Error::NotSupportedYet {
        operation: operation.to_string(),
    }
}

impl Wallet {
    pub(crate) fn swap_load<T: serde::de::DeserializeOwned>(
        &self,
        swap_id: &str,
        column: impl Fn(&DbSwap) -> Option<&String>,
        what: &str,
    ) -> Result<T, Error> {
        let swap = self
            .database
            .get_swap(swap_id)?
            .ok_or_else(|| swap_invalid(format!("unknown swap {swap_id}")))?;
        let serialized = column(&swap)
            .ok_or_else(|| swap_invalid(format!("missing local swap {what} for {swap_id}")))?;
        serde_json::from_str(serialized)
            .map_err(InternalError::from)
            .map_err(Error::from)
    }

    /// The counterparty's declared inputs must not be coins of this wallet: this wallet signs
    /// every input it can, so a smuggled coin would be spent for the counterparty's benefit.
    pub(crate) fn swap_ensure_inputs_foreign(
        &self,
        counterparty_inputs: &[OnchainSwapInput],
        own_inputs: &[OnchainSwapInput],
    ) -> Result<(), Error> {
        for input in counterparty_inputs {
            let outpoint = swap_input_to_outpoint(input);
            if own_inputs
                .iter()
                .any(|own| swap_input_to_outpoint(own) == outpoint)
                || self.bdk_wallet.get_utxo(outpoint).is_some()
                || self
                    .bdk_wallet
                    .is_mine(swap_parse_script(&input.script_pubkey_hex)?)
            {
                return Err(swap_invalid(format!(
                    "swap input {} belongs to this wallet",
                    input.outpoint
                )));
            }
        }
        Ok(())
    }

    pub(crate) async fn swap_ensure_inputs_confirmed(
        &self,
        inputs: &[OnchainSwapInput],
        min_confirmations: u8,
    ) -> Result<(), Error> {
        if min_confirmations == 0 {
            return Ok(());
        }
        for input in inputs {
            let confirmations = self
                .indexer()
                .get_tx_confirmations(&input.outpoint.txid)
                .await?
                .unwrap_or_default();
            if confirmations < min_confirmations as u64 {
                return Err(swap_invalid(format!(
                    "swap input {} has {confirmations} confirmations, required {min_confirmations}",
                    input.outpoint
                )));
            }
        }
        Ok(())
    }

    /// Refuse swap inputs that are spent (mempool included), unknown, or whose declared amount or
    /// script differs from the chain: signing over them wastes the swap, as the transaction can
    /// never be mined, and the PSBT checks rely on the declared values.
    pub(crate) async fn swap_ensure_inputs_on_chain(
        &self,
        inputs: &[OnchainSwapInput],
    ) -> Result<(), Error> {
        for input in inputs {
            let Some(txout) = self
                .indexer()
                .get_unspent_txout(&swap_input_to_outpoint(input))
                .await?
            else {
                return Err(swap_invalid(format!(
                    "swap input {} is already spent or does not exist",
                    input.outpoint
                )));
            };
            if txout != swap_input_txout(input)? {
                return Err(swap_invalid(format!(
                    "swap input {} does not match the chain",
                    input.outpoint
                )));
            }
        }
        Ok(())
    }

    /// Outpoints reserved by the requests of swaps still in progress.
    fn swap_reserved_inputs(&self) -> Result<HashSet<(String, u32)>, Error> {
        let mut reserved = HashSet::new();
        for swap in self.database.iter_swaps()? {
            if matches!(
                swap.stage,
                OnchainSwapStage::Cancelled | OnchainSwapStage::Accepted
            ) {
                continue;
            }
            if let Some(raw) = swap.request {
                let request: OnchainSwapRequest =
                    serde_json::from_str(&raw).map_err(InternalError::from)?;
                for input in request.taker_inputs {
                    reserved.insert((input.outpoint.txid, input.outpoint.vout));
                }
            }
        }
        Ok(reserved)
    }

    /// Select BTC inputs worth at least `needed_sat` (all eligible ones when `needed_sat` is 0).
    ///
    /// Vanilla outputs are eligible, and so are empty colored outputs that no asset or pending
    /// receive reserves, keeping the two smallest of those for RGB receives.
    pub(crate) async fn swap_select_btc_inputs(
        &self,
        needed_sat: u64,
        min_confirmations: u8,
        excluded: &HashSet<(&str, u32)>,
    ) -> Result<Vec<OnchainSwapInput>, Error> {
        let db = self.database.get_db_data(false)?;
        let unspent = self.database.get_unspent_txos(db.txos.clone())?;
        let allocations = self.database.get_rgb_allocations(
            unspent,
            Some(db.colorings),
            Some(db.batch_transfers),
            Some(db.asset_transfers),
            Some(db.transfers),
        )?;
        let mut free = self
            .get_input_unspents(&allocations)?
            .into_iter()
            .filter(|u| u.rgb_allocations.is_empty())
            .collect::<Vec<_>>();
        free.sort_by_key(|u| u.utxo.btc_amount.parse::<u64>().unwrap_or(0));
        let reusable = free
            .into_iter()
            .skip(2)
            .map(|u| (u.utxo.txid, u.utxo.vout))
            .collect::<HashSet<_>>();
        let reserved = self.swap_reserved_inputs()?;
        let mut selected = vec![];
        let mut total = 0u64;
        for output in self.bdk_wallet.list_unspent().filter(|o| {
            let key = (o.outpoint.txid.to_string(), o.outpoint.vout);
            (o.keychain == KeychainKind::Internal || reusable.contains(&key))
                && !o.is_spent
                && !excluded.contains(&(key.0.as_str(), key.1))
                && !reserved.contains(&key)
        }) {
            if min_confirmations > 0 {
                let confs = self
                    .indexer()
                    .get_tx_confirmations(&output.outpoint.txid.to_string())
                    .await?
                    .unwrap_or_default();
                if confs < min_confirmations as u64 {
                    continue;
                }
            }
            total = total
                .checked_add(output.txout.value.to_sat())
                .ok_or_else(|| swap_invalid("swap amounts overflow"))?;
            selected.push(swap_build_input(&output));
            if needed_sat > 0 && total >= needed_sat {
                break;
            }
        }
        if total < needed_sat {
            return Err(Error::InsufficientBitcoins {
                needed: needed_sat,
                available: total,
            });
        }
        Ok(selected)
    }

    /// Confirmed BTC eligible for a swap, including empty unreserved RGB change.
    pub async fn get_swap_funding_balance(&self, online: Online) -> Result<u64, Error> {
        self.check_online(online)?;
        Ok(swap_selected_inputs_total(
            &self.swap_select_btc_inputs(0, 1, &HashSet::new()).await?,
        ))
    }

    /// Accept a maker offer as the taker and return the taker's request message.
    ///
    /// Not supported yet: the request carries an RGB receive that must be bound to an RGB proxy
    /// 1.0 invoice. Always returns [`Error::NotSupportedYet`].
    pub async fn accept_swap_offer(
        &mut self,
        online: Online,
        offer: OnchainSwapOffer,
        min_confirmations: u8,
        skip_sync: bool,
        exclude_outpoints: Vec<Outpoint>,
    ) -> Result<OnchainSwapRequest, Error> {
        self.check_online(online)?;
        let _ = (offer, min_confirmations, skip_sync, exclude_outpoints);
        Err(not_supported_yet("accept_swap_offer"))
    }

    /// Complete a maker proposal as the taker.
    ///
    /// Not supported yet: the maker's consignment and asset history travel through the RGB
    /// proxy. Always returns [`Error::NotSupportedYet`].
    pub async fn complete_swap_proposal(
        &mut self,
        online: Online,
        proposal: OnchainSwapProposal,
        min_confirmations: u8,
        skip_sync: bool,
    ) -> Result<OnchainSwapCompletion, Error> {
        self.check_online(online)?;
        let _ = (proposal, min_confirmations, skip_sync);
        Err(not_supported_yet("complete_swap_proposal"))
    }

    /// Settle a completed on-chain swap as the taker.
    ///
    /// Not supported yet: the received consignment travels through the RGB proxy. Always returns
    /// [`Error::NotSupportedYet`].
    pub async fn accept_swap_transfers(
        &mut self,
        online: Online,
        completion: OnchainSwapCompletion,
        role: OnchainSwapRole,
        skip_sync: bool,
    ) -> Result<OnchainSwapReceiveResult, Error> {
        self.check_online(online)?;
        let _ = (completion, role, skip_sync);
        Err(not_supported_yet("accept_swap_transfers"))
    }

    /// Cancel an on-chain swap whose transaction has not been broadcast, releasing the inputs its
    /// prepared RGB leg reserved.
    pub async fn cancel_swap(&mut self, online: Online, swap_id: String) -> Result<(), Error> {
        info!(self.logger, "Cancelling on-chain swap {swap_id}...");
        self.check_online(online)?;
        let swap = self
            .database
            .get_swap(&swap_id)?
            .ok_or_else(|| swap_invalid(format!("unknown swap {swap_id}")))?;
        if matches!(
            swap.stage,
            OnchainSwapStage::Broadcast | OnchainSwapStage::Accepted
        ) {
            return Err(Error::CannotFailBatchTransfer);
        }
        if let Some(txid) = &swap.txid
            && self.indexer().get_tx_confirmations(txid).await?.is_some()
        {
            return Err(Error::CannotFailBatchTransfer);
        }
        if let Some(idx) = swap.outgoing_batch_transfer_idx
            && let Some(batch_transfer) = self
                .database
                .iter_batch_transfers()?
                .into_iter()
                .find(|b| b.idx == idx)
            && batch_transfer.status == TransferStatus::Initiated
        {
            let mut failed: DbBatchTransferActMod = batch_transfer.into();
            failed.status = ActiveValue::Set(TransferStatus::Failed);
            self.database.update_batch_transfer(&mut failed)?;
        }
        self.database.update_swap(&mut DbSwapActMod {
            swap_id: ActiveValue::Unchanged(swap_id),
            stage: ActiveValue::Set(OnchainSwapStage::Cancelled),
            updated_at: ActiveValue::Set(now().unix_timestamp()),
            ..Default::default()
        })?;
        self.update_backup_info(false)?;
        self.flush().await?;
        info!(self.logger, "Cancel swap completed");
        Ok(())
    }

    /// Cancel every on-chain swap whose offer has expired and whose transaction has not been
    /// broadcast, as [`cancel_swap`](Wallet::cancel_swap) does. Returns the cancelled swap IDs.
    pub async fn cancel_expired_swaps(&mut self, online: Online) -> Result<Vec<String>, Error> {
        info!(self.logger, "Cancelling expired on-chain swaps...");
        self.check_online(online.clone())?;
        let current_timestamp = now().unix_timestamp();
        let expired = self
            .database
            .iter_swaps()?
            .into_iter()
            .filter(|s| {
                !matches!(
                    s.stage,
                    OnchainSwapStage::Broadcast
                        | OnchainSwapStage::Accepted
                        | OnchainSwapStage::Cancelled
                ) && s
                    .expiration_timestamp
                    .is_some_and(|e| e <= current_timestamp)
            })
            .map(|s| s.swap_id)
            .collect::<Vec<_>>();
        let mut swap_ids = vec![];
        for swap_id in expired {
            match self.cancel_swap(online.clone(), swap_id.clone()).await {
                Ok(()) => swap_ids.push(swap_id),
                Err(Error::CannotFailBatchTransfer) => {}
                Err(e) => return Err(e),
            }
        }
        info!(self.logger, "Cancel expired swaps completed");
        Ok(swap_ids)
    }

    /// List the on-chain swaps this wallet has state for, including cancelled ones.
    pub fn list_swaps(&self) -> Result<Vec<OnchainSwapSummary>, Error> {
        let mut swaps = self
            .database
            .iter_swaps()?
            .into_iter()
            .map(|s| OnchainSwapSummary {
                swap_id: s.swap_id,
                role: s.role,
                stage: s.stage,
                expiration_timestamp: s.expiration_timestamp.map(|e| e as u64),
                txid: s.txid,
            })
            .collect::<Vec<_>>();
        swaps.sort_by(|a, b| a.swap_id.cmp(&b.swap_id));
        Ok(swaps)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::block_on;
    use crate::wallet::offline::address_reuse_tests::make_test_wallet;

    // nothing listens here: the tests never reach the indexer
    const INDEXER_URL: &str = "http://127.0.0.1:1";

    fn setup() -> (Wallet, Online) {
        let mut wallet = make_test_wallet(false);
        wallet.online_data = Some(OnlineData::for_test(INDEXER_URL));
        let online = Online {
            id: 1,
            indexer_url: INDEXER_URL.to_string(),
        };
        (wallet, online)
    }

    fn leg(kind: OnchainSwapLegKind, asset_id: Option<&str>, amount: u64) -> OnchainSwapLeg {
        OnchainSwapLeg {
            kind,
            asset_id: asset_id.map(str::to_string),
            amount,
        }
    }

    fn input(txid_byte: &str, vout: u32, script_pubkey_hex: String) -> OnchainSwapInput {
        OnchainSwapInput {
            outpoint: Outpoint {
                txid: txid_byte.repeat(32),
                vout,
            },
            amount_sat: 10_000,
            script_pubkey_hex,
        }
    }

    fn offer(swap_id: &str) -> OnchainSwapOffer {
        OnchainSwapOffer {
            swap_id: swap_id.to_string(),
            maker_gives: leg(OnchainSwapLegKind::Rgb, Some("rgb:asset"), 10),
            maker_receives: leg(OnchainSwapLegKind::Btc, None, 1_000),
            bitcoin_network: BitcoinNetwork::Regtest,
            network_fee_sat: 500,
            rgb_output_sat: 1_000,
            expiration_timestamp: None,
            maker_btc_address: None,
            maker_rgb_recipient_id: None,
            maker_rgb_script_pubkey_hex: None,
            maker_rgb_blinding: None,
            extra_outputs: vec![],
        }
    }

    fn store_swap(
        wallet: &Wallet,
        swap_id: &str,
        stage: OnchainSwapStage,
        expiration: Option<i64>,
    ) {
        wallet
            .database
            .set_swap(DbSwapActMod {
                swap_id: ActiveValue::Set(swap_id.to_string()),
                role: ActiveValue::Set(OnchainSwapRole::Taker),
                stage: ActiveValue::Set(stage),
                expiration_timestamp: ActiveValue::Set(expiration),
                offer: ActiveValue::Set(serde_json::to_string(&offer(swap_id)).unwrap()),
                created_at: ActiveValue::Set(1),
                updated_at: ActiveValue::Set(1),
                ..Default::default()
            })
            .unwrap();
    }

    #[test]
    fn proxy_flows_are_not_supported_yet() {
        let (mut wallet, online) = setup();
        let not_supported = |res: Result<(), Error>, operation: &str| {
            assert_eq!(
                res,
                Err(Error::NotSupportedYet {
                    operation: operation.to_string()
                })
            );
        };
        not_supported(
            block_on(wallet.accept_swap_offer(online.clone(), offer("s"), 1, true, vec![]))
                .map(|_| ()),
            "accept_swap_offer",
        );
        let request = OnchainSwapRequest {
            offer: offer("s"),
            taker_inputs: vec![],
            taker_btc_address: None,
            taker_rgb_recipient_id: None,
            taker_rgb_script_pubkey_hex: None,
            taker_rgb_blinding: None,
            taker_change_script_pubkey_hex: String::new(),
        };
        let proposal = OnchainSwapProposal {
            request,
            maker_inputs: vec![],
            maker_change_script_pubkey_hex: String::new(),
            psbt: String::new(),
            txid: String::new(),
            consignments: vec![],
            maker_history: None,
            lock_time: 0,
        };
        not_supported(
            block_on(wallet.complete_swap_proposal(online.clone(), proposal.clone(), 1, true))
                .map(|_| ()),
            "complete_swap_proposal",
        );
        let completion = OnchainSwapCompletion {
            proposal,
            psbt: String::new(),
            finalized_psbt: None,
            txid: String::new(),
            consignments: vec![],
            taker_history: None,
        };
        not_supported(
            block_on(wallet.accept_swap_transfers(
                online,
                completion,
                OnchainSwapRole::Taker,
                true,
            ))
            .map(|_| ()),
            "accept_swap_transfers",
        );
        assert!(wallet.list_swaps().unwrap().is_empty());
    }

    #[test]
    fn lists_and_cancels_stored_swaps() {
        let (mut wallet, online) = setup();
        store_swap(&wallet, "b", OnchainSwapStage::Requested, Some(1));
        store_swap(&wallet, "a", OnchainSwapStage::Requested, Some(i64::MAX));
        store_swap(&wallet, "c", OnchainSwapStage::Broadcast, Some(1));
        let ids = |wallet: &Wallet| {
            wallet
                .list_swaps()
                .unwrap()
                .into_iter()
                .map(|s| (s.swap_id, s.stage))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(&wallet),
            vec![
                (s!("a"), OnchainSwapStage::Requested),
                (s!("b"), OnchainSwapStage::Requested),
                (s!("c"), OnchainSwapStage::Broadcast),
            ]
        );

        // only the expired swap that was not broadcast is cancelled
        assert_eq!(
            block_on(wallet.cancel_expired_swaps(online.clone())).unwrap(),
            vec![s!("b")]
        );
        assert_eq!(ids(&wallet)[1], (s!("b"), OnchainSwapStage::Cancelled));
        assert_eq!(
            block_on(wallet.cancel_swap(online.clone(), s!("c"))),
            Err(Error::CannotFailBatchTransfer)
        );
        block_on(wallet.cancel_swap(online.clone(), s!("a"))).unwrap();
        assert_eq!(ids(&wallet)[0], (s!("a"), OnchainSwapStage::Cancelled));
        assert!(block_on(wallet.cancel_swap(online, s!("missing"))).is_err());
    }

    #[test]
    fn requested_swaps_reserve_their_inputs() {
        let (wallet, _) = setup();
        store_swap(&wallet, "live", OnchainSwapStage::Requested, None);
        store_swap(&wallet, "done", OnchainSwapStage::Cancelled, None);
        for (swap_id, byte) in [("live", "aa"), ("done", "bb")] {
            let request = OnchainSwapRequest {
                offer: offer(swap_id),
                taker_inputs: vec![input(byte, 1, String::new())],
                taker_btc_address: None,
                taker_rgb_recipient_id: None,
                taker_rgb_script_pubkey_hex: None,
                taker_rgb_blinding: None,
                taker_change_script_pubkey_hex: String::new(),
            };
            wallet
                .database
                .update_swap(&mut DbSwapActMod {
                    swap_id: ActiveValue::Unchanged(swap_id.to_string()),
                    request: ActiveValue::Set(Some(serde_json::to_string(&request).unwrap())),
                    ..Default::default()
                })
                .unwrap();
        }
        assert_eq!(
            wallet.swap_reserved_inputs().unwrap(),
            HashSet::from([("aa".repeat(32), 1)])
        );
    }

    #[test]
    fn counterparty_inputs_must_be_foreign() {
        let (mut wallet, _) = setup();
        let own_script = wallet.get_new_address().unwrap().script_pubkey();
        let foreign_script =
            ScriptBuf::new_p2wsh(&ScriptBuf::from_bytes(vec![0x51]).wscript_hash());
        let own = vec![input("aa", 0, own_script.to_hex_string())];
        let foreign = input("bb", 0, foreign_script.to_hex_string());
        wallet
            .swap_ensure_inputs_foreign(std::slice::from_ref(&foreign), &own)
            .unwrap();
        // one of this wallet's declared inputs, or any input paying one of its scripts
        assert!(wallet.swap_ensure_inputs_foreign(&own, &own).is_err());
        let smuggled = input("cc", 0, own_script.to_hex_string());
        assert!(
            wallet
                .swap_ensure_inputs_foreign(&[smuggled], &[])
                .unwrap_err()
                .to_string()
                .contains("belongs to this wallet")
        );
    }

    #[test]
    fn received_assignments_must_match_the_leg() {
        let rgb = leg(OnchainSwapLegKind::Rgb, Some("rgb:asset"), 10);
        swap_validate_assignments_for_leg(
            &[Assignment::Fungible(4), Assignment::Fungible(6)],
            &rgb,
        )
        .unwrap();
        assert!(swap_validate_assignments_for_leg(&[Assignment::Fungible(9)], &rgb).is_err());
        assert!(swap_validate_assignments_for_leg(&[], &rgb).is_err());
        let btc = leg(OnchainSwapLegKind::Btc, None, 10);
        swap_validate_assignments_for_leg(&[], &btc).unwrap();
    }

    #[test]
    fn rgb_for_rgb_is_refused() {
        assert!(swap_ensure_supported(SwapDirection::RgbForRgb).is_err());
        swap_ensure_supported(SwapDirection::RgbForBtc).unwrap();
        swap_ensure_supported(SwapDirection::BtcForRgb).unwrap();
    }
}
