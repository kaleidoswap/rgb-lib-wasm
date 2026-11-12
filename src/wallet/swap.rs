//! On-chain swap taker.
//!
//! Swap consignments travel inside the swap messages, base64-encoded, never through an RGB proxy.
//! This wallet takes swaps between RGB and BTC; RGB-for-RGB swaps are refused.

use super::*;
use crate::utils::recipient_id_from_script_buf;

/// Max size of a swap consignment received from the counterparty.
pub(crate) const SWAP_MAX_CONSIGNMENT_BYTES: usize = 32 * 1024 * 1024;

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

    /// The height of the chain tip.
    pub(crate) async fn get_tip_height(&self) -> Result<u32, Error> {
        Ok(match self {
            Indexer::EsploraAsync(client) => {
                client.get_height().await.map_err(IndexerError::from)?
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

/// Decode a base64 swap consignment received from the counterparty.
pub(crate) fn swap_decode_consignment(consignment_b64: &str) -> Result<Vec<u8>, Error> {
    // base64 expands 3 bytes into 4 chars: check before allocating the decoded bytes
    if consignment_b64.len() / 4 * 3 > SWAP_MAX_CONSIGNMENT_BYTES {
        return Err(swap_invalid("RGB swap consignment is too large"));
    }
    let bytes = general_purpose::STANDARD
        .decode(consignment_b64)
        .map_err(|_| swap_invalid("RGB swap consignment is not valid base64"))?;
    if bytes.is_empty() {
        return Err(swap_invalid("RGB swap consignment is empty"));
    }
    Ok(bytes)
}

/// The consignment of the RGB leg this wallet receives: exactly one for the leg's asset, paying
/// the swap output `vout` of `txid` to `recipient_id` with `blinding`.
pub(crate) fn swap_received_consignment<'a>(
    consignments: &'a [OnchainSwapConsignment],
    leg: &OnchainSwapLeg,
    txid: &str,
    vout: u32,
    blinding: u64,
    recipient_id: &str,
) -> Result<&'a OnchainSwapConsignment, Error> {
    let asset_id = leg
        .asset_id
        .as_deref()
        .ok_or_else(|| swap_invalid("RGB swap legs require an asset ID"))?;
    let matching = consignments
        .iter()
        .filter(|consignment| consignment.asset_id == asset_id)
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return Err(swap_invalid(format!(
            "expected exactly one RGB swap consignment for {asset_id}, got {}",
            matching.len()
        )));
    }
    let consignment = matching[0];
    if consignment.txid != txid {
        return Err(swap_invalid("RGB swap consignment txid mismatch"));
    }
    if consignment.vout != vout {
        return Err(swap_invalid("RGB swap consignment vout mismatch"));
    }
    if consignment.blinding != blinding {
        return Err(swap_invalid("RGB swap consignment blinding mismatch"));
    }
    if consignment.recipient_id != recipient_id {
        return Err(swap_invalid("RGB swap consignment recipient mismatch"));
    }
    Ok(consignment)
}

/// Where the RGB leg the taker receives lands: the vout `swap_build_psbt` gives it, the blinding
/// and the recipient ID of the request.
pub(crate) fn swap_taker_rgb_receive(
    request: &OnchainSwapRequest,
    taker_rgb_vout: Option<u32>,
) -> Result<(u32, u64, &str), Error> {
    let vout = taker_rgb_vout.ok_or_else(|| swap_invalid("missing taker RGB vout"))?;
    let blinding = request
        .taker_rgb_blinding
        .ok_or_else(|| swap_invalid("missing taker RGB blinding"))?;
    let recipient_id = request
        .taker_rgb_recipient_id
        .as_deref()
        .ok_or_else(|| swap_invalid("missing taker RGB recipient ID"))?;
    Ok((vout, blinding, recipient_id))
}

/// The RGB leg a wallet sends in a swap, with the inputs it spends.
pub(crate) struct SwapRgbSend<'a> {
    pub(crate) leg: &'a OnchainSwapLeg,
    pub(crate) inputs: &'a [OnchainSwapInput],
    pub(crate) change_script_hex: &'a str,
    pub(crate) recipient_id: &'a str,
    pub(crate) recipient_vout: u32,
    pub(crate) blinding: u64,
}

impl SwapRgbSend<'_> {
    fn input_outpoints(&self) -> HashSet<BdkOutPoint> {
        self.inputs.iter().map(swap_input_to_outpoint).collect()
    }

    fn coloring_info(
        &self,
        wallet: &Wallet,
        psbt: &Psbt,
    ) -> Result<(ContractId, rust_only::ColoringInfo, bool), Error> {
        let available = swap_input_rgb_amounts(wallet, &self.input_outpoints())?;
        let change_script = swap_parse_script(self.change_script_hex)?;
        // RGB change must land on the colored keychain, where the wallet tracks it. A side
        // without RGB change gets a vanilla change, so no change vout here, and coloring fails
        // closed if RGB change turns up anyway.
        let colored_change = matches!(
            wallet.bdk_wallet.derivation_of_spk(change_script.clone()),
            Some((KeychainKind::External, _))
        );
        let change_vout = psbt
            .unsigned_tx
            .output
            .iter()
            .position(|o| colored_change && o.script_pubkey == change_script)
            .map(|vout| vout as u32);
        swap_rgb_leg_coloring_info(
            self.leg,
            psbt,
            self.recipient_vout,
            change_vout,
            self.blinding,
            &available,
        )
    }
}

/// The RGB leg this wallet colored into a swap PSBT, recorded once the PSBT is signed.
pub(crate) struct SwapOutgoingLeg {
    /// Colored witness transaction ID
    pub(crate) txid: String,
    pub(crate) fascia: Fascia,
    /// Coloring keyed by final PSBT vouts
    pub(crate) coloring_info: rust_only::ColoringInfo,
    /// What the spent inputs carry
    pub(crate) spent: rust_only::SpentByContract,
    pub(crate) consignment: OnchainSwapConsignment,
}

/// A received RGB leg, validated but not accepted yet.
pub(crate) struct SwapReceivedLeg {
    pub(crate) transfer: RgbTransfer,
    pub(crate) valid_transfer: ValidTransfer,
    pub(crate) asset_schema: AssetSchema,
    pub(crate) assignments: Vec<Assignment>,
    pub(crate) history: PreFetchResolver,
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
        let mut seen = HashSet::new();
        for input in inputs {
            if !seen.insert(swap_input_to_outpoint(input)) {
                return Err(swap_invalid(format!(
                    "swap input {} is declared more than once",
                    input.outpoint
                )));
            }
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
        let mut reserved = self.swap_reserved_inputs()?;
        // a pending send_btc_begin / create_utxos_begin PSBT relies on its coins
        reserved.extend(
            self.get_reserved_outpoints()?
                .into_iter()
                .map(|o| (o.txid, o.vout)),
        );
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

    /// Select this side's inputs for what it `gives` plus `extra_sat` of its own costs, and return
    /// them with the keychain its change output must use.
    ///
    /// The change goes to the colored keychain only when the inputs carry RGB change (of the
    /// swapped asset or of others sharing the inputs), which coloring then assigns to it.
    /// Otherwise it goes to the vanilla keychain.
    async fn swap_select_inputs(
        &mut self,
        online: Online,
        gives: &OnchainSwapLeg,
        extra_sat: u64,
        min_confirmations: u8,
        excluded: &HashSet<(&str, u32)>,
    ) -> Result<(Vec<OnchainSwapInput>, KeychainKind), Error> {
        if matches!(gives.kind, OnchainSwapLegKind::Btc) {
            let needed = gives
                .amount
                .checked_add(extra_sat)
                .ok_or_else(|| swap_invalid("swap amounts overflow"))?;
            let inputs = self
                .swap_select_btc_inputs(needed, min_confirmations, excluded)
                .await?;
            return Ok((inputs, KeychainKind::Internal));
        }
        let asset_id = gives.asset_id.clone().expect("RGB leg has asset ID");
        let contract_id = ContractId::from_str(&asset_id)
            .map_err(|e| swap_invalid(format!("invalid RGB asset ID: {e}")))?;
        self.database.check_asset_exists(asset_id.clone())?;
        let assignments = AssignmentsCollection {
            fungible: gives.amount,
            ..Default::default()
        };
        let (_, _, mut input_unspents, _) = self._get_transfer_begin_data(online, 1)?;
        input_unspents.retain(|u| !excluded.contains(&(u.utxo.txid.as_str(), u.utxo.vout)));
        let selected = self._select_rgb_inputs(asset_id, &assignments, input_unspents)?;
        let selected_outpoints = selected
            .txo_map
            .into_values()
            .map(BdkOutPoint::from)
            .collect::<HashSet<_>>();
        let colored_inputs = self
            .bdk_wallet
            .list_unspent()
            .filter(|o| selected_outpoints.contains(&o.outpoint))
            .map(|o| swap_build_input(&o))
            .collect::<Vec<_>>();
        if colored_inputs.is_empty() {
            return Err(swap_invalid("could not resolve selected RGB inputs"));
        }
        self.swap_ensure_inputs_confirmed(&colored_inputs, min_confirmations)
            .await?;
        // the BTC top-up must not pick the colored inputs again
        let colored_keys = colored_inputs
            .iter()
            .map(|i| (i.outpoint.txid.clone(), i.outpoint.vout))
            .collect::<Vec<_>>();
        let mut top_up_excluded = excluded.clone();
        top_up_excluded.extend(colored_keys.iter().map(|(t, v)| (t.as_str(), *v)));
        // coloring assigns everything the declared inputs carry, so the RGB change is computed on
        // the same (final) input set
        let carries_rgb_change =
            |wallet: &Wallet, inputs: &[OnchainSwapInput]| {
                let outpoints = inputs.iter().map(swap_input_to_outpoint).collect();
                Ok::<_, Error>(swap_input_rgb_amounts(wallet, &outpoints)?.into_iter().any(
                    |(cid, amount)| amount > if cid == contract_id { gives.amount } else { 0 },
                ))
            };
        let dust = self
            .bdk_wallet
            .public_descriptor(KeychainKind::External)
            .dust_value()
            .to_sat();
        let colored_total = swap_selected_inputs_total(&colored_inputs);
        let mut reserve_change = carries_rgb_change(self, &colored_inputs)?;
        loop {
            // RGB change rides on this side's change output, which then has to be above dust
            let needed = if reserve_change {
                extra_sat
                    .checked_add(dust)
                    .ok_or_else(|| swap_invalid("swap amounts overflow"))?
            } else {
                extra_sat
            };
            let mut inputs = colored_inputs.clone();
            if colored_total < needed {
                // colored UTXOs usually hold too few sats to pay for this side, so top them up
                // with plain BTC inputs
                let btc_inputs = self
                    .swap_select_btc_inputs(
                        needed - colored_total,
                        min_confirmations,
                        &top_up_excluded,
                    )
                    .await
                    .map_err(|e| match e {
                        Error::InsufficientBitcoins { available, .. } => {
                            Error::InsufficientBitcoins {
                                needed,
                                available: colored_total.saturating_add(available),
                            }
                        }
                        e => e,
                    })?;
                inputs.extend(btc_inputs);
            }
            let rgb_change = carries_rgb_change(self, &inputs)?;
            if rgb_change && !reserve_change {
                // a plain coin turned out to carry RGB, so make room for a colored change
                reserve_change = true;
                continue;
            }
            let change_keychain = if rgb_change {
                KeychainKind::External
            } else {
                KeychainKind::Internal
            };
            return Ok((inputs, change_keychain));
        }
    }

    /// What the swap takes from this wallet, net of what it pays back to it, from the wallet's own
    /// view of its coins rather than what the PSBT says they are worth.
    fn swap_ensure_own_net_outflow(
        &self,
        psbt: &Psbt,
        proposal: &OnchainSwapProposal,
        own_inputs: &[OnchainSwapInput],
    ) -> Result<(), Error> {
        let mut own_in = 0u64;
        for input in own_inputs {
            let utxo = self
                .bdk_wallet
                .get_utxo(swap_input_to_outpoint(input))
                .ok_or_else(|| {
                    swap_invalid(format!(
                        "swap input {} is not an unspent coin of this wallet",
                        input.outpoint
                    ))
                })?;
            if utxo.txout != swap_input_txout(input)? {
                return Err(swap_invalid(format!(
                    "swap input {} does not match this wallet's coin",
                    input.outpoint
                )));
            }
            own_in = own_in
                .checked_add(utxo.txout.value.to_sat())
                .ok_or_else(|| swap_invalid("swap amounts overflow"))?;
        }
        let own_out = psbt
            .unsigned_tx
            .output
            .iter()
            .filter(|o| self.bdk_wallet.is_mine(o.script_pubkey.clone()))
            .try_fold(0u64, |total, o| total.checked_add(o.value.to_sat()))
            .ok_or_else(|| swap_invalid("swap amounts overflow"))?;
        swap_ensure_net_btc_outflow(proposal, OnchainSwapRole::Taker, own_in, own_out)
    }

    /// Sign the taker's declared inputs, refusing a PSBT that would make this wallet sign anything
    /// else or take more BTC from it than the swap terms allow.
    fn swap_sign_psbt(&self, psbt: &mut Psbt, proposal: &OnchainSwapProposal) -> Result<(), Error> {
        let own_inputs = &proposal.request.taker_inputs;
        self.swap_ensure_inputs_foreign(&proposal.maker_inputs, own_inputs)?;
        self.swap_ensure_own_net_outflow(psbt, proposal, own_inputs)?;
        let before = psbt.clone();
        self._sign_psbt(
            psbt,
            Some(SignOptions {
                trust_witness_utxo: true,
                ..Default::default()
            }),
        )?;
        swap_ensure_only_own_inputs_signed(&before, psbt, own_inputs)
    }

    /// Color the RGB leg this wallet sends into `psbt` and build the recipient's consignment,
    /// without recording anything: [`Wallet::swap_record_outgoing`] does once the PSBT is signed.
    fn swap_color_rgb_leg(
        &self,
        psbt: &mut Psbt,
        send: &SwapRgbSend,
        swap_id: &str,
    ) -> Result<SwapOutgoingLeg, Error> {
        let (contract_id, coloring_info, shift) = send.coloring_info(self, psbt)?;
        let prev_outputs = send.input_outpoints();
        // what the inputs carry, per outpoint, before the stash changes
        let mut spent: rust_only::SpentByContract = HashMap::new();
        {
            let runtime = self.rgb_runtime()?;
            for cid in coloring_info.asset_info_map.keys() {
                let by_outpoint = spent.entry(*cid).or_default();
                for (explicit_seal, opout_state_map) in
                    runtime.contract_assignments_for(*cid, prev_outputs.iter().copied())?
                {
                    for (opout, state) in opout_state_map {
                        by_outpoint
                            .entry(explicit_seal.to_outpoint())
                            .or_default()
                            .push(Assignment::from_opout_and_state(opout, &state));
                    }
                }
            }
        }
        self.color_psbt_stage_for_inputs(psbt, coloring_info.clone(), prev_outputs)?;
        let fascia = self.color_psbt_finalize(psbt, Some(swap_mpc_entropy(swap_id)))?;
        let seal = BuilderSeal::Revealed(GraphSeal::with_blinded_vout(
            send.recipient_vout,
            send.blinding,
        ));
        let transfer = self.generate_transfer(contract_id, vec![seal], psbt.get_txid(), &fascia)?;
        let mut consignment_bytes = vec![];
        transfer.save(&mut consignment_bytes)?;
        let txid = psbt.unsigned_tx.compute_txid().to_string();
        let mut recorded = coloring_info;
        if shift {
            for info in recorded.asset_info_map.values_mut() {
                info.output_map = info
                    .output_map
                    .drain()
                    .map(|(vout, amount)| (vout + 1, amount))
                    .collect();
            }
        }
        Ok(SwapOutgoingLeg {
            consignment: OnchainSwapConsignment {
                asset_id: contract_id.to_string(),
                consignment: general_purpose::STANDARD.encode(consignment_bytes),
                txid: txid.clone(),
                vout: send.recipient_vout,
                blinding: send.blinding,
                recipient_id: send.recipient_id.to_string(),
            },
            txid,
            fascia,
            coloring_info: recorded,
            spent,
        })
    }

    /// Record the RGB leg a swap sends as an `Initiated` batch transfer. The stash is untouched
    /// until the swap transaction is broadcast ([`Wallet::swap_apply_outgoing`]).
    fn swap_record_outgoing(
        &self,
        psbt: &Psbt,
        leg: &SwapOutgoingLeg,
        expiration_timestamp: Option<u64>,
    ) -> Result<i32, Error> {
        let runtime = self.rgb_runtime()?;
        self.psbt_op_record_batch(
            &runtime,
            psbt,
            &leg.txid,
            &leg.spent,
            &leg.coloring_info,
            1,
            expiration_timestamp,
        )
    }

    /// Validate the consignment of the RGB leg this wallet receives, whose witness transaction may
    /// not be broadcast yet: it comes from the consignment, the rest of the history from the
    /// indexer.
    async fn swap_validate_received_leg(
        &self,
        consignment: &OnchainSwapConsignment,
        leg: &OnchainSwapLeg,
    ) -> Result<SwapReceivedLeg, Error> {
        let bytes = swap_decode_consignment(&consignment.consignment)?;
        let transfer = RgbTransfer::load(&bytes[..]).map_err(|_| Error::InvalidConsignment)?;
        let expected_asset_id = leg.asset_id.clone().expect("RGB leg has asset ID");
        let asset_id = transfer.contract_id().to_string();
        if asset_id != expected_asset_id {
            return Err(swap_invalid(format!(
                "RGB swap consignment asset mismatch: expected {expected_asset_id}, got {asset_id}"
            )));
        }
        let asset_schema: AssetSchema = transfer.schema_id().try_into()?;
        self.check_schema_support(&asset_schema)?;
        let witness_id = RgbTxid::from_str(&consignment.txid).map_err(|_| Error::InvalidTxid)?;
        let history = self
            .prefetch_witnesses(
                transfer
                    .bundled_witnesses()
                    .map(|bw| bw.witness_id())
                    .filter(|id| *id != witness_id),
            )
            .await?;
        let valid_transfer =
            self.validate_transfer_offchain(&transfer, witness_id, asset_schema, &history)?;
        swap_validate_witness_output(
            &transfer,
            witness_id,
            &consignment.recipient_id,
            consignment.vout,
        )?;
        // an assignment a later transition of the consignment spends is no longer there
        let spent = Self::known_spent_opouts(&transfer);
        let assignments = self
            .extract_received_assignments(&transfer, witness_id, Some(consignment.vout), None)
            .into_iter()
            .filter(|(opout, _)| !spent.contains(opout))
            .map(|(_, assignment)| assignment)
            .collect::<Vec<_>>();
        if assignments.is_empty() {
            return Err(swap_invalid(
                "RGB swap consignment did not assign expected state",
            ));
        }
        swap_validate_assignments_for_leg(&assignments, leg)?;
        Ok(SwapReceivedLeg {
            transfer,
            valid_transfer,
            asset_schema,
            assignments,
            history,
        })
    }

    /// Record the received RGB leg of a swap as an incoming witness transfer waiting for the swap
    /// transaction to confirm, with its consignment kept for the refresh that settles it.
    #[allow(clippy::too_many_arguments)]
    fn swap_record_incoming(
        &mut self,
        asset_id: &str,
        txid: &str,
        vout: u32,
        btc_amount_sat: u64,
        recipient_id: &str,
        assignments: &[Assignment],
        consignment_bytes: Vec<u8>,
    ) -> Result<(), Error> {
        let batch_transfer_idx = self.database.set_batch_transfer(DbBatchTransferActMod {
            txid: ActiveValue::Set(Some(txid.to_string())),
            status: ActiveValue::Set(TransferStatus::WaitingConfirmations),
            expiration: ActiveValue::Set(None),
            created_at: ActiveValue::Set(now().unix_timestamp()),
            min_confirmations: ActiveValue::Set(1),
            ..Default::default()
        })?;
        let asset_transfer_idx = self.database.set_asset_transfer(DbAssetTransferActMod {
            user_driven: ActiveValue::Set(true),
            batch_transfer_idx: ActiveValue::Set(batch_transfer_idx),
            asset_id: ActiveValue::Set(Some(asset_id.to_string())),
            ..Default::default()
        })?;
        let requested = assignments.iter().try_fold(0u64, |acc, a| {
            acc.checked_add(a.main_amount())
                .ok_or_else(|| swap_invalid("swap assignment amounts overflow"))
        })?;
        self.database.set_transfer(DbTransferActMod {
            asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
            requested_assignment: ActiveValue::Set(Some(Assignment::Fungible(requested))),
            incoming: ActiveValue::Set(true),
            recipient_id: ActiveValue::Set(Some(recipient_id.to_string())),
            recipient_type: ActiveValue::Set(Some(RecipientTypeFull::Witness { vout: Some(vout) })),
            ack: ActiveValue::Set(Some(true)),
            ..Default::default()
        })?;
        let outpoint = Outpoint {
            txid: txid.to_string(),
            vout,
        };
        let txo_idx = match self.database.get_txo(&outpoint)? {
            Some(txo) => txo.idx,
            None => self.database.set_txo(DbTxoActMod {
                txid: ActiveValue::Set(txid.to_string()),
                vout: ActiveValue::Set(vout),
                btc_amount: ActiveValue::Set(btc_amount_sat.to_string()),
                spent: ActiveValue::Set(false),
                exists: ActiveValue::Set(false),
                pending_witness: ActiveValue::Set(true),
                ..Default::default()
            })?,
        };
        for assignment in assignments {
            self.database.set_coloring(DbColoringActMod {
                txo_idx: ActiveValue::Set(txo_idx),
                asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                r#type: ActiveValue::Set(ColoringType::Receive),
                assignment: ActiveValue::Set(assignment.clone()),
                ..Default::default()
            })?;
        }
        self.received_consignments
            .insert(recipient_id.to_string(), consignment_bytes);
        Ok(())
    }

    /// Make the broadcast swap TX known to BDK: parties that did not broadcast it would not
    /// discover it with a sync, leaving their swap outputs unknown.
    async fn swap_apply_witness_tx(
        &mut self,
        completion: &OnchainSwapCompletion,
    ) -> Result<(), Error> {
        let txid = Txid::from_str(&completion.txid).map_err(|_| Error::InvalidTxid)?;
        let tx = match completion.finalized_psbt.as_deref() {
            Some(finalized_psbt) => {
                let psbt = Psbt::from_str(finalized_psbt)?;
                swap_validate_proposal_psbt(&completion.proposal, &psbt)?;
                psbt.extract_tx().map_err(InternalError::from)?
            }
            // the maker broadcast the swap without sending back its finalized completion: the
            // taker only has the one it signed. Its PSBT must be the proposal's transaction, and
            // the txid commits to everything in it but the witnesses, which the network checked
            None => {
                let psbt = Psbt::from_str(&completion.psbt)?;
                swap_validate_proposal_psbt(&completion.proposal, &psbt)?;
                if psbt.unsigned_tx.compute_txid() != txid {
                    return Err(swap_invalid("swap completion txid mismatch"));
                }
                self.indexer()
                    .get_tx_with_status(&txid)
                    .await?
                    .map(|(tx, _, _)| tx)
                    .ok_or_else(|| {
                        swap_invalid(
                            "swap completion is not finalized and its transaction is not known \
                             to the indexer",
                        )
                    })?
            }
        };
        if tx.compute_txid() != txid {
            return Err(swap_invalid("swap completion txid mismatch"));
        }
        if self
            .indexer()
            .get_tx_confirmations(&completion.txid)
            .await?
            .is_none()
        {
            return Err(swap_invalid("swap transaction is not known to the indexer"));
        }
        let seen_at = now().unix_timestamp() as u64;
        self.bdk_wallet.apply_unconfirmed_txs([(tx, seen_at)]);
        self.bdk_wallet.persist(&mut self.bdk_database)?;
        Ok(())
    }

    /// Consume this wallet's prepared RGB leg of a swap into the stash, now that the swap
    /// transaction is known to the indexer, and mark the inputs it spends.
    async fn swap_apply_outgoing(
        &mut self,
        swap: &DbSwap,
        completion: &OnchainSwapCompletion,
    ) -> Result<(), Error> {
        let (Some(batch_transfer_idx), Some(fascia)) = (
            swap.outgoing_batch_transfer_idx,
            swap.outgoing_fascia.as_deref(),
        ) else {
            return Err(swap_invalid(format!(
                "swap {} has no prepared RGB leg",
                swap.swap_id
            )));
        };
        let batch_transfer = self
            .database
            .iter_batch_transfers()?
            .into_iter()
            .find(|b| b.idx == batch_transfer_idx)
            .ok_or_else(|| Error::Inconsistency {
                details: format!("swap {} outgoing batch transfer is missing", swap.swap_id),
            })?;
        if batch_transfer.txid.as_deref() != Some(&completion.txid) {
            return Err(swap_invalid(
                "swap txid does not match the prepared RGB leg",
            ));
        }
        // a leg the expiry sweep failed is applied all the same: its transaction is on chain
        if !matches!(
            batch_transfer.status,
            TransferStatus::Initiated | TransferStatus::Failed
        ) {
            return Ok(());
        }
        let txid = Txid::from_str(&completion.txid).map_err(|_| Error::InvalidTxid)?;
        let (tx, height, time) = self
            .indexer()
            .get_tx_with_status(&txid)
            .await?
            .ok_or_else(|| swap_invalid("swap transaction is not known to the indexer"))?;
        // never downgrade a mined witness to tentative
        let witness_ord = height
            .zip(time)
            .and_then(|(h, t)| NonZeroU32::new(h).zip(Some(t)))
            .and_then(|(h, t)| WitnessPos::bitcoin(h, t as i64))
            .map_or(WitnessOrd::Tentative, WitnessOrd::Mined);
        let fascia: Fascia = serde_json::from_str(fascia).map_err(InternalError::from)?;
        self.consume_fascia_in_memory(fascia, Some(witness_ord))?;
        for input in &tx.input {
            if let Some(txo) = self
                .database
                .get_txo(&Outpoint::from(input.previous_output))?
            {
                let mut spent: DbTxoActMod = txo.into();
                spent.spent = ActiveValue::Set(true);
                self.database.update_txo(spent)?;
            }
        }
        let mut updated: DbBatchTransferActMod = batch_transfer.into();
        updated.status = ActiveValue::Set(TransferStatus::WaitingConfirmations);
        self.database.update_batch_transfer(&mut updated)?;
        Ok(())
    }

    /// Accept a maker offer as the taker and return the taker's request message.
    ///
    /// The taker validates the offer, selects its inputs (BTC inputs when it gives BTC, RGB inputs
    /// topped up with BTC when it gives RGB) and derives its receive destination and change
    /// script. Inputs in `exclude_outpoints`, and those of the other swaps in progress, are never
    /// selected. Nothing is colored yet: the taker colors its RGB leg when completing the
    /// proposal. The returned [`OnchainSwapRequest`] must be forwarded to the maker.
    pub async fn accept_swap_offer(
        &mut self,
        online: Online,
        offer: OnchainSwapOffer,
        min_confirmations: u8,
        skip_sync: bool,
        exclude_outpoints: Vec<Outpoint>,
    ) -> Result<OnchainSwapRequest, Error> {
        info!(self.logger, "Accepting on-chain swap offer...");
        self.check_online(online.clone())?;
        let direction = swap_validate_legs(&offer.maker_gives, &offer.maker_receives)?;
        swap_ensure_supported(direction)?;
        swap_ensure_not_expired(&offer)?;
        if offer.bitcoin_network != self.bitcoin_network() {
            return Err(Error::BitcoinNetworkMismatch);
        }
        swap_validate_extra_outputs(&offer.extra_outputs, offer.bitcoin_network)?;
        if self.database.get_swap(&offer.swap_id)?.is_some() {
            return Err(swap_invalid(format!(
                "swap {} was already accepted",
                offer.swap_id
            )));
        }
        if !skip_sync {
            self.sync_db_txos(false).await?;
        }
        let taker_gives = offer.maker_receives.clone();
        let taker_receives = offer.maker_gives.clone();
        let extra_sat = swap_side_rgb_output_cost(&taker_receives, offer.rgb_output_sat)
            .checked_add(offer.network_fee_sat)
            .and_then(|v| {
                v.checked_add(
                    swap_extra_funded_by(&offer.extra_outputs, OnchainSwapRole::Taker).ok()?,
                )
            })
            .ok_or_else(|| swap_invalid("swap amounts overflow"))?;
        let reserved = self.swap_reserved_inputs()?;
        let excluded = exclude_outpoints
            .iter()
            .map(|o| (o.txid.as_str(), o.vout))
            .chain(reserved.iter().map(|(txid, vout)| (txid.as_str(), *vout)))
            .collect::<HashSet<_>>();
        let (taker_inputs, change_keychain) = self
            .swap_select_inputs(
                online,
                &taker_gives,
                extra_sat,
                min_confirmations,
                &excluded,
            )
            .await?;
        // with skip_sync the local view of these coins can be stale
        self.swap_ensure_inputs_on_chain(&taker_inputs).await?;
        let (taker_btc_address, taker_rgb_recipient_id, taker_rgb_script, taker_rgb_blinding) =
            if matches!(taker_receives.kind, OnchainSwapLegKind::Btc) {
                let address = self._get_new_address(KeychainKind::Internal)?;
                (Some(address.to_string()), None, None, None)
            } else {
                let script = self
                    ._get_new_address(KeychainKind::External)?
                    .script_pubkey();
                (
                    None,
                    Some(recipient_id_from_script_buf(
                        script.clone(),
                        self.bitcoin_network(),
                    )),
                    Some(script.to_hex_string()),
                    Some(swap_random_blinding()),
                )
            };
        // the change is colored only when it has to carry RGB change, else it stays vanilla
        let taker_change_script_pubkey_hex = self
            ._get_new_address(change_keychain)?
            .script_pubkey()
            .to_hex_string();
        let request = OnchainSwapRequest {
            offer,
            taker_inputs,
            taker_btc_address,
            taker_rgb_recipient_id,
            taker_rgb_script_pubkey_hex: taker_rgb_script,
            taker_rgb_blinding,
            taker_change_script_pubkey_hex,
        };
        let created_at = now().unix_timestamp();
        self.database.set_swap(DbSwapActMod {
            swap_id: ActiveValue::Set(request.offer.swap_id.clone()),
            role: ActiveValue::Set(OnchainSwapRole::Taker),
            stage: ActiveValue::Set(OnchainSwapStage::Requested),
            expiration_timestamp: ActiveValue::Set(
                request.offer.expiration_timestamp.map(|e| e as i64),
            ),
            offer: ActiveValue::Set(
                serde_json::to_string(&request.offer).map_err(InternalError::from)?,
            ),
            request: ActiveValue::Set(Some(
                serde_json::to_string(&request).map_err(InternalError::from)?,
            )),
            created_at: ActiveValue::Set(created_at),
            updated_at: ActiveValue::Set(created_at),
            ..Default::default()
        })?;
        self.update_backup_info(false)?;
        self.flush().await?;
        info!(self.logger, "Accept swap offer completed");
        Ok(request)
    }

    /// Complete a maker proposal as the taker.
    ///
    /// The taker checks that the proposal answers its stored request, that its lock time is not
    /// ahead of the chain, that the PSBT is exactly the one the agreed terms produce and that the
    /// maker's inputs are not coins of this wallet and are unspent. When the taker receives RGB,
    /// the maker's consignment (in the proposal) must give it the agreed amount at its output;
    /// when it sends RGB, it colors its leg into the PSBT, records it as an `Initiated` transfer
    /// and returns the consignment in the completion. It then signs its own inputs only. The
    /// maker signs last.
    ///
    /// Retrying with the same proposal returns the completion already signed.
    ///
    /// **Residual risk**: the completion carries this wallet's signatures on a transaction with
    /// no expiry: it stays valid for as long as the inputs this wallet declared are unspent.
    /// [`cancel_swap`](Wallet::cancel_swap) only drops the local state; spending one of those
    /// inputs is what rules out a late broadcast.
    pub async fn complete_swap_proposal(
        &mut self,
        online: Online,
        proposal: OnchainSwapProposal,
        min_confirmations: u8,
        skip_sync: bool,
    ) -> Result<OnchainSwapCompletion, Error> {
        info!(self.logger, "Completing on-chain swap proposal...");
        self.check_online(online)?;
        let offer = &proposal.request.offer;
        let swap_id = offer.swap_id.clone();
        let swap = self
            .database
            .get_swap(&swap_id)?
            .ok_or_else(|| swap_invalid(format!("unknown swap {swap_id}")))?;
        if swap.role != OnchainSwapRole::Taker {
            return Err(swap_invalid(format!("swap {swap_id} is not a taker swap")));
        }
        if let Some(raw) = &swap.completion {
            let completion: OnchainSwapCompletion =
                serde_json::from_str(raw).map_err(InternalError::from)?;
            if completion.proposal != proposal {
                return Err(swap_invalid(format!(
                    "swap {swap_id} was already completed with another proposal"
                )));
            }
            return Ok(completion);
        }
        if swap.stage != OnchainSwapStage::Requested {
            return Err(swap_invalid(format!(
                "swap {swap_id} is {:?}, expected Requested",
                swap.stage
            )));
        }
        let local_request: OnchainSwapRequest =
            self.swap_load(&swap_id, |s| s.request.as_ref(), "request")?;
        swap_ensure_state_matches(&local_request, &proposal.request, "request")?;
        let direction = swap_validate_legs(&offer.maker_gives, &offer.maker_receives)?;
        swap_ensure_supported(direction)?;
        swap_ensure_not_expired(offer)?;
        if offer.bitcoin_network != self.bitcoin_network() {
            return Err(Error::BitcoinNetworkMismatch);
        }
        swap_ensure_lock_time_current(&proposal, self.indexer().get_tip_height().await?)?;
        let mut psbt = Psbt::from_str(&proposal.psbt)?;
        swap_validate_proposal_psbt(&proposal, &psbt)?;
        if proposal.txid != psbt.unsigned_tx.compute_txid().to_string() {
            return Err(swap_invalid("swap proposal txid mismatch"));
        }
        // Security: the maker picked `maker_inputs` and built the PSBT from them, so the checks
        // above can't catch one of this wallet's coins listed as a maker input. Refuse it before
        // coloring anything.
        self.swap_ensure_inputs_foreign(&proposal.maker_inputs, &proposal.request.taker_inputs)?;
        let (_, maker_rgb_vout, taker_rgb_vout) = swap_build_psbt(&proposal)?;
        let all_inputs = proposal
            .maker_inputs
            .iter()
            .chain(proposal.request.taker_inputs.iter())
            .cloned()
            .collect::<Vec<_>>();
        swap_restore_input_metadata(&mut psbt, &all_inputs)?;
        // refuse spent inputs before coloring anything; checked again right before signing
        self.swap_ensure_inputs_on_chain(&all_inputs).await?;

        let mut consignments = proposal.consignments.clone();
        let mut outgoing = None;
        match direction {
            // the maker gives RGB: its consignment must pay this wallet's output
            SwapDirection::RgbForBtc => {
                let (vout, blinding, recipient_id) =
                    swap_taker_rgb_receive(&proposal.request, taker_rgb_vout)?;
                let consignment = swap_received_consignment(
                    &proposal.consignments,
                    &offer.maker_gives,
                    &proposal.txid,
                    vout,
                    blinding,
                    recipient_id,
                )?;
                self.swap_validate_received_leg(consignment, &offer.maker_gives)
                    .await?;
            }
            // the maker gives BTC: this wallet colors the RGB leg it sends
            SwapDirection::BtcForRgb => {
                let send = SwapRgbSend {
                    leg: &offer.maker_receives,
                    inputs: &proposal.request.taker_inputs,
                    change_script_hex: &proposal.request.taker_change_script_pubkey_hex,
                    recipient_id: offer
                        .maker_rgb_recipient_id
                        .as_deref()
                        .ok_or_else(|| swap_invalid("missing maker RGB receive data"))?,
                    recipient_vout: maker_rgb_vout
                        .ok_or_else(|| swap_invalid("missing maker RGB receive data"))?,
                    blinding: offer
                        .maker_rgb_blinding
                        .ok_or_else(|| swap_invalid("missing maker RGB receive data"))?,
                };
                let leg = self.swap_color_rgb_leg(&mut psbt, &send, &swap_id)?;
                consignments.push(leg.consignment.clone());
                outgoing = Some(leg);
                swap_restore_input_metadata(&mut psbt, &all_inputs)?;
            }
            SwapDirection::RgbForRgb => unreachable!("RGB-for-RGB swaps are refused"),
        }

        if !skip_sync {
            self.sync_db_txos(false).await?;
        }
        self.swap_ensure_inputs_confirmed(&proposal.maker_inputs, min_confirmations)
            .await?;
        self.swap_ensure_inputs_confirmed(&proposal.request.taker_inputs, min_confirmations)
            .await?;
        self.swap_ensure_inputs_on_chain(&all_inputs).await?;
        self.swap_sign_psbt(&mut psbt, &proposal)?;
        // the maker signs last, so this normally fails and the completion carries no finalized
        // PSBT
        let finalized_psbt = self
            .finalize_psbt(psbt.to_string(), Some(SignOptions::default()))
            .ok();
        let txid = psbt.unsigned_tx.compute_txid().to_string();
        let mut update = DbSwapActMod {
            swap_id: ActiveValue::Unchanged(swap_id.clone()),
            stage: ActiveValue::Set(OnchainSwapStage::Proposed),
            txid: ActiveValue::Set(Some(txid.clone())),
            proposal: ActiveValue::Set(Some(
                serde_json::to_string(&proposal).map_err(InternalError::from)?,
            )),
            updated_at: ActiveValue::Set(now().unix_timestamp()),
            ..Default::default()
        };
        if let Some(leg) = &outgoing {
            let batch_transfer_idx =
                self.swap_record_outgoing(&psbt, leg, offer.expiration_timestamp)?;
            update.outgoing_batch_transfer_idx = ActiveValue::Set(Some(batch_transfer_idx));
            update.outgoing_fascia = ActiveValue::Set(Some(
                serde_json::to_string(&leg.fascia).map_err(InternalError::from)?,
            ));
        }
        let completion = OnchainSwapCompletion {
            psbt: psbt.to_string(),
            finalized_psbt,
            txid,
            consignments,
            taker_history: None,
            proposal,
        };
        update.completion = ActiveValue::Set(Some(
            serde_json::to_string(&completion).map_err(InternalError::from)?,
        ));
        self.database.update_swap(&mut update)?;
        self.update_backup_info(false)?;
        self.flush().await?;
        info!(self.logger, "Complete swap proposal completed");
        Ok(completion)
    }

    /// Settle a broadcast on-chain swap as the taker.
    ///
    /// Call it once the maker has broadcast the swap transaction, with the completion the maker
    /// sent back or, when it sent none, the one this wallet signed: it must answer the proposal
    /// this wallet signed. When the taker receives RGB, the maker's consignment is validated and
    /// accepted into the wallet's RGB state, recorded as an incoming transfer that settles on
    /// refresh once confirmed, and the [`Assignment`]s received are returned. When the taker sends
    /// RGB, its prepared RGB leg is consumed into the stash and the result is empty.
    pub async fn accept_swap_transfers(
        &mut self,
        online: Online,
        completion: OnchainSwapCompletion,
        role: OnchainSwapRole,
        skip_sync: bool,
    ) -> Result<OnchainSwapReceiveResult, Error> {
        info!(self.logger, "Accepting on-chain swap transfers...");
        self.check_online(online)?;
        if role != OnchainSwapRole::Taker {
            return Err(swap_invalid("this wallet can only take swaps"));
        }
        let offer = &completion.proposal.request.offer;
        let swap_id = offer.swap_id.clone();
        let swap = self
            .database
            .get_swap(&swap_id)?
            .ok_or_else(|| swap_invalid(format!("unknown swap {swap_id}")))?;
        match swap.stage {
            OnchainSwapStage::Proposed | OnchainSwapStage::Broadcast => {}
            stage => {
                return Err(swap_invalid(format!(
                    "swap {swap_id} is {stage:?}, expected Proposed"
                )));
            }
        }
        let local: OnchainSwapCompletion =
            self.swap_load(&swap_id, |s| s.completion.as_ref(), "completion")?;
        swap_ensure_state_matches(&local.proposal, &completion.proposal, "proposal")?;
        if completion.txid != local.txid {
            return Err(swap_invalid(
                "swap completion txid does not match local state",
            ));
        }
        let direction = swap_validate_legs(&offer.maker_gives, &offer.maker_receives)?;
        swap_ensure_supported(direction)?;
        self.swap_apply_witness_tx(&completion).await?;

        let mut assignments = vec![];
        match direction {
            SwapDirection::BtcForRgb => {
                self.swap_apply_outgoing(&swap, &completion).await?;
                if !skip_sync {
                    self.sync_db_txos(false).await?;
                }
            }
            SwapDirection::RgbForBtc => {
                if !skip_sync {
                    self.sync_db_txos(false).await?;
                }
                let receives = &offer.maker_gives;
                let (_, _, taker_rgb_vout) = swap_build_psbt(&completion.proposal)?;
                let (vout, blinding, recipient_id) =
                    swap_taker_rgb_receive(&completion.proposal.request, taker_rgb_vout)?;
                let consignment = swap_received_consignment(
                    &completion.consignments,
                    receives,
                    &completion.txid,
                    vout,
                    blinding,
                    recipient_id,
                )?;
                let received = self
                    .swap_validate_received_leg(consignment, receives)
                    .await?;
                let witness_id =
                    RgbTxid::from_str(&consignment.txid).map_err(|_| Error::InvalidTxid)?;
                let contract_id = received.transfer.contract_id();
                let resolver = OffchainResolverWasm {
                    witness_id,
                    consignment: &received.transfer,
                    fallback: &received.history,
                };
                let mut runtime = self.rgb_runtime()?;
                runtime.store_secret_seal(GraphSeal::with_blinded_vout(vout, blinding))?;
                let valid_contract = received.valid_transfer.clone().into_valid_contract();
                runtime.import_contract(valid_contract.clone(), &resolver)?;
                if self.database.get_asset(contract_id.to_string())?.is_none() {
                    self.save_new_asset_internal(
                        &runtime,
                        contract_id,
                        received.asset_schema,
                        valid_contract,
                        received.valid_transfer.clone(),
                    )?;
                }
                runtime.accept_transfer(received.valid_transfer.clone(), &resolver)?;
                drop(runtime);
                let consignment_bytes = swap_decode_consignment(&consignment.consignment)?;
                self.swap_record_incoming(
                    &contract_id.to_string(),
                    &consignment.txid,
                    vout,
                    offer.rgb_output_sat,
                    recipient_id,
                    &received.assignments,
                    consignment_bytes,
                )?;
                assignments = received.assignments;
            }
            SwapDirection::RgbForRgb => unreachable!("RGB-for-RGB swaps are refused"),
        }
        self.database.update_swap(&mut DbSwapActMod {
            swap_id: ActiveValue::Unchanged(swap_id),
            stage: ActiveValue::Set(OnchainSwapStage::Accepted),
            txid: ActiveValue::Set(Some(completion.txid.clone())),
            completion: ActiveValue::Set(Some(
                serde_json::to_string(&completion).map_err(InternalError::from)?,
            )),
            updated_at: ActiveValue::Set(now().unix_timestamp()),
            ..Default::default()
        })?;
        self.update_backup_info(false)?;
        self.flush().await?;
        info!(self.logger, "Accept swap transfers completed");
        Ok(OnchainSwapReceiveResult { assignments })
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
    const ASSET_1: &str = "rgb:Ar4ouaLv-b7f7Dc_-z5EMvtu-FA5KNh1-nlae~jk-8xMBo7E";
    const ASSET_2: &str = "rgb:4bBH~Lrb-rx8sB_n-WAJLPcn-X5tFL9q-dFDGbSz-8yApPws";
    const ADDRESS: &str = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080";

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

    /// The maker gives 10 of ASSET_1 for 1_000 sat.
    fn offer(swap_id: &str) -> OnchainSwapOffer {
        OnchainSwapOffer {
            swap_id: swap_id.to_string(),
            maker_gives: leg(OnchainSwapLegKind::Rgb, Some(ASSET_1), 10),
            maker_receives: leg(OnchainSwapLegKind::Btc, None, 1_000),
            bitcoin_network: BitcoinNetwork::Regtest,
            network_fee_sat: 500,
            rgb_output_sat: 1_000,
            expiration_timestamp: None,
            maker_btc_address: Some(ADDRESS.to_string()),
            maker_rgb_recipient_id: None,
            maker_rgb_script_pubkey_hex: None,
            maker_rgb_blinding: None,
            extra_outputs: vec![],
        }
    }

    fn request(offer: OnchainSwapOffer) -> OnchainSwapRequest {
        OnchainSwapRequest {
            offer,
            taker_inputs: vec![input("aa", 0, "51".to_string())],
            taker_btc_address: None,
            taker_rgb_recipient_id: Some(s!("taker-rgb-recipient")),
            taker_rgb_script_pubkey_hex: Some(s!("51")),
            taker_rgb_blinding: Some(7),
            taker_change_script_pubkey_hex: s!("51"),
        }
    }

    fn proposal(request: OnchainSwapRequest) -> OnchainSwapProposal {
        OnchainSwapProposal {
            request,
            maker_inputs: vec![input("bb", 0, "51".to_string())],
            maker_change_script_pubkey_hex: s!("51"),
            psbt: String::new(),
            txid: String::new(),
            consignments: vec![],
            maker_history: None,
            lock_time: 0,
        }
    }

    fn completion(proposal: OnchainSwapProposal, txid: &str) -> OnchainSwapCompletion {
        OnchainSwapCompletion {
            proposal,
            psbt: String::new(),
            finalized_psbt: None,
            txid: txid.to_string(),
            consignments: vec![],
            taker_history: None,
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

    fn store_request(wallet: &Wallet, request: &OnchainSwapRequest) {
        wallet
            .database
            .update_swap(&mut DbSwapActMod {
                swap_id: ActiveValue::Unchanged(request.offer.swap_id.clone()),
                request: ActiveValue::Set(Some(serde_json::to_string(request).unwrap())),
                ..Default::default()
            })
            .unwrap();
    }

    fn store_completion(wallet: &Wallet, completion: &OnchainSwapCompletion) {
        wallet
            .database
            .update_swap(&mut DbSwapActMod {
                swap_id: ActiveValue::Unchanged(completion.proposal.request.offer.swap_id.clone()),
                completion: ActiveValue::Set(Some(serde_json::to_string(completion).unwrap())),
                ..Default::default()
            })
            .unwrap();
    }

    fn assert_invalid<T: std::fmt::Debug>(res: Result<T, Error>, details: &str) {
        let err = res.unwrap_err();
        assert!(
            matches!(&err, Error::InvalidDetails { details: d } if d.contains(details)),
            "expected InvalidDetails containing {details:?}, got {err:?}"
        );
    }

    /// A field name as the wire format spells it.
    fn key(snake: &str) -> String {
        if !cfg!(feature = "camel_case") {
            return snake.to_string();
        }
        let mut out = String::new();
        let mut upper = false;
        for c in snake.chars() {
            if c == '_' {
                upper = true;
            } else if upper {
                out.extend(c.to_uppercase());
                upper = false;
            } else {
                out.push(c);
            }
        }
        out
    }

    fn keys_of(value: &serde_json::Value) -> std::collections::BTreeSet<String> {
        value.as_object().unwrap().keys().cloned().collect()
    }

    fn expected(names: &[&str]) -> std::collections::BTreeSet<String> {
        names.iter().map(|n| key(n)).collect()
    }

    #[test]
    fn wire_types_use_the_native_field_names() {
        let mut offer = offer("s");
        offer.extra_outputs = vec![OnchainSwapExtraOutput {
            address: ADDRESS.to_string(),
            amount_sat: 500,
            funded_by: OnchainSwapRole::Taker,
        }];
        let mut proposal = proposal(request(offer));
        proposal.lock_time = 100;
        proposal.consignments = vec![OnchainSwapConsignment {
            asset_id: ASSET_1.to_string(),
            consignment: s!("AAAA"),
            txid: "cc".repeat(32),
            vout: 3,
            blinding: 7,
            recipient_id: s!("taker-rgb-recipient"),
        }];
        proposal.maker_history = Some(OnchainSwapAssetHistory {
            asset_id: ASSET_1.to_string(),
            consignment: s!("AAAA"),
            recipient_id: s!("swap-history"),
        });
        let completion = completion(proposal, &"cc".repeat(32));
        let json = serde_json::to_value(&completion).unwrap();

        assert_eq!(
            keys_of(&json),
            expected(&[
                "proposal",
                "psbt",
                "finalized_psbt",
                "txid",
                "consignments",
                "taker_history",
            ])
        );
        let proposal = &json[key("proposal")];
        assert_eq!(
            keys_of(proposal),
            expected(&[
                "request",
                "maker_inputs",
                "maker_change_script_pubkey_hex",
                "psbt",
                "txid",
                "consignments",
                "maker_history",
                "lock_time",
            ])
        );
        assert_eq!(
            keys_of(&proposal[key("consignments")][0]),
            expected(&[
                "asset_id",
                "consignment",
                "txid",
                "vout",
                "blinding",
                "recipient_id",
            ])
        );
        assert_eq!(
            keys_of(&proposal[key("maker_history")]),
            expected(&["asset_id", "consignment", "recipient_id"])
        );
        let request = &proposal[key("request")];
        assert_eq!(
            keys_of(request),
            expected(&[
                "offer",
                "taker_inputs",
                "taker_btc_address",
                "taker_rgb_recipient_id",
                "taker_rgb_script_pubkey_hex",
                "taker_rgb_blinding",
                "taker_change_script_pubkey_hex",
            ])
        );
        assert_eq!(
            keys_of(&request[key("taker_inputs")][0]),
            expected(&["outpoint", "amount_sat", "script_pubkey_hex"])
        );
        let offer = &request[key("offer")];
        assert_eq!(
            keys_of(offer),
            expected(&[
                "swap_id",
                "maker_gives",
                "maker_receives",
                "bitcoin_network",
                "network_fee_sat",
                "rgb_output_sat",
                "expiration_timestamp",
                "maker_btc_address",
                "maker_rgb_recipient_id",
                "maker_rgb_script_pubkey_hex",
                "maker_rgb_blinding",
                "extra_outputs",
            ])
        );
        assert_eq!(
            keys_of(&offer[key("maker_gives")]),
            expected(&["kind", "asset_id", "amount"])
        );
        let extra = &offer[key("extra_outputs")][0];
        assert_eq!(
            keys_of(extra),
            expected(&["address", "amount_sat", "funded_by"])
        );
        assert_eq!(extra[key("funded_by")], "Taker");
        assert_eq!(offer[key("maker_gives")]["kind"], "Rgb");

        // round trip
        let parsed: OnchainSwapCompletion = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(parsed, completion);

        // messages from before the extra outputs and the lock time parse with their defaults
        let mut old = json;
        let proposal = old[key("proposal")].as_object_mut().unwrap();
        proposal.remove(&key("lock_time"));
        proposal[&key("request")][key("offer")]
            .as_object_mut()
            .unwrap()
            .remove(&key("extra_outputs"));
        // amounts may come as strings
        proposal[&key("request")][key("offer")][key("network_fee_sat")] = "500".into();
        let parsed: OnchainSwapCompletion = serde_json::from_value(old).unwrap();
        assert_eq!(parsed.proposal.lock_time, 0);
        assert!(parsed.proposal.request.offer.extra_outputs.is_empty());
        assert_eq!(parsed.proposal.request.offer.network_fee_sat, 500);
    }

    #[test]
    fn validates_a_natively_built_proposal() {
        // what a native maker sends for an offer giving RGB for BTC, with an extra output the
        // taker funds and a lock time
        let mut offer = offer("native");
        offer.extra_outputs = vec![OnchainSwapExtraOutput {
            address: ADDRESS.to_string(),
            amount_sat: 600,
            funded_by: OnchainSwapRole::Taker,
        }];
        let mut request = request(offer);
        request.taker_inputs[0].amount_sat = 1_000 + 1_000 + 500 + 600 + 2_000;
        let mut proposal = proposal(request);
        proposal.maker_inputs[0].amount_sat = 1_000;
        proposal.lock_time = 120;
        let (psbt, maker_rgb_vout, taker_rgb_vout) = swap_build_psbt(&proposal).unwrap();
        proposal.psbt = psbt.to_string();
        proposal.txid = psbt.unsigned_tx.compute_txid().to_string();
        let json = serde_json::to_string(&proposal).unwrap();
        let received: OnchainSwapProposal = serde_json::from_str(&json).unwrap();
        let psbt = Psbt::from_str(&received.psbt).unwrap();
        swap_validate_proposal_psbt(&received, &psbt).unwrap();
        assert_eq!(maker_rgb_vout, None);
        // OP_RETURN, maker BTC, taker RGB, extra output, maker change, taker change
        assert_eq!(taker_rgb_vout, Some(2));
        let values = psbt
            .unsigned_tx
            .output
            .iter()
            .map(|o| o.value.to_sat())
            .collect::<Vec<_>>();
        assert_eq!(values, vec![0, 1_000, 1_000, 600, 1_000, 2_000]);
        assert_eq!(swap_tx_fee(&received).unwrap(), 500);
        assert_eq!(
            psbt.unsigned_tx.lock_time,
            LockTime::from_height(120).unwrap()
        );
        swap_ensure_lock_time_current(&received, 118).unwrap();
        assert!(swap_ensure_lock_time_current(&received, 116).is_err());
        // the taker pays 1_000 + 500 + 600 and gets 1_000 (RGB output) + 2_000 back
        swap_ensure_net_btc_outflow(&received, OnchainSwapRole::Taker, 5_100, 3_000).unwrap();
        assert!(
            swap_ensure_net_btc_outflow(&received, OnchainSwapRole::Taker, 5_100, 2_000).is_err()
        );

        // a maker that drops the extra output, or moves it to the maker, is refused
        let mut tampered = received.clone();
        tampered.request.offer.extra_outputs.clear();
        assert!(swap_validate_proposal_psbt(&tampered, &psbt).is_err());
        let mut tampered = received.clone();
        tampered.request.offer.extra_outputs[0].funded_by = OnchainSwapRole::Maker;
        assert!(swap_validate_proposal_psbt(&tampered, &psbt).is_err());
        let mut tampered = received;
        tampered.lock_time = 0;
        assert!(swap_validate_proposal_psbt(&tampered, &psbt).is_err());
    }

    #[test]
    fn accept_swap_offer_refuses_bad_offers() {
        let (mut wallet, online) = setup();
        let accept = |wallet: &mut Wallet, offer: OnchainSwapOffer| {
            block_on(wallet.accept_swap_offer(online.clone(), offer, 1, true, vec![]))
        };

        let mut rgb_for_rgb = offer("a");
        rgb_for_rgb.maker_receives = leg(OnchainSwapLegKind::Rgb, Some(ASSET_2), 5);
        assert_invalid(accept(&mut wallet, rgb_for_rgb), "RGB-for-RGB");

        let mut expired = offer("a");
        expired.expiration_timestamp = Some(1);
        assert_invalid(accept(&mut wallet, expired), "expired");

        let mut other_network = offer("a");
        other_network.bitcoin_network = BitcoinNetwork::Testnet;
        assert_eq!(
            accept(&mut wallet, other_network).unwrap_err(),
            Error::BitcoinNetworkMismatch
        );

        let mut dust = offer("a");
        dust.extra_outputs = vec![OnchainSwapExtraOutput {
            address: ADDRESS.to_string(),
            amount_sat: 1,
            funded_by: OnchainSwapRole::Taker,
        }];
        assert_invalid(accept(&mut wallet, dust), "dust");

        let mut no_asset = offer("a");
        no_asset.maker_gives.asset_id = None;
        assert_invalid(accept(&mut wallet, no_asset), "asset ID");

        store_swap(&wallet, "taken", OnchainSwapStage::Requested, None);
        assert_invalid(accept(&mut wallet, offer("taken")), "already accepted");
        assert_eq!(wallet.list_swaps().unwrap().len(), 1);
    }

    #[test]
    fn complete_swap_proposal_refuses_proposals_it_did_not_ask_for() {
        let (mut wallet, online) = setup();
        let complete = |wallet: &mut Wallet, proposal: OnchainSwapProposal| {
            block_on(wallet.complete_swap_proposal(online.clone(), proposal, 1, true))
        };

        assert_invalid(
            complete(&mut wallet, proposal(request(offer("missing")))),
            "unknown swap",
        );

        store_swap(&wallet, "s", OnchainSwapStage::Requested, None);
        let local = request(offer("s"));
        store_request(&wallet, &local);
        let mut other = local.clone();
        other.taker_rgb_blinding = Some(8);
        assert_invalid(
            complete(&mut wallet, proposal(other)),
            "request does not match",
        );

        // the stored request itself is checked again
        store_swap(&wallet, "expired", OnchainSwapStage::Requested, Some(1));
        let mut expired = offer("expired");
        expired.expiration_timestamp = Some(1);
        let expired = request(expired);
        store_request(&wallet, &expired);
        assert_invalid(complete(&mut wallet, proposal(expired)), "expired");

        store_swap(&wallet, "cancelled", OnchainSwapStage::Cancelled, None);
        let cancelled = request(offer("cancelled"));
        store_request(&wallet, &cancelled);
        assert_invalid(complete(&mut wallet, proposal(cancelled)), "Cancelled");
    }

    #[test]
    fn a_retried_completion_returns_the_signed_one() {
        let (mut wallet, online) = setup();
        store_swap(&wallet, "s", OnchainSwapStage::Proposed, None);
        let local = request(offer("s"));
        store_request(&wallet, &local);
        let signed = completion(proposal(local), &"cc".repeat(32));
        store_completion(&wallet, &signed);
        assert_eq!(
            block_on(wallet.complete_swap_proposal(
                online.clone(),
                signed.proposal.clone(),
                1,
                true
            ))
            .unwrap(),
            signed
        );
        let mut other = signed.proposal.clone();
        other.lock_time = 1;
        assert_invalid(
            block_on(wallet.complete_swap_proposal(online, other, 1, true)),
            "another proposal",
        );
    }

    #[test]
    fn accept_swap_transfers_needs_the_signed_completion() {
        let (mut wallet, online) = setup();
        let accept = |wallet: &mut Wallet, completion: OnchainSwapCompletion, role| {
            block_on(wallet.accept_swap_transfers(online.clone(), completion, role, true))
        };
        let signed = completion(proposal(request(offer("s"))), &"cc".repeat(32));

        assert_invalid(
            accept(&mut wallet, signed.clone(), OnchainSwapRole::Maker),
            "only take swaps",
        );
        assert_invalid(
            accept(&mut wallet, signed.clone(), OnchainSwapRole::Taker),
            "unknown swap",
        );

        store_swap(&wallet, "s", OnchainSwapStage::Requested, None);
        assert_invalid(
            accept(&mut wallet, signed.clone(), OnchainSwapRole::Taker),
            "expected Proposed",
        );

        wallet
            .database
            .update_swap(&mut DbSwapActMod {
                swap_id: ActiveValue::Unchanged(s!("s")),
                stage: ActiveValue::Set(OnchainSwapStage::Proposed),
                ..Default::default()
            })
            .unwrap();
        store_completion(&wallet, &signed);
        let mut other_proposal = signed.clone();
        other_proposal.proposal.lock_time = 1;
        assert_invalid(
            accept(&mut wallet, other_proposal, OnchainSwapRole::Taker),
            "proposal does not match",
        );
        let mut other_txid = signed;
        other_txid.txid = "dd".repeat(32);
        assert_invalid(
            accept(&mut wallet, other_txid, OnchainSwapRole::Taker),
            "txid does not match",
        );
    }

    #[test]
    fn received_consignment_must_match_the_swap_output() {
        let rgb = leg(OnchainSwapLegKind::Rgb, Some(ASSET_1), 10);
        let txid = "cc".repeat(32);
        let good = OnchainSwapConsignment {
            asset_id: ASSET_1.to_string(),
            consignment: s!("AAAA"),
            txid: txid.clone(),
            vout: 2,
            blinding: 7,
            recipient_id: s!("r"),
        };
        let pick = |consignments: &[OnchainSwapConsignment]| {
            swap_received_consignment(consignments, &rgb, &txid, 2, 7, "r").cloned()
        };
        assert_eq!(pick(std::slice::from_ref(&good)).unwrap(), good);
        // another asset's consignment is ignored
        let mut other_asset = good.clone();
        other_asset.asset_id = ASSET_2.to_string();
        assert_eq!(pick(&[other_asset, good.clone()]).unwrap(), good);
        assert_invalid(pick(&[]), "exactly one");
        assert_invalid(pick(&[good.clone(), good.clone()]), "exactly one");
        type Tamper = fn(&mut OnchainSwapConsignment);
        let tampers: [(Tamper, &str); 4] = [
            (|c| c.txid = "dd".repeat(32), "txid"),
            (|c| c.vout = 3, "vout"),
            (|c| c.blinding = 8, "blinding"),
            (|c| c.recipient_id = s!("x"), "recipient"),
        ];
        for (tamper, details) in tampers {
            let mut bad = good.clone();
            tamper(&mut bad);
            assert_invalid(pick(&[bad]), details);
        }
    }

    #[test]
    fn received_consignments_are_bounded_base64() {
        assert_invalid(swap_decode_consignment("not base64!"), "base64");
        assert_invalid(swap_decode_consignment(""), "empty");
        let too_large = "A".repeat(SWAP_MAX_CONSIGNMENT_BYTES / 3 * 4 + 8);
        assert_invalid(swap_decode_consignment(&too_large), "too large");
        assert_eq!(
            swap_decode_consignment(&general_purpose::STANDARD.encode([1u8, 2, 3])).unwrap(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn the_taker_receive_comes_from_the_request() {
        let request = request(offer("s"));
        assert_eq!(
            swap_taker_rgb_receive(&request, Some(2)).unwrap(),
            (2, 7, "taker-rgb-recipient")
        );
        assert_invalid(swap_taker_rgb_receive(&request, None), "vout");
        let mut no_blinding = request.clone();
        no_blinding.taker_rgb_blinding = None;
        assert_invalid(swap_taker_rgb_receive(&no_blinding, Some(2)), "blinding");
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
            let mut request = request(offer(swap_id));
            request.taker_inputs = vec![input(byte, 1, String::new())];
            store_request(&wallet, &request);
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
        let rgb = leg(OnchainSwapLegKind::Rgb, Some(ASSET_1), 10);
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
