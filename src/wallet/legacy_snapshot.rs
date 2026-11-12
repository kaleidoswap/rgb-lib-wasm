//! Migration of the IndexedDB snapshots written by rgb-lib-wasm 1.0.0-beta.3.
//!
//! Version 1.0.0-beta.3 saved a wallet under its directory alone, with the RGB stock as three
//! strict-encoded blobs (stash, state and index) in the legacy rgb-ops layout and the received
//! consignments in the legacy (V0) container format. The current version saves under the
//! directory and network, with the RGB tables of [`MemRgbStore`].
//!
//! The conversion of the RGB stock mirrors the one native rgb-lib runs on its legacy filesystem
//! store, and the defaults of the new rgb-lib table fields mirror its database migrations.
//! The BDK changeset is not converted: the next full scan rebuilds it. Pending transfer
//! artifacts are dropped, as the outgoing transfers they belong to are failed (they were never
//! broadcast and cannot complete with the current proxy protocol).

use std::collections::{BTreeSet, HashMap};

use bdk_wallet::KeychainKind;
use bdk_wallet::bitcoin::Network as BdkNetwork;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use amplify::{
    confinement::{Confined, U32 as U32MAX},
    s,
};
use rgbstd::{
    ChainNet, ContractId, KnownTransition, OpId, Operation as _,
    containers::{FileContent, Transfer as RgbTransfer, legacy::ConsignmentV0},
    contract::{KnownState, OpWitness, OutputAssignment},
    persistence::{
        AllocKind, AllocSeal, AllocationWrite, GlobalStateWrite, MemIndexV0, MemStashV0,
        MemStateV0, RgbStore, Stock, encode,
    },
    rgbcore::{bitcoin::OutPoint, commit_verify::Conceal},
};
use strict_encoding::{StreamReader, StrictDecode, StrictDeserialize, StrictEncode, StrictReader};

use crate::database::{memory_db::InMemoryDb, rgb_store::MemRgbStore};
use crate::{AssetSchema, BitcoinNetwork, utils::MemStock};

/// The outcome of the migration of a 1.0.0-beta.3 wallet snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum LegacyMigration {
    /// The 1.0.0-beta.3 snapshot was converted. The wallet holds its RGB state.
    Migrated,
    /// The 1.0.0-beta.3 snapshot could not be converted. The wallet holds none of its RGB
    /// state, so its RGB operations must not be used. The migration is tried again on the
    /// next open.
    Failed {
        /// Why the conversion failed
        error: String,
    },
}

/// The fields of a 1.0.0-beta.3 snapshot that are migrated.
#[derive(Deserialize)]
struct LegacySnapshot {
    db: Value,
    #[serde(default)]
    bdk_changeset: Option<Value>,
    #[serde(default)]
    received_consignments: HashMap<String, Vec<u8>>,
    #[serde(default)]
    stock_stash_b64: Option<String>,
    #[serde(default)]
    stock_state_b64: Option<String>,
    #[serde(default)]
    stock_index_b64: Option<String>,
    #[serde(default)]
    reuse_address_index: HashMap<KeychainKind, u32>,
}

/// A 1.0.0-beta.3 snapshot converted to the current layout.
pub(crate) struct MigratedSnapshot {
    pub(crate) db: InMemoryDb,
    pub(crate) stock: MemStock,
    pub(crate) received_consignments: HashMap<String, Vec<u8>>,
    pub(crate) reuse_address_index: HashMap<KeychainKind, u32>,
}

/// Convert the 1.0.0-beta.3 snapshot `json` of a wallet on `network`.
///
/// `schemas` are the schemas the wallet supports. `now` is the time, in seconds, recorded on
/// the transfers the migration fails.
///
/// Return `Ok(None)` when the snapshot belongs to a wallet on another network: 1.0.0-beta.3
/// used the same key for the wallets of every network.
pub(crate) fn migrate_legacy_snapshot(
    json: &str,
    network: BitcoinNetwork,
    schemas: &[AssetSchema],
    now: i64,
) -> Result<Option<MigratedSnapshot>, String> {
    let legacy: LegacySnapshot =
        serde_json::from_str(json).map_err(|e| format!("cannot parse the snapshot: {e}"))?;

    if !same_network(legacy.bdk_changeset.as_ref(), network)? {
        return Ok(None);
    }

    let Some(stock) = migrate_stock(&legacy, network.into(), schemas)? else {
        return Ok(None);
    };
    let db = migrate_db(legacy.db, now)?;
    let mut received_consignments = HashMap::new();
    for (recipient_id, bytes) in legacy.received_consignments {
        let bytes = migrate_consignment(&bytes)
            .map_err(|e| format!("cannot migrate the consignment of {recipient_id}: {e}"))?;
        received_consignments.insert(recipient_id, bytes);
    }

    Ok(Some(MigratedSnapshot {
        db,
        stock,
        received_consignments,
        reuse_address_index: legacy.reuse_address_index,
    }))
}

/// Whether the BDK changeset of a legacy snapshot, if any, is of a wallet on `network`.
///
/// Signet and custom signets share the BDK network (and the genesis block): the RGB contracts
/// tell them apart, see [`migrate_stock`].
fn same_network(changeset: Option<&Value>, network: BitcoinNetwork) -> Result<bool, String> {
    let Some(changeset) = changeset else {
        return Ok(true);
    };
    if let Some(snapshot_network) = changeset.get("network").filter(|n| !n.is_null()) {
        let wallet_network = serde_json::to_value(BdkNetwork::from(network))
            .map_err(|e| format!("cannot encode the network: {e}"))?;
        if *snapshot_network != wallet_network {
            return Ok(false);
        }
    }
    Ok(true)
}

fn decode_b64<T: StrictDeserialize>(b64: &Option<String>, what: &str) -> Result<T, String> {
    use base64::{Engine as _, engine::general_purpose};

    let b64 = b64
        .as_deref()
        .ok_or_else(|| format!("the snapshot has no RGB {what}"))?;
    let bytes = general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("cannot decode the RGB {what}: {e}"))?;
    let bytes = Confined::<Vec<u8>, 0, U32MAX>::try_from(bytes)
        .map_err(|e| format!("cannot decode the RGB {what}: {e}"))?;
    T::from_strict_serialized::<U32MAX>(bytes)
        .map_err(|e| format!("cannot decode the RGB {what}: {e}"))
}

/// Convert the legacy RGB stash, state and index into a store holding the definitions of
/// `schemas`, as native rgb-lib does with its legacy filesystem store.
///
/// Return `Ok(None)` when every contract is on another chain than `chain_net`: the snapshot is
/// of a wallet on another network.
fn migrate_stock(
    legacy: &LegacySnapshot,
    chain_net: ChainNet,
    schemas: &[AssetSchema],
) -> Result<Option<MemStock>, String> {
    let stash: MemStashV0 = decode_b64(&legacy.stock_stash_b64, "stash")?;
    let state: MemStateV0 = decode_b64(&legacy.stock_state_b64, "state")?;
    let index: MemIndexV0 = decode_b64(&legacy.stock_index_b64, "index")?;

    if !stash.geneses().is_empty() && stash.geneses().values().all(|g| g.chain_net != chain_net) {
        return Ok(None);
    }

    // schemata, AluVM libraries and the type system are not migrated: the current store keeps
    // the type libraries a schema was defined by (which a legacy stash does not hold), so the
    // supported schema definitions are imported instead
    let mut stock = MemStock::default();
    for schema in schemas {
        stock
            .import_schema_definition(schema.schema_definition())
            .map_err(|e| format!("cannot import the {schema} schema: {e}"))?;
    }
    let mut store = stock.as_store().clone();
    let ok = |r: Result<(), std::convert::Infallible>| r.unwrap_or_else(|e| match e {});

    for (contract_id, genesis) in stash.geneses().iter() {
        if genesis.chain_net != chain_net {
            return Err(format!(
                "contract {contract_id} is on {}, not on {chain_net}",
                genesis.chain_net
            ));
        }
        if store.schema(genesis.schema_id).unwrap_or(None).is_none() {
            return Err(format!(
                "contract {contract_id} uses schema {}, which this version does not support",
                genesis.schema_id
            ));
        }
        ok(store.put_genesis(genesis));
    }
    for bundle in stash.bundles().values() {
        ok(store.put_bundle(bundle));
    }
    for (txid, witness) in stash.witnesses().iter() {
        // a legacy witness may hold only the txid of its transaction, which cannot be resolved
        // here (no network access while opening the wallet)
        let witness = witness
            .clone()
            .into_v1(None)
            .map_err(|e| format!("cannot migrate RGB witness {txid}: {e}"))?;
        ok(store.put_witness(&witness));
    }
    for graph_seal in stash.secret_seals().iter() {
        ok(store.put_secret_seal(graph_seal, graph_seal.conceal()));
    }

    for (txid, ord) in state.witnesses().iter() {
        ok(store.put_witness_ord(*txid, *ord));
    }
    for opid in state.invalid_ops().iter() {
        ok(store.set_op_validity(*opid, false));
    }
    for (contract_id, mcs) in state.contracts().iter() {
        // a global entry is filed under a `GlobalOut`, which names the witness transaction of
        // its operation, not the bundle it sits in: the bundle is resolved through the index.
        // An entry stored without a bundle would read as genesis state, which is always valid,
        // so a transition the index cannot place in a bundle stops the migration
        for (ty, global) in mcs.global().iter() {
            for (out, data) in global.known().iter() {
                let (bundle_id, transition_type) = match out.op_witness {
                    OpWitness::Genesis => (None, None),
                    OpWitness::Transition(_, transition_type) => {
                        let bundle_id = index
                            .op_bundle_index()
                            .get(&out.opid)
                            .copied()
                            .ok_or_else(|| {
                                format!(
                                    "the RGB index has no bundle for operation {} of contract \
                                     {contract_id}",
                                    out.opid
                                )
                            })?;
                        (Some(bundle_id), Some(transition_type))
                    }
                };
                ok(store.put_global(GlobalStateWrite {
                    contract_id: *contract_id,
                    type_id: *ty,
                    opid: out.opid,
                    index: out.index,
                    nonce: out.nonce,
                    bundle_id,
                    transition_type,
                    value: data,
                }));
            }
        }
        for oa in mcs.rights().iter() {
            migrate_assignment(&mut store, *contract_id, AllocKind::Declarative, oa)?;
        }
        for oa in mcs.fungibles().iter() {
            migrate_assignment(&mut store, *contract_id, AllocKind::Fungible, oa)?;
        }
        for oa in mcs.data().iter() {
            migrate_assignment(&mut store, *contract_id, AllocKind::Structured, oa)?;
        }
    }

    for (contract_id, ci) in index.contract_index().iter() {
        for (output_seal, opouts) in ci.outpoint_opouts().iter() {
            let outpoint = OutPoint::new(output_seal.txid, output_seal.vout.into_u32());
            for opout in opouts.iter() {
                ok(store.put_outpoint_opout(*contract_id, outpoint, *opout));
            }
        }
    }
    // the legacy secret seal index is not contract-scoped: the contract is resolved through the
    // operation of the opout, its bundle for a transition, the genesis itself for a genesis
    let genesis_contracts: HashMap<OpId, ContractId> = stash
        .geneses()
        .iter()
        .map(|(contract_id, genesis)| (genesis.id(), *contract_id))
        .collect();
    for (secret_seal, opouts) in index.terminal_index().iter() {
        for opout in opouts.iter() {
            let contract_id = index
                .op_bundle_index()
                .get(&opout.op)
                .and_then(|bundle_id| index.bundle_contract_index().get(bundle_id))
                .or_else(|| genesis_contracts.get(&opout.op))
                .ok_or_else(|| {
                    format!(
                        "the RGB index has no contract for operation {} of secret seal \
                         {secret_seal}",
                        opout.op
                    )
                })?;
            ok(store.put_secret_opout(*contract_id, *secret_seal, *opout));
        }
    }
    for (opid, bundle_id) in index.op_bundle_index().iter() {
        ok(store.put_op_bundle(*opid, *bundle_id));
    }
    for (bundle_id, bundle) in stash.bundles().iter() {
        for KnownTransition { opid, transition } in &bundle.known_transitions {
            for input in &transition.inputs {
                ok(store.put_op_input(input, *opid, *bundle_id));
            }
        }
    }
    for (bundle_id, contract_id) in index.bundle_contract_index().iter() {
        ok(store.put_bundle_contract(*bundle_id, *contract_id));
    }
    for (bundle_id, txids) in index.bundle_witness_index().iter() {
        for txid in txids.iter() {
            ok(store.put_bundle_witness(*bundle_id, *txid));
        }
    }

    Ok(Some(Stock::with(store)))
}

fn migrate_assignment<State: StrictEncode + KnownState>(
    store: &mut MemRgbStore,
    contract_id: ContractId,
    kind: AllocKind,
    oa: &OutputAssignment<State>,
) -> Result<(), String> {
    let value = encode(&oa.state).map_err(|e| format!("cannot encode RGB state: {e}"))?;
    // the legacy state holds the seal resolved, the current one stores it as defined. A seal
    // naming the witness of its own assignment was defined on the witness transaction: a
    // transition cannot name the txid of the transaction committing to it
    let seal = match oa.witness {
        Some(witness) if witness == oa.seal.txid => AllocSeal::witness_vout(oa.seal.vout),
        _ => AllocSeal::explicit(oa.seal),
    };
    store
        .put_allocation(AllocationWrite {
            contract_id,
            kind,
            opout: oa.opout,
            seal,
            bundle_id: oa.bundle_id,
            value: &value,
        })
        .unwrap_or_else(|e| match e {});
    Ok(())
}

/// Prefix of every RGB container file, followed by a 3-byte magic.
const RGB_FILE_PREFIX: &[u8; 4] = b"RGB\0";

/// Convert a legacy (V0) transfer consignment, as saved by 1.0.0-beta.3, to the current V1
/// container. A consignment already in the V1 format is returned unchanged.
fn migrate_consignment(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let body = bytes
        .strip_prefix(RGB_FILE_PREFIX)
        .and_then(|rest| rest.strip_prefix(b"TFR"))
        .filter(|body| !body.is_empty())
        .ok_or_else(|| s!("not a transfer consignment"))?;
    // the strict-encoded consignment starts with its version byte: 0 for legacy V0 containers
    if body[0] != 0 {
        RgbTransfer::load(bytes).map_err(|e| e.to_string())?;
        return Ok(bytes.to_vec());
    }
    let stream = StreamReader::new::<U32MAX>(std::io::Cursor::new(body));
    let mut reader = StrictReader::with(stream);
    let v0 = ConsignmentV0::<true>::strict_decode(&mut reader).map_err(|e| e.to_string())?;
    let v1 = v0.into_v1(None).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    v1.save(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// The field of `row`, or null.
fn field<'a>(row: &'a Value, name: &str) -> &'a Value {
    row.get(name).unwrap_or(&Value::Null)
}

/// The rows of `table`, an empty list when the snapshot has none.
fn rows<'a>(db: &'a Map<String, Value>, table: &str) -> &'a [Value] {
    db.get(table)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// Add the fields the current tables have and the legacy rows lack, with the value `defaults`
/// gives them. A field the row has is kept.
fn add_fields(
    db: &mut Map<String, Value>,
    table: &str,
    mut defaults: impl FnMut(&Map<String, Value>) -> Vec<(&'static str, Value)>,
) -> Result<(), String> {
    let Some(rows) = db.get_mut(table) else {
        return Ok(());
    };
    let rows = rows
        .as_array_mut()
        .ok_or_else(|| format!("table {table} is not a list"))?;
    for row in rows {
        let row = row
            .as_object_mut()
            .ok_or_else(|| format!("a row of table {table} is not an object"))?;
        for (name, value) in defaults(row) {
            row.entry(name).or_insert(value);
        }
    }
    Ok(())
}

/// Convert the legacy rgb-lib tables, with the rules native rgb-lib applies to its database:
/// - the outgoing transfers not broadcast yet are failed: they cannot complete with the
///   current proxy protocol, and their inputs are released;
/// - the main transition of an outgoing asset transfer is a burn when it has a transfer
///   without a recipient, an inflation when it has an issue coloring, a transfer otherwise;
/// - the legacy transfers did not encrypt, and only used the proxy of the recipient;
/// - the legacy vanilla transactions were all broadcast.
fn migrate_db(db: Value, now: i64) -> Result<InMemoryDb, String> {
    let Value::Object(mut db) = db else {
        return Err(s!("the rgb-lib database is not an object"));
    };

    let id = |row: &Value, name: &str| field(row, name).as_i64();
    // asset transfers of each batch transfer, and whether the batch is incoming
    let mut incoming_batches = BTreeSet::new();
    let mut batch_of_asset_transfer = HashMap::new();
    for at in rows(&db, "asset_transfers") {
        if let (Some(idx), Some(batch)) = (id(at, "idx"), id(at, "batch_transfer_idx")) {
            batch_of_asset_transfer.insert(idx, batch);
        }
    }
    let mut no_recipient = BTreeSet::new();
    for t in rows(&db, "transfers") {
        let Some(at) = id(t, "asset_transfer_idx") else {
            continue;
        };
        if field(t, "incoming").as_bool() == Some(true)
            && let Some(batch) = batch_of_asset_transfer.get(&at)
        {
            incoming_batches.insert(*batch);
        }
        if field(t, "recipient_id").is_null() {
            no_recipient.insert(at);
        }
    }
    let issued: BTreeSet<i64> = rows(&db, "colorings")
        .iter()
        .filter(|c| field(c, "type") == "Issue")
        .filter_map(|c| id(c, "asset_transfer_idx"))
        .collect();
    let outgoing = |batch: Option<i64>| batch.is_some_and(|b| !incoming_batches.contains(&b));

    add_fields(&mut db, "assets", |_| {
        vec![
            ("linked_from_contract", Value::Null),
            ("linked_to_contract", Value::Null),
        ]
    })?;
    add_fields(&mut db, "asset_transfers", |at| {
        let idx = at.get("idx").and_then(Value::as_i64);
        let batch = at.get("batch_transfer_idx").and_then(Value::as_i64);
        let transition_type = match idx {
            _ if !outgoing(batch) => Value::Null,
            Some(idx) if no_recipient.contains(&idx) => json!("Burn"),
            Some(idx) if issued.contains(&idx) => json!("Inflate"),
            _ => json!("Transfer"),
        };
        vec![("transition_type", transition_type)]
    })?;
    if let Some(batches) = db.get_mut("batch_transfers").and_then(Value::as_array_mut) {
        for bt in batches.iter_mut().filter_map(Value::as_object_mut) {
            let status = bt.get("status").and_then(Value::as_str);
            if outgoing(bt.get("idx").and_then(Value::as_i64))
                && matches!(status, Some("Initiated" | "WaitingCounterparty"))
            {
                bt.insert(s!("status"), json!("Failed"));
                bt.insert(s!("updated_at"), json!(now));
            }
        }
    }
    add_fields(&mut db, "batch_transfers", |_| {
        vec![
            ("operation_secret_key", Value::Null),
            ("broadcast_deadline", Value::Null),
            ("retention_deadline", Value::Null),
        ]
    })?;
    add_fields(&mut db, "transfers", |_| {
        vec![
            ("recipient_pubkey", Value::Null),
            ("consignment_hash", Value::Null),
            ("recipient_encryption", json!("Unsupported")),
            ("ack_signature", Value::Null),
            ("proxy_transfer_id", Value::Null),
            ("invoice_expiration", Value::Null),
            ("burn_reason", Value::Null),
        ]
    })?;
    add_fields(&mut db, "transfer_transport_endpoints", |_| {
        vec![("kind", json!("RecipientProxy"))]
    })?;
    add_fields(&mut db, "wallet_transactions", |_| {
        vec![
            ("status", json!("Settled")),
            ("inputs", json!([])),
            ("sent", json!(0)),
            ("received", json!(0)),
            ("fee", json!(0)),
        ]
    })?;

    serde_json::from_value(Value::Object(db))
        .map_err(|e| format!("cannot convert the rgb-lib database: {e}"))
}

