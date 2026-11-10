//! RGB Rust-only methods module
//!
//! This module defines additional utility methods that are not exposed via FFI

use super::*;

/// RGB asset-specific information to color a transaction
#[derive(Clone, Debug)]
pub struct AssetColoringInfo {
    /// Map of vouts and asset amounts to color the transaction outputs
    pub output_map: HashMap<u32, u64>,
    /// Static blinding to keep the transaction construction deterministic
    pub static_blinding: Option<u64>,
}

/// RGB information to color a transaction
#[derive(Clone, Debug)]
pub struct ColoringInfo {
    /// Asset-specific information
    pub asset_info_map: HashMap<ContractId, AssetColoringInfo>,
    /// Static blinding to keep the transaction construction deterministic
    pub static_blinding: Option<u64>,
    /// Nonce for offchain TXs ordering
    pub nonce: Option<u64>,
}

/// Map of contract ID and list of its beneficiaries
pub type AssetBeneficiariesMap = BTreeMap<ContractId, Vec<BuilderSeal<GraphSeal>>>;

/// Indexer protocol
#[derive(Clone, Debug)]
pub enum IndexerProtocol {
    /// An indexer implementing the esplora protocol
    Esplora,
}

impl fmt::Display for IndexerProtocol {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Result of consignment validation (offchain or indexer-based).
#[cfg(feature = "esplora")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidateConsignmentResult {
    /// Whether the consignment is valid.
    pub valid: bool,
    /// Warnings from validation (when valid).
    pub warnings: Option<Vec<String>>,
    /// Error category (when invalid): "invalid" or "resolver".
    pub error: Option<String>,
    /// Detailed error/failure description (when invalid).
    pub details: Option<String>,
}

/// Expiration applied by `psbt_op_prepare` when the caller gives none.
pub const PSBT_OP_DEFAULT_EXPIRATION_SECS: u64 = 24 * 60 * 60;

const PSBT_OP_ID_LEN: usize = 32;

/// When a prepared PSBT operation may be failed by the expiry sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PsbtOpExpiry {
    /// [`PSBT_OP_DEFAULT_EXPIRATION_SECS`] from now.
    Default,
    /// At this unix timestamp.
    At(u64),
    /// Never: the caller broadcasts the transaction itself and applies or aborts the operation
    /// explicitly, so an operation whose transaction the indexer hasn't seen yet is never failed.
    Never,
}

impl From<Option<u64>> for PsbtOpExpiry {
    fn from(expiration_timestamp: Option<u64>) -> Self {
        expiration_timestamp.map_or(Self::Default, Self::At)
    }
}

/// Status of a PSBT operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PsbtOperationStatus {
    /// Colored PSBT stored; RGB stash not updated.
    Prepared,
    /// Fascia consumed into the RGB stash after broadcast.
    Applied,
    /// Aborted before apply (never broadcast).
    Failed,
    /// Reconciled as settled against the batch transfer.
    Settled,
}

/// Caller-reported / indexer-observed broadcast lifecycle of a PSBT operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PsbtBroadcastState {
    /// Caller has not reported a broadcast attempt.
    #[default]
    NotAttempted,
    /// Caller reported a broadcast attempt; indexer has not confirmed the tx.
    Attempted,
    /// Indexer has seen the witness tx.
    Observed,
    /// Broadcast was attempted and the indexer currently does not see the tx.
    Ambiguous,
}

/// One assignment a PSBT operation's RGB transitions make, read back from its committed fascia
/// (not from the [`ColoringInfo`] the operation was prepared with).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct PsbtOpAllocation {
    /// Contract the assignment belongs to, in its canonical spelling.
    pub asset_id: String,
    /// Output of the witness transaction the assignment is sealed to; `None` when the seal is
    /// concealed or points to another transaction.
    pub vout: Option<u32>,
    /// The assigned state.
    pub assignment: Assignment,
}

/// Result of [`Wallet::psbt_op_prepare`].
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct PsbtOpPrepareResult {
    /// Opaque operation ID.
    pub operation_id: String,
    /// Colored unsigned PSBT (caller signs and broadcasts).
    pub colored_psbt: String,
    /// What the operation's RGB transitions assign, read back from the committed fascia.
    pub allocations: Vec<PsbtOpAllocation>,
}

/// A stored PSBT operation (lookup after a lost `psbt_op_prepare` response).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct PsbtOperation {
    /// Opaque operation ID.
    pub operation_id: String,
    /// Colored unsigned PSBT.
    pub colored_psbt: String,
    /// High-level operation status.
    pub status: PsbtOperationStatus,
    /// Broadcast lifecycle.
    pub broadcast: PsbtBroadcastState,
    /// Witness txid committed at prepare.
    pub txid: String,
    /// What the operation's RGB transitions assign, read back from the committed fascia.
    pub allocations: Vec<PsbtOpAllocation>,
}

/// Allocations a PSBT operation spends from an outpoint this wallet doesn't hold.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PsbtOpForeignInput {
    /// Contract ID
    pub asset_id: String,
    /// The spent outpoint
    pub outpoint: Outpoint,
    /// What it carries
    pub assignments: Vec<Assignment>,
}

/// Caller intent for HTLC / special accept paths.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct ExpectedTransfer {
    /// Expected contract / asset ID
    pub asset_id: String,
    /// Expected RGB schema
    pub asset_schema: AssetSchema,
    /// Expected assignment type and amount (`Any` is rejected)
    pub assignment: Assignment,
}

/// A consignment fetched from a proxy, before any validation.
///
/// **Unchecked:** `txid` and `vout` are what the proxy reported. Pass it to
/// [`Wallet::accept_transfer_pinned`], which pins the witness output before accepting.
#[cfg(feature = "esplora")]
#[derive(Debug, Clone)]
pub struct FetchedConsignment {
    /// The consignment.
    pub consignment: RgbTransfer,
    /// Witness transaction ID reported by the proxy.
    pub txid: String,
    /// Witness output reported by the proxy.
    pub vout: u32,
}

/// Result of [`Wallet::accept_transfer_pinned`].
#[cfg(feature = "esplora")]
#[derive(Debug, Clone)]
pub struct AcceptedTransfer {
    /// The accepted consignment.
    pub consignment: RgbTransfer,
    /// Assignments received on the pinned witness output.
    pub assignments: Vec<Assignment>,
    /// The pinned witness output.
    pub outpoint: Outpoint,
}

#[cfg(feature = "esplora")]
type SpentByContract = HashMap<ContractId, HashMap<OutPoint, Vec<Assignment>>>;

/// What `fascia`'s transitions assign. Every owned-state type the wallet's schemas define is
/// reported; any other is an error rather than left out.
#[cfg(feature = "esplora")]
pub(crate) fn fascia_allocations(fascia: &Fascia) -> Result<Vec<PsbtOpAllocation>, Error> {
    fn witness_vout(seal: Option<GraphSeal>) -> Option<u32> {
        seal.filter(|seal| seal.txid == TxPtr::WitnessTx)
            .map(|seal| seal.vout.into_u32())
    }
    let mut allocations = vec![];
    for (contract_id, bundle) in fascia.bundles() {
        let asset_id = contract_id.to_string();
        let unknown = |ass_type: &rgbstd::AssignmentType| Error::Inconsistency {
            details: format!("fascia for {asset_id} assigns unknown owned state {ass_type}"),
        };
        for KnownTransition { transition, .. } in bundle.known_transitions.iter() {
            for (ass_type, typed_assigns) in transition.assignments.iter() {
                let mut found = vec![];
                for assign in typed_assigns.as_fungible() {
                    let amount = assign.as_revealed_state().as_u64();
                    let assignment =
                        Assignment::fungible(*ass_type, amount).ok_or_else(|| unknown(ass_type))?;
                    found.push((witness_vout(assign.revealed_seal()), assignment));
                }
                for assign in typed_assigns.as_structured() {
                    found.push((
                        witness_vout(assign.revealed_seal()),
                        Assignment::NonFungible,
                    ));
                }
                if !typed_assigns.as_declarative().is_empty() {
                    return Err(unknown(ass_type));
                }
                allocations.extend(
                    found
                        .into_iter()
                        .map(|(vout, assignment)| PsbtOpAllocation {
                            asset_id: asset_id.clone(),
                            vout,
                            assignment,
                        }),
                );
            }
        }
    }
    allocations.sort();
    Ok(allocations)
}

fn psbt_has_input_signatures(psbt: &Psbt) -> bool {
    psbt.inputs.iter().any(|input| {
        !input.partial_sigs.is_empty()
            || input.tap_key_sig.is_some()
            || !input.tap_script_sigs.is_empty()
            || input.final_script_sig.is_some()
            || input.final_script_witness.is_some()
    })
}

/// Check that `output[vout]` of the witness transaction pays the script of `witness_recipient_id`.
#[cfg(feature = "esplora")]
fn check_witness_output(
    tx: &BdkTransaction,
    vout: u32,
    witness_recipient_id: &str,
    chain_net: ChainNet,
) -> Result<(), Error> {
    let xchainnet_beneficiary = XChainNet::<Beneficiary>::from_str(witness_recipient_id)
        .map_err(|_| Error::InvalidRecipientID)?;
    if xchainnet_beneficiary.chain_network() != chain_net {
        return Err(Error::InvalidRecipientNetwork);
    }
    let expected_script = match xchainnet_beneficiary.into_inner() {
        Beneficiary::WitnessVout(pay_2_vout, _) => pay_2_vout.to_script(),
        Beneficiary::BlindedSeal(_) => return Err(Error::InvalidRecipientID),
    };
    let Some(output) = tx.output.get(vout as usize) else {
        return Err(Error::WitnessOutputMismatch {
            details: format!("witness vout {vout} out of range for transaction"),
        });
    };
    if output.script_pubkey != expected_script {
        return Err(Error::WitnessOutputMismatch {
            details: s!("witness output script does not match witness_recipient_id"),
        });
    }
    Ok(())
}

#[cfg(feature = "esplora")]
fn validate_expected_transfer_intent(
    consignment: &RgbTransfer,
    asset_schema: &AssetSchema,
    expected: &ExpectedTransfer,
) -> Result<(), Error> {
    if matches!(expected.assignment, Assignment::Any) {
        return Err(Error::InvalidAssignment);
    }
    let contract_id = consignment.contract_id().to_string();
    if contract_id != expected.asset_id {
        return Err(Error::UnexpectedTransfer {
            details: format!(
                "contract id mismatch: got {contract_id}, expected {}",
                expected.asset_id
            ),
        });
    }
    if asset_schema != &expected.asset_schema {
        return Err(Error::UnexpectedTransfer {
            details: format!(
                "schema mismatch: got {asset_schema}, expected {}",
                expected.asset_schema
            ),
        });
    }
    Ok(())
}

#[cfg(feature = "esplora")]
fn validate_expected_transfer_assignments(
    received: &[Assignment],
    expected: &ExpectedTransfer,
) -> Result<(), Error> {
    if received.is_empty() {
        return Err(Error::UnexpectedTransfer {
            details: s!("no assignments for the given witness seal"),
        });
    }
    if received != [expected.assignment.clone()] {
        return Err(Error::UnexpectedTransfer {
            details: format!(
                "assignment mismatch: got {received:?}, expected {:?}",
                expected.assignment
            ),
        });
    }
    Ok(())
}

/// Whether a prepared operation may be rolled back, given what the indexer reports and how far its
/// broadcast got. `Err` carries why not; the state transition it implies is applied by the caller.
fn psbt_op_abort_decision(
    row: &DbPsbtOp,
    tx_seen: bool,
    batch_expired: bool,
) -> Result<(), (Option<PsbtBroadcastState>, Error)> {
    if tx_seen {
        return Err((
            Some(PsbtBroadcastState::Observed).filter(|b| *b != row.broadcast),
            Error::InvalidPsbtOperationStatus {
                details: format!(
                    "witness tx {} is known to the indexer; refuse abort",
                    row.txid
                ),
            },
        ));
    }
    if row.broadcast != PsbtBroadcastState::NotAttempted && !batch_expired {
        let next = (row.broadcast == PsbtBroadcastState::Attempted)
            .then_some(PsbtBroadcastState::Ambiguous);
        return Err((
            next,
            Error::InvalidPsbtOperationStatus {
                details: format!(
                    "operation {} broadcast is {:?}; automatic rollback is refused until the batch transfer expires",
                    row.operation_id, row.broadcast
                ),
            },
        ));
    }
    Ok(())
}

impl Wallet {
    /// Return all contract IDs currently held in the RGB stock.
    pub fn rgb_contract_ids(&self) -> Result<Vec<ContractId>, Error> {
        let runtime = self.rgb_runtime()?;
        Ok(runtime
            .contracts()?
            .into_iter()
            .map(|contract| contract.id)
            .collect())
    }

    /// Color a PSBT.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub fn color_psbt(
        &self,
        psbt: &mut Psbt,
        coloring_info: ColoringInfo,
    ) -> Result<(Fascia, AssetBeneficiariesMap), Error> {
        info!(self.logger, "Coloring PSBT...");
        let shift_output_map_for_opreturn_first = self.prepare_psbt_for_coloring(psbt)?;
        let runtime = self.rgb_runtime()?;
        let prev_outputs = psbt
            .unsigned_tx
            .input
            .iter()
            .map(|txin| txin.previous_output)
            .collect::<HashSet<OutPoint>>();
        let static_blinding = coloring_info.static_blinding;
        let asset_beneficiaries = self.color_psbt_stage_runtime(
            &runtime,
            psbt,
            coloring_info,
            prev_outputs,
            false,
            shift_output_map_for_opreturn_first,
        )?;
        let fascia = self.color_psbt_finalize(psbt, static_blinding)?;
        info!(self.logger, "Color PSBT completed");
        Ok((fascia, asset_beneficiaries))
    }

    /// Insert the OP_RETURN output if the PSBT has none.
    ///
    /// Returns whether `output_map` keys should be +1-shifted: `true` when any output is P2TR, as
    /// the OP_RETURN is then placed first.
    fn prepare_psbt_for_coloring(&self, psbt: &mut Psbt) -> Result<bool, Error> {
        let mut transaction = match psbt.clone().extract_tx() {
            Ok(tx) => tx,
            Err(ExtractTxError::MissingInputValue { tx }) => tx, // required for non-standard TXs
            Err(e) => return Err(InternalError::from(e).into()),
        };
        let mut opreturn_first = false;
        if transaction.output.iter().any(|o| o.script_pubkey.is_p2tr()) {
            opreturn_first = true;
        }

        if !transaction
            .output
            .iter()
            .any(|o| o.script_pubkey.is_op_return())
        {
            let opreturn_output = TxOut {
                value: BdkAmount::ZERO,
                script_pubkey: ScriptBuf::new_op_return([]),
            };
            if opreturn_first {
                transaction.output.insert(0, opreturn_output);
            } else {
                transaction.output.push(opreturn_output);
            }
            *psbt = Psbt::from_unsigned_tx(transaction).unwrap();
        }

        Ok(opreturn_first)
    }

    fn color_psbt_stage_runtime(
        &self,
        runtime: &RgbRuntime,
        psbt: &mut Psbt,
        coloring_info: ColoringInfo,
        prev_outputs: HashSet<OutPoint>,
        require_full_allocation: bool,
        shift_output_map_for_opreturn_first: bool,
    ) -> Result<AssetBeneficiariesMap, Error> {
        let mut all_transitions: HashMap<ContractId, Transition> = HashMap::new();
        let mut asset_beneficiaries: AssetBeneficiariesMap = bmap![];
        let assignment_name = FieldName::from(RGB_STATE_ASSET_OWNER);

        for (contract_id, asset_coloring_info) in coloring_info.asset_info_map.clone() {
            let schema = AssetSchema::get_from_contract_id(contract_id, runtime)?;

            let mut asset_transition_builder =
                runtime.transition_builder(contract_id, "transfer")?;

            let mut asset_available_amt: u64 = 0;
            for (_, opout_state_map) in
                runtime.contract_assignments_for(contract_id, prev_outputs.iter().copied())?
            {
                for (opout, state) in opout_state_map {
                    // only the asset-owner assignment is re-assigned below, so a right on a spent
                    // input would be destroyed (and an inflation right counted as spendable value)
                    if require_full_allocation && opout.ty != schemata::OS_ASSET {
                        let right = if opout.ty == schemata::OS_INFLATION {
                            "an inflation right"
                        } else {
                            "a non-asset assignment"
                        };
                        return Err(Error::InvalidColoringInfo {
                            details: format!(
                                "a PSBT input carries {right} for contract {contract_id}, which this method cannot re-assign"
                            ),
                        });
                    }
                    if let AllocatedState::Amount(amt) = &state {
                        asset_available_amt = asset_available_amt
                            .checked_add(amt.as_u64())
                            .expect("total available asset amount cannot exceed u64::MAX");
                    }
                    asset_transition_builder = asset_transition_builder.add_input(opout, state)?;
                }
            }

            let mut beneficiaries = vec![];
            let mut output_seals: Vec<(BuilderSeal<GraphSeal>, u64)> = vec![];
            let mut sending_amt: u64 = 0;
            // ascending vouts keep errors and beneficiary order stable between identical calls
            for (mut vout, amount) in BTreeMap::from_iter(asset_coloring_info.output_map) {
                if amount == 0 {
                    continue;
                }
                if shift_output_map_for_opreturn_first {
                    vout = vout
                        .checked_add(1)
                        .ok_or_else(|| Error::InvalidColoringInfo {
                            details: s!("vout in output_map is too large"),
                        })?;
                }
                sending_amt =
                    sending_amt
                        .checked_add(amount)
                        .ok_or_else(|| Error::InvalidColoringInfo {
                            details: s!("total amount in output_map exceeds u64::MAX"),
                        })?;
                if vout as usize >= psbt.unsigned_tx.output.len() {
                    return Err(Error::InvalidColoringInfo {
                        details: s!("invalid vout in output_map, does not exist in the given PSBT"),
                    });
                }
                if psbt.unsigned_tx.output[vout as usize]
                    .script_pubkey
                    .is_op_return()
                {
                    return Err(Error::InvalidColoringInfo {
                        details: format!("output_map vout {vout} points to the OP_RETURN output"),
                    });
                }
                let graph_seal = if let Some(blinding) = asset_coloring_info.static_blinding {
                    GraphSeal::with_blinded_vout(vout, blinding)
                } else {
                    GraphSeal::new_random_vout(vout)
                };
                let seal = BuilderSeal::Revealed(graph_seal);
                beneficiaries.push(seal);
                output_seals.push((seal, amount));
            }
            if sending_amt > asset_available_amt {
                return Err(Error::InvalidColoringInfo {
                    details: format!(
                        "total amount in output_map ({sending_amt}) greater than available ({asset_available_amt})"
                    ),
                });
            }
            if require_full_allocation && sending_amt < asset_available_amt {
                return Err(Error::InvalidColoringInfo {
                    details: format!(
                        "total amount in output_map ({sending_amt}) less than available ({asset_available_amt}); full allocation required"
                    ),
                });
            }

            for (seal, amount) in output_seals {
                match schema {
                    AssetSchema::Nia | AssetSchema::Ifa => {
                        asset_transition_builder = asset_transition_builder.add_fungible_state(
                            assignment_name.clone(),
                            seal,
                            amount,
                        )?;
                    }
                }
            }

            if let Some(nonce) = coloring_info.nonce {
                asset_transition_builder = asset_transition_builder.set_nonce(nonce);
            }

            let transition = asset_transition_builder.complete_transition()?;
            all_transitions.insert(contract_id, transition);
            asset_beneficiaries.insert(contract_id, beneficiaries);
        }

        for (contract_id, transition) in all_transitions {
            for opout in transition.inputs() {
                psbt.set_rgb_contract_consumer(contract_id, opout, transition.id())
                    .map_err(InternalError::from)?;
            }
            psbt.push_rgb_transition(transition)
                .map_err(InternalError::from)?;
        }

        info!(self.logger, "Stage RGB coloring completed");
        Ok(asset_beneficiaries)
    }

    /// Finalize the RGB commitment on a PSBT that has had one or more contracts staged via
    /// [`Wallet::color_psbt_stage`]. Must be called exactly once per swap PSBT, after all staging
    /// is complete.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub fn color_psbt_finalize(
        &self,
        psbt: &mut Psbt,
        static_blinding: Option<u64>,
    ) -> Result<Fascia, Error> {
        info!(self.logger, "Finalizing RGB commitment on PSBT...");
        let opreturn_index = psbt
            .unsigned_tx
            .output
            .iter()
            .enumerate()
            .find(|(_, o)| o.script_pubkey.is_op_return())
            .ok_or_else(|| Error::InvalidColoringInfo {
                details: s!("PSBT has no OP_RETURN output to host the RGB commitment"),
            })?
            .0;
        let opreturn_output = psbt.outputs.get_mut(opreturn_index).unwrap();
        opreturn_output.set_opret_host();
        if let Some(blinding) = static_blinding {
            opreturn_output
                .set_mpc_entropy(blinding)
                .map_err(InternalError::from)?;
        }

        psbt.set_rgb_close_method(CloseMethod::OpretFirst);
        psbt.set_as_unmodifiable();
        let fascia = psbt.rgb_commit().map_err(|e| Error::Internal {
            details: e.to_string(),
        })?;

        info!(self.logger, "Finalize RGB commitment completed");
        Ok(fascia)
    }

    /// Stage RGB coloring information onto a PSBT without finalizing the MPC commitment.
    ///
    /// Multiple parties may each call this method on the same PSBT (each for their own contracts),
    /// in which case [`Wallet::color_psbt_finalize`] must be invoked once after all staging is
    /// complete to produce the [`Fascia`]. `output_map` indexing follows [`Self::color_psbt`].
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub fn color_psbt_stage(
        &self,
        psbt: &mut Psbt,
        coloring_info: ColoringInfo,
    ) -> Result<AssetBeneficiariesMap, Error> {
        info!(self.logger, "Staging RGB coloring onto PSBT...");
        let prev_outputs = psbt
            .unsigned_tx
            .input
            .iter()
            .map(|txin| txin.previous_output)
            .collect::<HashSet<OutPoint>>();
        let shift_output_map_for_opreturn_first = self.prepare_psbt_for_coloring(psbt)?;
        let runtime = self.rgb_runtime()?;
        self.color_psbt_stage_runtime(
            &runtime,
            psbt,
            coloring_info,
            prev_outputs,
            false,
            shift_output_map_for_opreturn_first,
        )
    }

    /// Stage coloring spending only `prev_outputs`, requiring every allocation they carry to be
    /// reassigned (no implicit burn).
    pub(crate) fn color_psbt_stage_for_inputs(
        &self,
        psbt: &mut Psbt,
        coloring_info: ColoringInfo,
        prev_outputs: HashSet<OutPoint>,
    ) -> Result<AssetBeneficiariesMap, Error> {
        let runtime = self.rgb_runtime()?;
        self.reject_uncolored_input_contracts(&runtime, &prev_outputs, &coloring_info)?;
        let shift_output_map_for_opreturn_first = self.prepare_psbt_for_coloring(psbt)?;
        self.color_psbt_stage_runtime(
            &runtime,
            psbt,
            coloring_info,
            prev_outputs,
            true,
            shift_output_map_for_opreturn_first,
        )
    }

    /// Reject coloring that would silently burn the allocations of a contract that is spent by
    /// `inputs` but not listed in `coloring_info`.
    fn reject_uncolored_input_contracts(
        &self,
        runtime: &RgbRuntime,
        inputs: &HashSet<OutPoint>,
        coloring_info: &ColoringInfo,
    ) -> Result<(), Error> {
        let colored_contracts: BTreeSet<ContractId> =
            coloring_info.asset_info_map.keys().copied().collect();
        let assigning = runtime.contracts_assigning(inputs.iter().copied())?;
        let uncolored: Vec<ContractId> =
            assigning.difference(&colored_contracts).copied().collect();
        if !uncolored.is_empty() {
            let contracts = uncolored
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Error::InvalidColoringInfo {
                details: format!(
                    "PSBT inputs carry RGB allocations for contracts not listed in coloring_info: {contracts}"
                ),
            });
        }
        Ok(())
    }

    /// Produce an [`RgbTransfer`] consignment for a single contract given its beneficiaries
    /// (typically obtained from a prior [`Wallet::color_psbt_stage`] call) and the witness
    /// transaction ID of the PSBT being colored, built from the colored PSBT's `fascia`.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub fn generate_transfer(
        &self,
        contract_id: ContractId,
        beneficiaries: Vec<BuilderSeal<GraphSeal>>,
        witness_txid: RgbTxid,
        fascia: &Fascia,
    ) -> Result<RgbTransfer, Error> {
        let runtime = self.rgb_runtime()?;
        let mut beneficiaries_witness = vec![];
        let mut beneficiaries_blinded = vec![];
        for builder_seal in beneficiaries {
            match builder_seal {
                BuilderSeal::Revealed(seal) => {
                    let explicit_seal = ExplicitSeal::with(witness_txid, seal.vout);
                    beneficiaries_witness.push(explicit_seal);
                }
                BuilderSeal::Concealed(secret_seal) => {
                    beneficiaries_blinded.push(secret_seal);
                }
            };
        }
        Ok(runtime.transfer_from_fascia(
            contract_id,
            beneficiaries_witness,
            beneficiaries_blinded,
            fascia,
        )?)
    }

    /// Color a PSBT, consume the RGB fascia and return the related consignment.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub async fn color_psbt_and_consume(
        &self,
        psbt: &mut Psbt,
        coloring_info: ColoringInfo,
    ) -> Result<Vec<RgbTransfer>, Error> {
        info!(self.logger, "Coloring PSBT and consuming...");
        let (fascia, asset_beneficiaries) = self.color_psbt(psbt, coloring_info.clone())?;

        let witness_txid = psbt.get_txid();

        let mut runtime = self.rgb_runtime()?;
        runtime.consume_fascia(fascia, None)?;

        let mut transfers = vec![];
        for (contract_id, beneficiaries) in asset_beneficiaries {
            let mut beneficiaries_witness = vec![];
            let mut beneficiaries_blinded = vec![];
            for builder_seal in beneficiaries {
                match builder_seal {
                    BuilderSeal::Revealed(seal) => {
                        let explicit_seal = ExplicitSeal::with(witness_txid, seal.vout);
                        beneficiaries_witness.push(explicit_seal);
                    }
                    BuilderSeal::Concealed(secret_seal) => {
                        beneficiaries_blinded.push(secret_seal);
                    }
                };
            }
            transfers.push(runtime.transfer(
                contract_id,
                beneficiaries_witness,
                beneficiaries_blinded,
                Some(witness_txid),
            )?);
        }
        drop(runtime);
        self.flush().await?;

        info!(self.logger, "Color PSBT and consume completed");
        Ok(transfers)
    }

    /// Consume an RGB fascia.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub fn consume_fascia_in_memory(
        &self,
        fascia: Fascia,
        witness_ord: Option<WitnessOrd>,
    ) -> Result<(), Error> {
        let mut runtime = self.rgb_runtime()?;
        runtime.consume_fascia(fascia, witness_ord)?;
        Ok(())
    }

    /// Consume an RGB fascia and durably flush the updated stock.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub async fn consume_fascia(
        &self,
        fascia: Fascia,
        witness_ord: Option<WitnessOrd>,
    ) -> Result<(), Error> {
        info!(self.logger, "Consuming fascia...");
        self.consume_fascia_in_memory(fascia, witness_ord)?;
        self.flush().await?;
        info!(self.logger, "Consume fascia completed");
        Ok(())
    }

    /// Manually set the [`WitnessOrd`] of a witness TX.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    #[cfg(feature = "esplora")]
    pub fn upsert_witness(
        &self,
        witness_id: RgbTxid,
        witness_ord: WitnessOrd,
    ) -> Result<(), Error> {
        let mut runtime = self.rgb_runtime()?;
        runtime.upsert_witness(witness_id, witness_ord)?;
        Ok(())
    }

    /// Return `true` if a batch transfer with the given `txid` exists in the wallet database
    /// and is not in `Failed` status. Used to detect idempotent replay of `send_end` after a
    /// crash-inject reload: if the transfer is already present with a non-failed status, the
    /// broadcast already happened and the caller may treat the operation as succeeded.
    pub fn is_batch_transfer_sent(&self, txid: &str) -> Result<bool, Error> {
        let batch_transfers = self.database.iter_batch_transfers()?;
        Ok(batch_transfers
            .iter()
            .any(|bt| bt.txid.as_deref() == Some(txid) && bt.status != TransferStatus::Failed))
    }

    /// Return the current `Online` handle if the wallet has gone online, or `None` otherwise.
    pub fn get_online(&self) -> Option<Online> {
        self.online_data.as_ref().map(|od| Online {
            id: od.id,
            indexer_url: od.indexer_url.clone(),
        })
    }

    #[cfg(feature = "esplora")]
    pub(crate) fn save_new_asset_internal(
        &self,
        runtime: &RgbRuntime,
        contract_id: ContractId,
        asset_schema: AssetSchema,
        valid_contract: ValidContract,
        valid_transfer: ValidTransfer,
    ) -> Result<(), Error> {
        let timestamp = valid_contract.genesis.timestamp;
        let local_asset_data = match &asset_schema {
            AssetSchema::Nia => {
                let contract = runtime.contract_wrapper::<NonInflatableAsset>(contract_id)?;
                let spec = contract.spec();
                let ticker = spec.ticker().to_string();
                let name = spec.name().to_string();
                let details = spec.details().map(|d| d.to_string());
                let precision = spec.precision.into();
                let initial_supply = contract.total_issued_supply().into();
                let media_idx = if let Some(attachment) = contract.contract_terms().media {
                    Some(self.get_or_insert_media(
                        hex::encode(attachment.digest),
                        attachment.ty.to_string(),
                    )?)
                } else {
                    None
                };
                LocalAssetData {
                    name,
                    precision,
                    ticker: Some(ticker),
                    details,
                    media_idx,
                    initial_supply,
                    max_supply: None,
                    known_circulating_supply: None,
                    reject_list_url: None,
                }
            }
            AssetSchema::Ifa => {
                let contract = runtime.contract_wrapper::<InflatableFungibleAsset>(contract_id)?;
                let spec = contract.spec();
                let ticker = spec.ticker().to_string();
                let name = spec.name().to_string();
                let details = spec.details().map(|d| d.to_string());
                let precision = spec.precision.into();
                let media_idx = if let Some(attachment) = contract.contract_terms().media {
                    Some(self.get_or_insert_media(
                        hex::encode(attachment.digest),
                        attachment.ty.to_string(),
                    )?)
                } else {
                    None
                };
                let initial_supply = contract.total_issued_supply().into();
                let max_supply = contract.max_supply().into();
                let known_circulating_supply = IfaWrapper::with(valid_transfer.contract_data())
                    .total_issued_supply()
                    .into();
                let reject_list_url = contract.reject_list_url().map(|u| u.to_string());
                LocalAssetData {
                    name,
                    precision,
                    ticker: Some(ticker),
                    details,
                    media_idx,
                    initial_supply,
                    max_supply: Some(max_supply),
                    known_circulating_supply: Some(known_circulating_supply),
                    reject_list_url,
                }
            }
        };

        self.add_asset_to_db(
            contract_id.to_string(),
            &asset_schema,
            None,
            timestamp,
            local_asset_data,
        )?;

        Ok(())
    }

    /// Return the consignment file path for a send transfer of an asset.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub fn get_send_consignment_path(&self, asset_id: &str, transfer_id: &str) -> PathBuf {
        let transfer_dir = self.get_transfer_dir(transfer_id);
        let asset_transfer_dir = self.get_asset_transfer_dir(transfer_dir, asset_id);
        asset_transfer_dir.join(CONSIGNMENT_FILE)
    }

    /// Post a consignment to the proxy server.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    #[cfg(feature = "esplora")]
    pub async fn post_consignment(
        &self,
        proxy_url: &str,
        recipient_id: String,
        consignment_bytes: &[u8],
        txid: String,
        vout: Option<u32>,
    ) -> Result<(), Error> {
        info!(self.logger, "Posting consignment...");
        let consignment_res = self
            .wasm_proxy_client
            .post_consignment(
                proxy_url,
                recipient_id.clone(),
                consignment_bytes,
                txid.clone(),
                vout,
            )
            .await?;
        debug!(
            self.logger,
            "Consignment POST response: {:?}", consignment_res
        );

        if let Some(err) = consignment_res.error {
            if err.code == -101 {
                return Err(Error::RecipientIDAlreadyUsed);
            }
            return Err(Error::InvalidTransportEndpoint {
                details: format!("proxy error: {}", err.message),
            });
        }
        if consignment_res.result.is_none() {
            return Err(Error::InvalidTransportEndpoint {
                details: s!("invalid result"),
            });
        }

        info!(self.logger, "Post consignment completed");
        Ok(())
    }

    /// Get the height at which a transaction was mined.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    #[cfg(feature = "esplora")]
    pub async fn get_tx_height(&self, online: Online, txid: String) -> Result<Option<u32>, Error> {
        info!(self.logger, "Getting TX height...");
        self.check_online(online)?;
        let _ = RgbTxid::from_str(&txid).map_err(|_| Error::InvalidTxid)?;
        let height = self.indexer().get_tx_height(&txid).await?;
        info!(self.logger, "Get TX height completed");
        Ok(height)
    }

    /// Accept an RGB transfer by retrieving and validating its consignment from a proxy server.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    #[cfg(feature = "esplora")]
    pub async fn accept_transfer(
        &mut self,
        online: Online,
        txid: String,
        vout: u32,
        consignment_endpoint: RgbTransport,
        blinding: u64,
    ) -> Result<(RgbTransfer, Vec<Assignment>), Error> {
        info!(self.logger, "Accepting transfer...");
        self.check_online(online)?;
        let witness_id = RgbTxid::from_str(&txid).map_err(|_| Error::InvalidTxid)?;
        let proxy_url = TransportEndpoint::try_from(consignment_endpoint)?.endpoint;

        let consignment_res = self
            ._get_consignment_async(&proxy_url, txid.clone())
            .await?;
        let consignment_bytes = general_purpose::STANDARD
            .decode(consignment_res.consignment)
            .map_err(InternalError::from)?;
        let consignment = RgbTransfer::load(&consignment_bytes[..]).map_err(InternalError::from)?;

        let schema_id = consignment.schema_id().to_string();
        let asset_schema: AssetSchema = schema_id.try_into()?;
        self.check_schema_support(&asset_schema)?;
        debug!(
            self.logger,
            "Got consignment for asset with {} schema", asset_schema
        );

        let mut runtime = self.rgb_runtime()?;

        let graph_seal = GraphSeal::with_blinded_vout(vout, blinding);
        runtime.store_secret_seal(graph_seal)?;

        let wasm_resolver =
            crate::utils::WasmResolver::from_consignment(&consignment, self.chain_net());
        let resolver = crate::utils::OffchainResolverWasm {
            witness_id,
            consignment: &consignment,
            fallback: &wasm_resolver,
        };

        debug!(self.logger, "Validating consignment...");
        let asset_schema: AssetSchema = consignment.schema_id().try_into()?;
        let trusted_typesystem = asset_schema.types();
        let validation_config = ValidationConfig {
            chain_net: self.chain_net(),
            trusted_typesystem,
            ..Default::default()
        };
        let valid_consignment = match consignment.clone().validate(&resolver, &validation_config) {
            Ok(consignment) => consignment,
            Err(ValidationError::InvalidConsignment(e)) => {
                error!(self.logger, "Consignment is invalid: {}", e);
                return Err(Error::InvalidConsignment);
            }
            Err(ValidationError::ResolverError(e)) => {
                warn!(self.logger, "Network error during consignment validation");
                return Err(Error::Network {
                    details: e.to_string(),
                });
            }
        };
        let validity = valid_consignment.validation_status().validity();
        debug!(self.logger, "Consignment validity: {:?}", validity);

        let valid_contract = valid_consignment.clone().into_valid_contract();
        runtime
            .import_contract(valid_contract, &resolver)
            .expect("failure importing validated contract");

        let received_rgb_assignments =
            self.extract_received_assignments(&consignment, witness_id, Some(vout), None);

        runtime.accept_transfer(valid_consignment, &resolver)?;
        drop(runtime);
        self.flush().await?;

        info!(self.logger, "Accept transfer completed");
        Ok((
            consignment,
            received_rgb_assignments.into_values().collect(),
        ))
    }

    /// Update RGB witnesses.
    ///
    /// Pre-fetches witness data from esplora before calling the sync RGB stock method.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    #[cfg(feature = "esplora")]
    pub async fn update_witnesses(
        &mut self,
        online: Online,
        after_height: u32,
        force_witnesses: Vec<RgbTxid>,
    ) -> Result<UpdateRes, Error> {
        info!(self.logger, "Updating witnesses...");
        self.check_online(online)?;

        let mut cache = HashMap::new();
        for witness_id in &force_witnesses {
            let txid_str = witness_id.to_string();
            let txid = Txid::from_str(&txid_str).map_err(|_| Error::InvalidTxid)?;
            if let Some((tx, block_height, block_time)) =
                self.indexer().get_tx_with_status(&txid).await?
            {
                let witness_ord = match block_height.zip(block_time) {
                    Some((h, t)) => {
                        if let Some(height) = NonZeroU32::new(h) {
                            if let Some(pos) = WitnessPos::bitcoin(height, t as i64) {
                                WitnessOrd::Mined(pos)
                            } else {
                                WitnessOrd::Tentative
                            }
                        } else {
                            WitnessOrd::Tentative
                        }
                    }
                    None => WitnessOrd::Tentative,
                };
                cache.insert(*witness_id, WitnessStatus::Resolved(tx, witness_ord));
            }
        }

        let resolver = crate::utils::PreFetchResolver::new(cache, self.chain_net());
        let update_res =
            self.rgb_runtime()?
                .update_witnesses(&resolver, after_height, force_witnesses)?;

        info!(self.logger, "Update witnesses completed");
        Ok(update_res)
    }
}

#[cfg(feature = "esplora")]
impl Wallet {
    fn new_psbt_op_id() -> String {
        let mut bytes = [0u8; PSBT_OP_ID_LEN / 2];
        getrandom::getrandom(&mut bytes).expect("system randomness should be available");
        hex::encode(bytes)
    }

    fn psbt_op_row(&self, operation_id: &str) -> Result<DbPsbtOp, Error> {
        let not_found = || Error::PsbtOperationNotFound {
            operation_id: operation_id.to_string(),
        };
        let valid = operation_id.len() == PSBT_OP_ID_LEN
            && operation_id
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if !valid {
            return Err(not_found());
        }
        self.database
            .get_psbt_op(operation_id)?
            .ok_or_else(not_found)
    }

    fn psbt_op_from_row(row: &DbPsbtOp) -> Result<PsbtOperation, Error> {
        let fascia: Fascia = serde_json::from_str(&row.fascia).map_err(InternalError::from)?;
        Ok(PsbtOperation {
            operation_id: row.operation_id.clone(),
            colored_psbt: row.colored_psbt.clone(),
            status: row.status,
            broadcast: row.broadcast,
            txid: row.txid.clone(),
            allocations: fascia_allocations(&fascia)?,
        })
    }

    fn psbt_op_set(
        &self,
        row: &DbPsbtOp,
        status: Option<PsbtOperationStatus>,
        broadcast: Option<PsbtBroadcastState>,
    ) -> Result<DbPsbtOp, Error> {
        Ok(self.database.update_psbt_op(&mut DbPsbtOpActMod {
            operation_id: ActiveValue::Unchanged(row.operation_id.clone()),
            status: status.map_or(ActiveValue::NotSet, ActiveValue::Set),
            broadcast: broadcast.map_or(ActiveValue::NotSet, ActiveValue::Set),
            updated_at: ActiveValue::Set(now().unix_timestamp()),
            ..Default::default()
        })?)
    }

    fn psbt_op_batch(&self, row: &DbPsbtOp) -> Result<Option<DbBatchTransfer>, Error> {
        let Some(idx) = row.batch_transfer_idx else {
            return Ok(None);
        };
        Ok(self
            .database
            .iter_batch_transfers()?
            .into_iter()
            .find(|b| b.idx == idx))
    }

    fn psbt_op_fail_batch_if_initiated(&self, row: &DbPsbtOp) -> Result<(), Error> {
        match self.psbt_op_batch(row)? {
            Some(batch_transfer) if batch_transfer.status == TransferStatus::Initiated => {
                let mut failed: DbBatchTransferActMod = batch_transfer.into();
                failed.status = ActiveValue::Set(TransferStatus::Failed);
                self.database.update_batch_transfer(&mut failed)?;
                Ok(())
            }
            Some(batch_transfer) if batch_transfer.status != TransferStatus::Failed => {
                Err(Error::InvalidPsbtOperationStatus {
                    details: format!(
                        "HTLC batch transfer {} is {:?}, expected Initiated or Failed",
                        batch_transfer.idx, batch_transfer.status
                    ),
                })
            }
            _ => Ok(()),
        }
    }

    fn psbt_op_require_status(row: &DbPsbtOp, expected: PsbtOperationStatus) -> Result<(), Error> {
        if row.status != expected {
            return Err(Error::InvalidPsbtOperationStatus {
                details: format!(
                    "operation {} is {:?}, expected {expected:?}",
                    row.operation_id, row.status
                ),
            });
        }
        Ok(())
    }

    /// Check `input_outpoints` and the PSBT layout before coloring an externally built PSBT:
    /// inputs left out must carry no allocation, every contract on the selected ones must be
    /// colored, the OP_RETURN must exist (first when an output is P2TR) and nothing may be signed.
    fn prepare_color_psbt_for_outpoints(
        &self,
        psbt: &Psbt,
        coloring_info: &ColoringInfo,
        input_outpoints: Vec<OutPoint>,
    ) -> Result<(RgbRuntime, HashSet<OutPoint>), Error> {
        if psbt_has_input_signatures(psbt) {
            return Err(Error::InvalidColoringInfo {
                details: s!(
                    "cannot color a signed PSBT: RGB commitment rewrites the OP_RETURN output"
                ),
            });
        }
        let override_set: HashSet<OutPoint> = input_outpoints.into_iter().collect();
        if override_set.is_empty() {
            return Err(Error::InvalidColoringInfo {
                details: s!("input_outpoints is empty"),
            });
        }
        let psbt_inputs: HashSet<OutPoint> = psbt
            .unsigned_tx
            .input
            .iter()
            .map(|txin| txin.previous_output)
            .collect();
        if !override_set.is_subset(&psbt_inputs) {
            return Err(Error::InvalidColoringInfo {
                details: s!("input_outpoints must be a subset of PSBT inputs"),
            });
        }
        let runtime = self.rgb_runtime()?;
        let omitted: Vec<OutPoint> = psbt_inputs.difference(&override_set).copied().collect();
        if !omitted.is_empty() {
            let assigning = runtime.contracts_assigning(omitted)?;
            if !assigning.is_empty() {
                let contracts = assigning
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(Error::InvalidColoringInfo {
                    details: format!(
                        "PSBT inputs omitted from input_outpoints carry RGB allocations for {contracts}"
                    ),
                });
            }
        }
        self.reject_uncolored_input_contracts(&runtime, &override_set, coloring_info)?;
        let outputs = &psbt.unsigned_tx.output;
        let has_p2tr = outputs.iter().any(|o| o.script_pubkey.is_p2tr());
        match outputs.iter().position(|o| o.script_pubkey.is_op_return()) {
            None => {
                return Err(Error::InvalidColoringInfo {
                    details: s!("PSBT must include an OP_RETURN output"),
                });
            }
            Some(0) => {}
            Some(_) if has_p2tr => {
                return Err(Error::InvalidColoringInfo {
                    details: s!("OP_RETURN must be the first PSBT output"),
                });
            }
            Some(_) => {}
        }
        Ok((runtime, override_set))
    }

    /// Scripts reserved by an open `witness_receive` (pending-script row or in-flight incoming
    /// witness transfer): an output paying one is that receive's, not change.
    fn incoming_witness_receive_script_hexes(&self) -> Result<HashSet<String>, Error> {
        let mut scripts: HashSet<String> = self
            .database
            .iter_pending_witness_scripts()?
            .into_iter()
            .map(|row| row.script)
            .collect();
        let in_flight: HashSet<i32> = self
            .database
            .iter_batch_transfers()?
            .into_iter()
            .filter(|bt| !bt.status.settled() && !bt.status.failed())
            .map(|bt| bt.idx)
            .collect();
        let in_flight_asset: HashSet<i32> = self
            .database
            .iter_asset_transfers()?
            .into_iter()
            .filter(|at| in_flight.contains(&at.batch_transfer_idx))
            .map(|at| at.idx)
            .collect();
        for transfer in self.database.iter_transfers()? {
            if !transfer.incoming
                || !in_flight_asset.contains(&transfer.asset_transfer_idx)
                || !matches!(
                    transfer.recipient_type,
                    Some(RecipientTypeFull::Witness { .. })
                )
            {
                continue;
            }
            if let Some(recipient_id) = transfer.recipient_id
                && let Ok(Some(script)) = script_buf_from_recipient_id(recipient_id)
            {
                scripts.insert(script.to_hex_string());
            }
        }
        Ok(scripts)
    }

    /// Record a PSBT operation as an `Initiated` outgoing batch transfer: the allocations it spends
    /// from this wallet's coins, the change it gets back and the recipients it pays.
    #[allow(clippy::too_many_arguments)]
    fn psbt_op_record_batch(
        &self,
        runtime: &RgbRuntime,
        psbt: &Psbt,
        txid: &str,
        spent: &SpentByContract,
        coloring_info: &ColoringInfo,
        min_confirmations: u8,
        expiration_timestamp: Option<u64>,
    ) -> Result<i32, Error> {
        let db_data = self.database.get_db_data(false)?;
        if let Some(existing) = db_data.batch_transfers.iter().find(|bt| {
            bt.txid.as_deref() == Some(txid)
                && matches!(
                    bt.status,
                    TransferStatus::Initiated
                        | TransferStatus::WaitingConfirmations
                        | TransferStatus::Settled
                )
                && !bt
                    .incoming(&db_data.asset_transfers, &db_data.transfers)
                    .unwrap_or(true)
        }) {
            return Err(Error::BatchTransferAlreadyExists {
                txid: txid.to_string(),
                idx: existing.idx,
            });
        }
        let incoming_witness_scripts = self.incoming_witness_receive_script_hexes()?;
        let batch_transfer_idx = self.database.set_batch_transfer(DbBatchTransferActMod {
            txid: ActiveValue::Set(Some(txid.to_string())),
            status: ActiveValue::Set(TransferStatus::Initiated),
            expiration: ActiveValue::Set(expiration_timestamp.map(|t| t as i64)),
            created_at: ActiveValue::Set(now().unix_timestamp()),
            min_confirmations: ActiveValue::Set(min_confirmations),
            ..Default::default()
        })?;

        for (contract_id, by_outpoint) in spent {
            let asset_schema = AssetSchema::get_from_contract_id(*contract_id, runtime)?;
            let asset_transfer_idx = self.database.set_asset_transfer(DbAssetTransferActMod {
                user_driven: ActiveValue::Set(true),
                batch_transfer_idx: ActiveValue::Set(batch_transfer_idx),
                asset_id: ActiveValue::Set(Some(contract_id.to_string())),
                ..Default::default()
            })?;
            for (outpoint, assignments) in by_outpoint {
                let Some(txo) = self.database.get_txo(&(*outpoint).into())? else {
                    continue;
                };
                for assignment in assignments {
                    self.database.set_coloring(DbColoringActMod {
                        txo_idx: ActiveValue::Set(txo.idx),
                        asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                        r#type: ActiveValue::Set(ColoringType::Input),
                        assignment: ActiveValue::Set(assignment.clone()),
                        ..Default::default()
                    })?;
                }
            }

            let mut external_destinations: Vec<(u32, u64, String)> = vec![];
            let mut change_destinations: Vec<(u32, u64, String)> = vec![];
            if let Some(asset_coloring) = coloring_info.asset_info_map.get(contract_id) {
                for (vout, amount) in BTreeMap::from_iter(asset_coloring.output_map.clone()) {
                    if amount == 0 {
                        continue;
                    }
                    let txout = psbt
                        .unsigned_tx
                        .output
                        .get(vout as usize)
                        .filter(|o| !o.script_pubkey.is_op_return())
                        .ok_or_else(|| Error::InvalidColoringInfo {
                            details: s!(
                                "invalid vout in output_map, does not exist in the given PSBT"
                            ),
                        })?;
                    let recipient_id = try_recipient_id_from_script_buf(
                        &txout.script_pubkey,
                        self.bitcoin_network(),
                    )
                    .map_err(|_| Error::InvalidColoringInfo {
                        details: format!(
                            "output_map vout {vout} script is not a standard address payload"
                        ),
                    })?;
                    let owned_by_witness_receive =
                        incoming_witness_scripts.contains(&txout.script_pubkey.to_hex_string());
                    if self.bdk_wallet.is_mine(txout.script_pubkey.clone())
                        && !owned_by_witness_receive
                    {
                        let outpoint = Outpoint {
                            txid: txid.to_string(),
                            vout,
                        };
                        let txo_idx = match self.database.get_txo(&outpoint)? {
                            Some(txo) => txo.idx,
                            None => self.database.set_txo(DbTxoActMod {
                                txid: ActiveValue::Set(txid.to_string()),
                                vout: ActiveValue::Set(vout),
                                btc_amount: ActiveValue::Set(txout.value.to_sat().to_string()),
                                spent: ActiveValue::Set(false),
                                exists: ActiveValue::Set(false),
                                pending_witness: ActiveValue::Set(false),
                                ..Default::default()
                            })?,
                        };
                        self.database.set_coloring(DbColoringActMod {
                            txo_idx: ActiveValue::Set(txo_idx),
                            asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                            r#type: ActiveValue::Set(ColoringType::Change),
                            assignment: ActiveValue::Set(Assignment::Fungible(amount)),
                            ..Default::default()
                        })?;
                        change_destinations.push((vout, amount, recipient_id));
                    } else {
                        external_destinations.push((vout, amount, recipient_id));
                    }
                }
            }
            let destinations = if external_destinations.is_empty() {
                change_destinations
            } else {
                external_destinations
            };
            if destinations.is_empty() {
                self.database.set_transfer(DbTransferActMod {
                    asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                    requested_assignment: ActiveValue::Set(
                        by_outpoint.values().flatten().next().cloned(),
                    ),
                    incoming: ActiveValue::Set(false),
                    recipient_id: ActiveValue::Set(Some(format!("color:{txid}"))),
                    recipient_type: ActiveValue::Set(Some(RecipientTypeFull::Witness {
                        vout: None,
                    })),
                    ..Default::default()
                })?;
            }
            for (vout, amount, recipient_id) in destinations {
                let requested = match asset_schema {
                    AssetSchema::Nia | AssetSchema::Ifa => Assignment::Fungible(amount),
                };
                self.database.set_transfer(DbTransferActMod {
                    asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                    requested_assignment: ActiveValue::Set(Some(requested)),
                    incoming: ActiveValue::Set(false),
                    recipient_id: ActiveValue::Set(Some(recipient_id)),
                    recipient_type: ActiveValue::Set(Some(RecipientTypeFull::Witness {
                        vout: Some(vout),
                    })),
                    ..Default::default()
                })?;
            }
        }
        Ok(batch_transfer_idx)
    }

    /// [`Self::psbt_op_prepare_with_expiry`] without persisting to IndexedDB.
    pub(crate) fn psbt_op_prepare_in_memory(
        &self,
        psbt: &mut Psbt,
        coloring_info: ColoringInfo,
        input_outpoints: Vec<OutPoint>,
        min_confirmations: u8,
        expiry: PsbtOpExpiry,
    ) -> Result<PsbtOpPrepareResult, Error> {
        info!(self.logger, "Preparing HTLC color operation...");
        let created_at = now().unix_timestamp();
        let expiration_timestamp = match expiry {
            PsbtOpExpiry::Default => Some(created_at as u64 + PSBT_OP_DEFAULT_EXPIRATION_SECS),
            PsbtOpExpiry::At(timestamp) => Some(timestamp),
            PsbtOpExpiry::Never => None,
        };
        let (runtime, override_set) =
            self.prepare_color_psbt_for_outpoints(psbt, &coloring_info, input_outpoints)?;
        let mut colored = psbt.clone();
        let asset_beneficiaries = self.color_psbt_stage_runtime(
            &runtime,
            &mut colored,
            coloring_info.clone(),
            override_set.clone(),
            true,
            false,
        )?;
        let fascia = self.color_psbt_finalize(&mut colored, coloring_info.static_blinding)?;

        let witness_txid = colored.get_txid();
        let mut spent: SpentByContract = HashMap::new();
        let mut consignments = BTreeMap::new();
        for (contract_id, beneficiaries) in &asset_beneficiaries {
            let by_outpoint = spent.entry(*contract_id).or_default();
            for (explicit_seal, opout_state_map) in
                runtime.contract_assignments_for(*contract_id, override_set.iter().copied())?
            {
                for (opout, state) in opout_state_map {
                    by_outpoint
                        .entry(explicit_seal.to_outpoint())
                        .or_default()
                        .push(Assignment::from_opout_and_state(opout, &state));
                }
            }
            let mut beneficiaries_witness = vec![];
            let mut beneficiaries_blinded = vec![];
            for builder_seal in beneficiaries {
                match builder_seal {
                    BuilderSeal::Revealed(seal) => {
                        beneficiaries_witness.push(ExplicitSeal::with(witness_txid, seal.vout));
                    }
                    BuilderSeal::Concealed(secret_seal) => {
                        beneficiaries_blinded.push(*secret_seal);
                    }
                };
            }
            let transfer = runtime.transfer_from_fascia(
                *contract_id,
                beneficiaries_witness,
                beneficiaries_blinded,
                &fascia,
            )?;
            let mut consignment_bytes = Vec::new();
            transfer.save(&mut consignment_bytes)?;
            consignments.insert(contract_id.to_string(), consignment_bytes);
        }

        let txid = colored.unsigned_tx.compute_txid().to_string();
        let allocations = fascia_allocations(&fascia)?;
        let mut foreign_inputs = vec![];
        for (contract_id, by_outpoint) in &spent {
            for (outpoint, assignments) in by_outpoint {
                let outpoint: Outpoint = (*outpoint).into();
                if self.database.get_txo(&outpoint)?.is_none() {
                    foreign_inputs.push(PsbtOpForeignInput {
                        asset_id: contract_id.to_string(),
                        outpoint,
                        assignments: assignments.clone(),
                    });
                }
            }
        }

        let batch_transfer_idx = self.psbt_op_record_batch(
            &runtime,
            &colored,
            &txid,
            &spent,
            &coloring_info,
            min_confirmations,
            expiration_timestamp,
        )?;
        drop(runtime);

        let operation_id = Self::new_psbt_op_id();
        let colored_psbt = colored.to_string();
        self.database.set_psbt_op(DbPsbtOpActMod {
            operation_id: ActiveValue::Set(operation_id.clone()),
            status: ActiveValue::Set(PsbtOperationStatus::Prepared),
            broadcast: ActiveValue::Set(PsbtBroadcastState::NotAttempted),
            txid: ActiveValue::Set(txid),
            colored_psbt: ActiveValue::Set(colored_psbt.clone()),
            fascia: ActiveValue::Set(serde_json::to_string(&fascia).map_err(InternalError::from)?),
            foreign_inputs: ActiveValue::Set(foreign_inputs),
            consignments: ActiveValue::Set(consignments),
            batch_transfer_idx: ActiveValue::Set(Some(batch_transfer_idx)),
            expiration_timestamp: ActiveValue::Set(expiration_timestamp.map(|t| t as i64)),
            created_at: ActiveValue::Set(created_at),
            updated_at: ActiveValue::Set(created_at),
            ..Default::default()
        })?;
        self.update_backup_info(false)?;
        *psbt = colored;

        info!(
            self.logger,
            "HTLC prepare completed (operation_id={operation_id})"
        );
        Ok(PsbtOpPrepareResult {
            operation_id,
            colored_psbt,
            allocations,
        })
    }

    /// Color an HTLC (or other externally built) PSBT spending `input_outpoints`, which may be
    /// outpoints this wallet doesn't hold (e.g. an HTLC whose allocation was accepted with
    /// [`Self::accept_transfer_pinned`]), store the operation and record its accounting as an
    /// `Initiated` batch transfer. The RGB stash is untouched until [`Self::psbt_op_apply`].
    ///
    /// `output_map` keys are final PSBT vout indices: the PSBT must already carry its OP_RETURN
    /// output (first when any output is P2TR) and nothing is inserted or shifted. Every contract
    /// on the selected inputs must be colored and fully reassigned. Signed PSBTs are rejected.
    ///
    /// `expiration_timestamp` defaults to [`PSBT_OP_DEFAULT_EXPIRATION_SECS`] from now; see
    /// [`Self::psbt_op_prepare_with_expiry`] to disable expiry.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub async fn psbt_op_prepare(
        &self,
        psbt: &mut Psbt,
        coloring_info: ColoringInfo,
        input_outpoints: Vec<OutPoint>,
        min_confirmations: u8,
        expiration_timestamp: Option<u64>,
    ) -> Result<PsbtOpPrepareResult, Error> {
        self.psbt_op_prepare_with_expiry(
            psbt,
            coloring_info,
            input_outpoints,
            min_confirmations,
            expiration_timestamp.into(),
        )
        .await
    }

    /// [`Self::psbt_op_prepare`] with an explicit [`PsbtOpExpiry`].
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub async fn psbt_op_prepare_with_expiry(
        &self,
        psbt: &mut Psbt,
        coloring_info: ColoringInfo,
        input_outpoints: Vec<OutPoint>,
        min_confirmations: u8,
        expiry: PsbtOpExpiry,
    ) -> Result<PsbtOpPrepareResult, Error> {
        let res = self.psbt_op_prepare_in_memory(
            psbt,
            coloring_info,
            input_outpoints,
            min_confirmations,
            expiry,
        )?;
        self.flush().await?;
        Ok(res)
    }

    pub(crate) fn psbt_op_mark_broadcast_in_memory(&self, operation_id: &str) -> Result<(), Error> {
        let row = self.psbt_op_row(operation_id)?;
        Self::psbt_op_require_status(&row, PsbtOperationStatus::Prepared)?;
        if let Some(batch_transfer) = self.psbt_op_batch(&row)?
            && batch_transfer.status == TransferStatus::Failed
        {
            return Err(Error::InvalidPsbtOperationStatus {
                details: format!(
                    "operation {operation_id} batch transfer {} is Failed; it cannot be broadcast",
                    batch_transfer.idx
                ),
            });
        }
        if row.broadcast == PsbtBroadcastState::NotAttempted {
            self.psbt_op_set(&row, None, Some(PsbtBroadcastState::Attempted))?;
        }
        Ok(())
    }

    /// Record that the caller is about to broadcast the prepared witness tx. Idempotent; refused
    /// once the operation was aborted. Call it, and let it resolve, before broadcasting: it is
    /// what makes [`Self::psbt_op_abort`] refuse a rollback the network may contradict.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub async fn psbt_op_mark_broadcast(&self, operation_id: &str) -> Result<(), Error> {
        self.psbt_op_mark_broadcast_in_memory(operation_id)?;
        self.flush().await
    }

    /// Consume the operation's fascia into the stash with the witness ordering the indexer
    /// reported, and move its batch transfer to `WaitingConfirmations`.
    pub(crate) fn psbt_op_apply_in_memory(
        &self,
        operation_id: &str,
        witness_ord: WitnessOrd,
    ) -> Result<(), Error> {
        let row = self.psbt_op_row(operation_id)?;
        Self::psbt_op_require_status(&row, PsbtOperationStatus::Prepared)?;
        let batch_transfer = self
            .psbt_op_batch(&row)?
            .ok_or_else(|| Error::Inconsistency {
                details: format!("HTLC operation {operation_id} has no linked batch transfer"),
            })?;
        if batch_transfer.status != TransferStatus::Initiated {
            return Err(Error::InvalidPsbtOperationStatus {
                details: format!(
                    "HTLC batch transfer {} is {:?}, expected Initiated",
                    batch_transfer.idx, batch_transfer.status
                ),
            });
        }
        let fascia: Fascia = serde_json::from_str(&row.fascia).map_err(InternalError::from)?;
        let psbt = Psbt::from_str(&row.colored_psbt)?;
        self.consume_fascia_in_memory(fascia, Some(witness_ord))?;
        for input in &psbt.unsigned_tx.input {
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
        self.psbt_op_set(
            &row,
            Some(PsbtOperationStatus::Applied),
            Some(PsbtBroadcastState::Observed),
        )?;
        self.update_backup_info(false)?;
        Ok(())
    }

    /// Apply a prepared operation's fascia into the RGB stash once its witness tx is known to the
    /// indexer, and move its batch transfer to `WaitingConfirmations`.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub async fn psbt_op_apply(&mut self, online: Online, operation_id: &str) -> Result<(), Error> {
        info!(self.logger, "Applying HTLC operation {operation_id}...");
        self.check_online(online)?;
        let row = self.psbt_op_row(operation_id)?;
        Self::psbt_op_require_status(&row, PsbtOperationStatus::Prepared)?;
        let txid = Txid::from_str(&row.txid).map_err(|_| Error::InvalidTxid)?;
        let Some((_, height, time)) = self.indexer().get_tx_with_status(&txid).await? else {
            return Err(Error::InvalidPsbtOperationStatus {
                details: format!(
                    "witness tx {} is not known to the indexer; broadcast it before psbt_op_apply",
                    row.txid
                ),
            });
        };
        // never downgrade a mined witness to tentative
        let witness_ord = height
            .zip(time)
            .and_then(|(h, t)| NonZeroU32::new(h).zip(Some(t)))
            .and_then(|(h, t)| WitnessPos::bitcoin(h, t as i64))
            .map_or(WitnessOrd::Tentative, WitnessOrd::Mined);
        self.psbt_op_apply_in_memory(operation_id, witness_ord)?;
        self.flush().await?;
        info!(self.logger, "HTLC apply completed");
        Ok(())
    }

    /// Roll back a prepared operation whose witness tx `tx_seen` says whether the indexer knows.
    pub(crate) fn psbt_op_abort_in_memory(
        &self,
        operation_id: &str,
        tx_seen: bool,
    ) -> Result<(), Error> {
        let row = self.psbt_op_row(operation_id)?;
        if row.status == PsbtOperationStatus::Failed {
            return self.psbt_op_fail_batch_if_initiated(&row);
        }
        Self::psbt_op_require_status(&row, PsbtOperationStatus::Prepared)?;
        let now = now().unix_timestamp();
        let batch_expired = match self.psbt_op_batch(&row)? {
            Some(batch_transfer) => batch_transfer.expiration.is_some_and(|e| e < now),
            None => true,
        };
        if let Err((broadcast, e)) = psbt_op_abort_decision(&row, tx_seen, batch_expired) {
            if broadcast.is_some() {
                self.psbt_op_set(&row, None, broadcast)?;
            }
            return Err(e);
        }
        self.psbt_op_fail_batch_if_initiated(&row)?;
        self.psbt_op_set(&row, Some(PsbtOperationStatus::Failed), None)?;
        self.update_backup_info(false)?;
        Ok(())
    }

    /// Abort a prepared operation whose witness tx did not reach the network, failing its batch
    /// transfer. After a reported broadcast attempt the rollback is refused until the batch
    /// transfer expires (never, with [`PsbtOpExpiry::Never`]), and it is always refused once the
    /// indexer knows the tx.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub async fn psbt_op_abort(&mut self, online: Online, operation_id: &str) -> Result<(), Error> {
        info!(self.logger, "Aborting HTLC operation {operation_id}...");
        self.check_online(online)?;
        let row = self.psbt_op_row(operation_id)?;
        let tx_seen = row.status == PsbtOperationStatus::Prepared
            && self
                .indexer()
                .get_tx_confirmations(&row.txid)
                .await?
                .is_some();
        let res = self.psbt_op_abort_in_memory(operation_id, tx_seen);
        // a refusal may still have recorded what the indexer said about the broadcast
        self.flush().await?;
        res?;
        info!(self.logger, "HTLC abort completed");
        Ok(())
    }

    /// Find a stored PSBT operation by witness txid (lost `operation_id` / retry).
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub fn psbt_op_by_txid(&self, txid: &str) -> Result<PsbtOperation, Error> {
        let matches: Vec<DbPsbtOp> = self
            .database
            .iter_psbt_ops()?
            .into_iter()
            .filter(|row| row.txid == txid)
            .collect();
        if matches.is_empty() {
            return Err(Error::PsbtOperationNotFound {
                operation_id: txid.to_string(),
            });
        }
        let live: Vec<&DbPsbtOp> = matches
            .iter()
            .filter(|row| row.status != PsbtOperationStatus::Failed)
            .collect();
        match (matches.as_slice(), live.as_slice()) {
            ([row], _) => Self::psbt_op_from_row(row),
            (_, [row]) => Self::psbt_op_from_row(row),
            _ => Err(Error::Internal {
                details: format!("multiple HTLC operations share txid {txid}"),
            }),
        }
    }

    /// List the PSBT operations whose colored PSBT spends `outpoint`, in any status, ordered by
    /// creation time (to the second).
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub fn psbt_ops_spending(&self, outpoint: Outpoint) -> Result<Vec<PsbtOperation>, Error> {
        let spent = OutPoint::from_str(&outpoint.to_string()).map_err(|_| Error::InvalidTxid)?;
        let mut rows = self.database.iter_psbt_ops()?;
        rows.sort_by(|a, b| (a.created_at, &a.operation_id).cmp(&(b.created_at, &b.operation_id)));
        let mut ops = vec![];
        for row in rows {
            let psbt = Psbt::from_str(&row.colored_psbt).map_err(|e| Error::Inconsistency {
                details: format!(
                    "HTLC operation {} has an unreadable colored PSBT: {e}",
                    row.operation_id
                ),
            })?;
            if psbt
                .unsigned_tx
                .input
                .iter()
                .any(|i| i.previous_output == spent)
            {
                ops.push(Self::psbt_op_from_row(&row)?);
            }
        }
        Ok(ops)
    }

    /// Read a PSBT operation's status, marking it `Failed` when its batch transfer was failed
    /// and `Settled` when its batch transfer settled.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub fn psbt_op_reconcile(&self, operation_id: &str) -> Result<PsbtOperationStatus, Error> {
        let row = self.psbt_op_row(operation_id)?;
        let Some(batch_transfer) = self.psbt_op_batch(&row)? else {
            return Ok(row.status);
        };
        let next = match (row.status, batch_transfer.status) {
            (PsbtOperationStatus::Prepared, TransferStatus::Failed) => PsbtOperationStatus::Failed,
            (PsbtOperationStatus::Applied, TransferStatus::Settled) => PsbtOperationStatus::Settled,
            (status, _) => return Ok(status),
        };
        self.psbt_op_set(&row, Some(next), None)?;
        self.update_backup_info(false)?;
        self.trigger_auto_backup();
        Ok(next)
    }

    /// Fail the prepared operations whose batch transfer expired before the indexer saw their
    /// witness tx. Operations prepared with [`PsbtOpExpiry::Never`] have no expiration.
    pub(crate) async fn psbt_op_sweep_expired(&self) -> Result<bool, Error> {
        let now = now().unix_timestamp();
        let mut changed = false;
        for row in self.database.iter_psbt_ops()? {
            if row.status != PsbtOperationStatus::Prepared {
                continue;
            }
            let Some(batch_transfer) = self.psbt_op_batch(&row)? else {
                continue;
            };
            let expired = batch_transfer.expiration.is_some_and(|e| e < now);
            match batch_transfer.status {
                TransferStatus::Failed => {}
                TransferStatus::Initiated if expired => {
                    if self
                        .indexer()
                        .get_tx_confirmations(&row.txid)
                        .await?
                        .is_some()
                    {
                        if row.broadcast != PsbtBroadcastState::Observed {
                            self.psbt_op_set(&row, None, Some(PsbtBroadcastState::Observed))?;
                            changed = true;
                        }
                        continue;
                    }
                    self.psbt_op_fail_batch_if_initiated(&row)?;
                }
                _ => continue,
            }
            self.psbt_op_set(&row, Some(PsbtOperationStatus::Failed), None)?;
            changed = true;
        }
        Ok(changed)
    }

    /// Hand the consignment an applied operation produced for `asset_id` to the pending witness
    /// receive of this wallet it pays (e.g. the receive a claim spends to), settling that receive
    /// without a proxy.
    pub(crate) async fn psbt_op_provide_in_memory(
        &mut self,
        operation_id: &str,
        asset_id: &str,
    ) -> Result<RefreshResult, Error> {
        let asset_id = ContractId::from_str(asset_id)
            .map_err(|_| Error::AssetNotFound {
                asset_id: asset_id.to_string(),
            })?
            .to_string();
        let row = self.psbt_op_row(operation_id)?;
        if !matches!(
            row.status,
            PsbtOperationStatus::Applied | PsbtOperationStatus::Settled
        ) {
            return Err(Error::InvalidPsbtOperationStatus {
                details: format!(
                    "operation {operation_id} is {:?}, expected Applied or Settled",
                    row.status
                ),
            });
        }
        let consignment_bytes = row.consignments.get(&asset_id).ok_or_else(|| {
            Error::CannotProvideOutOfBandConsignment {
                details: format!("operation {operation_id} has no consignment for {asset_id}"),
            }
        })?;
        self.provide_consignment_to_receives(consignment_bytes.clone())
            .await
    }

    /// Hand the consignment an applied operation produced for `asset_id` to the pending witness
    /// receive of this wallet it pays (e.g. the witness receive a claim spends to), as if a
    /// counterparty had delivered it. The receive then settles through [`Wallet::refresh`].
    ///
    /// `asset_id` may be in any spelling rgb-lib parses. The operation must be applied. It is an
    /// error when no pending receive matches, including once the receive has already been handed
    /// the consignment.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub async fn psbt_op_provide_receive_consignment(
        &mut self,
        online: Online,
        operation_id: &str,
        asset_id: &str,
    ) -> Result<RefreshResult, Error> {
        info!(
            self.logger,
            "Providing HTLC operation {operation_id} consignment to its receive..."
        );
        self.check_online(online)?;
        let res = self
            .psbt_op_provide_in_memory(operation_id, asset_id)
            .await?;
        self.update_backup_info(false)?;
        self.flush().await?;
        info!(self.logger, "Provide HTLC operation consignment completed");
        Ok(res)
    }

    /// Fetch an RGB consignment by recipient ID (proxy lookup key).
    ///
    /// **Unchecked:** the returned `txid` and `vout` come from the proxy and are **not**
    /// validated here. Accept it with [`Self::accept_transfer_pinned`], which pins the witness
    /// output first.
    ///
    /// <div class="warning">This method is meant for special usage on HTLC outpoints</div>
    pub async fn fetch_consignment_by_recipient_id_unchecked(
        &self,
        recipient_id: String,
        consignment_endpoint: &str,
    ) -> Result<FetchedConsignment, Error> {
        let proxy_url = TransportEndpoint::new(consignment_endpoint.to_string())?.endpoint;
        let consignment_res = self
            ._get_consignment_async(&proxy_url, recipient_id)
            .await?;
        let vout = consignment_res.vout.ok_or_else(|| Error::Internal {
            details: s!("missing vout in consignment response"),
        })?;
        let consignment_bytes = general_purpose::STANDARD
            .decode(consignment_res.consignment)
            .map_err(InternalError::from)?;
        let consignment = RgbTransfer::load(&consignment_bytes[..]).map_err(InternalError::from)?;
        Ok(FetchedConsignment {
            consignment,
            txid: consignment_res.txid,
            vout,
        })
    }

    /// Fetch a consignment by proxy key, pin the witness output to `witness_recipient_id`, and
    /// accept the transfer.
    ///
    /// <div class="warning">This method is meant for special usage on HTLC outpoints</div>
    #[allow(clippy::too_many_arguments)]
    pub async fn fetch_and_accept_transfer_by_recipient_id(
        &mut self,
        online: Online,
        proxy_recipient_id: String,
        witness_recipient_id: String,
        consignment_endpoint: &str,
        blinding: u64,
        min_confirmations: u8,
        expected: ExpectedTransfer,
    ) -> Result<(RgbTransfer, Vec<Assignment>), Error> {
        self.check_online(online.clone())?;
        let fetched = self
            .fetch_consignment_by_recipient_id_unchecked(proxy_recipient_id, consignment_endpoint)
            .await?;
        let accepted = self
            .accept_transfer_pinned(
                online,
                fetched,
                witness_recipient_id,
                blinding,
                min_confirmations,
                expected,
            )
            .await?;
        Ok((accepted.consignment, accepted.assignments))
    }

    /// Accept a consignment fetched with [`Self::fetch_consignment_by_recipient_id_unchecked`]:
    /// pin `output[vout]` of the witness transaction to `witness_recipient_id` with at least
    /// `min_confirmations` (else [`Error::InsufficientConfirmations`]), check the transfer
    /// matches `expected`, and accept it.
    ///
    /// The asset is not saved: a wallet that never held it calls [`Self::save_new_asset`] before
    /// coloring a spend of the received allocation.
    ///
    /// <div class="warning">This method is meant for special usage on HTLC outpoints</div>
    pub async fn accept_transfer_pinned(
        &mut self,
        online: Online,
        fetched: FetchedConsignment,
        witness_recipient_id: String,
        blinding: u64,
        min_confirmations: u8,
        expected: ExpectedTransfer,
    ) -> Result<AcceptedTransfer, Error> {
        info!(self.logger, "Accepting pinned transfer...");
        self.check_online(online)?;
        let txid = Txid::from_str(&fetched.txid).map_err(|_| Error::InvalidTxid)?;
        let Some((tx, height, _)) = self.indexer().get_tx_with_status(&txid).await? else {
            return Err(Error::Network {
                details: s!("witness transaction not found on indexer"),
            });
        };
        check_witness_output(&tx, fetched.vout, &witness_recipient_id, self.chain_net())?;
        if min_confirmations > 0 {
            let got = match height {
                Some(_) => self
                    .indexer()
                    .get_tx_confirmations(&fetched.txid)
                    .await?
                    .unwrap_or_default(),
                None => 0,
            };
            if got < min_confirmations as u64 {
                return Err(Error::InsufficientConfirmations {
                    needed: min_confirmations,
                    got,
                });
            }
        }
        let outpoint = Outpoint {
            txid: fetched.txid.clone(),
            vout: fetched.vout,
        };
        let (consignment, assignments) = self.accept_transfer_with_consignment(
            fetched.consignment,
            &fetched.txid,
            fetched.vout,
            blinding,
            &expected,
        )?;
        self.flush().await?;
        info!(self.logger, "Accept pinned transfer completed");
        Ok(AcceptedTransfer {
            consignment,
            assignments,
            outpoint,
        })
    }

    /// Validate `consignment` against `expected` and accept it into the stash, storing the seal of
    /// `vout` of the witness transaction `txid`.
    pub(crate) fn accept_transfer_with_consignment(
        &self,
        consignment: RgbTransfer,
        txid: &str,
        vout: u32,
        blinding: u64,
        expected: &ExpectedTransfer,
    ) -> Result<(RgbTransfer, Vec<Assignment>), Error> {
        let witness_id = RgbTxid::from_str(txid).map_err(|_| Error::InvalidTxid)?;
        let asset_schema: AssetSchema = consignment.schema_id().try_into()?;
        self.check_schema_support(&asset_schema)?;
        validate_expected_transfer_intent(&consignment, &asset_schema, expected)?;

        let wasm_resolver = WasmResolver::from_consignment(&consignment, self.chain_net());
        let resolver = OffchainResolverWasm {
            witness_id,
            consignment: &consignment,
            fallback: &wasm_resolver,
        };
        let validation_config = ValidationConfig {
            chain_net: self.chain_net(),
            trusted_typesystem: asset_schema.types(),
            ..Default::default()
        };
        let valid_consignment = match consignment.clone().validate(&resolver, &validation_config) {
            Ok(consignment) => consignment,
            Err(ValidationError::InvalidConsignment(e)) => {
                error!(self.logger, "Consignment is invalid: {}", e);
                return Err(Error::InvalidConsignment);
            }
            Err(ValidationError::ResolverError(e)) => {
                return Err(Error::Network {
                    details: e.to_string(),
                });
            }
        };

        let mut received: Vec<Assignment> = self
            .extract_received_assignments(&consignment, witness_id, Some(vout), None)
            .into_values()
            .collect();
        received.sort();
        validate_expected_transfer_assignments(&received, expected)?;

        let mut runtime = self.rgb_runtime()?;
        runtime.store_secret_seal(GraphSeal::with_blinded_vout(vout, blinding))?;
        runtime.import_contract(valid_consignment.clone().into_valid_contract(), &resolver)?;
        runtime.accept_transfer(valid_consignment, &resolver)?;
        Ok((consignment, received))
    }

    /// Extract the metadata of a new RGB asset and save the asset into the DB. Idempotent once the
    /// asset is saved, provided the RGB stock knows the contract (accept the transfer first).
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub async fn save_new_asset(
        &self,
        online: Online,
        consignment: RgbTransfer,
        offchain_txid: String,
    ) -> Result<(), Error> {
        info!(self.logger, "Saving new asset...");
        self.check_online(online)?;
        self.save_new_asset_in_memory(consignment, &offchain_txid)?;
        self.flush().await?;
        info!(self.logger, "Save new asset completed");
        Ok(())
    }

    pub(crate) fn save_new_asset_in_memory(
        &self,
        consignment: RgbTransfer,
        offchain_txid: &str,
    ) -> Result<(), Error> {
        let contract_id = consignment.contract_id();
        let witness_id = RgbTxid::from_str(offchain_txid).map_err(|_| Error::InvalidTxid)?;
        let wasm_resolver = WasmResolver::from_consignment(&consignment, self.chain_net());
        let resolver = OffchainResolverWasm {
            witness_id,
            consignment: &consignment,
            fallback: &wasm_resolver,
        };
        let asset_schema: AssetSchema = consignment.schema_id().try_into()?;
        let validation_config = ValidationConfig {
            chain_net: self.chain_net(),
            trusted_typesystem: asset_schema.types(),
            ..Default::default()
        };
        let valid_transfer = match consignment.clone().validate(&resolver, &validation_config) {
            Ok(consignment) => consignment,
            Err(ValidationError::InvalidConsignment(e)) => {
                error!(self.logger, "Consignment is invalid: {}", e);
                return Err(Error::InvalidConsignment);
            }
            Err(ValidationError::ResolverError(e)) => {
                return Err(Error::Network {
                    details: e.to_string(),
                });
            }
        };
        let valid_contract = valid_transfer.clone().into_valid_contract();
        let runtime = self.rgb_runtime()?;
        // the stock must already know the contract: the caller accepted the transfer
        runtime.genesis(contract_id)?;
        if self.database.get_asset(contract_id.to_string())?.is_some() {
            return Ok(());
        }
        self.save_new_asset_internal(
            &runtime,
            contract_id,
            asset_schema,
            valid_contract,
            valid_transfer,
        )?;
        drop(runtime);
        self.update_backup_info(false)?;
        Ok(())
    }
}

/// Check whether the provided URL points to a valid proxy.
///
/// An error is raised if the provided proxy URL is invalid or if the service is running an
/// unsupported protocol version.
#[cfg(feature = "esplora")]
pub async fn check_proxy_url(proxy_url: &str) -> Result<(), Error> {
    crate::utils::check_proxy_async(proxy_url).await
}

/// Validate a consignment using the witness bundled in the consignment (offchain).
///
/// This works before the witness transaction is broadcast. The consignment bytes are
/// the raw strict-encoded consignment (not base64). The `txid` is the witness
/// transaction ID. The fallback resolver uses witness data from the consignment itself.
///
/// Returns a `ValidateConsignmentResult` with validity status, warnings, and error details.
#[cfg(feature = "esplora")]
pub fn validate_consignment_offchain(
    consignment_bytes: &[u8],
    txid: &str,
    bitcoin_network: BitcoinNetwork,
) -> Result<ValidateConsignmentResult, Error> {
    let consignment = RgbTransfer::load(consignment_bytes).map_err(|e| Error::Internal {
        details: format!("Failed to load consignment: {e}"),
    })?;

    let witness_id = RgbTxid::from_str(txid).map_err(|_| Error::InvalidTxid)?;
    let chain_net: ChainNet = bitcoin_network.into();
    let asset_schema: AssetSchema = consignment.schema_id().try_into()?;
    let trusted_typesystem = asset_schema.types();

    let wasm_resolver = crate::utils::WasmResolver::from_consignment(&consignment, chain_net);
    let resolver = crate::utils::OffchainResolverWasm {
        witness_id,
        consignment: &consignment,
        fallback: &wasm_resolver,
    };

    let validation_config = ValidationConfig {
        chain_net,
        trusted_typesystem,
        ..Default::default()
    };

    match consignment.clone().validate(&resolver, &validation_config) {
        Ok(valid_consignment) => {
            let status = valid_consignment.validation_status();
            Ok(ValidateConsignmentResult {
                valid: true,
                warnings: Some(
                    status
                        .warnings
                        .iter()
                        .map(|w| w.to_string())
                        .collect::<Vec<_>>(),
                ),
                error: None,
                details: None,
            })
        }
        Err(ValidationError::InvalidConsignment(failure)) => Ok(ValidateConsignmentResult {
            valid: false,
            warnings: None,
            error: Some("invalid".to_string()),
            details: Some(failure.to_string()),
        }),
        Err(ValidationError::ResolverError(e)) => Ok(ValidateConsignmentResult {
            valid: false,
            warnings: None,
            error: Some("resolver".to_string()),
            details: Some(e.to_string()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_consignment_offchain_invalid_bytes() {
        let result = validate_consignment_offchain(
            b"not a valid consignment",
            "0000000000000000000000000000000000000000000000000000000000000000",
            BitcoinNetwork::Regtest,
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::Internal { details: ref d } if d.contains("Failed to load consignment")),
            "expected Internal error with load failure, got: {err:?}"
        );
    }

    #[test]
    fn validate_consignment_offchain_invalid_txid() {
        let result = validate_consignment_offchain(
            b"not a valid consignment",
            "not-a-txid",
            BitcoinNetwork::Regtest,
        );
        // Will fail on consignment load before txid parsing
        assert!(result.is_err());
    }

    #[test]
    fn validate_consignment_result_serde_roundtrip() {
        let result = ValidateConsignmentResult {
            valid: true,
            warnings: Some(vec!["warn1".to_string()]),
            error: None,
            details: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: ValidateConsignmentResult = serde_json::from_str(&json).unwrap();
        assert!(deserialized.valid);
        assert_eq!(deserialized.warnings.unwrap(), vec!["warn1".to_string()]);
        assert!(deserialized.error.is_none());

        let result_invalid = ValidateConsignmentResult {
            valid: false,
            warnings: None,
            error: Some("invalid".to_string()),
            details: Some("schema mismatch".to_string()),
        };
        let json = serde_json::to_string(&result_invalid).unwrap();
        let deserialized: ValidateConsignmentResult = serde_json::from_str(&json).unwrap();
        assert!(!deserialized.valid);
        assert_eq!(deserialized.error.unwrap(), "invalid");
    }

    #[test]
    fn indexer_protocol_display() {
        assert_eq!(IndexerProtocol::Esplora.to_string(), "Esplora");
    }
}

#[cfg(all(test, feature = "esplora"))]
mod psbt_op_tests {
    use super::*;
    use crate::keys::generate_keys;
    use bdk_wallet::bitcoin::Transaction;

    const FOREIGN_TXID: &str = "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b";
    const AMOUNT: u64 = 1_000;
    const BLINDING: u64 = 777;
    const PROXY: &str = "rpc://127.0.0.1:3000/json-rpc";

    fn test_wallet() -> Wallet {
        let keys = generate_keys(BitcoinNetwork::Regtest);
        Wallet::new(WalletData {
            data_dir: s!(":memory:"),
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
        })
        .unwrap()
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    /// A wallet whose stash holds `AMOUNT` of a fresh asset on an outpoint that is not among its
    /// coins, as an accepted HTLC lock is.
    fn wallet_with_foreign_allocation() -> (Wallet, ContractId, OutPoint) {
        let wallet = test_wallet();
        wallet
            .database
            .set_txo(DbTxoActMod {
                txid: ActiveValue::Set(FOREIGN_TXID.to_string()),
                vout: ActiveValue::Set(0),
                btc_amount: ActiveValue::Set(s!("10000")),
                spent: ActiveValue::Set(false),
                exists: ActiveValue::Set(true),
                pending_witness: ActiveValue::Set(false),
                ..Default::default()
            })
            .unwrap();
        let asset = wallet
            .issue_asset_nia(s!("HTLC"), s!("Htlc"), 0, vec![AMOUNT])
            .unwrap();
        wallet.database.remove_txo(FOREIGN_TXID, 0);
        let outpoint = OutPoint::new(Txid::from_str(FOREIGN_TXID).unwrap(), 0);
        (
            wallet,
            ContractId::from_str(&asset.asset_id).unwrap(),
            outpoint,
        )
    }

    /// A pending witness receive of `wallet`, returning its recipient ID and script.
    fn witness_receive(wallet: &mut Wallet) -> (String, ScriptBuf) {
        let receive = wallet
            .witness_receive(None, Assignment::Any, None, vec![PROXY.to_string()], 1)
            .unwrap();
        let script = script_buf_from_recipient_id(receive.recipient_id.clone())
            .unwrap()
            .unwrap();
        (receive.recipient_id, script)
    }

    /// An unsigned claim of `input` paying `script`, OP_RETURN first.
    fn claim_psbt(input: OutPoint, script: ScriptBuf, fee: u64) -> Psbt {
        let tx = Transaction {
            version: TxVersion::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: input,
                ..Default::default()
            }],
            output: vec![
                TxOut {
                    value: BdkAmount::ZERO,
                    script_pubkey: ScriptBuf::new_op_return([]),
                },
                TxOut {
                    value: BdkAmount::from_sat(10_000 - fee),
                    script_pubkey: script,
                },
            ],
        };
        let mut psbt = Psbt::from_unsigned_tx(tx).unwrap();
        psbt.inputs[0].witness_utxo = Some(TxOut {
            value: BdkAmount::from_sat(10_000),
            script_pubkey: ScriptBuf::new_p2wsh(&ScriptBuf::from_bytes(vec![0x51]).wscript_hash()),
        });
        psbt
    }

    fn coloring(contract_id: ContractId, vout: u32, amount: u64) -> ColoringInfo {
        ColoringInfo {
            asset_info_map: HashMap::from([(
                contract_id,
                AssetColoringInfo {
                    output_map: HashMap::from([(vout, amount)]),
                    static_blinding: Some(BLINDING),
                },
            )]),
            static_blinding: Some(BLINDING),
            nonce: None,
        }
    }

    fn batch(wallet: &Wallet, operation_id: &str) -> DbBatchTransfer {
        let row = wallet.psbt_op_row(operation_id).unwrap();
        wallet.psbt_op_batch(&row).unwrap().unwrap()
    }

    fn status(wallet: &Wallet, operation_id: &str) -> (PsbtOperationStatus, PsbtBroadcastState) {
        let row = wallet.psbt_op_row(operation_id).unwrap();
        (row.status, row.broadcast)
    }

    fn assert_op_status_err(res: Result<(), Error>) {
        assert!(
            matches!(res, Err(Error::InvalidPsbtOperationStatus { .. })),
            "{res:?}"
        );
    }

    #[test]
    fn colors_a_foreign_input_and_runs_the_claim_lifecycle() {
        let (mut wallet, contract_id, outpoint) = wallet_with_foreign_allocation();
        let asset_id = contract_id.to_string();
        let (recipient_id, script) = witness_receive(&mut wallet);
        let mut psbt = claim_psbt(outpoint, script.clone(), 500);

        let prepared = wallet
            .psbt_op_prepare_in_memory(
                &mut psbt,
                coloring(contract_id, 1, AMOUNT),
                vec![outpoint],
                1,
                PsbtOpExpiry::Never,
            )
            .unwrap();
        assert_eq!(
            prepared.allocations,
            vec![PsbtOpAllocation {
                asset_id: asset_id.clone(),
                vout: Some(1),
                assignment: Assignment::Fungible(AMOUNT),
            }]
        );
        assert_eq!(prepared.colored_psbt, psbt.to_string());
        let txid = psbt.unsigned_tx.compute_txid().to_string();
        let op = prepared.operation_id.clone();

        // the HTLC input is foreign, the payout belongs to the witness receive, not change
        let row = wallet.psbt_op_row(&op).unwrap();
        assert_eq!(
            row.foreign_inputs,
            vec![PsbtOpForeignInput {
                asset_id: asset_id.clone(),
                outpoint: outpoint.into(),
                assignments: vec![Assignment::Fungible(AMOUNT)],
            }]
        );
        assert_eq!(row.expiration_timestamp, None);
        assert!(row.consignments.contains_key(&asset_id));
        let batch_transfer = batch(&wallet, &op);
        assert_eq!(batch_transfer.status, TransferStatus::Initiated);
        assert_eq!(batch_transfer.txid.as_deref(), Some(txid.as_str()));
        let db_data = wallet.database.get_db_data(false).unwrap();
        let data = batch_transfer
            .get_transfers(&db_data.asset_transfers, &db_data.transfers)
            .unwrap();
        let transfers = &data.asset_transfers_data[0].transfers;
        assert_eq!(transfers.len(), 1);
        assert_eq!(
            transfers[0].recipient_id.as_deref(),
            Some(recipient_id.as_str())
        );
        assert!(
            !db_data
                .colorings
                .iter()
                .any(|c| c.r#type == ColoringType::Change)
        );

        // lookups
        assert_eq!(wallet.psbt_op_by_txid(&txid).unwrap().operation_id, op);
        let spending = wallet.psbt_ops_spending(outpoint.into()).unwrap();
        assert_eq!(spending.len(), 1);
        assert_eq!(spending[0].status, PsbtOperationStatus::Prepared);
        assert!(matches!(
            wallet.psbt_op_by_txid(FOREIGN_TXID),
            Err(Error::PsbtOperationNotFound { .. })
        ));
        assert!(matches!(
            wallet.psbt_op_row("not-an-id"),
            Err(Error::PsbtOperationNotFound { .. })
        ));

        // the same transaction cannot be prepared twice while the first is live
        let mut again = claim_psbt(outpoint, script, 500);
        assert!(matches!(
            wallet.psbt_op_prepare_in_memory(
                &mut again,
                coloring(contract_id, 1, AMOUNT),
                vec![outpoint],
                1,
                PsbtOpExpiry::Never,
            ),
            Err(Error::BatchTransferAlreadyExists { .. })
        ));

        // broadcast reported: rollback is refused for good with no expiry
        wallet.psbt_op_mark_broadcast_in_memory(&op).unwrap();
        wallet.psbt_op_mark_broadcast_in_memory(&op).unwrap();
        assert_eq!(
            status(&wallet, &op),
            (PsbtOperationStatus::Prepared, PsbtBroadcastState::Attempted)
        );
        assert_op_status_err(wallet.psbt_op_abort_in_memory(&op, false));
        assert_eq!(
            status(&wallet, &op),
            (PsbtOperationStatus::Prepared, PsbtBroadcastState::Ambiguous)
        );
        assert_eq!(batch(&wallet, &op).status, TransferStatus::Initiated);

        // apply consumes the fascia: the payout now carries the asset
        wallet
            .psbt_op_apply_in_memory(&op, WitnessOrd::Tentative)
            .unwrap();
        assert_eq!(
            status(&wallet, &op),
            (PsbtOperationStatus::Applied, PsbtBroadcastState::Observed)
        );
        assert_eq!(
            batch(&wallet, &op).status,
            TransferStatus::WaitingConfirmations
        );
        let payout = OutPoint::new(Txid::from_str(&txid).unwrap(), 1);
        let held: u64 = wallet
            .rgb_runtime()
            .unwrap()
            .contract_assignments_for(contract_id, [payout])
            .unwrap()
            .into_values()
            .flat_map(|states| states.into_values())
            .map(|state| match state {
                AllocatedState::Amount(amount) => amount.as_u64(),
                _ => 0,
            })
            .sum();
        assert_eq!(held, AMOUNT);
        assert_op_status_err(wallet.psbt_op_apply_in_memory(&op, WitnessOrd::Tentative));
        assert_op_status_err(wallet.psbt_op_abort_in_memory(&op, false));
        assert_op_status_err(wallet.psbt_op_mark_broadcast_in_memory(&op));

        // provide settles the witness receive without a proxy, once
        let res = block_on(wallet.psbt_op_provide_in_memory(&op, &asset_id)).unwrap();
        assert_eq!(res.len(), 1);
        let (receive_idx, refreshed) = res.into_iter().next().unwrap();
        assert_eq!(
            refreshed.updated_status,
            Some(TransferStatus::WaitingConfirmations)
        );
        let receive = wallet
            .database
            .iter_batch_transfers()
            .unwrap()
            .into_iter()
            .find(|b| b.idx == receive_idx)
            .unwrap();
        assert_eq!(receive.txid.as_deref(), Some(txid.as_str()));
        assert!(matches!(
            block_on(wallet.psbt_op_provide_in_memory(&op, &asset_id)),
            Err(Error::CannotProvideOutOfBandConsignment { .. })
        ));

        // reconcile follows the batch transfer
        assert_eq!(
            wallet.psbt_op_reconcile(&op).unwrap(),
            PsbtOperationStatus::Applied
        );
        let mut settled: DbBatchTransferActMod = batch(&wallet, &op).into();
        settled.status = ActiveValue::Set(TransferStatus::Settled);
        wallet.database.update_batch_transfer(&mut settled).unwrap();
        assert_eq!(
            wallet.psbt_op_reconcile(&op).unwrap(),
            PsbtOperationStatus::Settled
        );
    }

    #[test]
    fn provide_requires_an_applied_operation() {
        let (mut wallet, contract_id, outpoint) = wallet_with_foreign_allocation();
        let (_, script) = witness_receive(&mut wallet);
        let mut psbt = claim_psbt(outpoint, script, 500);
        let op = wallet
            .psbt_op_prepare_in_memory(
                &mut psbt,
                coloring(contract_id, 1, AMOUNT),
                vec![outpoint],
                1,
                PsbtOpExpiry::Never,
            )
            .unwrap()
            .operation_id;
        assert!(matches!(
            block_on(wallet.psbt_op_provide_in_memory(&op, &contract_id.to_string())),
            Err(Error::InvalidPsbtOperationStatus { .. })
        ));
    }

    #[test]
    fn abort_rolls_back_only_what_never_reached_the_network() {
        let (mut wallet, contract_id, outpoint) = wallet_with_foreign_allocation();
        let (_, script) = witness_receive(&mut wallet);
        let prepare = |wallet: &Wallet, fee: u64, expiry: PsbtOpExpiry| {
            let mut psbt = claim_psbt(outpoint, script.clone(), fee);
            wallet
                .psbt_op_prepare_in_memory(
                    &mut psbt,
                    coloring(contract_id, 1, AMOUNT),
                    vec![outpoint],
                    1,
                    expiry,
                )
                .unwrap()
                .operation_id
        };

        // never broadcast: rolled back, idempotently, and no longer broadcastable
        let first = prepare(&wallet, 500, PsbtOpExpiry::Default);
        let row = wallet.psbt_op_row(&first).unwrap();
        assert!(row.expiration_timestamp.unwrap() > now().unix_timestamp());
        wallet.psbt_op_abort_in_memory(&first, false).unwrap();
        assert_eq!(
            status(&wallet, &first),
            (
                PsbtOperationStatus::Failed,
                PsbtBroadcastState::NotAttempted
            )
        );
        assert_eq!(batch(&wallet, &first).status, TransferStatus::Failed);
        wallet.psbt_op_abort_in_memory(&first, false).unwrap();
        assert_op_status_err(wallet.psbt_op_mark_broadcast_in_memory(&first));
        assert_op_status_err(wallet.psbt_op_apply_in_memory(&first, WitnessOrd::Tentative));

        // the same transaction can be prepared again, and its txid then finds the live one
        let second = prepare(&wallet, 500, PsbtOpExpiry::Default);
        let txid = wallet.psbt_op_row(&second).unwrap().txid;
        assert_eq!(wallet.psbt_op_row(&first).unwrap().txid, txid);
        assert_eq!(wallet.psbt_op_by_txid(&txid).unwrap().operation_id, second);
        assert_eq!(wallet.psbt_ops_spending(outpoint.into()).unwrap().len(), 2);

        // known to the indexer: refused, and recorded as observed
        assert_op_status_err(wallet.psbt_op_abort_in_memory(&second, true));
        assert_eq!(
            status(&wallet, &second),
            (PsbtOperationStatus::Prepared, PsbtBroadcastState::Observed)
        );
        assert_op_status_err(wallet.psbt_op_abort_in_memory(&second, false));
        wallet
            .psbt_op_set(
                &wallet.psbt_op_row(&second).unwrap(),
                Some(PsbtOperationStatus::Failed),
                None,
            )
            .unwrap();
        let mut failed: DbBatchTransferActMod = batch(&wallet, &second).into();
        failed.status = ActiveValue::Set(TransferStatus::Failed);
        wallet.database.update_batch_transfer(&mut failed).unwrap();

        // broadcast attempted but the expiration passed: the rollback goes through
        let expired = prepare(&wallet, 600, PsbtOpExpiry::At(1));
        wallet.psbt_op_mark_broadcast_in_memory(&expired).unwrap();
        wallet.psbt_op_abort_in_memory(&expired, false).unwrap();
        assert_eq!(status(&wallet, &expired).0, PsbtOperationStatus::Failed);
    }

    #[test]
    fn prepare_enforces_the_psbt_layout_and_full_allocation() {
        let (mut wallet, contract_id, outpoint) = wallet_with_foreign_allocation();
        let (_, script) = witness_receive(&mut wallet);
        let base = claim_psbt(outpoint, script.clone(), 500);
        let try_prepare = |psbt: &Psbt, info: ColoringInfo, inputs: Vec<OutPoint>| {
            let mut psbt = psbt.clone();
            let res =
                wallet.psbt_op_prepare_in_memory(&mut psbt, info, inputs, 1, PsbtOpExpiry::Never);
            (res, psbt)
        };
        let invalid = |res: Result<PsbtOpPrepareResult, Error>| {
            assert!(
                matches!(res, Err(Error::InvalidColoringInfo { .. })),
                "{res:?}"
            );
        };

        // signed
        let mut signed = base.clone();
        signed.inputs[0].final_script_witness = Some(Witness::from_slice(&[vec![0x51]]));
        invalid(try_prepare(&signed, coloring(contract_id, 1, AMOUNT), vec![outpoint]).0);

        // no OP_RETURN, or not first next to a P2TR output
        let mut no_opret = base.clone();
        no_opret.unsigned_tx.output.remove(0);
        no_opret.outputs.remove(0);
        invalid(try_prepare(&no_opret, coloring(contract_id, 0, AMOUNT), vec![outpoint]).0);
        let p2tr = ScriptBuf::from_bytes([vec![0x51, 0x20], vec![7; 32]].concat());
        assert!(p2tr.is_p2tr());
        let mut opret_last = claim_psbt(outpoint, p2tr, 500);
        opret_last.unsigned_tx.output.swap(0, 1);
        invalid(
            try_prepare(
                &opret_last,
                coloring(contract_id, 0, AMOUNT),
                vec![outpoint],
            )
            .0,
        );

        // partial allocation (no implicit burn), the OP_RETURN as a seal, an empty input set
        invalid(try_prepare(&base, coloring(contract_id, 1, AMOUNT - 1), vec![outpoint]).0);
        invalid(try_prepare(&base, coloring(contract_id, 0, AMOUNT), vec![outpoint]).0);
        invalid(try_prepare(&base, coloring(contract_id, 1, AMOUNT), vec![]).0);

        // a failed prepare leaves the PSBT and the wallet untouched
        let (res, psbt) = try_prepare(&base, coloring(contract_id, 1, AMOUNT + 1), vec![outpoint]);
        invalid(res);
        assert_eq!(psbt, base);
        assert!(wallet.database.iter_psbt_ops().unwrap().is_empty());
        assert!(
            !wallet
                .database
                .iter_batch_transfers()
                .unwrap()
                .iter()
                .any(|b| b.status == TransferStatus::Initiated)
        );
    }

    #[test]
    fn accept_checks_the_expected_transfer() {
        let (mut wallet, contract_id, outpoint) = wallet_with_foreign_allocation();
        let asset_id = contract_id.to_string();
        let (_, script) = witness_receive(&mut wallet);
        let mut psbt = claim_psbt(outpoint, script, 500);
        let op = wallet
            .psbt_op_prepare_in_memory(
                &mut psbt,
                coloring(contract_id, 1, AMOUNT),
                vec![outpoint],
                1,
                PsbtOpExpiry::Never,
            )
            .unwrap()
            .operation_id;
        let row = wallet.psbt_op_row(&op).unwrap();
        let consignment = RgbTransfer::load(&row.consignments[&asset_id][..]).unwrap();
        let expected = |asset_id: &str, asset_schema, assignment| ExpectedTransfer {
            asset_id: asset_id.to_string(),
            asset_schema,
            assignment,
        };
        fn unexpected<T: fmt::Debug>(res: Result<T, Error>) {
            assert!(
                matches!(res, Err(Error::UnexpectedTransfer { .. })),
                "{res:?}"
            );
        }

        let good = expected(&asset_id, AssetSchema::Nia, Assignment::Fungible(AMOUNT));
        validate_expected_transfer_intent(&consignment, &AssetSchema::Nia, &good).unwrap();
        unexpected(validate_expected_transfer_intent(
            &consignment,
            &AssetSchema::Nia,
            &expected(FOREIGN_TXID, AssetSchema::Nia, Assignment::Fungible(AMOUNT)),
        ));
        unexpected(validate_expected_transfer_intent(
            &consignment,
            &AssetSchema::Nia,
            &expected(&asset_id, AssetSchema::Ifa, Assignment::Fungible(AMOUNT)),
        ));
        assert_eq!(
            validate_expected_transfer_intent(
                &consignment,
                &AssetSchema::Nia,
                &expected(&asset_id, AssetSchema::Nia, Assignment::Any),
            ),
            Err(Error::InvalidAssignment)
        );
        unexpected(validate_expected_transfer_assignments(&[], &good));
        unexpected(validate_expected_transfer_assignments(
            &[Assignment::Fungible(AMOUNT - 1)],
            &good,
        ));

        // another wallet accepts the colored claim as the lock it pays
        let other = test_wallet();
        unexpected(other.accept_transfer_with_consignment(
            consignment.clone(),
            &row.txid,
            1,
            BLINDING,
            &expected(
                &asset_id,
                AssetSchema::Nia,
                Assignment::Fungible(AMOUNT + 1),
            ),
        ));
        unexpected(other.accept_transfer_with_consignment(
            consignment.clone(),
            &row.txid,
            0,
            BLINDING,
            &good,
        ));
        let (_, assignments) = other
            .accept_transfer_with_consignment(consignment.clone(), &row.txid, 1, BLINDING, &good)
            .unwrap();
        assert_eq!(assignments, vec![Assignment::Fungible(AMOUNT)]);
        other
            .save_new_asset_in_memory(consignment.clone(), &row.txid)
            .unwrap();
        other
            .save_new_asset_in_memory(consignment, &row.txid)
            .unwrap();
        assert!(other.database.get_asset(asset_id).unwrap().is_some());
    }

    #[test]
    fn witness_output_must_pay_the_recipient() {
        let mut wallet = test_wallet();
        let (recipient_id, script) = witness_receive(&mut wallet);
        let tx = claim_psbt(
            OutPoint::new(Txid::from_str(FOREIGN_TXID).unwrap(), 0),
            script,
            500,
        )
        .unsigned_tx;
        let chain_net = wallet.chain_net();
        check_witness_output(&tx, 1, &recipient_id, chain_net).unwrap();
        assert!(matches!(
            check_witness_output(&tx, 0, &recipient_id, chain_net),
            Err(Error::WitnessOutputMismatch { .. })
        ));
        assert!(matches!(
            check_witness_output(&tx, 2, &recipient_id, chain_net),
            Err(Error::WitnessOutputMismatch { .. })
        ));
        assert_eq!(
            check_witness_output(&tx, 1, "not-a-recipient", chain_net),
            Err(Error::InvalidRecipientID)
        );
        assert_eq!(
            check_witness_output(&tx, 1, &recipient_id, ChainNet::BitcoinMainnet),
            Err(Error::InvalidRecipientNetwork)
        );
    }

    #[test]
    fn expiry_from_an_optional_timestamp() {
        assert_eq!(PsbtOpExpiry::from(None), PsbtOpExpiry::Default);
        assert_eq!(PsbtOpExpiry::from(Some(5)), PsbtOpExpiry::At(5));
        let parsed: PsbtOpExpiry = serde_json::from_str("\"never\"").unwrap();
        assert_eq!(parsed, PsbtOpExpiry::Never);
        let parsed: PsbtOpExpiry = serde_json::from_str("{\"at\":9}").unwrap();
        assert_eq!(parsed, PsbtOpExpiry::At(9));
    }
}
