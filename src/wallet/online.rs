//! RGB wallet
//!
//! This module defines the online methods of the [`Wallet`] structure and all its related data.

use super::*;
use crate::api::proxy::WasmProxyClient;

const SCHEMAS_SUPPORTING_INFLATION: [database::enums::AssetSchema; 1] = [AssetSchema::Ifa];

const TRANSFER_DATA_FILE: &str = "transfer_data.txt";
const SIGNED_PSBT_FILE: &str = "signed.psbt";

pub(crate) const UTXO_SIZE: u32 = 1000;
pub(crate) const UTXO_NUM: u8 = 5;

pub(crate) const MIN_FEE_RATE: u64 = 1;

pub(crate) const DURATION_SEND_TRANSFER: i64 = 3600;

pub(crate) const MIN_BLOCK_ESTIMATION: u16 = 1;
pub(crate) const MAX_BLOCK_ESTIMATION: u16 = 1008;

enum PrepareTransferPsbtResult {
    Retry,
    Success(String),
}

type TransferEndData = (
    Psbt,
    String,
    PathBuf,
    InfoBatchTransfer,
    BTreeMap<String, InfoAssetTransfer>,
);

/// Collection of different RGB assignments.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignmentsCollection {
    /// Fungible assignments
    pub fungible: u64,
    /// Non-fungible assignments
    pub non_fungible: bool,
    /// Inflation assignments
    pub inflation: u64,
}

impl AssignmentsCollection {
    fn add_fungible(&mut self, amt: u64) {
        self.fungible += amt;
    }

    fn add_non_fungible(&mut self) {
        self.non_fungible = true;
    }

    fn add_inflation(&mut self, amt: u64) {
        self.inflation += amt;
    }

    fn add_opout_state(&mut self, opout: &Opout, state: &AllocatedState) {
        match state {
            AllocatedState::Amount(amt) if opout.ty == OS_ASSET => {
                self.add_fungible(amt.as_u64());
            }
            AllocatedState::Amount(amt) if opout.ty == OS_INFLATION => {
                self.add_inflation(amt.as_u64());
            }
            AllocatedState::Data(_) => {
                self.add_non_fungible();
            }
            _ => {}
        }
    }

    fn opout_contributes(&self, opout: &Opout, state: &AllocatedState, needed: &Self) -> bool {
        match (state, opout.ty) {
            (AllocatedState::Amount(_), OS_ASSET) => {
                needed.fungible.saturating_sub(self.fungible) > 0
            }
            (AllocatedState::Amount(_), OS_INFLATION) => {
                needed.inflation.saturating_sub(self.inflation) > 0
            }
            (AllocatedState::Data(_), _) => needed.non_fungible && !self.non_fungible,
            _ => false,
        }
    }

    fn change(&self, needed: &Self) -> Self {
        Self {
            fungible: self.fungible - needed.fungible,
            non_fungible: false,
            inflation: self.inflation - needed.inflation,
        }
    }

    fn enough(&self, needed: &Self) -> bool {
        if self.fungible < needed.fungible {
            return false;
        }
        if self.non_fungible != needed.non_fungible {
            return false;
        }
        if self.inflation < needed.inflation {
            return false;
        }
        true
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AssetSpend {
    txo_map: HashMap<i32, Outpoint>,
    assignments_collected: AssignmentsCollection,
    input_btc_amt: u64,
}

/// The result of a send operation
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct OperationResult {
    /// ID of the transaction
    pub txid: String,
    /// Batch transfer idx
    pub batch_transfer_idx: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BtcChange {
    vout: u32,
    amount: u64,
}

// map txo idx to assignments
type TxoAssignments = HashMap<i32, Vec<Assignment>>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct InfoBatchTransfer {
    btc_change: Option<BtcChange>,
    change_utxo_idx: Option<i32>,
    extra_allocations: HashMap<String, TxoAssignments>,
    donation: bool,
    min_confirmations: u8,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Deserialize, Serialize)]
enum TypeOfTransition {
    Inflate,
    Transfer,
}

impl TypeOfTransition {
    fn type_name(&self) -> &'static str {
        match self {
            Self::Inflate => "inflate",
            Self::Transfer => "transfer",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AssetInfo {
    contract_id: ContractId,
    reject_list_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct InfoAssetTransfer {
    asset_info: AssetInfo,
    recipients: Vec<LocalRecipient>,
    asset_spend: AssetSpend,
    change: AssignmentsCollection,
    original_assignments_needed: AssignmentsCollection,
    assignments_needed: AssignmentsCollection,
    assignments_spent: TxoAssignments,
    main_transition: TypeOfTransition,
}

/// In-memory storage for transfer artifacts (replaces filesystem directories).
#[derive(Clone, Default, Deserialize, Serialize)]
pub(crate) struct TransferArtifacts {
    pub(crate) batch_info: Option<InfoBatchTransfer>,
    pub(crate) asset_infos: BTreeMap<String, InfoAssetTransfer>,
    pub(crate) consignment_bytes: HashMap<String, Vec<u8>>,
    pub(crate) signed_psbt: Option<String>,
}

async fn pending_consignment_matches(
    proxy_client: &WasmProxyClient,
    endpoint: &str,
    recipient_id: &str,
    consignment_bytes: &[u8],
    txid: &str,
    vout: Option<u32>,
) -> bool {
    let Ok(response) = proxy_client
        .get_consignment(endpoint, recipient_id.to_owned())
        .await
    else {
        return false;
    };
    let Some(existing) = response.result else {
        return false;
    };
    let Ok(existing_bytes) = general_purpose::STANDARD.decode(existing.consignment) else {
        return false;
    };
    existing.txid == txid && existing.vout == vout && existing_bytes == consignment_bytes
}

pub(crate) enum Indexer {
    EsploraAsync(Box<esplora_client::AsyncClient<crate::utils::WasmSleeper>>),
}

impl Indexer {
    pub(crate) async fn block_hash(&self, height: usize) -> Result<String, IndexerError> {
        Ok(match self {
            Indexer::EsploraAsync(client) => {
                let hash = client.get_block_hash(height as u32).await?;
                hash.to_string()
            }
        })
    }

    pub(crate) async fn broadcast(&self, tx: &BdkTransaction) -> Result<(), IndexerError> {
        match self {
            Indexer::EsploraAsync(client) => {
                client.broadcast(tx).await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn fee_estimation(&self, blocks: u16) -> Result<f64, Error> {
        Ok(match self {
            Indexer::EsploraAsync(client) => {
                let estimate_map = client
                    .get_fee_estimates()
                    .await
                    .map_err(IndexerError::from)?; // in sat/vB
                if estimate_map.is_empty() {
                    return Err(Error::CannotEstimateFees);
                }
                // map needs to be sorted for interpolation to work
                let estimate_map = BTreeMap::from_iter(estimate_map);
                match estimate_map.get(&blocks) {
                    Some(estimate) => *estimate,
                    None => {
                        // find the two closest keys
                        let mut lower_key = None;
                        let mut upper_key = None;
                        for k in estimate_map.keys() {
                            match k.cmp(&blocks) {
                                Ordering::Less => {
                                    lower_key = Some(k);
                                }
                                Ordering::Greater => {
                                    upper_key = Some(k);
                                    break;
                                }
                                _ => unreachable!("already handled"),
                            }
                        }
                        // use linear interpolation formula
                        match (lower_key, upper_key) {
                            (Some(x1), Some(x2)) => {
                                let y1 = estimate_map[x1];
                                let y2 = estimate_map[x2];
                                y1 + (blocks as f64 - *x1 as f64) / (*x2 as f64 - *x1 as f64)
                                    * (y2 - y1)
                            }
                            _ => {
                                return Err(Error::Internal {
                                    details: s!("esplora map doesn't contain the expected keys"),
                                });
                            }
                        }
                    }
                }
            }
        })
    }

    pub(crate) async fn full_scan<K: Ord + Clone + Send, R: Into<FullScanRequest<K>> + Send>(
        &self,
        request: R,
    ) -> Result<FullScanResponse<K>, IndexerError> {
        match self {
            Indexer::EsploraAsync(client) => {
                use bdk_esplora::EsploraAsyncExt;
                client
                    .full_scan(request, INDEXER_STOP_GAP, INDEXER_PARALLEL_REQUESTS)
                    .await
                    .map_err(|e| IndexerError::EsploraAsync(e.to_string()))
            }
        }
    }

    pub(crate) async fn get_tx_confirmations(&self, txid: &str) -> Result<Option<u64>, Error> {
        Ok(match self {
            Indexer::EsploraAsync(client) => {
                let txid = Txid::from_str(txid).unwrap();
                let tx_status = client
                    .get_tx_status(&txid)
                    .await
                    .map_err(IndexerError::from)?;
                if let Some(tx_height) = tx_status.block_height {
                    let height = client.get_height().await.map_err(IndexerError::from)?;
                    Some((height - tx_height + 1) as u64)
                } else if client
                    .get_tx(&txid)
                    .await
                    .map_err(IndexerError::from)?
                    .is_none()
                {
                    None
                } else {
                    Some(0)
                }
            }
        })
    }

    pub(crate) async fn get_tx_height(&self, txid: &str) -> Result<Option<u32>, Error> {
        Ok(match self {
            Indexer::EsploraAsync(client) => {
                let txid = Txid::from_str(txid).unwrap();
                let tx_status = client
                    .get_tx_status(&txid)
                    .await
                    .map_err(IndexerError::from)?;
                tx_status.block_height
            }
        })
    }

    pub(crate) async fn get_tx_with_status(
        &self,
        txid: &Txid,
    ) -> Result<Option<(BdkTransaction, Option<u32>, Option<u64>)>, Error> {
        Ok(match self {
            Indexer::EsploraAsync(client) => {
                let tx = client.get_tx(txid).await.map_err(IndexerError::from)?;
                if let Some(tx) = tx {
                    let status = client
                        .get_tx_status(txid)
                        .await
                        .map_err(IndexerError::from)?;
                    Some((tx, status.block_height, status.block_time))
                } else {
                    None
                }
            }
        })
    }

    pub(crate) fn populate_tx_cache(
        &self,
        #[allow(unused)] bdk_wallet: &PersistedWallet<super::offline::BdkPersister>,
    ) {
        match self {
            Indexer::EsploraAsync(_) => {}
        }
    }

    pub(crate) async fn sync<I: 'static + Send>(
        &self,
        request: impl Into<SyncRequest<I>> + Send,
    ) -> Result<SyncResponse, IndexerError> {
        match self {
            Indexer::EsploraAsync(client) => {
                use bdk_esplora::EsploraAsyncExt;
                client
                    .sync(request, INDEXER_PARALLEL_REQUESTS)
                    .await
                    .map_err(|e| IndexerError::EsploraAsync(e.to_string()))
            }
        }
    }
}

pub(crate) struct OnlineData {
    pub(crate) id: u64,
    pub(crate) indexer_url: String,
    indexer: Indexer,
}

/// A transfer refresh filter.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct RefreshFilter {
    /// Transfer status
    pub status: RefreshTransferStatus,
    /// Whether the transfer is incoming
    pub incoming: bool,
}

/// A refreshed transfer
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct RefreshedTransfer {
    /// The updated transfer status, if it has changed
    pub updated_status: Option<TransferStatus>,
    /// Optional failure
    pub failure: Option<Error>,
}

/// The result of a refresh operation
pub type RefreshResult = HashMap<i32, RefreshedTransfer>;

pub(crate) trait RefreshResultTrait {
    fn transfers_changed(&self) -> bool;
}

impl RefreshResultTrait for RefreshResult {
    fn transfers_changed(&self) -> bool {
        self.values().any(|rt| rt.updated_status.is_some())
    }
}

/// The pending status of a [`Transfer`] (eligible for refresh).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum RefreshTransferStatus {
    /// Waiting for the counterparty to take action
    WaitingCounterparty = 1,
    /// Waiting for the transfer transaction to reach the minimum number of confirmations
    WaitingConfirmations = 2,
}

impl TryFrom<TransferStatus> for RefreshTransferStatus {
    type Error = &'static str;

    fn try_from(x: TransferStatus) -> Result<Self, Self::Error> {
        match x {
            TransferStatus::WaitingCounterparty => Ok(RefreshTransferStatus::WaitingCounterparty),
            TransferStatus::WaitingConfirmations => Ok(RefreshTransferStatus::WaitingConfirmations),
            _ => Err("ResfreshStatus only accepts pending statuses"),
        }
    }
}

impl Wallet {
    pub(crate) fn indexer(&self) -> &Indexer {
        &self.online_data.as_ref().unwrap().indexer
    }

    fn _check_fee_rate(&self, fee_rate: u64) -> Result<FeeRate, Error> {
        if fee_rate < MIN_FEE_RATE {
            return Err(Error::InvalidFeeRate {
                details: format!("value under minimum {MIN_FEE_RATE}"),
            });
        }
        let Some(fee_rate) = FeeRate::from_sat_per_vb(fee_rate) else {
            return Err(Error::InvalidFeeRate {
                details: s!("value overflows"),
            });
        };
        Ok(fee_rate)
    }

    pub(crate) fn check_online(&self, online: Online) -> Result<(), Error> {
        if let Some(online_data) = &self.online_data {
            if online_data.id != online.id || online_data.indexer_url != online.indexer_url {
                error!(self.logger, "Cannot change online object");
                return Err(Error::CannotChangeOnline);
            }
        } else {
            error!(self.logger, "Wallet is offline");
            return Err(Error::Offline);
        }
        Ok(())
    }

    fn _check_xprv(&self) -> Result<(), Error> {
        if self.watch_only {
            error!(self.logger, "Invalid operation for a watch only wallet");
            return Err(Error::WatchOnly);
        }
        Ok(())
    }

    fn _create_split_tx(
        &mut self,
        inputs: &[BdkOutPoint],
        addresses: &Vec<ScriptBuf>,
        size: u32,
        fee_rate: FeeRate,
    ) -> Result<Psbt, bdk_wallet::error::CreateTxError> {
        let change_script = if self.wallet_data.reuse_addresses {
            Some(
                self._get_new_address(KeychainKind::Internal)
                    .map_err(|_| bdk_wallet::error::CreateTxError::NoUtxosSelected)?
                    .script_pubkey(),
            )
        } else {
            None
        };

        let mut tx_builder = self.bdk_wallet.build_tx();
        tx_builder
            .add_utxos(inputs)
            .map_err(|_| bdk_wallet::error::CreateTxError::UnknownUtxo)?
            .manually_selected_only()
            .fee_rate(fee_rate);
        for address in addresses {
            tx_builder.add_recipient(address.clone(), BdkAmount::from_sat(size as u64));
        }
        if let Some(change) = change_script {
            tx_builder.drain_to(change);
        }
        tx_builder.finish()
    }

    pub(crate) fn get_script_pubkey(&self, address: &str) -> Result<ScriptBuf, Error> {
        Ok(parse_address_str(address, self.bitcoin_network())?.script_pubkey())
    }

    fn _get_unspendable_bdk_outpoints(&self) -> Result<Vec<BdkOutPoint>, Error> {
        Ok(self
            .database
            .iter_txos()?
            .into_iter()
            .map(BdkOutPoint::from)
            .collect())
    }

    fn _fail_batch_transfer(
        &self,
        batch_transfer: &DbBatchTransfer,
    ) -> Result<DbBatchTransfer, Error> {
        let mut updated_batch_transfer: DbBatchTransferActMod = batch_transfer.clone().into();
        updated_batch_transfer.status = ActiveValue::Set(TransferStatus::Failed);
        updated_batch_transfer.expiration = ActiveValue::Set(Some(now().unix_timestamp()));
        Ok(self
            .database
            .update_batch_transfer(&mut updated_batch_transfer)?)
    }

    fn _fail_batch_transfer_if_no_endpoints(
        &self,
        batch_transfer: &DbBatchTransfer,
        transfer_transport_endpoints_data: &[(DbTransferTransportEndpoint, DbTransportEndpoint)],
    ) -> Result<Option<DbBatchTransfer>, Error> {
        if transfer_transport_endpoints_data.is_empty() {
            Ok(Some(self._fail_batch_transfer(batch_transfer)?))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn extract_received_assignments(
        &self,
        consignment: &RgbTransfer,
        witness_id: RgbTxid,
        vout: Option<u32>,
        known_concealed: Option<SecretSeal>,
    ) -> HashMap<Opout, Assignment> {
        let mut received = HashMap::new();
        if let Some(bundle) = consignment
            .bundles
            .iter()
            .find(|ab| ab.witness_id() == witness_id)
        {
            for KnownTransition { transition, opid } in bundle.bundle.known_transitions.iter() {
                for (ass_type, typed_assigns) in transition.assignments.iter() {
                    for (no, fungible_assignment) in typed_assigns.as_fungible().iter().enumerate()
                    {
                        let opout = Opout::new(*opid, *ass_type, no as u16);
                        if let Assign::ConfidentialSeal { seal, state, .. } = fungible_assignment {
                            if Some(*seal) == known_concealed {
                                match *ass_type {
                                    OS_ASSET => {
                                        received
                                            .insert(opout, Assignment::Fungible(state.as_u64()));
                                    }
                                    OS_INFLATION => {
                                        received.insert(
                                            opout,
                                            Assignment::InflationRight(state.as_u64()),
                                        );
                                    }
                                    _ => {}
                                }
                            }
                        };
                        if let Assign::Revealed { seal, state, .. } = fungible_assignment {
                            if seal.txid == TxPtr::WitnessTx && Some(seal.vout.into_u32()) == vout {
                                match *ass_type {
                                    OS_ASSET => {
                                        received
                                            .insert(opout, Assignment::Fungible(state.as_u64()));
                                    }
                                    OS_INFLATION => {
                                        received.insert(
                                            opout,
                                            Assignment::InflationRight(state.as_u64()),
                                        );
                                    }
                                    _ => {}
                                }
                            }
                        };
                    }
                    for (no, structured_assignment) in
                        typed_assigns.as_structured().iter().enumerate()
                    {
                        let opout = Opout::new(*opid, *ass_type, no as u16);
                        if let Assign::ConfidentialSeal { seal, .. } = structured_assignment {
                            if Some(*seal) == known_concealed {
                                received.insert(opout, Assignment::NonFungible);
                            }
                        }
                        if let Assign::Revealed { seal, .. } = structured_assignment {
                            if seal.txid == TxPtr::WitnessTx && Some(seal.vout.into_u32()) == vout {
                                received.insert(opout, Assignment::NonFungible);
                            }
                        };
                    }
                }
            }
        }

        received
    }

    fn _select_rgb_inputs(
        &self,
        asset_id: String,
        assignments_needed: &AssignmentsCollection,
        unspents: Vec<LocalUnspent>,
    ) -> Result<AssetSpend, Error> {
        // sort unspents by the sum of main amounts
        fn cmp_localunspent_allocation_sum(a: &LocalUnspent, b: &LocalUnspent) -> Ordering {
            let a_sum: u64 = a
                .rgb_allocations
                .iter()
                .map(|a| a.assignment.main_amount())
                .sum();
            let b_sum: u64 = b
                .rgb_allocations
                .iter()
                .map(|a| a.assignment.main_amount())
                .sum();
            a_sum.cmp(&b_sum)
        }
        // sort unspents by the sum of inflation right amounts
        fn cmp_localunspent_inflation_sum(a: &LocalUnspent, b: &LocalUnspent) -> Ordering {
            let a_sum: u64 = a
                .rgb_allocations
                .iter()
                .map(|a| a.assignment.inflation_amount())
                .sum();
            let b_sum: u64 = b
                .rgb_allocations
                .iter()
                .map(|a| a.assignment.inflation_amount())
                .sum();
            a_sum.cmp(&b_sum)
        }

        debug!(self.logger, "Selecting inputs for asset '{}'...", asset_id);
        let mut txo_map: HashMap<i32, Outpoint> = HashMap::new();

        let mut mut_unspents = unspents;

        // sort unspents first by inflation rights amount, then main amount
        if assignments_needed.inflation > 0 {
            mut_unspents.sort_by(cmp_localunspent_inflation_sum);
        }
        if assignments_needed.fungible > 0 {
            mut_unspents.sort_by(cmp_localunspent_allocation_sum);
        }

        let mut assignments_collected = AssignmentsCollection::default();
        let mut input_btc_amt = 0;
        for unspent in mut_unspents {
            // get spendable allocations for the required asset
            let asset_allocations: Vec<LocalRgbAllocation> = unspent
                .rgb_allocations
                .into_iter()
                .filter(|a| a.asset_id == Some(asset_id.clone()) && a.status.settled())
                .collect();

            // skip UTXOs with no allocations
            if asset_allocations.is_empty() {
                continue;
            }

            // check if the unspent hosts any needed allocations
            let mut needed = false;
            if assignments_collected.fungible < assignments_needed.fungible
                && asset_allocations
                    .iter()
                    .any(|a| matches!(a.assignment, Assignment::Fungible(_)))
            {
                needed = true;
            }
            if !assignments_collected.non_fungible & assignments_needed.non_fungible
                && asset_allocations
                    .iter()
                    .any(|a| matches!(a.assignment, Assignment::NonFungible))
            {
                needed = true;
            }
            if assignments_collected.inflation < assignments_needed.inflation
                && asset_allocations
                    .iter()
                    .any(|a| matches!(a.assignment, Assignment::InflationRight(_)))
            {
                needed = true;
            }
            // skip UTXOs with no needed allocations
            if !needed {
                continue;
            }

            // add selected allocations to collected assignments
            asset_allocations
                .iter()
                .for_each(|a| a.assignment.add_to_assignments(&mut assignments_collected));
            txo_map.insert(unspent.utxo.idx, unspent.utxo.outpoint());

            input_btc_amt += unspent.utxo.btc_amount.parse::<u64>().unwrap();

            // stop as soon as we have the needed assignments
            if assignments_collected.enough(assignments_needed) {
                break;
            }
        }
        if !assignments_collected.enough(assignments_needed) {
            return Err(Error::InsufficientAssignments {
                asset_id,
                available: assignments_collected,
            });
        }

        debug!(
            self.logger,
            "Asset input assignments {:?}", assignments_collected
        );
        Ok(AssetSpend {
            txo_map,
            assignments_collected,
            input_btc_amt,
        })
    }

    fn _prepare_psbt(
        &mut self,
        input_outpoints: HashSet<BdkOutPoint>,
        witness_recipients: &Vec<(ScriptBuf, u64)>,
        fee_rate: FeeRate,
        lock_time: Option<u32>,
    ) -> Result<(Psbt, Option<BtcChange>), Error> {
        let change_addr = self.get_new_address()?.script_pubkey();
        let mut builder = self.bdk_wallet.build_tx();
        builder
            .add_data(&[0; 32])
            .add_utxos(&input_outpoints.into_iter().collect::<Vec<_>>())
            .map_err(InternalError::from)?
            .manually_selected_only()
            .fee_rate(fee_rate)
            .ordering(bdk_wallet::tx_builder::TxOrdering::Untouched);
        // When the caller pins a locktime (e.g. 0 for an LN funding tx that must be final),
        // honor it; otherwise keep BDK's anti-fee-sniping default. LDK's
        // funding_transaction_generated rejects a funding tx whose absolute timelock is non-final.
        if let Some(height) = lock_time {
            builder.nlocktime(
                bdk_wallet::bitcoin::locktime::absolute::LockTime::from_height(height).map_err(
                    |e| Error::Internal {
                        details: e.to_string(),
                    },
                )?,
            );
        }
        for (script_buf, amount_sat) in witness_recipients {
            builder.add_recipient(script_buf.clone(), BdkAmount::from_sat(*amount_sat));
        }
        builder.drain_to(change_addr.clone());

        let psbt = builder.finish().map_err(|e| match e {
            bdk_wallet::error::CreateTxError::CoinSelection(InsufficientFunds {
                needed,
                available,
            }) => Error::InsufficientBitcoins {
                needed: needed.to_sat(),
                available: available.to_sat(),
            },
            bdk_wallet::error::CreateTxError::OutputBelowDustLimit(_) => {
                Error::OutputBelowDustLimit
            }
            _ => Error::Internal {
                details: e.to_string(),
            },
        })?;

        let btc_change = psbt
            .unsigned_tx
            .output
            .iter()
            .enumerate()
            .find(|(_, o)| o.script_pubkey == change_addr)
            .map(|(i, o)| BtcChange {
                vout: i as u32,
                amount: o.value.to_sat(),
            });

        Ok((psbt, btc_change))
    }

    fn _try_prepare_psbt(
        &mut self,
        input_unspents: &[LocalUnspent],
        all_inputs: &mut HashSet<BdkOutPoint>,
        witness_recipients: &Vec<(ScriptBuf, u64)>,
        fee_rate: FeeRate,
        lock_time: Option<u32>,
    ) -> Result<(Psbt, Option<BtcChange>), Error> {
        Ok(loop {
            break match self._prepare_psbt(
                all_inputs.clone(),
                witness_recipients,
                fee_rate,
                lock_time,
            ) {
                Ok(res) => res,
                Err(Error::InsufficientBitcoins { .. }) => {
                    let used_txos: Vec<Outpoint> =
                        all_inputs.clone().into_iter().map(|o| o.into()).collect();
                    let mut free_utxos = self.get_available_allocations(
                        input_unspents,
                        used_txos.as_slice(),
                        Some(0),
                    )?;
                    // sort UTXOs by BTC amount
                    if !free_utxos.is_empty() {
                        // pre-parse BTC amounts to make sure no one will fail
                        for u in &free_utxos {
                            u.utxo
                                .btc_amount
                                .parse::<u64>()
                                .map_err(|e| Error::Internal {
                                    details: e.to_string(),
                                })?;
                        }
                        free_utxos.sort_by_key(|u| u.utxo.btc_amount.parse::<u64>().unwrap());
                    }
                    if let Some(a) = free_utxos.pop() {
                        all_inputs.insert(a.utxo.into());
                        continue;
                    }
                    return Err(self.detect_btc_unspendable_err()?);
                }
                Err(e) => return Err(e),
            };
        })
    }

    async fn _restore_missing_bdk_inputs(
        &mut self,
        input_outpoints: &HashSet<BdkOutPoint>,
    ) -> Result<(), Error> {
        let missing_txids: HashSet<Txid> = input_outpoints
            .iter()
            .filter(|outpoint| self.bdk_wallet.get_utxo(**outpoint).is_none())
            .map(|outpoint| outpoint.txid)
            .collect();
        if missing_txids.is_empty() {
            return Ok(());
        }

        let mut transactions = Vec::with_capacity(missing_txids.len());
        for txid in missing_txids {
            let tx = self
                .bdk_wallet
                .tx_graph()
                .get_tx(txid)
                .ok_or_else(|| Error::Internal {
                    details: format!(
                        "selected RGB input transaction {txid} is missing from the BDK graph"
                    ),
                })?;
            transactions.push(tx);
        }

        // Re-observe local full transactions immediately before PSBT construction so BDK can
        // resolve selected RGB outpoints even after an incremental sync evicted them.
        let last_seen = ((js_sys::Date::now() / 1000.0) as u64).saturating_add(1);
        self.bdk_wallet
            .apply_unconfirmed_txs(transactions.into_iter().map(|tx| (tx, last_seen)));
        self.bdk_wallet.persist(&mut self.bdk_database)?;
        Ok(())
    }

    fn _get_change_seal(
        &self,
        btc_change: &Option<BtcChange>,
        change_utxo_option: &mut Option<DbTxo>,
        change_utxo_idx: &mut Option<i32>,
        input_outpoints: &[Outpoint],
        unspents: &[LocalUnspent],
    ) -> Result<BlindSeal<TxPtr>, Error> {
        let graph_seal = if let Some(btc_change) = btc_change {
            GraphSeal::new_random_vout(btc_change.vout)
        } else {
            if change_utxo_option.is_none() {
                let change_utxo = self.get_utxo(input_outpoints, Some(unspents), true, None)?;
                debug!(
                    self.logger,
                    "Change outpoint '{}'",
                    change_utxo.outpoint().to_string()
                );
                *change_utxo_idx = Some(change_utxo.idx);
                *change_utxo_option = Some(change_utxo);
            }
            let change_utxo = change_utxo_option.clone().unwrap();
            let blind_seal = self.get_blind_seal(change_utxo).transmutate();
            GraphSeal::from(blind_seal)
        };
        Ok(graph_seal)
    }

    fn _check_dag(
        &self,
        dag_data: &OpoutsDagData,
        reject_opouts: &HashSet<Opout>,
        allow_opouts: &HashSet<Opout>,
        check_opouts: &HashSet<Opout>,
    ) -> Result<HashSet<Opout>, Error> {
        let (dag, index) = dag_data;
        let mut to_reject = HashSet::new();

        // for each opout we are checking, traverse its ancestor chain
        for check_opout in check_opouts {
            let &opout_node = index.get(check_opout).ok_or(Error::Internal {
                details: s!("opout not found in DAG"),
            })?;

            // traverse from this node to its ancestors, depth first
            let mut stack = vec![opout_node];
            let mut visited = HashSet::new();
            while let Some(node) = stack.pop() {
                if !visited.insert(node) {
                    continue;
                }
                let node_opout = &dag[node];
                // allow shields this path upward: do not traverse this branch further
                if allow_opouts.contains(node_opout) {
                    continue;
                }
                // encountering a reject node on an unshielded path: reject
                if reject_opouts.contains(node_opout) {
                    to_reject.insert(*check_opout);
                    break;
                }
                for (_edge, parent) in dag.parents(node).iter(dag) {
                    stack.push(parent);
                }
            }
        }
        Ok(to_reject)
    }

    fn _save_transfers(
        &self,
        txid: String,
        transfer_info_map: &BTreeMap<String, InfoAssetTransfer>,
        extra_allocations: HashMap<String, TxoAssignments>,
        change_utxo_idx: Option<i32>,
        btc_change: Option<BtcChange>,
        status: TransferStatus,
        min_confirmations: u8,
    ) -> Result<i32, Error> {
        let created_at = now().unix_timestamp();
        let expiration = Some(created_at + DURATION_SEND_TRANSFER);

        let batch_transfer = DbBatchTransferActMod {
            txid: ActiveValue::Set(Some(txid.clone())),
            status: ActiveValue::Set(status),
            expiration: ActiveValue::Set(expiration),
            created_at: ActiveValue::Set(created_at),
            min_confirmations: ActiveValue::Set(min_confirmations),
            ..Default::default()
        };
        let batch_transfer_idx = self.database.set_batch_transfer(batch_transfer)?;

        let change_utxo_idx = if let Some(btc_change) = btc_change {
            Some(
                match self.database.get_txo(&Outpoint {
                    txid: txid.clone(),
                    vout: btc_change.vout,
                })? {
                    Some(txo) => txo.idx,
                    None => {
                        let db_utxo = DbTxoActMod {
                            txid: ActiveValue::Set(txid.clone()),
                            vout: ActiveValue::Set(btc_change.vout),
                            btc_amount: ActiveValue::Set(btc_change.amount.to_string()),
                            spent: ActiveValue::Set(false),
                            exists: ActiveValue::Set(false),
                            pending_witness: ActiveValue::Set(false),
                            ..Default::default()
                        };
                        self.database.set_txo(db_utxo)?
                    }
                },
            )
        } else {
            change_utxo_idx
        };

        for (asset_id, transfer_info) in transfer_info_map {
            let asset_transfer = DbAssetTransferActMod {
                user_driven: ActiveValue::Set(true),
                batch_transfer_idx: ActiveValue::Set(batch_transfer_idx),
                asset_id: ActiveValue::Set(Some(asset_id.clone())),
                ..Default::default()
            };
            let asset_transfer_idx = self.database.set_asset_transfer(asset_transfer)?;

            for (input_idx, assignments) in &transfer_info.assignments_spent {
                for assignment in assignments {
                    let db_coloring = DbColoringActMod {
                        txo_idx: ActiveValue::Set(*input_idx),
                        asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                        r#type: ActiveValue::Set(ColoringType::Input),
                        assignment: ActiveValue::Set(assignment.clone()),
                        ..Default::default()
                    };
                    self.database.set_coloring(db_coloring)?;
                }
            }
            if transfer_info.change.fungible > 0 {
                let db_coloring = DbColoringActMod {
                    txo_idx: ActiveValue::Set(change_utxo_idx.unwrap()),
                    asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                    r#type: ActiveValue::Set(ColoringType::Change),
                    assignment: ActiveValue::Set(Assignment::Fungible(
                        transfer_info.change.fungible,
                    )),
                    ..Default::default()
                };
                self.database.set_coloring(db_coloring)?;
            }
            if transfer_info.change.inflation > 0 {
                let db_coloring = DbColoringActMod {
                    txo_idx: ActiveValue::Set(change_utxo_idx.unwrap()),
                    asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                    r#type: ActiveValue::Set(ColoringType::Change),
                    assignment: ActiveValue::Set(Assignment::InflationRight(
                        transfer_info.change.inflation,
                    )),
                    ..Default::default()
                };
                self.database.set_coloring(db_coloring)?;
            }

            for recipient in transfer_info.recipients.clone() {
                let recipient_type = if transfer_info.main_transition == TypeOfTransition::Inflate {
                    let vout = if let LocalRecipientData::Witness(local_witness_data) =
                        recipient.local_recipient_data
                    {
                        local_witness_data.vout
                    } else {
                        unreachable!("inflation uses witness recipients")
                    };
                    let txo_idx = self
                        .database
                        .get_txo(&Outpoint {
                            txid: txid.clone(),
                            vout,
                        })?
                        .expect("outpoint should be in the DB")
                        .idx;
                    let db_coloring = DbColoringActMod {
                        txo_idx: ActiveValue::Set(txo_idx),
                        asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                        r#type: ActiveValue::Set(ColoringType::Issue),
                        assignment: ActiveValue::Set(recipient.assignment.clone()),
                        ..Default::default()
                    };
                    self.database.set_coloring(db_coloring)?;
                    Some(RecipientTypeFull::Witness { vout: Some(vout) })
                } else {
                    None
                };

                let transfer = DbTransferActMod {
                    asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                    requested_assignment: ActiveValue::Set(Some(recipient.assignment)),
                    incoming: ActiveValue::Set(false),
                    recipient_id: ActiveValue::Set(Some(recipient.recipient_id.clone())),
                    recipient_type: ActiveValue::Set(recipient_type),
                    ..Default::default()
                };
                let transfer_idx = self.database.set_transfer(transfer)?;
                for transport_endpoint in recipient.transport_endpoints {
                    self.save_transfer_transport_endpoint(transfer_idx, &transport_endpoint)?;
                }
            }
        }

        for (asset_id, txo_assignments) in extra_allocations {
            let asset_transfer = DbAssetTransferActMod {
                user_driven: ActiveValue::Set(false),
                batch_transfer_idx: ActiveValue::Set(batch_transfer_idx),
                asset_id: ActiveValue::Set(Some(asset_id.clone())),
                ..Default::default()
            };
            let asset_transfer_idx = self.database.set_asset_transfer(asset_transfer)?;
            for (input_idx, assignments) in txo_assignments {
                for assignment in assignments {
                    let db_coloring = DbColoringActMod {
                        txo_idx: ActiveValue::Set(input_idx),
                        asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                        r#type: ActiveValue::Set(ColoringType::Input),
                        assignment: ActiveValue::Set(assignment.clone()),
                        ..Default::default()
                    };
                    self.database.set_coloring(db_coloring)?;
                    let db_coloring = DbColoringActMod {
                        txo_idx: ActiveValue::Set(change_utxo_idx.unwrap()),
                        asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                        r#type: ActiveValue::Set(ColoringType::Change),
                        assignment: ActiveValue::Set(assignment),
                        ..Default::default()
                    };
                    self.database.set_coloring(db_coloring)?;
                }
            }
        }

        Ok(batch_transfer_idx)
    }

    pub(crate) fn get_input_unspents(
        &self,
        unspents: &[LocalUnspent],
    ) -> Result<Vec<LocalUnspent>, Error> {
        let mut input_unspents = unspents.to_vec();
        // consider the following UTXOs unspendable:
        // - incoming and pending
        // - outgoing and in waiting counterparty status
        // - pending incoming witness
        // - pending incoming blinded
        // - inexistent
        input_unspents.retain(|u| {
            !(u.rgb_allocations
                .iter()
                .any(|a| a.incoming && a.status.pending()))
                && !(u
                    .rgb_allocations
                    .iter()
                    .any(|a| !a.incoming && a.status.waiting_counterparty()))
                && !u.utxo.pending_witness
                && u.pending_blinded == 0
                && u.utxo.exists
        });
        Ok(input_unspents)
    }

    fn _get_transfer_begin_data(
        &mut self,
        online: Online,
        fee_rate: u64,
    ) -> Result<(FeeRate, Vec<LocalUnspent>, Vec<LocalUnspent>, RgbRuntime), Error> {
        self.check_online(online)?;
        let fee_rate_checked = self._check_fee_rate(fee_rate)?;

        let db_data = self.database.get_db_data(false)?;

        let utxos = self.database.get_unspent_txos(db_data.txos.clone())?;

        let unspents = self.database.get_rgb_allocations(
            utxos,
            Some(db_data.colorings.clone()),
            Some(db_data.batch_transfers.clone()),
            Some(db_data.asset_transfers.clone()),
            Some(db_data.transfers.clone()),
        )?;

        let input_unspents = self.get_input_unspents(&unspents)?;

        let runtime = self.rgb_runtime()?;

        Ok((fee_rate_checked, unspents, input_unspents, runtime))
    }
}

impl Wallet {
    async fn _go_online_wasm(&self, indexer_url: String) -> Result<(Online, OnlineData), Error> {
        let online_id = now().unix_timestamp_nanos() as u64;
        let online = Online {
            id: online_id,
            indexer_url: indexer_url.clone(),
        };
        let indexer =
            crate::utils::build_indexer(&indexer_url).ok_or_else(|| Error::InvalidIndexer {
                details: s!("failed to build esplora async client"),
            })?;
        indexer
            .block_hash(0)
            .await
            .map_err(|_| Error::InvalidIndexer {
                details: s!("not a valid esplora server"),
            })?;
        indexer.populate_tx_cache(&self.bdk_wallet);
        let online_data = OnlineData {
            id: online.id,
            indexer_url,
            indexer,
        };
        Ok((online, online_data))
    }

    /// Return the existing or freshly generated set of wallet [`Online`] data (wasm32 async).
    pub async fn go_online(
        &mut self,
        _skip_consistency_check: bool,
        indexer_url: String,
    ) -> Result<Online, Error> {
        info!(self.logger, "Going online...");
        let online = if let Some(online_data) = &self.online_data {
            let online = Online {
                id: online_data.id,
                indexer_url,
            };
            if online_data.indexer_url != online.indexer_url {
                let (online, online_data) = self._go_online_wasm(online.indexer_url).await?;
                self.online_data = Some(online_data);
                online
            } else {
                self.check_online(online.clone())?;
                online
            }
        } else {
            let (online, online_data) = self._go_online_wasm(indexer_url).await?;
            self.online_data = Some(online_data);
            online
        };
        info!(self.logger, "Go online completed");
        Ok(online)
    }

    pub(crate) async fn sync_db_txos(&mut self, full_scan: bool) -> Result<(), Error> {
        debug!(self.logger, "Syncing TXOs...");

        // Use js_sys::Date::now() for WASM-compatible timestamp
        // (std::time::UNIX_EPOCH.elapsed() panics on wasm32-unknown-unknown)
        let start_time = (js_sys::Date::now() / 1000.0) as u64;
        let update: Update = if full_scan {
            let request = self.bdk_wallet.start_full_scan_at(start_time);
            self.indexer().full_scan(request).await?.into()
        } else {
            let request = self.bdk_wallet.start_sync_with_revealed_spks_at(start_time);
            self.indexer().sync(request).await?.into()
        };
        self.bdk_wallet
            .apply_update(update)
            .map_err(|e| Error::FailedBdkSync {
                details: e.to_string(),
            })?;
        self.bdk_wallet.persist(&mut self.bdk_database)?;

        let db_txos = self.database.iter_txos()?;

        let db_outpoints: HashSet<String> = db_txos
            .clone()
            .into_iter()
            .filter(|t| !t.spent && t.exists)
            .map(|u| u.outpoint().to_string())
            .collect();
        let bdk_utxos = self.bdk_wallet.list_unspent();
        let external_bdk_utxos: Vec<LocalOutput> = bdk_utxos
            .filter(|u| u.keychain == KeychainKind::External)
            .collect();

        let new_utxos: Vec<LocalOutput> = external_bdk_utxos
            .clone()
            .into_iter()
            .filter(|u| !db_outpoints.contains(&u.outpoint.to_string()))
            .collect();

        let pending_witness_scripts: Vec<String> = self
            .database
            .iter_pending_witness_scripts()?
            .into_iter()
            .map(|s| s.script)
            .collect();

        for new_utxo in new_utxos.iter().cloned() {
            let mut new_db_utxo: DbTxoActMod = new_utxo.clone().into();
            if !pending_witness_scripts.is_empty() {
                let pending_witness_script = new_utxo.txout.script_pubkey.to_hex_string();
                if pending_witness_scripts.contains(&pending_witness_script) {
                    new_db_utxo.pending_witness = ActiveValue::Set(true);
                    self.database
                        .del_pending_witness_script(pending_witness_script)?;
                }
            }
            self.database.set_txo(new_db_utxo.clone())?;
        }

        debug!(self.logger, "Synced TXOs");

        Ok(())
    }

    /// Incrementally sync the wallet and save new RGB UTXOs to the DB (wasm32 async).
    ///
    /// Re-queries only already-revealed SPKs. To rediscover UTXOs for unrevealed SPKs (e.g. a thin
    /// restored state) use [`Wallet::full_scan`].
    pub async fn sync(&mut self, online: Online) -> Result<(), Error> {
        info!(self.logger, "Syncing...");
        self.check_online(online)?;
        self.sync_db_txos(false).await?;
        info!(self.logger, "Sync completed");
        Ok(())
    }

    /// Run a BIP44 stop-gap full scan and save new RGB UTXOs to the DB (wasm32 async).
    ///
    /// Derives and queries SPKs up to the indexer stop-gap, rebuilding a thin BDK state (no
    /// revealed SPKs) from the indexer when an incremental [`Wallet::sync`] cannot.
    pub async fn full_scan(&mut self, online: Online) -> Result<(), Error> {
        info!(self.logger, "Full scanning...");
        self.check_online(online)?;
        self.sync_db_txos(true).await?;
        info!(self.logger, "Full scan completed");
        Ok(())
    }

    async fn _broadcast_tx(&self, tx: BdkTransaction) -> Result<BdkTransaction, Error> {
        let txid = tx.compute_txid().to_string();
        let indexer = self.indexer();
        match indexer.broadcast(&tx).await {
            Ok(_) => {
                debug!(self.logger, "Broadcasted TX with ID '{}'", txid);
                Ok(tx)
            }
            Err(e) => {
                match e {
                    IndexerError::EsploraAsync(ref msg) => {
                        if msg.contains("min relay fee not met") {
                            return Err(Error::MinFeeNotMet { txid: txid.clone() });
                        } else if msg.contains("Fee exceeds maximum configured") {
                            return Err(Error::MaxFeeExceeded { txid: txid.clone() });
                        }
                    }
                }
                if indexer.get_tx_confirmations(&txid).await?.is_none() {
                    return Err(Error::FailedBroadcast {
                        details: e.to_string(),
                    });
                }
                Ok(tx)
            }
        }
    }

    async fn _broadcast_psbt(
        &mut self,
        signed_psbt: Psbt,
        skip_sync: bool,
    ) -> Result<BdkTransaction, Error> {
        let tx = self
            ._broadcast_tx(signed_psbt.extract_tx().map_err(InternalError::from)?)
            .await?;

        let internal_unspents_outpoints: Vec<(String, u32)> = self
            .internal_unspents()
            .map(|u| (u.outpoint.txid.to_string(), u.outpoint.vout))
            .collect();

        for input in tx.clone().input {
            let txid = input.previous_output.txid.to_string();
            let vout = input.previous_output.vout;
            if internal_unspents_outpoints.contains(&(txid.clone(), vout)) {
                continue;
            }
            let mut db_txo: DbTxoActMod = self
                .database
                .get_txo(&Outpoint { txid, vout })?
                .expect("outpoint should be in the DB")
                .into();
            db_txo.spent = ActiveValue::Set(true);
            self.database.update_txo(db_txo)?;
        }

        if !skip_sync {
            self.sync_db_txos(false).await?;
        }

        // Keep the transaction in BDK's graph after the optional sync. Esplora may not have
        // indexed a freshly broadcast transaction yet and can mark it evicted during that sync,
        // leaving rgb-lib's TXO database aware of its outputs while BDK cannot later use them
        // as PSBT inputs.
        // Esplora's sync uses second-resolution timestamps for evictions. Advance the local
        // observation by one second so a same-second "not indexed yet" eviction cannot make the
        // transaction non-canonical.
        let last_seen = ((js_sys::Date::now() / 1000.0) as u64).saturating_add(1);
        self.bdk_wallet
            .apply_unconfirmed_txs([(tx.clone(), last_seen)]);
        self.bdk_wallet.persist(&mut self.bdk_database)?;

        Ok(tx)
    }

    /// Prepare a transaction to create new UTXOs for RGB allocations (wasm32 async).
    ///
    /// Signing of the returned PSBT needs to be carried out separately. The signed PSBT then needs
    /// to be fed to the [`create_utxos_end`](Wallet::create_utxos_end) function.
    ///
    /// This doesn't require the wallet to have private keys.
    ///
    /// Returns a PSBT ready to be signed.
    pub async fn create_utxos_begin(
        &mut self,
        online: Online,
        up_to: bool,
        num: Option<u8>,
        size: Option<u32>,
        fee_rate: u64,
        skip_sync: bool,
    ) -> Result<String, Error> {
        info!(self.logger, "Creating UTXOs (begin)...");
        self.check_online(online)?;
        let fee_rate_checked = self._check_fee_rate(fee_rate)?;

        if !skip_sync {
            self.sync_db_txos(false).await?;
        }

        let unspent_txos = self.database.get_unspent_txos(vec![])?;
        let unspents = self
            .database
            .get_rgb_allocations(unspent_txos, None, None, None, None)?;

        let mut utxos_to_create = num.unwrap_or(UTXO_NUM);
        if up_to {
            let allocatable = self.get_available_allocations(unspents, &[], None)?.len() as u8;
            if allocatable >= utxos_to_create {
                return Err(Error::AllocationsAlreadyAvailable);
            }
            utxos_to_create -= allocatable
        }
        debug!(self.logger, "Will try to create {} UTXOs", utxos_to_create);

        let inputs: Vec<BdkOutPoint> = self.internal_unspents().map(|u| u.outpoint).collect();
        let inputs: &[BdkOutPoint] = &inputs;
        let usable_btc_amount = self.get_uncolorable_btc_sum()?;
        let utxo_size = size.unwrap_or(UTXO_SIZE);
        if utxo_size == 0 {
            return Err(Error::InvalidAmountZero);
        }
        let possible_utxos = usable_btc_amount / utxo_size as u64;
        let max_possible_utxos: u8 = if possible_utxos > u8::MAX as u64 {
            u8::MAX
        } else {
            possible_utxos as u8
        };
        let mut btc_needed: u64 = (utxo_size as u64 * utxos_to_create as u64) + 1000;
        let mut btc_available: u64 = 0;
        let num_try_creating = min(utxos_to_create, max_possible_utxos);
        let mut addresses = vec![];
        for _i in 0..num_try_creating {
            addresses.push(self.get_new_address()?.script_pubkey());
        }
        while !addresses.is_empty() {
            match self._create_split_tx(inputs, &addresses, utxo_size, fee_rate_checked) {
                Ok(psbt) => {
                    info!(self.logger, "Create UTXOs (begin) completed");
                    return Ok(psbt.to_string());
                }
                Err(e) => {
                    (btc_needed, btc_available) = match e {
                        bdk_wallet::error::CreateTxError::CoinSelection(InsufficientFunds {
                            needed,
                            available,
                        }) => (needed.to_sat(), available.to_sat()),
                        bdk_wallet::error::CreateTxError::OutputBelowDustLimit(_) => {
                            return Err(Error::OutputBelowDustLimit);
                        }
                        _ => {
                            return Err(Error::Internal {
                                details: e.to_string(),
                            });
                        }
                    };
                    addresses.pop()
                }
            };
        }
        Err(Error::InsufficientBitcoins {
            needed: btc_needed,
            available: btc_available,
        })
    }

    /// Broadcast the provided PSBT to create new UTXOs (wasm32 async).
    ///
    /// The provided PSBT, prepared with the [`create_utxos_begin`](Wallet::create_utxos_begin)
    /// function, needs to have already been signed.
    ///
    /// This doesn't require the wallet to have private keys.
    ///
    /// Returns the number of created UTXOs, if `skip_sync` is set to true this will be 0.
    pub async fn create_utxos_end(
        &mut self,
        online: Online,
        signed_psbt: String,
        skip_sync: bool,
    ) -> Result<u8, Error> {
        info!(self.logger, "Creating UTXOs (end)...");
        self.check_online(online)?;

        let signed_psbt = Psbt::from_str(&signed_psbt)?;
        let tx = self._broadcast_psbt(signed_psbt, skip_sync).await?;

        self.database
            .set_wallet_transaction(DbWalletTransactionActMod {
                txid: ActiveValue::Set(tx.compute_txid().to_string()),
                r#type: ActiveValue::Set(WalletTransactionType::CreateUtxos),
                ..Default::default()
            })?;

        let mut num_utxos_created = 0;
        if !skip_sync {
            let bdk_utxos: Vec<LocalOutput> = self.bdk_wallet.list_unspent().collect();
            let txid = tx.compute_txid();
            for utxo in bdk_utxos.into_iter() {
                if utxo.outpoint.txid == txid && utxo.keychain == KeychainKind::External {
                    num_utxos_created += 1
                }
            }
        }

        self.update_backup_info(false)?;
        self.trigger_auto_backup();

        info!(self.logger, "Create UTXOs (end) completed");
        Ok(num_utxos_created)
    }

    /// Async version of `_prepare_rgb_psbt` for wasm32.
    ///
    /// Mirrors the native `_prepare_rgb_psbt` but stores consignment bytes and transfer info
    /// in `self.transfer_artifacts` instead of the filesystem, and uses
    /// `_get_reject_list_async` instead of `_get_reject_list`.
    async fn _prepare_rgb_psbt_wasm(
        &mut self,
        psbt: &mut Psbt,
        transfer_info_map: &mut BTreeMap<String, InfoAssetTransfer>,
        txid_key: &str,
        donation: bool,
        unspents: Vec<LocalUnspent>,
        runtime: &mut RgbRuntime,
        min_confirmations: u8,
        btc_change: Option<BtcChange>,
        rejected: &mut HashSet<Opout>,
    ) -> Result<bool, Error> {
        let mut change_utxo_option = None;
        let mut change_utxo_idx = None;

        let prev_outputs = psbt
            .unsigned_tx
            .input
            .iter()
            .map(|txin| txin.previous_output)
            .collect::<HashSet<OutPoint>>();

        let input_outpoints: Vec<Outpoint> =
            prev_outputs.iter().map(|o| Outpoint::from(*o)).collect();

        let mut all_transitions: HashMap<ContractId, Vec<Transition>> = HashMap::new();
        let mut asset_beneficiaries = bmap![];
        let mut extra_state = HashMap::<ContractId, Vec<(i32, Opout, AllocatedState)>>::new();
        let mut input_opouts: HashMap<ContractId, HashMap<Opout, AllocatedState>> = HashMap::new();
        for (asset_id, transfer_info) in transfer_info_map.iter_mut() {
            let asset_utxos = transfer_info.asset_spend.txo_map.values().cloned();
            let mut all_opout_state_vec = Vec::new();
            for (explicit_seal, opout_state_map) in runtime.contract_assignments_for(
                transfer_info.asset_info.contract_id,
                asset_utxos.clone(),
            )? {
                let txo_idx = self
                    .database
                    .get_txo(&explicit_seal.to_outpoint().into())?
                    .expect("outpoint should be in the DB")
                    .idx;
                all_opout_state_vec
                    .extend(opout_state_map.into_iter().map(|(o, s)| (txo_idx, o, s)));
            }

            all_opout_state_vec.sort_by_key(|(_, _, state)| match state {
                AllocatedState::Amount(amt) => amt.as_u64(),
                _ => 0,
            });

            let mut inputs_added = AssignmentsCollection::default();
            let mut asset_transition_builder = runtime.transition_builder(
                transfer_info.asset_info.contract_id,
                transfer_info.main_transition.clone().type_name(),
            )?;
            for (txo_idx, opout, state) in all_opout_state_vec {
                let mut should_add_as_input = !rejected.contains(&opout);
                if should_add_as_input {
                    should_add_as_input = inputs_added.opout_contributes(
                        &opout,
                        &state,
                        &transfer_info.assignments_needed,
                    );
                }
                if !should_add_as_input {
                    extra_state
                        .entry(transfer_info.asset_info.contract_id)
                        .or_default()
                        .push((txo_idx, opout, state.clone()));
                    continue;
                }

                inputs_added.add_opout_state(&opout, &state);
                transfer_info
                    .assignments_spent
                    .entry(txo_idx)
                    .or_default()
                    .push(Assignment::from_opout_and_state(opout, &state));
                asset_transition_builder =
                    asset_transition_builder.add_input(opout, state.clone())?;
                input_opouts
                    .entry(transfer_info.asset_info.contract_id)
                    .or_default()
                    .insert(opout, state);
            }

            let mut beneficiaries = vec![];
            for recipient in &transfer_info.recipients {
                let seal: BuilderSeal<GraphSeal> = match &recipient.local_recipient_data {
                    LocalRecipientData::Blind(secret_seal) => BuilderSeal::Concealed(*secret_seal),
                    LocalRecipientData::Witness(witness_data) => {
                        let graph_seal = if let Some(blinding) = witness_data.blinding {
                            GraphSeal::with_blinded_vout(witness_data.vout, blinding)
                        } else {
                            GraphSeal::new_random_vout(witness_data.vout)
                        };
                        BuilderSeal::Revealed(graph_seal)
                    }
                };

                beneficiaries.push(seal);

                match &recipient.assignment {
                    Assignment::Fungible(amt) => {
                        asset_transition_builder = asset_transition_builder.add_fungible_state(
                            RGB_STATE_ASSET_OWNER,
                            seal,
                            *amt,
                        )?;
                    }
                    Assignment::InflationRight(amt) => {
                        asset_transition_builder = asset_transition_builder.add_fungible_state(
                            RGB_STATE_INFLATION_ALLOWANCE,
                            seal,
                            *amt,
                        )?;
                    }
                    _ => unreachable!(),
                }
            }

            let change = inputs_added.change(&transfer_info.original_assignments_needed);

            if change != AssignmentsCollection::default() {
                transfer_info.change = change.clone();
                let seal = self._get_change_seal(
                    &btc_change,
                    &mut change_utxo_option,
                    &mut change_utxo_idx,
                    &input_outpoints,
                    unspents.as_slice(),
                )?;
                if change.fungible > 0 {
                    asset_transition_builder = asset_transition_builder.add_fungible_state(
                        RGB_STATE_ASSET_OWNER,
                        seal,
                        change.fungible,
                    )?;
                }
                if change.inflation > 0 {
                    asset_transition_builder = asset_transition_builder.add_fungible_state(
                        RGB_STATE_INFLATION_ALLOWANCE,
                        seal,
                        change.inflation,
                    )?;
                }
            };

            if transfer_info.main_transition == TypeOfTransition::Inflate {
                let inflation = transfer_info.original_assignments_needed.inflation;
                asset_transition_builder = asset_transition_builder
                    .add_global_state(RGB_GLOBAL_ISSUED_SUPPLY, Amount::from(inflation))
                    .unwrap()
                    .add_metadata(
                        RGB_METADATA_ALLOWED_INFLATION,
                        Amount::from(change.inflation),
                    )
                    .unwrap();
            }

            let transition = asset_transition_builder.complete_transition()?;
            all_transitions
                .entry(transfer_info.asset_info.contract_id)
                .or_default()
                .push(transition.clone());
            psbt.push_rgb_transition(transition)
                .map_err(InternalError::from)?;
            asset_beneficiaries.insert(asset_id.clone(), beneficiaries);
        }

        for id in runtime.contracts_assigning(prev_outputs.clone())? {
            if transfer_info_map.contains_key(&id.to_string()) {
                continue;
            }
            let state = runtime.contract_assignments_for(id, prev_outputs.clone())?;
            let entry = extra_state.entry(id).or_default();
            for (explicit_seal, opout_state_map) in state {
                let txo_idx = self
                    .database
                    .get_txo(&explicit_seal.to_outpoint().into())?
                    .expect("outpoint should be in the DB")
                    .idx;
                entry.extend(opout_state_map.into_iter().map(|(o, s)| (txo_idx, o, s)));
            }
        }

        let mut extra_allocations: HashMap<String, TxoAssignments> = HashMap::new();
        for (cid, opout_state_map) in extra_state {
            let schema = runtime.contract_schema(cid)?;
            for (txo_idx, opout, state) in opout_state_map {
                let transition_type = schema.default_transition_for_assignment(&opout.ty);
                let mut extra_builder = runtime.transition_builder_raw(cid, transition_type)?;
                let assignment = Assignment::from_opout_and_state(opout, &state);
                let seal = self._get_change_seal(
                    &btc_change,
                    &mut change_utxo_option,
                    &mut change_utxo_idx,
                    &input_outpoints,
                    unspents.as_slice(),
                )?;
                extra_builder = extra_builder
                    .add_input(opout, state.clone())?
                    .add_owned_state_raw(opout.ty, seal, state)?;
                let extra_transition = extra_builder.complete_transition()?;
                all_transitions
                    .entry(cid)
                    .or_default()
                    .push(extra_transition.clone());
                extra_allocations
                    .entry(cid.to_string())
                    .or_default()
                    .entry(txo_idx)
                    .or_default()
                    .push(assignment);
                psbt.push_rgb_transition(extra_transition)
                    .map_err(InternalError::from)?;
            }
        }

        psbt.set_opret_host();

        for (cid, transitions) in &all_transitions {
            for transition in transitions {
                for opout in transition.inputs() {
                    psbt.set_rgb_contract_consumer(*cid, opout, transition.id())
                        .map_err(InternalError::from)?;
                }
            }
        }

        psbt.set_rgb_close_method(CloseMethod::OpretFirst);
        let fascia = psbt.rgb_commit().map_err(|e| Error::Internal {
            details: e.to_string(),
        })?;

        let witness_txid = psbt.get_txid();

        runtime.consume_fascia(fascia, None)?;

        for (asset_id, transfer_info) in transfer_info_map.iter_mut() {
            let beneficiaries = asset_beneficiaries[asset_id].clone();
            let (beneficiaries_witness, beneficiaries_blinded) = beneficiaries.into_iter().fold(
                (Vec::new(), Vec::new()),
                |(mut witness, mut blinded), builder_seal| {
                    match builder_seal {
                        BuilderSeal::Revealed(seal) => {
                            let explicit_seal = ExplicitSeal::with(witness_txid, seal.vout);
                            witness.push(explicit_seal);
                        }
                        BuilderSeal::Concealed(secret_seal) => {
                            blinded.push(secret_seal);
                        }
                    }
                    (witness, blinded)
                },
            );

            let should_build_dag = transfer_info.main_transition == TypeOfTransition::Transfer
                && transfer_info.asset_info.reject_list_url.is_some();

            let consignment = if should_build_dag {
                let (consignment, dag_data) = runtime.transfer_with_dag(
                    transfer_info.asset_info.contract_id,
                    beneficiaries_witness,
                    beneficiaries_blinded,
                    Some(witness_txid),
                )?;

                let (reject_opouts, allow_opouts) = self
                    ._get_reject_list_async(
                        transfer_info.asset_info.reject_list_url.as_ref().unwrap(),
                    )
                    .await?;
                let asset_opouts = input_opouts
                    .get(&transfer_info.asset_info.contract_id)
                    .unwrap();
                let asset_input_opouts = asset_opouts.keys().cloned().collect();
                let to_reject = self._check_dag(
                    &dag_data,
                    &reject_opouts,
                    &allow_opouts,
                    &asset_input_opouts,
                )?;
                if !to_reject.is_empty() {
                    warn!(
                        self.logger,
                        "Found {} rejected input opout(s), retrying transfer",
                        to_reject.len()
                    );
                    for rejected_opout in &to_reject {
                        if let Some(state) = asset_opouts.get(rejected_opout) {
                            transfer_info
                                .assignments_needed
                                .add_opout_state(rejected_opout, state);
                        }
                        rejected.insert(*rejected_opout);
                    }
                    return Ok(false);
                }

                consignment
            } else {
                runtime.transfer(
                    transfer_info.asset_info.contract_id,
                    beneficiaries_witness,
                    beneficiaries_blinded,
                    Some(witness_txid),
                )?
            };

            // Store consignment bytes in memory instead of filesystem
            let mut consignment_bytes = Vec::new();
            consignment.save(&mut consignment_bytes)?;
            self.transfer_artifacts
                .entry(txid_key.to_string())
                .or_default()
                .consignment_bytes
                .insert(asset_id.clone(), consignment_bytes);

            // Store asset transfer info in memory
            self.transfer_artifacts
                .entry(txid_key.to_string())
                .or_default()
                .asset_infos
                .insert(asset_id.clone(), transfer_info.clone());
        }

        runtime.upsert_witness(witness_txid, WitnessOrd::Tentative)?;

        // Store batch transfer info in memory instead of filesystem
        let info_contents = InfoBatchTransfer {
            btc_change,
            change_utxo_idx,
            extra_allocations,
            donation,
            min_confirmations,
        };
        self.transfer_artifacts
            .entry(txid_key.to_string())
            .or_default()
            .batch_info = Some(info_contents);

        Ok(true)
    }

    async fn _prepare_transfer_psbt_wasm(
        &mut self,
        transfer_info_map: &mut BTreeMap<String, InfoAssetTransfer>,
        donation: bool,
        unspents: Vec<LocalUnspent>,
        input_unspents: &[LocalUnspent],
        witness_recipients: &Vec<(ScriptBuf, u64)>,
        fee_rate_checked: FeeRate,
        min_confirmations: u8,
        lock_time: Option<u32>,
        runtime: &mut RgbRuntime,
        rejected: &mut HashSet<Opout>,
    ) -> Result<PrepareTransferPsbtResult, Error> {
        // prepare BDK PSBT
        let mut all_inputs: HashSet<BdkOutPoint> = transfer_info_map
            .values()
            .flat_map(|ti| ti.asset_spend.txo_map.values().map(|o| o.clone().into()))
            .collect();
        self._restore_missing_bdk_inputs(&all_inputs).await?;
        let (mut psbt, btc_change) = self._try_prepare_psbt(
            input_unspents,
            &mut all_inputs,
            witness_recipients,
            fee_rate_checked,
            lock_time,
        )?;
        psbt.unsigned_tx.output[0].script_pubkey = ScriptBuf::new_op_return([]);

        // Get txid for in-memory storage key
        let txid = psbt
            .clone()
            .extract_tx()
            .map_err(InternalError::from)?
            .compute_txid()
            .to_string();

        // prepare RGB PSBT (stores artifacts in self.transfer_artifacts)
        match self
            ._prepare_rgb_psbt_wasm(
                &mut psbt,
                transfer_info_map,
                &txid,
                donation,
                unspents,
                runtime,
                min_confirmations,
                btc_change,
                rejected,
            )
            .await?
        {
            true => {}
            false => {
                return Ok(PrepareTransferPsbtResult::Retry);
            }
        }

        // Recompute txid after rgb_commit changed the OP_RETURN output
        let final_txid = psbt
            .clone()
            .extract_tx()
            .map_err(InternalError::from)?
            .compute_txid()
            .to_string();
        if final_txid != txid {
            if let Some(artifacts) = self.transfer_artifacts.remove(&txid) {
                self.transfer_artifacts.insert(final_txid, artifacts);
            }
        }

        Ok(PrepareTransferPsbtResult::Success(psbt.to_string()))
    }

    async fn _get_reject_list_async(
        &self,
        reject_list_url: &str,
    ) -> Result<(HashSet<Opout>, HashSet<Opout>), Error> {
        let list = self
            .wasm_proxy_client
            .get_reject_list(reject_list_url)
            .await?;
        let reject_list = list.trim();
        let mut opout_map = HashMap::with_capacity(reject_list.lines().count());
        for line in reject_list.lines() {
            let (is_allow, opout_str) = line.strip_prefix("!").map_or((false, line), |s| (true, s));
            let opout = match Opout::from_str(opout_str) {
                Ok(o) => o,
                Err(_) => {
                    warn!(self.logger, "Ignoring invalid opout in reject list: {line}");
                    continue;
                }
            };
            opout_map.insert(opout, is_allow);
        }
        let (allow_opouts, reject_opouts) = opout_map.into_iter().fold(
            (HashSet::new(), HashSet::new()),
            |(mut allow, mut reject), (o, allowed)| {
                if allowed {
                    allow.insert(o);
                } else {
                    reject.insert(o);
                }
                (allow, reject)
            },
        );

        Ok((reject_opouts, allow_opouts))
    }

    async fn _refuse_consignment_async(
        &self,
        proxy_url: String,
        recipient_id: String,
        updated_batch_transfer: &mut DbBatchTransferActMod,
    ) -> Result<Option<DbBatchTransfer>, Error> {
        debug!(
            self.logger,
            "Refusing invalid consignment for {recipient_id}"
        );
        match self
            .wasm_proxy_client
            .post_ack(&proxy_url, recipient_id, false)
            .await
        {
            Ok(r) => {
                debug!(self.logger, "Consignment NACK response: {:?}", r);
            }
            Err(e) if e.to_string().contains("Cannot change ACK") => {
                warn!(self.logger, "Found an ACK when trying NACK");
            }
            Err(e) => {
                error!(self.logger, "Failed to post NACK: {e}");
                return Err(e);
            }
        };
        updated_batch_transfer.status = ActiveValue::Set(TransferStatus::Failed);
        Ok(Some(
            self.database
                .update_batch_transfer(updated_batch_transfer)?,
        ))
    }

    pub(crate) async fn _get_consignment_async(
        &self,
        proxy_url: &str,
        recipient_id: String,
    ) -> Result<GetConsignmentResponse, Error> {
        let consignment_res = self
            .wasm_proxy_client
            .get_consignment(proxy_url, recipient_id)
            .await;

        if consignment_res.is_err() || consignment_res.as_ref().unwrap().result.as_ref().is_none() {
            debug!(
                self.logger,
                "Consignment GET response error: {:?}", &consignment_res
            );
            return Err(Error::NoConsignment);
        }

        let consignment_res = consignment_res.unwrap().result.unwrap();

        Ok(consignment_res)
    }

    async fn _broadcast_and_update_rgb_async(
        &mut self,
        runtime: &mut RgbRuntime,
        witness_id: RgbTxid,
        signed_psbt: Psbt,
        skip_sync: bool,
    ) -> Result<BdkTransaction, Error> {
        let tx = self._broadcast_psbt(signed_psbt, skip_sync).await?;
        runtime.upsert_witness(witness_id, WitnessOrd::Tentative)?;
        Ok(tx)
    }

    async fn _wait_consignment_async(
        &mut self,
        batch_transfer: &DbBatchTransfer,
        db_data: &DbData,
    ) -> Result<Option<DbBatchTransfer>, Error> {
        debug!(self.logger, "Waiting consignment...");

        let batch_transfer_data =
            batch_transfer.get_transfers(&db_data.asset_transfers, &db_data.transfers)?;
        let (asset_transfer, transfer) =
            self.database.get_incoming_transfer(&batch_transfer_data)?;
        let recipient_id = transfer
            .recipient_id
            .clone()
            .expect("transfer should have a recipient ID");
        debug!(self.logger, "Recipient ID: {recipient_id}");

        // check if a consignment has been posted
        let tte_data = self
            .database
            .get_transfer_transport_endpoints_data(transfer.idx)?;
        if let Some(updated_transfer) =
            self._fail_batch_transfer_if_no_endpoints(batch_transfer, &tte_data)?
        {
            return Ok(Some(updated_transfer));
        }
        let mut proxy_res = None;
        for (transfer_transport_endpoint, transport_endpoint) in tte_data {
            let result = match self
                ._get_consignment_async(&transport_endpoint.endpoint, recipient_id.clone())
                .await
            {
                Err(Error::NoConsignment) => {
                    info!(
                        self.logger,
                        "Skipping transport endpoint: {:?}", &transport_endpoint
                    );
                    continue;
                }
                Err(e) => return Err(e),
                Ok(r) => r,
            };

            proxy_res = Some((
                result.consignment,
                transport_endpoint.endpoint,
                result.txid,
                result.vout,
            ));
            let mut updated_transfer_transport_endpoint: DbTransferTransportEndpointActMod =
                transfer_transport_endpoint.into();
            updated_transfer_transport_endpoint.used = ActiveValue::Set(true);
            self.database
                .update_transfer_transport_endpoint(&mut updated_transfer_transport_endpoint)?;
            break;
        }

        let (consignment, proxy_url, txid, vout) = if let Some(res) = proxy_res {
            (res.0, res.1, res.2, res.3)
        } else {
            return Ok(None);
        };

        let mut updated_batch_transfer: DbBatchTransferActMod = batch_transfer.clone().into();

        // decode consignment bytes
        let consignment_bytes = match general_purpose::STANDARD.decode(consignment) {
            Ok(b) => b,
            Err(e) => {
                error!(self.logger, "Failed to decode consignment bytes: {e}");
                return self
                    ._refuse_consignment_async(proxy_url, recipient_id, &mut updated_batch_transfer)
                    .await;
            }
        };

        // store consignment bytes in memory
        self.received_consignments
            .insert(recipient_id.clone(), consignment_bytes.clone());

        let mut runtime = self.rgb_runtime()?;
        let consignment = match RgbTransfer::load(&consignment_bytes[..]) {
            Ok(c) => c,
            Err(e) => {
                error!(self.logger, "Failed to deserialize consignment: {e}");
                return self
                    ._refuse_consignment_async(proxy_url, recipient_id, &mut updated_batch_transfer)
                    .await;
            }
        };
        let contract_id = consignment.contract_id();
        let asset_id = contract_id.to_string();

        // validate consignment
        if let Some(aid) = asset_transfer.asset_id.clone() {
            if aid != asset_id {
                error!(
                    self.logger,
                    "Received a different asset than the expected one"
                );
                return self
                    ._refuse_consignment_async(proxy_url, recipient_id, &mut updated_batch_transfer)
                    .await;
            }
        }

        let witness_id = match RgbTxid::from_str(&txid) {
            Ok(txid) => txid,
            Err(_) => {
                error!(self.logger, "Received an invalid TXID from the proxy");
                return self
                    ._refuse_consignment_async(proxy_url, recipient_id, &mut updated_batch_transfer)
                    .await;
            }
        };

        let wasm_resolver = WasmResolver::from_consignment(&consignment, self.chain_net());
        let resolver = OffchainResolverWasm {
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
            build_opouts_dag: true,
            ..Default::default()
        };
        let valid_consignment = match consignment.clone().validate(&resolver, &validation_config) {
            Ok(consignment) => consignment,
            Err(ValidationError::InvalidConsignment(e)) => {
                error!(self.logger, "Consignment is invalid: {}", e);
                return self
                    ._refuse_consignment_async(proxy_url, recipient_id, &mut updated_batch_transfer)
                    .await;
            }
            Err(ValidationError::ResolverError(e)) => {
                warn!(self.logger, "Network error during consignment validation");
                return Err(Error::Network {
                    details: e.to_string(),
                });
            }
        };
        let validation_status = valid_consignment.validation_status();
        let validity = validation_status.validity();
        debug!(self.logger, "Consignment validity: {:?}", validity);

        // check the info provided via the proxy is correct
        if let Some(anchored_bundle) = consignment
            .bundles
            .iter()
            .find(|ab| ab.witness_id() == witness_id)
        {
            if let Some(RecipientTypeFull::Witness { .. }) = transfer.recipient_type {
                if let Some(vout) = vout {
                    if let PubWitness::Tx(tx) = &anchored_bundle.pub_witness {
                        if let Some(output) = tx.output.get(vout as usize) {
                            let script_pubkey =
                                script_buf_from_recipient_id(recipient_id.clone())?.unwrap();
                            if output.script_pubkey != script_pubkey {
                                error!(
                                    self.logger,
                                    "The provided vout pays an incorrect script pubkey"
                                );
                                return self
                                    ._refuse_consignment_async(
                                        proxy_url,
                                        recipient_id,
                                        &mut updated_batch_transfer,
                                    )
                                    .await;
                            }
                        } else {
                            error!(self.logger, "Cannot find the expected outpoint");
                            return self
                                ._refuse_consignment_async(
                                    proxy_url,
                                    recipient_id,
                                    &mut updated_batch_transfer,
                                )
                                .await;
                        }
                    } else {
                        error!(self.logger, "Consignment is missing the witness TX");
                        return self
                            ._refuse_consignment_async(
                                proxy_url,
                                recipient_id,
                                &mut updated_batch_transfer,
                            )
                            .await;
                    }
                } else {
                    error!(
                        self.logger,
                        "The vout should be provided when receiving via witness"
                    );
                    return self
                        ._refuse_consignment_async(
                            proxy_url,
                            recipient_id,
                            &mut updated_batch_transfer,
                        )
                        .await;
                }
            }
        } else {
            error!(
                self.logger,
                "Cannot find the provided TXID in the consignment"
            );
            return self
                ._refuse_consignment_async(proxy_url, recipient_id, &mut updated_batch_transfer)
                .await;
        }

        if !self.supports_schema(&asset_schema) {
            error!(
                self.logger,
                "The wallet doesn't support the provided schema: {}", asset_schema
            );
            return self
                ._refuse_consignment_async(proxy_url, recipient_id, &mut updated_batch_transfer)
                .await;
        }

        let known_concealed = if let Some(RecipientTypeFull::Blind { .. }) = transfer.recipient_type
        {
            let beneficiary = XChainNet::<Beneficiary>::from_str(&recipient_id)
                .expect("saved recipient ID is invalid");
            match beneficiary.into_inner() {
                Beneficiary::BlindedSeal(secret_seal) => Some(secret_seal),
                _ => unreachable!("beneficiary is blinded"),
            }
        } else {
            None
        };
        let received =
            self.extract_received_assignments(&consignment, witness_id, vout, known_concealed);
        if received.is_empty() {
            error!(self.logger, "Cannot find any receiving assignment");
            return self
                ._refuse_consignment_async(proxy_url, recipient_id, &mut updated_batch_transfer)
                .await;
        };

        if asset_schema == AssetSchema::Ifa {
            let url = if let Ok(ass) = self.database.check_asset_exists(asset_id.clone()) {
                ass.reject_list_url
            } else {
                let contract = IfaWrapper::with(valid_consignment.contract_data());
                contract.reject_list_url().map(|u| u.to_string())
            };
            if let Some(url) = &url {
                let (reject_opouts, allow_opouts) = self._get_reject_list_async(url).await?;

                let to_reject = self._check_dag(
                    validation_status
                        .dag_data_opt
                        .as_ref()
                        .expect("build_opouts_dag is true"),
                    &reject_opouts,
                    &allow_opouts,
                    &received.clone().into_keys().collect(),
                )?;

                if !to_reject.is_empty() {
                    error!(
                        self.logger,
                        "Found {} opout(s) that must be rejected",
                        to_reject.len()
                    );
                    return self
                        ._refuse_consignment_async(
                            proxy_url,
                            recipient_id,
                            &mut updated_batch_transfer,
                        )
                        .await;
                } else {
                    info!(
                        self.logger,
                        "Didn't find any opout(s) that should be rejected"
                    );
                }
            }
        }

        // add asset info to transfer if missing
        if asset_transfer.asset_id.is_none() {
            let exists_check = self.database.check_asset_exists(asset_id.clone());
            if exists_check.is_err() {
                // unknown asset
                debug!(self.logger, "Receiving unknown contract...");
                let valid_contract = valid_consignment.clone().into_valid_contract();

                let mut attachments = vec![];
                match asset_schema {
                    AssetSchema::Nia => {
                        let contract_data = valid_contract.contract_data();
                        let contract = NiaWrapper::with(contract_data);
                        if let Some(attachment) = contract.contract_terms().media {
                            attachments.push(attachment)
                        }
                    }
                    AssetSchema::Ifa => {
                        let contract_data = valid_contract.contract_data();
                        let contract = IfaWrapper::with(contract_data);
                        if let Some(attachment) = contract.contract_terms().media {
                            attachments.push(attachment)
                        }
                    }
                };
                for attachment in attachments {
                    let digest = hex::encode(attachment.digest);
                    // on wasm32, skip media file download/storage (no filesystem)
                    // media digest is recorded in DB but file is not stored locally
                    let media_res = self
                        .wasm_proxy_client
                        .get_media(&proxy_url, digest.clone())
                        .await?;
                    if let Some(media_res) = media_res.result {
                        let file_bytes = general_purpose::STANDARD
                            .decode(media_res)
                            .map_err(InternalError::from)?;
                        let file_hash: sha256::Hash = Sha256Hash::hash(&file_bytes[..]);
                        let actual_digest = file_hash.to_string();
                        if digest != actual_digest {
                            error!(
                                self.logger,
                                "Attached file has a different hash than the one in the contract"
                            );
                            return self
                                ._refuse_consignment_async(
                                    proxy_url,
                                    recipient_id,
                                    &mut updated_batch_transfer,
                                )
                                .await;
                        }
                        // on wasm32, we validate the media but don't write to filesystem
                    } else {
                        error!(
                            self.logger,
                            "Cannot find the media file but the contract defines one"
                        );
                        return self
                            ._refuse_consignment_async(
                                proxy_url,
                                recipient_id,
                                &mut updated_batch_transfer,
                            )
                            .await;
                    }
                }

                let wasm_resolver_import =
                    WasmResolver::from_consignment(&consignment, self.chain_net());
                runtime
                    .import_contract(valid_contract.clone(), &wasm_resolver_import)
                    .expect("failure importing received contract");
                debug!(self.logger, "Contract registered");
                self.save_new_asset_internal(
                    &runtime,
                    contract_id,
                    asset_schema,
                    valid_contract,
                    valid_consignment,
                )?;
            }

            let mut updated_asset_transfer: DbAssetTransferActMod = asset_transfer.clone().into();
            updated_asset_transfer.asset_id = ActiveValue::Set(Some(asset_id.clone()));
            self.database
                .update_asset_transfer(&mut updated_asset_transfer)?;
        }

        debug!(
            self.logger,
            "Consignment is valid. Received '{:?}' of contract '{}'", received, asset_id
        );

        match self
            .wasm_proxy_client
            .post_ack(&proxy_url, recipient_id, true)
            .await
        {
            Ok(r) => {
                debug!(self.logger, "Consignment ACK response: {:?}", r);
            }
            Err(e) if e.to_string().contains("Cannot change ACK") => {
                warn!(self.logger, "Found an NACK when trying ACK");
            }
            Err(e) => {
                error!(self.logger, "Failed to post ACK: {e}");
                return Err(e);
            }
        };

        let utxo_idx = match transfer.recipient_type {
            Some(RecipientTypeFull::Blind { ref unblinded_utxo }) => {
                self.database
                    .get_txo(unblinded_utxo)?
                    .expect("utxo must exist")
                    .idx
            }
            Some(RecipientTypeFull::Witness { .. }) => {
                let mut updated_transfer: DbTransferActMod = transfer.clone().into();
                updated_transfer.recipient_type =
                    ActiveValue::Set(Some(RecipientTypeFull::Witness { vout }));
                self.database.update_transfer(&mut updated_transfer)?;
                let db_utxo = DbTxoActMod {
                    txid: ActiveValue::Set(txid.clone()),
                    vout: ActiveValue::Set(vout.unwrap()),
                    btc_amount: ActiveValue::Set(s!("0")),
                    spent: ActiveValue::Set(false),
                    exists: ActiveValue::Set(false),
                    pending_witness: ActiveValue::Set(true),
                    ..Default::default()
                };
                self.database.set_txo(db_utxo)?
            }
            _ => return Err(InternalError::Unexpected.into()),
        };
        for assignment in received.into_values() {
            let db_coloring = DbColoringActMod {
                txo_idx: ActiveValue::Set(utxo_idx),
                asset_transfer_idx: ActiveValue::Set(asset_transfer.idx),
                r#type: ActiveValue::Set(ColoringType::Receive),
                assignment: ActiveValue::Set(assignment),
                ..Default::default()
            };
            self.database.set_coloring(db_coloring)?;
        }

        updated_batch_transfer.txid = ActiveValue::Set(Some(txid));
        updated_batch_transfer.status = ActiveValue::Set(TransferStatus::WaitingConfirmations);

        Ok(Some(
            self.database
                .update_batch_transfer(&mut updated_batch_transfer)?,
        ))
    }

    async fn _wait_ack_async(
        &mut self,
        batch_transfer: &DbBatchTransfer,
        db_data: &mut DbData,
        skip_sync: bool,
    ) -> Result<Option<DbBatchTransfer>, Error> {
        debug!(self.logger, "Waiting ACK...");

        let mut batch_transfer_data =
            batch_transfer.get_transfers(&db_data.asset_transfers, &db_data.transfers)?;
        for asset_transfer_data in batch_transfer_data.asset_transfers_data.iter_mut() {
            for transfer in asset_transfer_data.transfers.iter_mut() {
                if transfer.ack.is_some() {
                    continue;
                }
                let tte_data = self
                    .database
                    .get_transfer_transport_endpoints_data(transfer.idx)?;
                if let Some(updated_transfer) =
                    self._fail_batch_transfer_if_no_endpoints(batch_transfer, &tte_data)?
                {
                    return Ok(Some(updated_transfer));
                }
                let (_, transport_endpoint) = tte_data
                    .clone()
                    .into_iter()
                    .find(|(tte, _ce)| tte.used)
                    .expect("there should be 1 used TTE");
                let proxy_url = transport_endpoint.endpoint.clone();
                let recipient_id = transfer
                    .recipient_id
                    .clone()
                    .expect("transfer should have a recipient ID");
                debug!(self.logger, "Recipient ID: {recipient_id}");
                let ack_res = self
                    .wasm_proxy_client
                    .get_ack(&proxy_url, recipient_id)
                    .await?;
                debug!(self.logger, "Consignment ACK/NACK response: {:?}", ack_res);

                if ack_res.result.is_some() {
                    let mut updated_transfer: DbTransferActMod = transfer.clone().into();
                    updated_transfer.ack = ActiveValue::Set(ack_res.result);
                    self.database.update_transfer(&mut updated_transfer)?;
                    transfer.ack = ack_res.result;
                }
            }
        }

        let mut updated_batch_transfer: DbBatchTransferActMod = batch_transfer.clone().into();
        let mut batch_transfer_transfers: Vec<DbTransfer> = vec![];
        batch_transfer_data
            .asset_transfers_data
            .iter()
            .for_each(|atd| batch_transfer_transfers.extend(atd.transfers.clone()));
        if batch_transfer_transfers
            .iter()
            .any(|t| t.ack == Some(false))
        {
            updated_batch_transfer.status = ActiveValue::Set(TransferStatus::Failed);
        } else if batch_transfer_transfers.iter().all(|t| t.ack == Some(true)) {
            let txid = batch_transfer
                .txid
                .as_ref()
                .expect("batch transfer should have a TXID");
            // read signed PSBT from in-memory transfer artifacts
            let signed_psbt = self
                .transfer_artifacts
                .get(txid)
                .and_then(|a| a.signed_psbt.as_ref())
                .ok_or_else(|| Error::Internal {
                    details: s!("signed PSBT not found in transfer artifacts"),
                })?;
            let signed_psbt = Psbt::from_str(signed_psbt)?;
            let mut runtime = self.rgb_runtime()?;
            let witness_id = RgbTxid::from_str(&txid.to_string()).unwrap();
            self._broadcast_and_update_rgb_async(&mut runtime, witness_id, signed_psbt, skip_sync)
                .await?;
            // Clean up: signed PSBT no longer needed after broadcast
            self.transfer_artifacts.remove(txid);
            updated_batch_transfer.status = ActiveValue::Set(TransferStatus::WaitingConfirmations);
        } else {
            return Ok(None);
        }

        Ok(Some(
            self.database
                .update_batch_transfer(&mut updated_batch_transfer)?,
        ))
    }

    async fn _wait_confirmations_async(
        &mut self,
        batch_transfer: &DbBatchTransfer,
        db_data: &DbData,
        incoming: bool,
        skip_sync: bool,
    ) -> Result<Option<DbBatchTransfer>, Error> {
        debug!(self.logger, "Waiting confirmations...");
        let txid = batch_transfer
            .txid
            .clone()
            .expect("batch transfer should have a TXID");
        debug!(
            self.logger,
            "Getting details of transaction with ID '{}'...", txid
        );
        let confirmations = self.indexer().get_tx_confirmations(&txid).await?;
        debug!(self.logger, "Confirmations: {:?}", confirmations);

        if let Some(confirmations) = confirmations {
            if confirmations < batch_transfer.min_confirmations as u64 {
                return Ok(None);
            }
        } else {
            return Ok(None);
        }

        if incoming {
            let batch_transfer_data =
                batch_transfer.get_transfers(&db_data.asset_transfers, &db_data.transfers)?;
            let (asset_transfer, transfer) =
                self.database.get_incoming_transfer(&batch_transfer_data)?;
            let recipient_id = transfer
                .clone()
                .recipient_id
                .expect("transfer should have a recipient ID");
            debug!(self.logger, "Recipient ID: {recipient_id}");

            // load consignment from in-memory storage
            let consignment_bytes = self
                .received_consignments
                .get(&recipient_id)
                .ok_or_else(|| Error::Internal {
                    details: s!("consignment not found in memory"),
                })?
                .clone();
            let consignment =
                RgbTransfer::load(&consignment_bytes[..]).map_err(InternalError::from)?;

            if let Some(RecipientTypeFull::Witness { vout }) = transfer.recipient_type {
                if !skip_sync {
                    self.sync_db_txos(false).await?;
                }
                let outpoint = Outpoint {
                    txid: txid.clone(),
                    vout: vout.unwrap(),
                };
                let txo = self.database.get_txo(&outpoint)?.expect("txo must exist");
                let mut txo: DbTxoActMod = txo.into();
                txo.pending_witness = ActiveValue::Set(false);
                self.database.update_txo(txo)?;
            }

            // accept consignment using WasmResolver
            let wasm_resolver = WasmResolver::from_consignment(&consignment, self.chain_net());
            let asset_schema: AssetSchema = consignment.schema_id().try_into()?;
            let validation_config = ValidationConfig {
                chain_net: self.chain_net(),
                trusted_typesystem: asset_schema.types(),
                ..Default::default()
            };
            let valid_consignment = consignment
                .validate(&wasm_resolver, &validation_config)
                .map_err(|_| InternalError::Unexpected)?;
            let validation_status = valid_consignment.validation_status();
            let mut runtime = self.rgb_runtime()?;
            runtime.accept_transfer(valid_consignment.clone(), &wasm_resolver)?;
            if asset_schema == AssetSchema::Ifa {
                let contract_id = valid_consignment.contract_id();
                let contract_wrapper =
                    runtime.contract_wrapper::<InflatableFungibleAsset>(contract_id)?;
                let known_circulating_supply = contract_wrapper.total_issued_supply().into();
                let asset_id = asset_transfer.asset_id.unwrap();
                let db_asset = self.database.get_asset(asset_id).unwrap().unwrap();
                let db_known_circulating_supply = db_asset
                    .known_circulating_supply
                    .as_ref()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap();
                if db_known_circulating_supply < known_circulating_supply {
                    let mut updated_asset: DbAssetActMod = db_asset.into();
                    updated_asset.known_circulating_supply =
                        ActiveValue::Set(Some(known_circulating_supply.to_string()));
                    self.database.update_asset(&mut updated_asset)?;
                }
            }

            match validation_status.validity() {
                Validity::Valid => {}
                Validity::Warnings => {
                    if let Warning::UnsafeHistory(ref unsafe_history) =
                        validation_status.warnings[0]
                    {
                        warn!(
                            self.logger,
                            "Cannot accept transfer because of unsafe history: {unsafe_history:?}"
                        );
                        return Ok(None);
                    }
                }
            }
        }

        let mut updated_batch_transfer: DbBatchTransferActMod = batch_transfer.clone().into();
        updated_batch_transfer.status = ActiveValue::Set(TransferStatus::Settled);
        let updated = self
            .database
            .update_batch_transfer(&mut updated_batch_transfer)?;

        // Clean up: received consignment no longer needed after settle
        if incoming {
            let batch_transfer_data =
                batch_transfer.get_transfers(&db_data.asset_transfers, &db_data.transfers)?;
            if let Ok((_, transfer)) = self.database.get_incoming_transfer(&batch_transfer_data) {
                if let Some(recipient_id) = &transfer.recipient_id {
                    self.received_consignments.remove(recipient_id);
                }
            }
        }

        Ok(Some(updated))
    }

    async fn _wait_counterparty_async(
        &mut self,
        transfer: &DbBatchTransfer,
        db_data: &mut DbData,
        incoming: bool,
        skip_sync: bool,
    ) -> Result<Option<DbBatchTransfer>, Error> {
        if incoming {
            self._wait_consignment_async(transfer, db_data).await
        } else {
            self._wait_ack_async(transfer, db_data, skip_sync).await
        }
    }

    async fn _refresh_transfer_async(
        &mut self,
        transfer: &DbBatchTransfer,
        db_data: &mut DbData,
        filter: &[RefreshFilter],
        skip_sync: bool,
    ) -> Result<Option<DbBatchTransfer>, Error> {
        debug!(self.logger, "Refreshing transfer: {:?}", transfer);
        let incoming = transfer.incoming(&db_data.asset_transfers, &db_data.transfers)?;
        if !filter.is_empty() {
            let requested = RefreshFilter {
                status: RefreshTransferStatus::try_from(transfer.status).expect("pending status"),
                incoming,
            };
            if !filter.contains(&requested) {
                return Ok(None);
            }
        }
        match transfer.status {
            TransferStatus::WaitingCounterparty => {
                self._wait_counterparty_async(transfer, db_data, incoming, skip_sync)
                    .await
            }
            TransferStatus::WaitingConfirmations => {
                self._wait_confirmations_async(transfer, db_data, incoming, skip_sync)
                    .await
            }
            _ => Ok(None),
        }
    }

    /// Update pending RGB transfers (wasm32 async).
    ///
    /// See native [`refresh`](Wallet::refresh) for full documentation.
    pub async fn refresh(
        &mut self,
        online: Online,
        asset_id: Option<String>,
        filter: Vec<RefreshFilter>,
        skip_sync: bool,
    ) -> Result<RefreshResult, Error> {
        if let Some(aid) = asset_id.clone() {
            info!(self.logger, "Refreshing asset {}...", aid);
            self.database.check_asset_exists(aid)?;
        } else {
            info!(self.logger, "Refreshing assets...");
        }
        self.check_online(online)?;

        let mut db_data = self.database.get_db_data(false)?;

        if asset_id.is_some() {
            let batch_transfers_ids: Vec<i32> = db_data
                .asset_transfers
                .iter()
                .filter(|t| t.asset_id == asset_id)
                .map(|t| t.batch_transfer_idx)
                .collect();
            db_data
                .batch_transfers
                .retain(|t| batch_transfers_ids.contains(&t.idx));
        };
        db_data.batch_transfers.retain(|t| t.pending());

        let mut refresh_result = HashMap::new();
        for transfer in db_data.batch_transfers.clone().into_iter() {
            let mut failure = None;
            let mut updated_status = None;
            match self
                ._refresh_transfer_async(&transfer, &mut db_data, &filter, skip_sync)
                .await
            {
                Ok(Some(updated_transfer)) => updated_status = Some(updated_transfer.status),
                Err(e) => failure = Some(e),
                _ => {}
            }
            refresh_result.insert(
                transfer.idx,
                RefreshedTransfer {
                    updated_status,
                    failure,
                },
            );
        }

        info!(self.logger, "Refresh completed");
        Ok(refresh_result)
    }

    async fn _try_fail_batch_transfer_async(
        &mut self,
        batch_transfer: &DbBatchTransfer,
        throw_err: bool,
        db_data: &mut DbData,
    ) -> Result<(), Error> {
        let updated_batch_transfer = match self
            ._refresh_transfer_async(batch_transfer, db_data, &[], true)
            .await
        {
            Err(Error::MinFeeNotMet { txid: _ }) | Err(Error::MaxFeeExceeded { txid: _ }) => {
                Ok(None)
            }
            Err(e) => Err(e),
            Ok(v) => Ok(v),
        }?;
        // fail transfer if the status didn't change after a refresh
        if updated_batch_transfer.is_none() {
            self._fail_batch_transfer(batch_transfer)?;
        } else if throw_err {
            return Err(Error::CannotFailBatchTransfer);
        }

        Ok(())
    }

    /// Fail pending RGB transfers (wasm32 async).
    ///
    /// See native [`fail_transfers`](Wallet::fail_transfers) for full documentation.
    pub async fn fail_transfers(
        &mut self,
        online: Online,
        batch_transfer_idx: Option<i32>,
        no_asset_only: bool,
        skip_sync: bool,
    ) -> Result<bool, Error> {
        info!(
            self.logger,
            "Failing batch transfer with idx {:?}...", batch_transfer_idx
        );
        self.check_online(online)?;

        if !skip_sync {
            self.sync_db_txos(false).await?;
        }

        let mut db_data = self.database.get_db_data(false)?;
        let mut transfers_changed = false;

        if let Some(batch_transfer_idx) = batch_transfer_idx {
            let batch_transfer = &self
                .database
                .get_batch_transfer_or_fail(batch_transfer_idx, &db_data.batch_transfers)?;

            if !batch_transfer.waiting_counterparty() {
                return Err(Error::CannotFailBatchTransfer);
            }

            if no_asset_only {
                let asset_transfers =
                    batch_transfer.get_asset_transfers(&db_data.asset_transfers)?;
                let connected_assets = asset_transfers.iter().any(|t| t.asset_id.is_some());
                if connected_assets {
                    return Err(Error::CannotFailBatchTransfer);
                }
            }

            transfers_changed = true;
            self._try_fail_batch_transfer_async(batch_transfer, true, &mut db_data)
                .await?
        } else {
            // fail all transfers in status WaitingCounterparty
            let now = now().unix_timestamp();
            let mut expired_batch_transfers: Vec<DbBatchTransfer> = db_data
                .batch_transfers
                .clone()
                .into_iter()
                .filter(|t| t.waiting_counterparty() && t.expiration.unwrap_or(now) < now)
                .collect();
            for batch_transfer in expired_batch_transfers.iter_mut() {
                if no_asset_only {
                    let connected_assets = batch_transfer
                        .get_asset_transfers(&db_data.asset_transfers)?
                        .iter()
                        .any(|t| t.asset_id.is_some());
                    if connected_assets {
                        continue;
                    }
                }
                transfers_changed = true;
                self._try_fail_batch_transfer_async(batch_transfer, false, &mut db_data)
                    .await?
            }
        }

        info!(self.logger, "Fail transfers completed");
        Ok(transfers_changed)
    }

    /// Prepare the PSBT to send RGB assets (wasm32 async).
    ///
    /// See native [`send_begin`](Wallet::send_begin) for full documentation.
    pub async fn send_begin(
        &mut self,
        online: Online,
        recipient_map: HashMap<String, Vec<Recipient>>,
        donation: bool,
        fee_rate: u64,
        min_confirmations: u8,
        lock_time: Option<u32>,
    ) -> Result<String, Error> {
        info!(self.logger, "Sending (begin) to: {:?}...", recipient_map);

        let (fee_rate_checked, unspents, input_unspents, mut runtime) =
            self._get_transfer_begin_data(online, fee_rate)?;

        let chainnet: ChainNet = self.bitcoin_network().into();
        let mut witness_recipients: Vec<(ScriptBuf, u64)> = vec![];
        let mut recipient_vout = 1;
        let main_transition = TypeOfTransition::Transfer;
        let mut assets_data: BTreeMap<String, (AssetInfo, AssignmentsCollection)> = BTreeMap::new();
        let mut local_recipients: BTreeMap<String, Vec<LocalRecipient>> = BTreeMap::new();
        for (asset_id, recipients) in &recipient_map {
            let asset = self.database.check_asset_exists(asset_id.clone())?;
            let schema = asset.schema;
            self.check_schema_support(&schema)?;

            let mut original_assignments_needed = AssignmentsCollection::default();
            for recipient in recipients.clone() {
                self.check_transport_endpoints(&recipient.transport_endpoints)?;
                match (&recipient.assignment, schema) {
                    (Assignment::Fungible(amt), AssetSchema::Nia | AssetSchema::Ifa) => {
                        if *amt == 0 {
                            return Err(Error::InvalidAmountZero);
                        }
                    }
                    (Assignment::InflationRight(amt), AssetSchema::Ifa) => {
                        if *amt == 0 {
                            return Err(Error::InvalidAmountZero);
                        }
                    }
                    _ => {
                        return Err(Error::InvalidAssignment);
                    }
                }
                let mut transport_endpoints: Vec<LocalTransportEndpoint> = vec![];
                let mut found_valid = false;
                for endpoint_str in &recipient.transport_endpoints {
                    let transport_endpoint = TransportEndpoint::new(endpoint_str.clone())?;
                    let mut local_transport_endpoint = LocalTransportEndpoint {
                        transport_type: transport_endpoint.transport_type,
                        endpoint: transport_endpoint.endpoint.clone(),
                        used: false,
                        usable: false,
                    };
                    if crate::utils::check_proxy_async(&transport_endpoint.endpoint)
                        .await
                        .is_ok()
                    {
                        local_transport_endpoint.usable = true;
                        found_valid = true;
                    }
                    transport_endpoints.push(local_transport_endpoint);
                }

                if !found_valid {
                    return Err(Error::InvalidTransportEndpoints {
                        details: s!("no valid transport endpoints"),
                    });
                }

                let xchainnet_beneficiary =
                    XChainNet::<Beneficiary>::from_str(&recipient.recipient_id)
                        .map_err(|_| Error::InvalidRecipientID)?;

                if xchainnet_beneficiary.chain_network() != chainnet {
                    return Err(Error::InvalidRecipientNetwork);
                }

                let local_recipient_data = match xchainnet_beneficiary.into_inner() {
                    Beneficiary::BlindedSeal(secret_seal) => {
                        if recipient.witness_data.is_some() {
                            return Err(Error::InvalidRecipientData {
                                details: s!("cannot provide witness data for a blinded recipient"),
                            });
                        }
                        LocalRecipientData::Blind(secret_seal)
                    }
                    Beneficiary::WitnessVout(pay_2_vout, _) => {
                        if let Some(ref witness_data) = recipient.witness_data {
                            let script_buf = pay_2_vout.to_script();
                            witness_recipients.push((script_buf.clone(), witness_data.amount_sat));
                            let local_witness_data = LocalWitnessData {
                                amount_sat: witness_data.amount_sat,
                                blinding: witness_data.blinding,
                                vout: recipient_vout,
                            };
                            recipient_vout += 1;
                            LocalRecipientData::Witness(local_witness_data)
                        } else {
                            return Err(Error::InvalidRecipientData {
                                details: s!("missing witness data for a witness recipient"),
                            });
                        }
                    }
                };

                local_recipients
                    .entry(asset_id.clone())
                    .or_default()
                    .push(LocalRecipient {
                        recipient_id: recipient.recipient_id,
                        local_recipient_data,
                        assignment: recipient.assignment.clone(),
                        transport_endpoints,
                    });

                recipient
                    .assignment
                    .add_to_assignments(&mut original_assignments_needed);
            }
            let contract_id = ContractId::from_str(asset_id).expect("invalid contract ID");
            assets_data.insert(
                asset_id.clone(),
                (
                    AssetInfo {
                        contract_id,
                        reject_list_url: asset.reject_list_url,
                    },
                    original_assignments_needed,
                ),
            );
        }

        // Check for duplicate recipient IDs
        let receive_ids: Vec<String> = recipient_map
            .values()
            .flatten()
            .map(|r| r.recipient_id.clone())
            .collect();
        let mut receive_ids_dedup = receive_ids.clone();
        receive_ids_dedup.sort();
        receive_ids_dedup.dedup();
        if receive_ids.len() != receive_ids_dedup.len() {
            return Err(Error::RecipientIDDuplicated);
        }

        let mut rejected = HashSet::new();
        let mut transfer_info_map: BTreeMap<String, InfoAssetTransfer> = BTreeMap::new();

        let psbt_string = loop {
            for asset_id in recipient_map.keys() {
                let (asset_info, original_assignments_needed) =
                    assets_data.get(asset_id).unwrap().clone();
                let assignments_needed =
                    if let Some(existing_info) = transfer_info_map.get(asset_id) {
                        existing_info.assignments_needed.clone()
                    } else {
                        original_assignments_needed.clone()
                    };

                let asset_spend = self._select_rgb_inputs(
                    asset_id.clone(),
                    &assignments_needed,
                    input_unspents.clone(),
                )?;

                let transfer_info = InfoAssetTransfer {
                    asset_info,
                    recipients: local_recipients[asset_id].clone(),
                    asset_spend,
                    change: AssignmentsCollection::default(),
                    original_assignments_needed,
                    assignments_needed,
                    assignments_spent: HashMap::new(),
                    main_transition,
                };
                transfer_info_map.insert(asset_id.clone(), transfer_info);
            }

            match self
                ._prepare_transfer_psbt_wasm(
                    &mut transfer_info_map,
                    donation,
                    unspents.clone(),
                    &input_unspents,
                    &witness_recipients,
                    fee_rate_checked,
                    min_confirmations,
                    lock_time,
                    &mut runtime,
                    &mut rejected,
                )
                .await?
            {
                PrepareTransferPsbtResult::Retry => continue,
                PrepareTransferPsbtResult::Success(psbt_string) => break psbt_string,
            }
        };

        info!(self.logger, "Send (begin) completed");
        Ok(psbt_string)
    }

    /// Complete the send operation (wasm32 async).
    ///
    /// See native [`send_end`](Wallet::send_end) for full documentation.
    pub async fn send_end(
        &mut self,
        online: Online,
        signed_psbt: String,
        skip_sync: bool,
    ) -> Result<OperationResult, Error> {
        info!(self.logger, "Sending (end)...");
        self.check_online(online)?;

        let psbt = Psbt::from_str(&signed_psbt)?;
        let txid = psbt
            .clone()
            .extract_tx()
            .map_err(InternalError::from)?
            .compute_txid()
            .to_string();

        // Clone data out of transfer_artifacts to avoid borrow conflicts
        let artifacts = self
            .transfer_artifacts
            .remove(&txid)
            .ok_or(Error::UnknownTransfer { txid: txid.clone() })?;
        let info_contents = artifacts.batch_info.ok_or(Error::Internal {
            details: s!("missing batch info in transfer artifacts"),
        })?;
        let mut transfer_info_map = artifacts.asset_infos;
        let consignment_bytes_map = artifacts.consignment_bytes;

        // Store signed PSBT for later use by refresh() → _wait_ack_async()
        // (replaces filesystem "signed.psbt" from upstream)
        self.transfer_artifacts.insert(
            txid.clone(),
            TransferArtifacts {
                signed_psbt: Some(signed_psbt.clone()),
                ..Default::default()
            },
        );

        // Post consignment(s) to proxy server
        for (asset_id, info_contents_asset) in transfer_info_map.iter_mut() {
            let consignment_bytes = consignment_bytes_map.get(asset_id).ok_or(Error::Internal {
                details: s!("missing consignment bytes"),
            })?;

            for recipient in &mut info_contents_asset.recipients {
                let recipient_id = &recipient.recipient_id;
                let mut found_valid = false;
                for transport_endpoint in recipient.transport_endpoints.iter_mut() {
                    if transport_endpoint.used {
                        found_valid = true;
                        break;
                    }
                    if transport_endpoint.transport_type != TransportType::JsonRpc
                        || !transport_endpoint.usable
                    {
                        debug!(
                            self.logger,
                            "Skipping transport endpoint {:?}", transport_endpoint
                        );
                        continue;
                    }
                    let proxy_url = transport_endpoint.endpoint.clone();
                    debug!(
                        self.logger,
                        "Posting consignment for recipient ID: {recipient_id}"
                    );
                    let vout = recipient.local_recipient_data.vout();
                    match self
                        .wasm_proxy_client
                        .post_consignment(
                            &proxy_url,
                            recipient_id.clone(),
                            consignment_bytes,
                            txid.clone(),
                            vout,
                        )
                        .await
                    {
                        Err(Error::RecipientIDAlreadyUsed) => {
                            return Err(Error::RecipientIDAlreadyUsed);
                        }
                        Err(_) => continue,
                        Ok(res) => {
                            if let Some(err) = res.error {
                                if err.message.contains("already used") {
                                    return Err(Error::RecipientIDAlreadyUsed);
                                }
                                continue;
                            }
                        }
                    }

                    transport_endpoint.used = true;
                    found_valid = true;
                    break;
                }
                if !found_valid {
                    return Err(Error::NoValidTransportEndpoint);
                }
            }
        }

        let batch_transfer_idx = if info_contents.donation {
            // Broadcast immediately for donations
            let mut runtime = self.rgb_runtime()?;
            let tx = self._broadcast_psbt(psbt, skip_sync).await?;
            let _ = tx; // ensure broadcast completes
            runtime.upsert_witness(RgbTxid::from_str(&txid).unwrap(), WitnessOrd::Tentative)?;
            self._save_transfers(
                txid.clone(),
                &transfer_info_map,
                info_contents.extra_allocations,
                info_contents.change_utxo_idx,
                info_contents.btc_change,
                TransferStatus::WaitingConfirmations,
                info_contents.min_confirmations,
            )?
        } else {
            self._save_transfers(
                txid.clone(),
                &transfer_info_map,
                info_contents.extra_allocations,
                info_contents.change_utxo_idx,
                info_contents.btc_change,
                TransferStatus::WaitingCounterparty,
                info_contents.min_confirmations,
            )?
        };

        self.update_backup_info(false)?;
        self.trigger_auto_backup();

        info!(self.logger, "Send (end) completed");
        Ok(OperationResult {
            txid,
            batch_transfer_idx,
        })
    }

    /// Post consignments prepared by [`Wallet::send_begin`] without completing or broadcasting
    /// the transfer.
    ///
    /// This is required for protocols such as Lightning channel funding where the counterparty
    /// must validate the consignment before it is safe to broadcast the funding transaction.
    /// The transfer artifacts remain available for a later [`Wallet::send_end`] call.
    pub async fn post_pending_consignments(&mut self, txid: String) -> Result<(), Error> {
        let artifacts = self
            .transfer_artifacts
            .get_mut(&txid)
            .ok_or(Error::UnknownTransfer { txid: txid.clone() })?;

        for (asset_id, info_contents_asset) in artifacts.asset_infos.iter_mut() {
            let consignment_bytes =
                artifacts
                    .consignment_bytes
                    .get(asset_id)
                    .ok_or(Error::Internal {
                        details: s!("missing consignment bytes"),
                    })?;

            for recipient in &mut info_contents_asset.recipients {
                let mut found_valid = false;
                for transport_endpoint in recipient.transport_endpoints.iter_mut() {
                    if transport_endpoint.used {
                        found_valid = true;
                        break;
                    }
                    if transport_endpoint.transport_type != TransportType::JsonRpc
                        || !transport_endpoint.usable
                    {
                        continue;
                    }
                    let vout = recipient.local_recipient_data.vout();
                    let response = self
                        .wasm_proxy_client
                        .post_consignment(
                            &transport_endpoint.endpoint,
                            recipient.recipient_id.clone(),
                            consignment_bytes,
                            txid.clone(),
                            vout,
                        )
                        .await;
                    let already_used = match &response {
                        Err(Error::RecipientIDAlreadyUsed) => true,
                        Ok(response) => response
                            .error
                            .as_ref()
                            .is_some_and(|error| error.message.contains("already used")),
                        _ => false,
                    };
                    if already_used {
                        if pending_consignment_matches(
                            &self.wasm_proxy_client,
                            &transport_endpoint.endpoint,
                            &recipient.recipient_id,
                            consignment_bytes,
                            &txid,
                            vout,
                        )
                        .await
                        {
                            transport_endpoint.used = true;
                            found_valid = true;
                            break;
                        }
                        return Err(Error::RecipientIDAlreadyUsed);
                    }
                    match response {
                        Err(_) => continue,
                        Ok(res) => {
                            if res.error.is_some() {
                                continue;
                            }
                        }
                    }
                    transport_endpoint.used = true;
                    found_valid = true;
                    break;
                }
                if !found_valid {
                    return Err(Error::NoValidTransportEndpoint);
                }
            }
        }
        Ok(())
    }

    /// Post the pending funding consignment(s) for `txid` to the proxy keyed by the **txid**
    /// recipient_id (in addition to the witness/P2V recipient_id used by
    /// [`Self::post_pending_consignments`]).
    ///
    /// This mirrors the canonical native `rgb-lib` funding flow, whose acceptor
    /// (`accept_transfer`) resolves the funding consignment by raw bitcoin txid. Posting under the
    /// txid lets a WASM sender fund an RGB channel to a *native* acceptor without the acceptor
    /// having to resolve the P2V (witness) recipient_id. Unlike `post_pending_consignments` this
    /// does not require/mutate `transport_endpoint.used`, so it can run after it.
    pub async fn post_consignment_by_txid(&self, txid: String) -> Result<(), Error> {
        let artifacts = self
            .transfer_artifacts
            .get(&txid)
            .ok_or(Error::UnknownTransfer { txid: txid.clone() })?;

        for (asset_id, info_contents_asset) in artifacts.asset_infos.iter() {
            let consignment_bytes =
                artifacts
                    .consignment_bytes
                    .get(asset_id)
                    .ok_or(Error::Internal {
                        details: s!("missing consignment bytes"),
                    })?;

            for recipient in &info_contents_asset.recipients {
                let mut found_valid = false;
                for transport_endpoint in &recipient.transport_endpoints {
                    if transport_endpoint.transport_type != TransportType::JsonRpc
                        || !transport_endpoint.usable
                    {
                        continue;
                    }
                    let vout = recipient.local_recipient_data.vout();
                    let response = self
                        .wasm_proxy_client
                        .post_consignment(
                            &transport_endpoint.endpoint,
                            txid.clone(),
                            consignment_bytes,
                            txid.clone(),
                            vout,
                        )
                        .await;
                    let already_used = match &response {
                        Err(Error::RecipientIDAlreadyUsed) => true,
                        Ok(response) => response
                            .error
                            .as_ref()
                            .is_some_and(|error| error.message.contains("already used")),
                        _ => false,
                    };
                    if already_used {
                        if pending_consignment_matches(
                            &self.wasm_proxy_client,
                            &transport_endpoint.endpoint,
                            &txid,
                            consignment_bytes,
                            &txid,
                            vout,
                        )
                        .await
                        {
                            found_valid = true;
                            break;
                        }
                        return Err(Error::RecipientIDAlreadyUsed);
                    }
                    match response {
                        Err(_) => continue,
                        Ok(res) => {
                            if res.error.is_some() {
                                continue;
                            }
                        }
                    }
                    found_valid = true;
                    break;
                }
                if !found_valid {
                    return Err(Error::NoValidTransportEndpoint);
                }
            }
        }
        Ok(())
    }

    /// Prepare the PSBT to send bitcoins (wasm32 async).
    ///
    /// See native [`send_btc_begin`](Wallet::send_btc_begin) for full documentation.
    pub async fn send_btc_begin(
        &mut self,
        online: Online,
        address: String,
        amount: u64,
        fee_rate: u64,
        skip_sync: bool,
    ) -> Result<String, Error> {
        info!(self.logger, "Sending BTC (begin)...");
        self.check_online(online)?;
        let fee_rate_checked = self._check_fee_rate(fee_rate)?;

        if !skip_sync {
            self.sync_db_txos(false).await?;
        }

        let script_pubkey = self.get_script_pubkey(&address)?;

        let unspendable = self._get_unspendable_bdk_outpoints()?;

        let change_script = if self.wallet_data.reuse_addresses {
            Some(
                self._get_new_address(KeychainKind::Internal)?
                    .script_pubkey(),
            )
        } else {
            None
        };

        let mut tx_builder = self.bdk_wallet.build_tx();
        tx_builder
            .unspendable(unspendable)
            .add_recipient(script_pubkey, BdkAmount::from_sat(amount))
            .fee_rate(fee_rate_checked);

        if let Some(change) = change_script {
            tx_builder.drain_to(change);
        }

        let psbt = tx_builder.finish().map_err(|e| match e {
            bdk_wallet::error::CreateTxError::CoinSelection(InsufficientFunds {
                needed,
                available,
            }) => Error::InsufficientBitcoins {
                needed: needed.to_sat(),
                available: available.to_sat(),
            },
            bdk_wallet::error::CreateTxError::OutputBelowDustLimit(_) => {
                Error::OutputBelowDustLimit
            }
            _ => Error::Internal {
                details: e.to_string(),
            },
        })?;

        info!(self.logger, "Send BTC (begin) completed");
        Ok(psbt.to_string())
    }

    /// Broadcast the PSBT to send bitcoins (wasm32 async).
    ///
    /// See native [`send_btc_end`](Wallet::send_btc_end) for full documentation.
    pub async fn send_btc_end(
        &mut self,
        online: Online,
        signed_psbt: String,
        skip_sync: bool,
    ) -> Result<String, Error> {
        info!(self.logger, "Sending BTC (end)...");
        self.check_online(online)?;

        let signed_psbt = Psbt::from_str(&signed_psbt)?;
        let tx = self._broadcast_psbt(signed_psbt, skip_sync).await?;

        info!(self.logger, "Send BTC (end) completed");
        Ok(tx.compute_txid().to_string())
    }

    /// Get a fee estimation for the given number of blocks.
    pub async fn get_fee_estimation(&self, online: Online, blocks: u16) -> Result<f64, Error> {
        info!(self.logger, "Getting fee estimation...");
        self.check_online(online)?;

        if !(MIN_BLOCK_ESTIMATION..=MAX_BLOCK_ESTIMATION).contains(&blocks) {
            return Err(Error::InvalidEstimationBlocks);
        }

        let estimation = self.indexer().fee_estimation(blocks).await?;

        info!(self.logger, "Get fee estimation completed");
        Ok(estimation)
    }

    /// Prepare the PSBT to drain all wallet funds.
    pub async fn drain_to_begin(
        &mut self,
        online: Online,
        address: String,
        destroy_assets: bool,
        fee_rate: u64,
    ) -> Result<String, Error> {
        info!(
            self.logger,
            "Draining (begin) to '{}' destroying assets '{}'...", address, destroy_assets
        );
        self.check_online(online)?;
        let fee_rate_checked = self._check_fee_rate(fee_rate)?;

        self.sync_db_txos(false).await?;

        let script_pubkey = self.get_script_pubkey(&address)?;

        let mut unspendable = None;
        if !destroy_assets {
            unspendable = Some(self._get_unspendable_bdk_outpoints()?);
        }

        let mut tx_builder = self.bdk_wallet.build_tx();
        tx_builder
            .drain_wallet()
            .drain_to(script_pubkey)
            .fee_rate(fee_rate_checked);

        if let Some(unspendable) = unspendable {
            tx_builder.unspendable(unspendable);
        }

        let psbt = tx_builder.finish().map_err(|e| match e {
            bdk_wallet::error::CreateTxError::CoinSelection(InsufficientFunds {
                needed,
                available,
            }) => Error::InsufficientBitcoins {
                needed: needed.to_sat(),
                available: available.to_sat(),
            },
            bdk_wallet::error::CreateTxError::OutputBelowDustLimit(_) => {
                Error::OutputBelowDustLimit
            }
            _ => Error::Internal {
                details: e.to_string(),
            },
        })?;

        info!(self.logger, "Drain (begin) completed");
        Ok(psbt.to_string())
    }

    /// Broadcast the PSBT to drain wallet funds.
    pub async fn drain_to_end(
        &mut self,
        online: Online,
        signed_psbt: String,
    ) -> Result<String, Error> {
        info!(self.logger, "Draining (end)...");
        self.check_online(online)?;

        let signed_psbt = Psbt::from_str(&signed_psbt)?;
        let tx = self._broadcast_psbt(signed_psbt, false).await?;

        self.database
            .set_wallet_transaction(DbWalletTransactionActMod {
                txid: ActiveValue::Set(tx.compute_txid().to_string()),
                r#type: ActiveValue::Set(WalletTransactionType::Drain),
                ..Default::default()
            })?;

        self.update_backup_info(false)?;
        self.trigger_auto_backup();

        info!(self.logger, "Drain (end) completed");
        Ok(tx.compute_txid().to_string())
    }

    /// Prepare the PSBT to inflate an IFA asset.
    pub async fn inflate_begin(
        &mut self,
        online: Online,
        asset_id: String,
        inflation_amounts: Vec<u64>,
        fee_rate: u64,
        min_confirmations: u8,
    ) -> Result<String, Error> {
        info!(
            self.logger,
            "Inflating (begin) amounts: {:?}...", inflation_amounts
        );

        let asset = self.database.check_asset_exists(asset_id.clone())?;
        let schema = asset.schema;
        self.check_schema_support(&schema)?;
        if !SCHEMAS_SUPPORTING_INFLATION.contains(&schema) {
            return Err(Error::UnsupportedInflation {
                asset_schema: schema,
            });
        }

        let known_circulating_supply = asset
            .known_circulating_supply
            .unwrap()
            .parse::<u64>()
            .unwrap();
        let inflation =
            self.get_total_inflation_amount(&inflation_amounts, known_circulating_supply)?;
        if inflation == 0 {
            return Err(Error::NoInflationAmounts);
        }

        let (fee_rate_checked, unspents, input_unspents, mut runtime) =
            self._get_transfer_begin_data(online, fee_rate)?;

        let assignments_needed = AssignmentsCollection {
            inflation,
            ..Default::default()
        };
        let asset_spend = self._select_rgb_inputs(
            asset_id.clone(),
            &assignments_needed,
            input_unspents.clone(),
        )?;

        let network: ChainNet = self.bitcoin_network().into();
        let amount_sat = asset_spend.input_btc_amt / inflation_amounts.len() as u64;
        let dust = self
            .bdk_wallet
            .public_descriptor(KeychainKind::External)
            .dust_value()
            .to_sat();
        let amount_sat = max(amount_sat, dust);
        let mut local_recipients = vec![];
        let mut witness_recipients: Vec<(ScriptBuf, u64)> = vec![];
        for (idx, amt) in inflation_amounts.iter().enumerate() {
            let script_pubkey = self
                ._get_new_address(KeychainKind::External)?
                .script_pubkey();
            let beneficiary = beneficiary_from_script_buf(script_pubkey.clone());
            let beneficiary = XChainNet::with(network, beneficiary);
            let recipient_id = beneficiary.to_string();
            witness_recipients.push((script_pubkey, amount_sat));
            let vout = idx as u32 + 1;
            local_recipients.push(LocalRecipient {
                recipient_id,
                local_recipient_data: LocalRecipientData::Witness(LocalWitnessData {
                    amount_sat,
                    blinding: None,
                    vout,
                }),
                assignment: Assignment::Fungible(*amt),
                transport_endpoints: vec![],
            })
        }

        let contract_id = ContractId::from_str(&asset_id).expect("invalid contract ID");
        let asset_info = AssetInfo {
            contract_id,
            reject_list_url: asset.reject_list_url,
        };
        let transfer_info = InfoAssetTransfer {
            asset_info,
            recipients: local_recipients,
            asset_spend,
            change: AssignmentsCollection::default(),
            original_assignments_needed: assignments_needed.clone(),
            assignments_needed,
            assignments_spent: HashMap::new(),
            main_transition: TypeOfTransition::Inflate,
        };
        let mut transfer_info_map: BTreeMap<String, InfoAssetTransfer> =
            BTreeMap::from([(asset_id.clone(), transfer_info)]);

        let mut rejected = HashSet::new();
        let psbt_string = match self
            ._prepare_transfer_psbt_wasm(
                &mut transfer_info_map,
                false,
                unspents,
                &input_unspents,
                &witness_recipients,
                fee_rate_checked,
                min_confirmations,
                None,
                &mut runtime,
                &mut rejected,
            )
            .await?
        {
            PrepareTransferPsbtResult::Retry => {
                unreachable!("unimplemented retry logic for inflate transition")
            }
            PrepareTransferPsbtResult::Success(psbt_string) => psbt_string,
        };

        info!(self.logger, "Inflation (begin) completed");
        Ok(psbt_string)
    }

    /// Complete the inflate operation and broadcast.
    pub async fn inflate_end(
        &mut self,
        online: Online,
        signed_psbt: String,
    ) -> Result<OperationResult, Error> {
        info!(self.logger, "Inflating (end)...");
        self.check_online(online)?;

        let psbt = Psbt::from_str(&signed_psbt)?;
        let txid = psbt
            .clone()
            .extract_tx()
            .map_err(InternalError::from)?
            .compute_txid()
            .to_string();

        let artifacts = self
            .transfer_artifacts
            .remove(&txid)
            .ok_or(Error::UnknownTransfer { txid: txid.clone() })?;
        let info_contents = artifacts.batch_info.ok_or(Error::Internal {
            details: s!("missing batch info in transfer artifacts"),
        })?;
        let transfer_info_map = artifacts.asset_infos;

        // Store signed PSBT for refresh() persistence
        self.transfer_artifacts.insert(
            txid.clone(),
            TransferArtifacts {
                signed_psbt: Some(signed_psbt.clone()),
                ..Default::default()
            },
        );

        let mut runtime = self.rgb_runtime()?;
        let tx = self._broadcast_psbt(psbt, false).await?;
        let _ = tx;
        runtime.upsert_witness(RgbTxid::from_str(&txid).unwrap(), WitnessOrd::Tentative)?;
        // Drop runtime so Stock returns to RefCell before auto-save
        drop(runtime);

        let batch_transfer_idx = self._save_transfers(
            txid.clone(),
            &transfer_info_map,
            info_contents.extra_allocations,
            info_contents.change_utxo_idx,
            info_contents.btc_change,
            TransferStatus::WaitingConfirmations,
            info_contents.min_confirmations,
        )?;

        let (asset_id, transfer_info) = transfer_info_map.into_iter().next().unwrap();
        let inflation = transfer_info.original_assignments_needed.inflation;
        let db_asset = self.database.get_asset(asset_id).unwrap().unwrap();
        let updated_known_circulating_supply = db_asset
            .known_circulating_supply
            .as_ref()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            + inflation;
        let mut updated_asset: DbAssetActMod = db_asset.into();
        updated_asset.known_circulating_supply =
            ActiveValue::Set(Some(updated_known_circulating_supply.to_string()));
        self.database.update_asset(&mut updated_asset)?;

        self.update_backup_info(false)?;
        self.trigger_auto_backup();

        info!(self.logger, "Inflate (end) completed");
        Ok(OperationResult {
            txid,
            batch_transfer_idx,
        })
    }

    /// List vanilla (non-colored) unspent outputs.
    pub async fn list_unspents_vanilla(
        &mut self,
        online: Online,
        min_confirmations: u8,
        skip_sync: bool,
    ) -> Result<Vec<LocalOutput>, Error> {
        info!(self.logger, "Listing unspents vanilla...");
        self.check_online(online)?;

        if !skip_sync {
            self.sync_db_txos(false).await?;
        }

        let all_unspents: Vec<LocalOutput> = self.internal_unspents().collect();

        let res = if min_confirmations > 0 {
            let mut filtered = Vec::new();
            for u in all_unspents {
                let confirmations = self
                    .indexer()
                    .get_tx_confirmations(&u.outpoint.txid.to_string())
                    .await?;
                if let Some(confs) = confirmations {
                    if confs >= min_confirmations as u64 {
                        filtered.push(u);
                    }
                }
            }
            filtered
        } else {
            all_unspents
        };

        info!(self.logger, "List unspents vanilla completed");
        Ok(res)
    }
}

// ─── On-chain swap ───────────────────────────────────────────────────────────

/// This wallet takes swaps between RGB and BTC; RGB-for-RGB swaps are not supported.
fn swap_ensure_supported(direction: SwapDirection) -> Result<(), Error> {
    if direction == SwapDirection::RgbForRgb {
        return Err(swap_invalid(
            "this wallet does not support RGB-for-RGB swaps",
        ));
    }
    Ok(())
}

/// The RGB leg a wallet sends in a swap, with the inputs it spends.
struct SwapRgbSend<'a> {
    leg: &'a OnchainSwapLeg,
    inputs: &'a [OnchainSwapInput],
    change_script_hex: &'a str,
    recipient_id: &'a str,
    recipient_vout: u32,
    blinding: u64,
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
        // RGB change must land on the colored keychain, where the wallet tracks it
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

/// Amounts per contract carried by the given wallet inputs.
fn swap_input_rgb_amounts(
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

fn swap_validate_assignments_for_leg(
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

fn swap_validate_witness_output(
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
    let PubWitness::Tx(tx) = &anchored_bundle.pub_witness else {
        return Err(swap_invalid(
            "RGB swap consignment is missing the witness transaction",
        ));
    };
    let output = tx
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

fn swap_endpoint_proxy_url(endpoint: Option<&str>) -> Result<String, Error> {
    let endpoint = endpoint.ok_or_else(|| Error::InvalidTransportEndpoints {
        details: s!("swap consignment missing proxy endpoint"),
    })?;
    let transport = RgbTransport::from_str(endpoint)?;
    Ok(TransportEndpoint::try_from(transport)?.endpoint)
}

impl Wallet {
    fn swap_load<T: serde::de::DeserializeOwned>(
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
    fn swap_ensure_inputs_foreign(
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

    async fn swap_ensure_inputs_confirmed(
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

    async fn swap_select_btc_inputs(
        &self,
        needed_sat: u64,
        min_confirmations: u8,
        excluded: &HashSet<(&str, u32)>,
    ) -> Result<Vec<OnchainSwapInput>, Error> {
        let mut selected = vec![];
        let mut total = 0u64;
        for output in self.internal_unspents().filter(|o| {
            let txid = o.outpoint.txid.to_string();
            !o.is_spent && !excluded.contains(&(txid.as_str(), o.outpoint.vout))
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
            if total >= needed_sat {
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

    /// Fetch the consignment a swap counterparty posted to the proxy, validate it against the
    /// witness transaction and return it with the assignments it gives to `vout`.
    async fn swap_fetch_received_transfer(
        &mut self,
        consignment: &OnchainSwapConsignment,
        leg: &OnchainSwapLeg,
        blinding: u64,
    ) -> Result<(RgbTransfer, ValidTransfer, Vec<Assignment>), Error> {
        let expected_asset_id = leg.asset_id.clone().expect("RGB leg has asset ID");
        let witness_id = RgbTxid::from_str(&consignment.txid).map_err(|_| Error::InvalidTxid)?;
        let proxy_url = swap_endpoint_proxy_url(consignment.endpoint.as_deref())?;
        let consignment_res = self
            ._get_consignment_async(&proxy_url, consignment.recipient_id.clone())
            .await?;
        let consignment_bytes = general_purpose::STANDARD
            .decode(consignment_res.consignment)
            .map_err(InternalError::from)?;
        let transfer = RgbTransfer::load(&consignment_bytes[..]).map_err(InternalError::from)?;
        let asset_id = transfer.contract_id().to_string();
        if asset_id != expected_asset_id {
            return Err(swap_invalid(format!(
                "RGB swap consignment asset mismatch: expected {expected_asset_id}, got {asset_id}"
            )));
        }
        let asset_schema: AssetSchema = transfer.schema_id().try_into()?;
        self.check_schema_support(&asset_schema)?;

        let mut runtime = self.rgb_runtime()?;
        runtime.store_secret_seal(GraphSeal::with_blinded_vout(consignment.vout, blinding))?;
        let wasm_resolver = WasmResolver::from_consignment(&transfer, self.chain_net());
        let resolver = OffchainResolverWasm {
            witness_id,
            consignment: &transfer,
            fallback: &wasm_resolver,
        };
        let validation_config = ValidationConfig {
            chain_net: self.chain_net(),
            trusted_typesystem: asset_schema.types(),
            ..Default::default()
        };
        let valid_transfer = match transfer.clone().validate(&resolver, &validation_config) {
            Ok(valid_transfer) => valid_transfer,
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
        swap_validate_witness_output(
            &transfer,
            witness_id,
            &consignment.recipient_id,
            consignment.vout,
        )?;
        let assignments = self
            .extract_received_assignments(&transfer, witness_id, Some(consignment.vout), None)
            .into_values()
            .collect::<Vec<_>>();
        if assignments.is_empty() {
            return Err(swap_invalid(
                "RGB swap consignment did not assign expected state",
            ));
        }
        swap_validate_assignments_for_leg(&assignments, leg)?;
        Ok((transfer, valid_transfer, assignments))
    }

    async fn swap_select_rgb_inputs(
        &mut self,
        online: Online,
        leg: &OnchainSwapLeg,
        extra_sat: u64,
        min_confirmations: u8,
        excluded: &HashSet<(&str, u32)>,
    ) -> Result<Vec<OnchainSwapInput>, Error> {
        let asset_id = leg.asset_id.clone().expect("RGB leg has asset ID");
        self.database.check_asset_exists(asset_id.clone())?;
        let assignments = AssignmentsCollection {
            fungible: leg.amount,
            non_fungible: false,
            inflation: 0,
        };
        let (_, _, mut input_unspents, _) = self._get_transfer_begin_data(online, 1)?;
        input_unspents.retain(|u| !excluded.contains(&(u.utxo.txid.as_str(), u.utxo.vout)));
        let selected = self._select_rgb_inputs(asset_id, &assignments, input_unspents)?;
        let selected_outpoints = selected
            .txo_map
            .into_values()
            .map(BdkOutPoint::from)
            .collect::<HashSet<_>>();
        let swap_inputs = self
            .bdk_wallet
            .list_unspent()
            .filter(|o| selected_outpoints.contains(&o.outpoint))
            .map(|o| swap_build_input(&o))
            .collect::<Vec<_>>();
        if swap_inputs.is_empty() {
            return Err(swap_invalid("could not resolve selected RGB inputs"));
        }
        let total = swap_selected_inputs_total(&swap_inputs);
        if total < extra_sat {
            return Err(Error::InsufficientBitcoins {
                needed: extra_sat,
                available: total,
            });
        }
        self.swap_ensure_inputs_confirmed(&swap_inputs, min_confirmations)
            .await?;
        Ok(swap_inputs)
    }

    /// Color the RGB leg this wallet sends in a swap, post its consignment to the proxy and record
    /// it as an `Initiated` batch transfer. The stash is not touched until the swap transaction
    /// is broadcast ([`Wallet::swap_apply_outgoing`]), so an abandoned swap leaves no trace there.
    async fn swap_color_outgoing(
        &mut self,
        psbt: &mut Psbt,
        send: &SwapRgbSend<'_>,
        offer: &OnchainSwapOffer,
    ) -> Result<Vec<OnchainSwapConsignment>, Error> {
        let (contract_id, coloring_info, shift) = send.coloring_info(self, psbt)?;
        let prev_outputs = send.input_outpoints();
        // what the inputs carry, per outpoint, before the stash changes
        let mut spent: HashMap<OutPoint, Vec<Assignment>> = HashMap::new();
        {
            let runtime = self.rgb_runtime()?;
            for (explicit_seal, opout_state_map) in
                runtime.contract_assignments_for(contract_id, prev_outputs.iter().copied())?
            {
                for (opout, state) in opout_state_map {
                    spent
                        .entry(explicit_seal.to_outpoint())
                        .or_default()
                        .push(Assignment::from_opout_and_state(opout, &state));
                }
            }
        }
        self.color_psbt_stage_for_inputs(psbt, coloring_info.clone(), prev_outputs)?;
        let fascia = self.color_psbt_finalize(psbt, Some(send.blinding))?;
        let txid = psbt.unsigned_tx.compute_txid().to_string();
        let seal = BuilderSeal::Revealed(GraphSeal::with_blinded_vout(
            send.recipient_vout,
            send.blinding,
        ));
        let transfer = self.generate_transfer(contract_id, vec![seal], psbt.get_txid(), &fascia)?;
        let mut consignment_bytes = Vec::new();
        transfer.save(&mut consignment_bytes)?;

        let proxy_url = offer.proxy_url.as_deref().expect("proxy URL was validated");
        let endpoint = swap_proxy_transport_endpoint(proxy_url)?;
        let res = self
            .wasm_proxy_client
            .post_consignment(
                proxy_url,
                send.recipient_id.to_string(),
                &consignment_bytes,
                txid.clone(),
                Some(send.recipient_vout),
            )
            .await?;
        if let Some(err) = res.error {
            return Err(if err.message.contains("already used") {
                Error::RecipientIDAlreadyUsed
            } else {
                Error::Proxy {
                    details: err.message,
                }
            });
        }

        let batch_transfer_idx = self.swap_record_outgoing(
            psbt,
            &txid,
            contract_id,
            &spent,
            &coloring_info,
            shift,
            offer.expiration_timestamp,
        )?;
        self.database.update_swap(&mut DbSwapActMod {
            swap_id: ActiveValue::Unchanged(offer.swap_id.clone()),
            outgoing_batch_transfer_idx: ActiveValue::Set(Some(batch_transfer_idx)),
            outgoing_fascia: ActiveValue::Set(Some(
                serde_json::to_string(&fascia).map_err(InternalError::from)?,
            )),
            updated_at: ActiveValue::Set(now().unix_timestamp()),
            ..Default::default()
        })?;
        Ok(vec![OnchainSwapConsignment {
            asset_id: contract_id.to_string(),
            path: String::new(),
            endpoint: Some(endpoint),
            txid,
            vout: send.recipient_vout,
            blinding: send.blinding,
            recipient_id: send.recipient_id.to_string(),
        }])
    }

    /// Record the RGB leg a swap sends as an `Initiated` batch transfer: the allocations it
    /// spends, the change it gets back and the recipient it pays.
    #[allow(clippy::too_many_arguments)]
    fn swap_record_outgoing(
        &self,
        psbt: &Psbt,
        txid: &str,
        contract_id: ContractId,
        spent: &HashMap<OutPoint, Vec<Assignment>>,
        coloring_info: &rust_only::ColoringInfo,
        shift_output_map_for_opreturn_first: bool,
        expiration_timestamp: Option<u64>,
    ) -> Result<i32, Error> {
        let batch_transfer_idx = self.database.set_batch_transfer(DbBatchTransferActMod {
            txid: ActiveValue::Set(Some(txid.to_string())),
            status: ActiveValue::Set(TransferStatus::Initiated),
            expiration: ActiveValue::Set(expiration_timestamp.map(|t| t as i64)),
            created_at: ActiveValue::Set(now().unix_timestamp()),
            min_confirmations: ActiveValue::Set(1),
            ..Default::default()
        })?;
        let asset_transfer_idx = self.database.set_asset_transfer(DbAssetTransferActMod {
            user_driven: ActiveValue::Set(true),
            batch_transfer_idx: ActiveValue::Set(batch_transfer_idx),
            asset_id: ActiveValue::Set(Some(contract_id.to_string())),
            ..Default::default()
        })?;
        for (outpoint, assignments) in spent {
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
        if let Some(asset_coloring) = coloring_info.asset_info_map.get(&contract_id) {
            for (mut vout, amount) in BTreeMap::from_iter(asset_coloring.output_map.clone()) {
                if amount == 0 {
                    continue;
                }
                if shift_output_map_for_opreturn_first {
                    vout += 1;
                }
                let txout = psbt
                    .unsigned_tx
                    .output
                    .get(vout as usize)
                    .filter(|o| !o.script_pubkey.is_op_return())
                    .ok_or_else(|| Error::InvalidColoringInfo {
                        details: s!("invalid vout in output_map, does not exist in the given PSBT"),
                    })?;
                let recipient_id = recipient_id_from_script_buf(
                    txout.script_pubkey.clone(),
                    self.bitcoin_network(),
                );
                if self.bdk_wallet.is_mine(txout.script_pubkey.clone()) {
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
        for (vout, amount, recipient_id) in destinations {
            self.database.set_transfer(DbTransferActMod {
                asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                requested_assignment: ActiveValue::Set(Some(Assignment::Fungible(amount))),
                incoming: ActiveValue::Set(false),
                recipient_id: ActiveValue::Set(Some(recipient_id)),
                recipient_type: ActiveValue::Set(Some(RecipientTypeFull::Witness {
                    vout: Some(vout),
                })),
                ..Default::default()
            })?;
        }
        Ok(batch_transfer_idx)
    }

    /// Consume this wallet's prepared RGB leg of a swap into the stash, once the swap transaction
    /// is known to the indexer, and mark the inputs it spends.
    async fn swap_apply_outgoing(
        &mut self,
        completion: &OnchainSwapCompletion,
    ) -> Result<(), Error> {
        let swap_id = &completion.proposal.request.offer.swap_id;
        let swap = self
            .database
            .get_swap(swap_id)?
            .ok_or_else(|| swap_invalid(format!("unknown swap {swap_id}")))?;
        let (Some(batch_transfer_idx), Some(fascia)) =
            (swap.outgoing_batch_transfer_idx, swap.outgoing_fascia)
        else {
            return Ok(());
        };
        let batch_transfer = self
            .database
            .iter_batch_transfers()?
            .into_iter()
            .find(|b| b.idx == batch_transfer_idx)
            .ok_or_else(|| Error::Inconsistency {
                details: format!("swap {swap_id} outgoing batch transfer is missing"),
            })?;
        if batch_transfer.txid.as_deref() != Some(&completion.txid) {
            return Err(swap_invalid(
                "swap txid does not match the prepared RGB leg",
            ));
        }
        if batch_transfer.status != TransferStatus::Initiated {
            return Ok(());
        }
        let fascia: Fascia = serde_json::from_str(&fascia).map_err(InternalError::from)?;
        self.consume_fascia(fascia, None).await?;
        let psbt = Psbt::from_str(
            completion
                .finalized_psbt
                .as_deref()
                .ok_or_else(|| swap_invalid("swap completion is not finalized"))?,
        )?;
        let internal: HashSet<(String, u32)> = self
            .internal_unspents()
            .map(|u| (u.outpoint.txid.to_string(), u.outpoint.vout))
            .collect();
        for input in &psbt.unsigned_tx.input {
            let outpoint = Outpoint::from(input.previous_output);
            if internal.contains(&(outpoint.txid.clone(), outpoint.vout)) {
                continue;
            }
            if let Some(txo) = self.database.get_txo(&outpoint)? {
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
    /// The taker validates the offer, selects the BTC inputs it pays with and derives the
    /// destination for the RGB asset it receives. Inputs in `exclude_outpoints` are never
    /// selected. The returned [`OnchainSwapRequest`] must be forwarded to the maker.
    ///
    /// Only offers where the maker gives RGB and receives BTC are supported.
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
        swap_validate_proxy_url(&offer.proxy_url)?;
        swap_ensure_not_expired(&offer)?;
        if offer.bitcoin_network != self.bitcoin_network() {
            return Err(Error::BitcoinNetworkMismatch);
        }
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
        let fees = offer
            .network_fee_sat
            .checked_add(offer.platform_fee_sat)
            .ok_or_else(|| swap_invalid("swap amounts overflow"))?;
        let excluded = exclude_outpoints
            .iter()
            .map(|o| (o.txid.as_str(), o.vout))
            .collect::<HashSet<_>>();
        let (taker_inputs, change_keychain) = match direction {
            SwapDirection::RgbForBtc => {
                let needed = taker_gives
                    .amount
                    .checked_add(swap_side_rgb_output_cost(
                        &taker_receives,
                        offer.rgb_output_sat,
                    ))
                    .and_then(|v| v.checked_add(fees))
                    .ok_or_else(|| swap_invalid("swap amounts overflow"))?;
                (
                    self.swap_select_btc_inputs(needed, min_confirmations, &excluded)
                        .await?,
                    KeychainKind::Internal,
                )
            }
            // the RGB inputs carry the sats that pay the fees, and RGB change goes back to the
            // colored keychain
            _ => (
                self.swap_select_rgb_inputs(
                    online,
                    &taker_gives,
                    fees,
                    min_confirmations,
                    &excluded,
                )
                .await?,
                KeychainKind::External,
            ),
        };
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
    /// The taker checks that the PSBT is exactly the one the agreed terms produce (including the
    /// platform fee output), that the maker's RGB consignment gives it the agreed asset at the
    /// agreed output, and only then signs. The maker always signs last, so the returned
    /// [`OnchainSwapCompletion`] is never broadcastable by the taker.
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
        let local_request: OnchainSwapRequest =
            self.swap_load(&offer.swap_id, |s| s.request.as_ref(), "request")?;
        swap_ensure_state_matches(&local_request, &proposal.request, "request")?;
        let direction = swap_validate_legs(&offer.maker_gives, &offer.maker_receives)?;
        swap_ensure_supported(direction)?;
        swap_validate_proxy_url(&offer.proxy_url)?;
        swap_ensure_not_expired(offer)?;
        if offer.bitcoin_network != self.bitcoin_network() {
            return Err(Error::BitcoinNetworkMismatch);
        }
        self.swap_ensure_inputs_foreign(&proposal.maker_inputs, &proposal.request.taker_inputs)?;
        let mut psbt = Psbt::from_str(&proposal.psbt)?;
        swap_validate_proposal_psbt(&proposal, &psbt)?;
        if proposal.txid != psbt.unsigned_tx.compute_txid().to_string() {
            return Err(swap_invalid("swap proposal txid mismatch"));
        }
        let (_expected_psbt, maker_rgb_vout, taker_rgb_vout) = swap_build_psbt(&proposal)?;
        let all_inputs = proposal
            .maker_inputs
            .iter()
            .chain(proposal.request.taker_inputs.iter())
            .cloned()
            .collect::<Vec<_>>();
        swap_restore_input_metadata(&mut psbt, &all_inputs)?;

        let mut consignments = proposal.consignments.clone();
        match direction {
            SwapDirection::RgbForBtc => {
                let vout = taker_rgb_vout.ok_or_else(|| swap_invalid("missing taker RGB vout"))?;
                let blinding = proposal
                    .request
                    .taker_rgb_blinding
                    .ok_or_else(|| swap_invalid("missing taker RGB blinding"))?;
                let recipient_id = proposal
                    .request
                    .taker_rgb_recipient_id
                    .as_deref()
                    .ok_or_else(|| swap_invalid("missing taker RGB recipient ID"))?;
                let asset_id = offer
                    .maker_gives
                    .asset_id
                    .as_deref()
                    .expect("RGB leg has asset ID");
                let matching = proposal
                    .consignments
                    .iter()
                    .filter(|c| c.asset_id == asset_id)
                    .collect::<Vec<_>>();
                if matching.len() != 1 {
                    return Err(swap_invalid(format!(
                        "expected exactly one RGB swap consignment for {asset_id}, got {}",
                        matching.len()
                    )));
                }
                let consignment = matching[0];
                if consignment.txid != proposal.txid
                    || consignment.vout != vout
                    || consignment.blinding != blinding
                    || consignment.recipient_id != recipient_id
                {
                    return Err(swap_invalid(
                        "RGB swap consignment does not match the swap output",
                    ));
                }
                self.swap_fetch_received_transfer(consignment, &offer.maker_gives, blinding)
                    .await?;
            }
            _ => {
                let send = SwapRgbSend {
                    leg: &offer.maker_receives,
                    inputs: &proposal.request.taker_inputs,
                    change_script_hex: &proposal.request.taker_change_script_pubkey_hex,
                    recipient_id: offer
                        .maker_rgb_recipient_id
                        .as_deref()
                        .ok_or_else(|| swap_invalid("missing maker RGB recipient ID"))?,
                    recipient_vout: maker_rgb_vout
                        .ok_or_else(|| swap_invalid("missing maker RGB vout"))?,
                    blinding: offer
                        .maker_rgb_blinding
                        .ok_or_else(|| swap_invalid("missing maker RGB blinding"))?,
                };
                consignments.extend(self.swap_color_outgoing(&mut psbt, &send, offer).await?);
                swap_restore_input_metadata(&mut psbt, &all_inputs)?;
            }
        }

        if !skip_sync {
            self.sync_db_txos(false).await?;
        }
        self.swap_ensure_inputs_confirmed(&proposal.maker_inputs, min_confirmations)
            .await?;
        self.swap_ensure_inputs_confirmed(&proposal.request.taker_inputs, min_confirmations)
            .await?;
        self._sign_psbt(
            &mut psbt,
            Some(SignOptions {
                trust_witness_utxo: true,
                ..Default::default()
            }),
        )?;
        swap_ensure_only_own_inputs_signed(&psbt, &proposal.request.taker_inputs)?;
        let txid = psbt.unsigned_tx.compute_txid().to_string();
        let completion = OnchainSwapCompletion {
            psbt: psbt.to_string(),
            finalized_psbt: None,
            txid,
            consignments,
            taker_history: None,
            proposal,
        };
        info!(self.logger, "Complete swap proposal completed");
        Ok(completion)
    }

    /// Record the received RGB leg of a swap as a transfer waiting for the swap transaction to
    /// confirm. The consignment was validated and accepted synchronously, so refreshing only has
    /// to follow the confirmations.
    fn swap_record_incoming(
        &self,
        asset_id: &str,
        txid: &str,
        vout: u32,
        btc_amount_sat: u64,
        assignments: &[Assignment],
    ) -> Result<(), Error> {
        let created_at = now().unix_timestamp();
        let batch_transfer_idx = self.database.set_batch_transfer(DbBatchTransferActMod {
            txid: ActiveValue::Set(Some(txid.to_string())),
            status: ActiveValue::Set(TransferStatus::WaitingConfirmations),
            expiration: ActiveValue::Set(None),
            created_at: ActiveValue::Set(created_at),
            min_confirmations: ActiveValue::Set(1),
            ..Default::default()
        })?;
        let asset_transfer_idx = self.database.set_asset_transfer(DbAssetTransferActMod {
            user_driven: ActiveValue::Set(true),
            batch_transfer_idx: ActiveValue::Set(batch_transfer_idx),
            asset_id: ActiveValue::Set(Some(asset_id.to_string())),
            ..Default::default()
        })?;
        let outpoint = Outpoint {
            txid: txid.to_string(),
            vout,
        };
        let txo_idx = match self.database.get_txo(&outpoint)? {
            Some(txo) => txo.idx,
            None => {
                self.database.set_txo(DbTxoActMod {
                    txid: ActiveValue::Set(txid.to_string()),
                    vout: ActiveValue::Set(vout),
                    btc_amount: ActiveValue::Set(btc_amount_sat.to_string()),
                    spent: ActiveValue::Set(false),
                    exists: ActiveValue::Set(false),
                    pending_witness: ActiveValue::Set(true),
                    ..Default::default()
                })?;
                self.database
                    .get_txo(&outpoint)?
                    .map(|t| t.idx)
                    .ok_or_else(|| swap_invalid("failed to locate swap output txo after insert"))?
            }
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
        Ok(())
    }

    /// Make the broadcast swap TX known to BDK: parties that did not broadcast it would not
    /// discover it with a sync, leaving their swap outputs unknown.
    async fn swap_apply_witness_tx(
        &mut self,
        completion: &OnchainSwapCompletion,
    ) -> Result<(), Error> {
        let finalized_psbt = completion
            .finalized_psbt
            .as_deref()
            .ok_or_else(|| swap_invalid("swap completion is not finalized"))?;
        let psbt = Psbt::from_str(finalized_psbt)?;
        swap_validate_proposal_psbt(&completion.proposal, &psbt)?;
        let tx = psbt.extract_tx().map_err(InternalError::from)?;
        if tx.compute_txid().to_string() != completion.txid {
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

    fn swap_mark_accepted(&self, completion: &OnchainSwapCompletion) -> Result<(), Error> {
        self.database.update_swap(&mut DbSwapActMod {
            swap_id: ActiveValue::Unchanged(completion.proposal.request.offer.swap_id.clone()),
            stage: ActiveValue::Set(OnchainSwapStage::Accepted),
            txid: ActiveValue::Set(Some(completion.txid.clone())),
            completion: ActiveValue::Set(Some(
                serde_json::to_string(completion).map_err(InternalError::from)?,
            )),
            updated_at: ActiveValue::Set(now().unix_timestamp()),
            ..Default::default()
        })?;
        Ok(())
    }

    /// Settle a completed on-chain swap as the taker.
    ///
    /// Call it once the maker has broadcast the swap transaction. When the taker receives RGB, the
    /// consignment the maker posted is validated and accepted into the wallet's RGB state and the
    /// [`Assignment`]s received are returned. When the taker sends RGB, its prepared RGB leg is
    /// consumed into the stash and the result is empty.
    pub async fn accept_swap_transfers(
        &mut self,
        online: Online,
        completion: OnchainSwapCompletion,
        role: OnchainSwapRole,
        skip_sync: bool,
    ) -> Result<OnchainSwapReceiveResult, Error> {
        info!(self.logger, "Accepting on-chain swap transfers...");
        self.check_online(online)?;
        let offer = &completion.proposal.request.offer;
        if role != OnchainSwapRole::Taker {
            return Err(swap_invalid("this wallet can only take swaps"));
        }
        let direction = swap_validate_legs(&offer.maker_gives, &offer.maker_receives)?;
        swap_ensure_supported(direction)?;
        swap_validate_proxy_url(&offer.proxy_url)?;
        let local_request: OnchainSwapRequest =
            self.swap_load(&offer.swap_id, |s| s.request.as_ref(), "request")?;
        swap_ensure_state_matches(&local_request, &completion.proposal.request, "request")?;
        self.swap_apply_witness_tx(&completion).await?;
        if direction == SwapDirection::BtcForRgb {
            self.swap_apply_outgoing(&completion).await?;
            if !skip_sync {
                self.sync_db_txos(false).await?;
            }
            self.swap_mark_accepted(&completion)?;
            self.update_backup_info(false)?;
            self.flush().await?;
            info!(self.logger, "Accept swap transfers completed");
            return Ok(OnchainSwapReceiveResult {
                assignments: vec![],
            });
        }
        if !skip_sync {
            self.sync_db_txos(false).await?;
        }
        let receives = &offer.maker_gives;
        let matching = completion
            .consignments
            .iter()
            .filter(|c| receives.asset_id.as_deref() == Some(c.asset_id.as_str()))
            .collect::<Vec<_>>();
        if matching.len() != 1 {
            return Err(swap_invalid(format!(
                "expected exactly one RGB swap consignment, got {}",
                matching.len()
            )));
        }
        let consignment = matching[0];
        let blinding = completion
            .proposal
            .request
            .taker_rgb_blinding
            .ok_or_else(|| swap_invalid("missing taker RGB blinding"))?;
        if consignment.txid != completion.txid
            || consignment.recipient_id
                != completion
                    .proposal
                    .request
                    .taker_rgb_recipient_id
                    .clone()
                    .unwrap_or_default()
            || consignment.blinding != blinding
        {
            return Err(swap_invalid(
                "RGB swap consignment does not match the swap output",
            ));
        }
        let (transfer, valid_transfer, assignments) = self
            .swap_fetch_received_transfer(consignment, receives, blinding)
            .await?;

        let witness_id = RgbTxid::from_str(&consignment.txid).map_err(|_| Error::InvalidTxid)?;
        let contract_id = transfer.contract_id();
        let asset_schema: AssetSchema = transfer.schema_id().try_into()?;
        let wasm_resolver = WasmResolver::from_consignment(&transfer, self.chain_net());
        let resolver = OffchainResolverWasm {
            witness_id,
            consignment: &transfer,
            fallback: &wasm_resolver,
        };
        let mut runtime = self.rgb_runtime()?;
        runtime.store_secret_seal(GraphSeal::with_blinded_vout(consignment.vout, blinding))?;
        let valid_contract = valid_transfer.clone().into_valid_contract();
        runtime
            .import_contract(valid_contract.clone(), &resolver)
            .expect("failure importing validated contract");
        if self.database.get_asset(contract_id.to_string())?.is_none() {
            self.save_new_asset_internal(
                &runtime,
                contract_id,
                asset_schema,
                valid_contract,
                valid_transfer.clone(),
            )?;
        }
        runtime.accept_transfer(valid_transfer, &resolver)?;
        drop(runtime);
        self.swap_record_incoming(
            &contract_id.to_string(),
            &consignment.txid,
            consignment.vout,
            offer.rgb_output_sat,
            &assignments,
        )?;
        self.swap_mark_accepted(&completion)?;
        self.update_backup_info(false)?;
        self.flush().await?;
        info!(self.logger, "Accept swap transfers completed");
        Ok(OnchainSwapReceiveResult { assignments })
    }

    /// Cancel an on-chain swap whose transaction this wallet has not seen broadcast.
    ///
    /// Returns [`Error::CannotFailBatchTransfer`] once the swap transaction is known to the
    /// indexer or the swap has been settled. A maker holding a fully signed transaction can still
    /// broadcast it: to rule that out, spend the inputs.
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
        // release the inputs the prepared RGB leg reserved
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
