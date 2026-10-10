//! RGB wallet
//!
//! This module defines the offline methods of the [`Wallet`] structure and all its related data.

use super::*;

pub(crate) const RGB_LIB_DB_NAME: &str = "rgb_lib_db";
const BDK_DB_NAME: &str = "bdk_db";

pub(crate) const MEDIA_DIR: &str = "media_files";
const TRANSFERS_DIR: &str = "transfers";
const ASSETS_DIR: &str = "assets";

const MIN_BTC_REQUIRED: u64 = 2000;

pub(crate) const NUM_KNOWN_SCHEMAS: usize = 2;

pub(crate) const MAX_TRANSPORT_ENDPOINTS: usize = 3;

pub(crate) const DURATION_RCV_TRANSFER: u32 = 86400;

pub(crate) const ASSET_ID_PREFIX: &str = "rgb:";
pub(crate) const CONSIGNMENT_FILE: &str = "consignment_out";

const CONSIGNMENT_RCV_FILE: &str = "rcv_compose.rgbc";

pub(crate) const RGB_STATE_ASSET_OWNER: &str = "assetOwner";
pub(crate) const RGB_STATE_INFLATION_ALLOWANCE: &str = "inflationAllowance";
pub(crate) const RGB_GLOBAL_ISSUED_SUPPLY: &str = "issuedSupply";
pub(crate) const RGB_GLOBAL_REJECT_LIST_URL: &str = "rejectListUrl";
#[cfg(feature = "esplora")]
pub(crate) const RGB_METADATA_ALLOWED_INFLATION: &str = "allowedInflation";

pub(crate) const SCHEMA_ID_NIA: &str =
    "rgb:sch:RWhwUfTMpuP2Zfx1~j4nswCANGeJrYOqDcKelaMV4zU#remote-digital-pegasus";
pub(crate) const SCHEMA_ID_IFA: &str =
    "rgb:sch:IpjJhFLz3oywYKQxO3KmFgR0Aa415nlTNrNyEFqMZCE#shoe-colombo-mango";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct LocalAssetData {
    pub(crate) name: String,
    pub(crate) precision: u8,
    pub(crate) ticker: Option<String>,
    pub(crate) details: Option<String>,
    pub(crate) media_idx: Option<i32>,
    pub(crate) initial_supply: u64,
    pub(crate) max_supply: Option<u64>,
    pub(crate) known_circulating_supply: Option<u64>,
    pub(crate) reject_list_url: Option<String>,
}

/// The bitcoin balances (in sats) for the vanilla and colored wallets.
///
/// The settled balances include the confirmed balance.
/// The future balances also include the immature balance and the untrusted and trusted pending
/// balances.
/// The spendable balances include the settled balance and also the untrusted and trusted pending
/// balances.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct BtcBalance {
    /// Funds that will never hold RGB assets
    pub vanilla: Balance,
    /// Funds that may hold RGB assets
    pub colored: Balance,
}

/// An asset media file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Media {
    /// Path of the media file
    pub file_path: String,
    /// Digest of the media file
    pub digest: String,
    /// Mime type of the media file
    pub mime: String,
}

impl Media {
    pub(crate) fn get_digest(&self) -> String {
        PathBuf::from(&self.file_path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string()
    }

    pub(crate) fn from_attachment<P: AsRef<Path>>(attachment: &Attachment, media_dir: P) -> Self {
        let digest = hex::encode(attachment.digest);
        let file_path = media_dir
            .as_ref()
            .join(&digest)
            .to_string_lossy()
            .to_string();
        Self {
            digest,
            mime: attachment.ty.to_string(),
            file_path,
        }
    }
}

impl Media {
    pub(crate) fn from_db_media<P: AsRef<Path>>(db_media: &DbMedia, media_dir: P) -> Self {
        let digest = db_media.digest.clone();
        let file_path = media_dir
            .as_ref()
            .join(&digest)
            .to_string_lossy()
            .to_string();
        Self {
            digest,
            mime: db_media.mime.clone(),
            file_path,
        }
    }
}

/// Metadata of an RGB asset.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Metadata {
    /// Asset schema type
    pub asset_schema: AssetSchema,
    /// Initial issued supply
    pub initial_supply: u64,
    /// Max issued supply
    pub max_supply: u64,
    /// Known circulating supply
    pub known_circulating_supply: u64,
    /// Timestamp of asset genesis
    pub timestamp: i64,
    /// Asset name
    pub name: String,
    /// Asset precision
    pub precision: u8,
    /// Asset ticker
    pub ticker: Option<String>,
    /// Asset details
    pub details: Option<String>,
    /// Reject list URL
    pub reject_list_url: Option<String>,
}

/// A Non-Inflatable Asset.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct AssetNIA {
    /// ID of the asset
    pub asset_id: String,
    /// Ticker of the asset
    pub ticker: String,
    /// Name of the asset
    pub name: String,
    /// Details of the asset
    pub details: Option<String>,
    /// Precision, also known as divisibility, of the asset
    pub precision: u8,
    /// Total issued amount
    pub issued_supply: u64,
    /// Timestamp of asset genesis
    pub timestamp: i64,
    /// Timestamp of asset import
    pub added_at: i64,
    /// Current balance of the asset
    pub balance: Balance,
    /// Asset media attachment
    pub media: Option<Media>,
}

impl AssetNIA {
    pub(crate) fn get_asset_details(
        wallet: &Wallet,
        asset: &DbAsset,
        transfers: Option<Vec<DbTransfer>>,
        asset_transfers: Option<Vec<DbAssetTransfer>>,
        batch_transfers: Option<Vec<DbBatchTransfer>>,
        colorings: Option<Vec<DbColoring>>,
        txos: Option<Vec<DbTxo>>,
        medias: Option<Vec<DbMedia>>,
    ) -> Result<AssetNIA, Error> {
        let media = {
            let medias = if let Some(m) = medias {
                m
            } else {
                wallet.database.iter_media()?
            };
            medias
                .iter()
                .find(|m| Some(m.idx) == asset.media_idx)
                .map(|m| Media::from_db_media(m, wallet.get_media_dir()))
        };
        let balance = wallet.database.get_asset_balance(
            asset.id.clone(),
            transfers,
            asset_transfers,
            batch_transfers,
            colorings,
            txos,
        )?;
        let initial_supply = asset.initial_supply.parse::<u64>().unwrap();
        Ok(AssetNIA {
            asset_id: asset.id.clone(),
            ticker: asset.ticker.clone().unwrap(),
            name: asset.name.clone(),
            details: asset.details.clone(),
            precision: asset.precision,
            issued_supply: initial_supply,
            timestamp: asset.timestamp,
            added_at: asset.added_at,
            balance,
            media,
        })
    }
}

/// An Inflatable Fungible Asset.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct AssetIFA {
    /// ID of the asset
    pub asset_id: String,
    /// Ticker of the asset
    pub ticker: String,
    /// Name of the asset
    pub name: String,
    /// Details of the asset
    pub details: Option<String>,
    /// Precision, also known as divisibility, of the asset
    pub precision: u8,
    /// Initial issued supply
    pub initial_supply: u64,
    /// Max issued supply
    pub max_supply: u64,
    /// Known circulating supply
    pub known_circulating_supply: u64,
    /// Timestamp of asset genesis
    pub timestamp: i64,
    /// Timestamp of asset import
    pub added_at: i64,
    /// Current balance of the asset
    pub balance: Balance,
    /// Asset media attachment
    pub media: Option<Media>,
    /// Reject list URL
    pub reject_list_url: Option<String>,
}

impl AssetIFA {
    pub(crate) fn get_asset_details(
        wallet: &Wallet,
        asset: &DbAsset,
        transfers: Option<Vec<DbTransfer>>,
        asset_transfers: Option<Vec<DbAssetTransfer>>,
        batch_transfers: Option<Vec<DbBatchTransfer>>,
        colorings: Option<Vec<DbColoring>>,
        txos: Option<Vec<DbTxo>>,
        medias: Option<Vec<DbMedia>>,
    ) -> Result<AssetIFA, Error> {
        let media = {
            let medias = if let Some(m) = medias {
                m
            } else {
                wallet.database.iter_media()?
            };
            medias
                .iter()
                .find(|m| Some(m.idx) == asset.media_idx)
                .map(|m| Media::from_db_media(m, wallet.get_media_dir()))
        };
        let balance = wallet.database.get_asset_balance(
            asset.id.clone(),
            transfers,
            asset_transfers,
            batch_transfers,
            colorings,
            txos,
        )?;
        let initial_supply = asset.initial_supply.parse::<u64>().unwrap();
        let max_supply = asset.max_supply.as_ref().unwrap().parse::<u64>().unwrap();
        let known_circulating_supply = asset
            .known_circulating_supply
            .as_ref()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        Ok(AssetIFA {
            asset_id: asset.id.clone(),
            ticker: asset.ticker.clone().unwrap(),
            name: asset.name.clone(),
            details: asset.details.clone(),
            precision: asset.precision,
            initial_supply,
            max_supply,
            known_circulating_supply,
            timestamp: asset.timestamp,
            added_at: asset.added_at,
            balance,
            media,
            reject_list_url: asset.reject_list_url.clone(),
        })
    }
}

/// List of RGB assets, grouped by asset schema.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Assets {
    /// List of NIA assets
    pub nia: Option<Vec<AssetNIA>>,
    /// List of IFA assets
    pub ifa: Option<Vec<AssetIFA>>,
}

/// A balance.
///
/// This structure is used both for RGB assets and BTC balances (in sats). When used for a BTC
/// balance it can be used both for the vanilla wallet and the colored wallet.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Balance {
    /// Settled balance, based on operations that have reached the final status
    pub settled: u64,
    /// Future balance, including settled operations plus ones are not yet finalized
    pub future: u64,
    /// Spendable balance, only including balance that can actually be spent. It's a subset of the
    /// settled balance. For the RGB balance this excludes the allocations on UTXOs related to
    /// pending operations
    pub spendable: u64,
}

/// Data to receive an RGB transfer.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct ReceiveData {
    /// Invoice string
    pub invoice: String,
    /// ID of the receive operation (blinded UTXO or Bitcoin script)
    pub recipient_id: String,
    /// Expiration of the receive operation
    pub expiration_timestamp: Option<i64>,
    /// Batch transfer idx
    pub batch_transfer_idx: i32,
}

/// The type of an RGB recipient
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum RecipientType {
    /// Receive via blinded UTXO
    Blind,
    /// Receive via witness TX
    Witness,
}

/// RGB recipient information used to be paid
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct RecipientInfo {
    /// Recipient ID
    pub recipient_id: String,
    /// Recipient type
    pub recipient_type: RecipientType,
    /// Recipient network
    pub network: BitcoinNetwork,
}

impl RecipientInfo {
    /// Builds a new [`RecipientInfo`] from the provided string, checking that it is valid.
    pub fn new(recipient_id: String) -> Result<Self, Error> {
        let xchainnet_beneficiary = XChainNet::<Beneficiary>::from_str(&recipient_id)
            .map_err(|_| Error::InvalidRecipientID)?;
        let recipient_type = match xchainnet_beneficiary.into_inner() {
            Beneficiary::WitnessVout(_, _) => RecipientType::Witness,
            Beneficiary::BlindedSeal(_) => RecipientType::Blind,
        };
        Ok(Self {
            recipient_id,
            recipient_type,
            network: xchainnet_beneficiary.chain_network().try_into()?,
        })
    }
}

/// An RGB transport endpoint.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct TransportEndpoint {
    /// Endpoint address
    pub endpoint: String,
    /// Endpoint transport type
    pub transport_type: TransportType,
}

impl TransportEndpoint {
    /// Builds a new [`TransportEndpoint::endpoint`] from the provided string, checking that it is
    /// valid.
    pub fn new(transport_endpoint: String) -> Result<Self, Error> {
        let rgb_transport = RgbTransport::from_str(&transport_endpoint)?;
        TransportEndpoint::try_from(rgb_transport)
    }

    /// Return the transport type of this transport endpoint.
    pub fn transport_type(&self) -> TransportType {
        self.transport_type
    }
}

impl TryFrom<RgbTransport> for TransportEndpoint {
    type Error = Error;

    fn try_from(x: RgbTransport) -> Result<Self, Self::Error> {
        match x {
            RgbTransport::JsonRpc { tls, host } => Ok(TransportEndpoint {
                endpoint: format!("http{}://{host}", if tls { "s" } else { "" }),
                transport_type: TransportType::JsonRpc,
            }),
            _ => Err(Error::UnsupportedTransportType),
        }
    }
}

/// Supported database types.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum DatabaseType {
    /// A SQLite database
    Sqlite,
}

/// A bitcoin address.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Address {
    /// The bitcoin address string
    address_string: String,
    /// The bitcoin network of the address
    bitcoin_network: BitcoinNetwork,
}

impl Address {
    /// Parse the provided `address_string`.
    /// Throws an error if the provided string is not a valid bitcoin address for the given
    /// network.
    pub fn new(address_string: String, bitcoin_network: BitcoinNetwork) -> Result<Self, Error> {
        parse_address_str(&address_string, bitcoin_network)?;
        Ok(Address {
            address_string,
            bitcoin_network,
        })
    }
}

/// An RGB invoice.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Invoice {
    /// The RGB invoice string
    invoice_string: String,
    /// The data of the RGB invoice
    pub(crate) invoice_data: InvoiceData,
}

impl Invoice {
    /// Parse the provided `invoice_string`.
    /// Throws an error if the provided string is not a valid RGB invoice.
    pub fn new(invoice_string: String) -> Result<Self, Error> {
        let decoded = RgbInvoice::from_str(&invoice_string).map_err(|e| Error::InvalidInvoice {
            details: e.to_string(),
        })?;
        let asset_id = decoded.contract.map(|cid| cid.to_string());
        let asset_schema = if let Some(schema_id) = decoded.schema {
            Some(<AssetSchema as std::convert::TryFrom<_>>::try_from(
                schema_id,
            )?)
        } else {
            None
        };
        let assignment_name = decoded.assignment_name.map(|a| a.to_string());
        let assignment = match asset_schema {
            None => match (decoded.assignment_state, assignment_name.as_deref()) {
                (Some(InvoiceState::Amount(v)), Some(RGB_STATE_ASSET_OWNER)) => {
                    Assignment::Fungible(v.value())
                }
                (Some(InvoiceState::Amount(v)), Some(RGB_STATE_INFLATION_ALLOWANCE)) => {
                    Assignment::InflationRight(v.value())
                }
                (Some(InvoiceState::Amount(_)), _) => Assignment::Any,
                (Some(InvoiceState::Data(_)), Some(RGB_STATE_ASSET_OWNER) | None) => {
                    Assignment::NonFungible
                }
                (None, None) => Assignment::Any,
                (_, _) => {
                    return Err(Error::InvalidInvoice {
                        details: s!("unsupported assignment"),
                    });
                }
            },
            Some(AssetSchema::Nia) => {
                match (decoded.assignment_state, assignment_name.as_deref()) {
                    (Some(InvoiceState::Amount(v)), Some(RGB_STATE_ASSET_OWNER) | None) => {
                        Assignment::Fungible(v.value())
                    }
                    (None, Some(RGB_STATE_ASSET_OWNER) | None) => Assignment::Fungible(0),
                    (_, _) => {
                        return Err(Error::InvalidInvoice {
                            details: s!("invalid assignment"),
                        });
                    }
                }
            }
            Some(AssetSchema::Ifa) => {
                match (decoded.assignment_state, assignment_name.as_deref()) {
                    (Some(InvoiceState::Amount(v)), Some(RGB_STATE_ASSET_OWNER)) => {
                        Assignment::Fungible(v.value())
                    }
                    (None, Some(RGB_STATE_ASSET_OWNER)) => Assignment::Fungible(0),
                    (Some(InvoiceState::Amount(v)), Some(RGB_STATE_INFLATION_ALLOWANCE)) => {
                        Assignment::InflationRight(v.value())
                    }
                    (None, Some(RGB_STATE_INFLATION_ALLOWANCE)) => Assignment::InflationRight(0),
                    (Some(InvoiceState::Amount(_)), None) => Assignment::Any,
                    (None, None) => Assignment::Any,
                    (_, _) => {
                        return Err(Error::InvalidInvoice {
                            details: s!("invalid assignment"),
                        });
                    }
                }
            }
        };
        let recipient_id = decoded.beneficiary.to_string();
        let transport_endpoints: Vec<String> =
            decoded.transports.iter().map(|t| t.to_string()).collect();

        let layer_1 = decoded.beneficiary.layer1();
        let network = match layer_1 {
            Layer1::Bitcoin => decoded.beneficiary.chain_network().try_into().unwrap(),
            _ => {
                return Err(Error::UnsupportedLayer1 {
                    layer_1: layer_1.to_string(),
                });
            }
        };

        let invoice_data = InvoiceData {
            recipient_id,
            asset_schema,
            asset_id,
            assignment,
            assignment_name,
            expiration_timestamp: decoded.expiry,
            transport_endpoints,
            network,
        };

        Ok(Invoice {
            invoice_string,
            invoice_data,
        })
    }

    /// Return the data associated with this [`Invoice`].
    pub fn invoice_data(&self) -> InvoiceData {
        self.invoice_data.clone()
    }

    /// Return the string associated with this [`Invoice`].
    pub fn invoice_string(&self) -> String {
        self.invoice_string.clone()
    }
}

/// The data of an RGB invoice.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct InvoiceData {
    /// ID of the receive operation (blinded UTXO or Bitcoin script)
    pub recipient_id: String,
    /// RGB schema
    pub asset_schema: Option<AssetSchema>,
    /// RGB asset ID
    pub asset_id: Option<String>,
    /// RGB assignment
    pub assignment: Assignment,
    /// RGB assignment name
    pub assignment_name: Option<String>,
    /// Bitcoin network
    pub network: BitcoinNetwork,
    /// Invoice expiration
    pub expiration_timestamp: Option<i64>,
    /// Transport endpoints
    pub transport_endpoints: Vec<String>,
}

/// Data for operations that require the wallet to be online.
///
/// Methods not requiring an `Online` object don't need network access and can be performed
/// offline. Methods taking an optional `Online` will operate offline when it's missing and will
/// use local data only.
///
/// <div class="warning">This should not be manually constructed but should be obtained from the
/// [`Wallet::go_online`] method.</div>
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Online {
    /// Unique ID for this object
    pub id: u64,
    /// URL of the indexer server to be used for online operations
    pub indexer_url: String,
}

/// Bitcoin transaction outpoint.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Outpoint {
    /// ID of the transaction
    pub txid: String,
    /// Output index
    pub vout: u32,
}

impl fmt::Display for Outpoint {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "{}:{}", self.txid, self.vout)
    }
}

impl From<OutPoint> for Outpoint {
    fn from(x: OutPoint) -> Outpoint {
        Outpoint {
            txid: x.txid.to_string(),
            vout: x.vout,
        }
    }
}

impl From<DbTxo> for Outpoint {
    fn from(x: DbTxo) -> Outpoint {
        Outpoint {
            txid: x.txid,
            vout: x.vout,
        }
    }
}

impl From<Outpoint> for OutPoint {
    fn from(x: Outpoint) -> OutPoint {
        OutPoint::from_str(&x.to_string()).expect("outpoint should be parsable")
    }
}

/// A recipient of an RGB transfer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Recipient {
    /// Recipient ID
    pub recipient_id: String,
    /// Witness data (to be provided only with a witness recipient)
    pub witness_data: Option<WitnessData>,
    /// RGB assignment
    pub assignment: Assignment,
    /// Transport endpoints
    pub transport_endpoints: Vec<String>,
}

/// The information needed to receive RGB assets in witness mode.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct WitnessData {
    /// The Bitcoin amount (in sats) to send to the recipient
    #[serde(deserialize_with = "from_str_or_number_mandatory")]
    pub amount_sat: u64,
    /// An optional blinding
    #[serde(deserialize_with = "from_str_or_number_optional")]
    pub blinding: Option<u64>,
}

/// The role of the wallet for an on-chain swap step.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum OnchainSwapRole {
    /// The party creating the offer
    Maker,
    /// The party accepting the offer
    Taker,
}

/// The last step this wallet reached in an on-chain swap.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum OnchainSwapStage {
    /// The maker created the offer
    Offered,
    /// The taker accepted the offer
    Requested,
    /// The maker built the proposal
    Proposed,
    /// This wallet broadcast the swap transaction
    Broadcast,
    /// This wallet accepted the swap transfers
    Accepted,
    /// The swap was cancelled before broadcast
    Cancelled,
}

/// A swap known to this wallet, as returned by [`Wallet::list_swaps`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct OnchainSwapSummary {
    /// Swap ID
    pub swap_id: String,
    /// This wallet's role
    pub role: OnchainSwapRole,
    /// Last step reached
    pub stage: OnchainSwapStage,
    /// Offer expiration
    pub expiration_timestamp: Option<u64>,
    /// Swap transaction ID, once the maker has built the proposal
    pub txid: Option<String>,
}

/// The type of an on-chain swap leg.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum OnchainSwapLegKind {
    /// A bitcoin amount in satoshis
    Btc,
    /// An RGB asset amount
    Rgb,
}

/// A single side of an on-chain swap.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct OnchainSwapLeg {
    /// Leg kind
    pub kind: OnchainSwapLegKind,
    /// RGB asset ID, required when `kind` is `Rgb`
    pub asset_id: Option<String>,
    /// Amount in satoshis for BTC legs, or asset units for RGB legs
    #[serde(deserialize_with = "from_str_or_number_mandatory")]
    pub amount: u64,
}

/// A consignment produced or consumed by an on-chain swap.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct OnchainSwapConsignment {
    /// RGB asset ID
    pub asset_id: String,
    /// Local consignment cache path, empty for proxy-delivered swap payloads
    pub path: String,
    /// Proxy transport endpoint used to retrieve the consignment
    pub endpoint: Option<String>,
    /// Witness transaction ID
    pub txid: String,
    /// Witness vout receiving the RGB assignment
    pub vout: u32,
    /// Seal blinding needed by the receiver
    #[serde(deserialize_with = "from_str_or_number_mandatory")]
    pub blinding: u64,
    /// Proxy lookup key (the receiver's RGB recipient ID). Distinct per consignment so that
    /// RGB-for-RGB swaps with two consignments under the same witness transaction do not collide
    /// in the proxy server's keyspace.
    pub recipient_id: String,
}

/// Asset history needed by a counterparty to validate/import an RGB asset before a swap step.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct OnchainSwapAssetHistory {
    /// RGB asset ID
    pub asset_id: String,
    /// Local history cache path, empty for proxy-delivered swap payloads
    pub path: String,
    /// Proxy transport endpoint used to retrieve the history file
    pub endpoint: Option<String>,
    /// Proxy lookup key
    pub recipient_id: String,
}

/// A Bitcoin input selected for an on-chain swap.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct OnchainSwapInput {
    /// The selected outpoint
    pub outpoint: Outpoint,
    /// Previous output value in satoshis
    #[serde(deserialize_with = "from_str_or_number_mandatory")]
    pub amount_sat: u64,
    /// Previous output script pubkey
    pub script_pubkey_hex: String,
}

/// A maker-created on-chain swap offer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct OnchainSwapOffer {
    /// Swap ID
    pub swap_id: String,
    /// Maker gives this leg
    pub maker_gives: OnchainSwapLeg,
    /// Maker receives this leg
    pub maker_receives: OnchainSwapLeg,
    /// Bitcoin network
    pub bitcoin_network: BitcoinNetwork,
    /// Network fee to reserve for the swap transaction
    #[serde(deserialize_with = "from_str_or_number_mandatory")]
    pub network_fee_sat: u64,
    /// Sat value used for each RGB witness output
    #[serde(deserialize_with = "from_str_or_number_mandatory")]
    pub rgb_output_sat: u64,
    /// Optional expiration timestamp
    pub expiration_timestamp: Option<u64>,
    /// Maker BTC receive address, present if maker receives BTC
    pub maker_btc_address: Option<String>,
    /// Maker RGB receive recipient ID, present if maker receives RGB
    pub maker_rgb_recipient_id: Option<String>,
    /// Maker RGB receive script pubkey, present if maker receives RGB
    pub maker_rgb_script_pubkey_hex: Option<String>,
    /// Maker RGB receive blinding, present if maker receives RGB
    pub maker_rgb_blinding: Option<u64>,
    /// Required proxy URL for consignment and asset-history delivery
    pub proxy_url: Option<String>,
    /// Platform fee reserved for a third-party facilitator, in sats. Funded by the taker
    /// alongside `network_fee_sat`. Zero when there is no facilitator.
    #[serde(default, deserialize_with = "from_str_or_number_mandatory")]
    pub platform_fee_sat: u64,
    /// Facilitator's fee-receive script pubkey, required when `platform_fee_sat > 0`
    pub fee_recipient_script_pubkey_hex: Option<String>,
}

/// A taker response to an on-chain swap offer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct OnchainSwapRequest {
    /// Swap offer
    pub offer: OnchainSwapOffer,
    /// Taker inputs serialized for coordinated PSBT construction
    pub taker_inputs: Vec<OnchainSwapInput>,
    /// Taker BTC receive address, present if taker receives BTC
    pub taker_btc_address: Option<String>,
    /// Taker RGB receive recipient ID, present if taker receives RGB
    pub taker_rgb_recipient_id: Option<String>,
    /// Taker RGB receive script pubkey, present if taker receives RGB
    pub taker_rgb_script_pubkey_hex: Option<String>,
    /// Taker RGB receive blinding, present if taker receives RGB
    pub taker_rgb_blinding: Option<u64>,
    /// Taker BTC/RGB change script pubkey
    pub taker_change_script_pubkey_hex: String,
}

/// A maker-created proposal containing the coordinated PSBT.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct OnchainSwapProposal {
    /// Swap request
    pub request: OnchainSwapRequest,
    /// Maker inputs serialized for coordinated PSBT construction
    pub maker_inputs: Vec<OnchainSwapInput>,
    /// Maker BTC/RGB change script pubkey
    pub maker_change_script_pubkey_hex: String,
    /// PSBT after maker-side processing
    pub psbt: String,
    /// Witness transaction ID after any maker coloring
    pub txid: String,
    /// Consignments produced by the maker
    pub consignments: Vec<OnchainSwapConsignment>,
    /// Maker asset history, required when the maker sends RGB and the counterparty needs the
    /// contract history before completing the swap.
    pub maker_history: Option<OnchainSwapAssetHistory>,
}

/// A taker-completed swap ready for maker verification/broadcast.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct OnchainSwapCompletion {
    /// Swap proposal
    pub proposal: OnchainSwapProposal,
    /// Fully or partially signed PSBT after taker-side processing
    pub psbt: String,
    /// Finalized PSBT when finalization succeeds
    pub finalized_psbt: Option<String>,
    /// Witness transaction ID
    pub txid: String,
    /// Consignments produced by both parties
    pub consignments: Vec<OnchainSwapConsignment>,
    /// Taker asset history, required when the taker sends RGB and the maker needs the contract
    /// history before processing the completion.
    pub taker_history: Option<OnchainSwapAssetHistory>,
}

/// Result of accepting swap RGB transfers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct OnchainSwapReceiveResult {
    /// Assignments accepted from received consignments
    pub assignments: Vec<Assignment>,
}

/// An RGB allocation.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct RgbAllocation {
    /// Asset ID
    pub asset_id: Option<String>,
    /// RGB assignment
    pub assignment: Assignment,
    /// Defines if the allocation is settled, meaning it refers to a transfer in the
    /// [`TransferStatus::Settled`] status
    pub settled: bool,
}

impl From<LocalRgbAllocation> for RgbAllocation {
    fn from(x: LocalRgbAllocation) -> RgbAllocation {
        RgbAllocation {
            asset_id: x.asset_id.clone(),
            assignment: x.assignment.clone(),
            settled: x.settled(),
        }
    }
}

/// A Bitcoin transaction.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Transaction {
    /// Type of transaction
    pub transaction_type: TransactionType,
    /// Transaction ID
    pub txid: String,
    /// Received value (in sats), computed as the sum of owned output amounts included in this
    /// transaction
    pub received: u64,
    /// Sent value (in sats), computed as the sum of owned input amounts included in this
    /// transaction
    pub sent: u64,
    /// Fee value (in sats)
    pub fee: u64,
    /// Height and Unix timestamp of the block containing the transaction if confirmed, `None` if
    /// unconfirmed
    pub confirmation_time: Option<BlockTime>,
}

/// Block height and timestamp of a block.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct BlockTime {
    /// Confirmation block height
    pub height: u32,
    /// Confirmation block timestamp
    pub timestamp: u64,
}

/// The type of a transaction.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum TransactionType {
    /// Transaction used to perform an RGB send
    RgbSend,
    /// Transaction used to drain the RGB wallet
    Drain,
    /// Transaction used to create UTXOs
    CreateUtxos,
    /// Transaction not created by rgb-lib directly
    User,
}

/// An asset filter for listing transfers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub enum AssetFilter {
    /// Match transfers of any asset, including those not connected to one
    Any,
    /// Match only transfers not connected to a specific asset
    NoAsset,
    /// Match only transfers of the asset with the given ID
    Id(String),
}

/// An RGB transfer.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Transfer {
    /// ID of the transfer
    pub idx: i32,
    /// ID of the batch transfer containing this transfer
    pub batch_transfer_idx: i32,
    /// Timestamp of the transfer creation
    pub created_at: i64,
    /// Timestamp of the transfer last update
    pub updated_at: i64,
    /// Status of the transfer
    pub status: TransferStatus,
    /// Requested RGB assignment
    pub requested_assignment: Option<Assignment>,
    /// RGB assignmnents
    pub assignments: Vec<Assignment>,
    /// Type of the transfer
    pub kind: TransferKind,
    /// ID of the Bitcoin transaction anchoring the transfer
    pub txid: Option<String>,
    /// Recipient ID (blinded UTXO or Bitcoin script) of an incoming transfer
    pub recipient_id: Option<String>,
    /// UTXO of an incoming transfer
    pub receive_utxo: Option<Outpoint>,
    /// Change UTXO of an outgoing transfer
    pub change_utxo: Option<Outpoint>,
    /// Expiration of the transfer
    pub expiration: Option<i64>,
    /// Transport endpoints for the transfer
    pub transport_endpoints: Vec<TransferTransportEndpoint>,
    /// Invoice string of the incoming transfer
    pub invoice_string: Option<String>,
    /// Consignment path
    pub consignment_path: Option<String>,
}

impl Transfer {
    fn from_db_transfer(
        x: &DbTransfer,
        td: TransferData,
        transport_endpoints: Vec<TransferTransportEndpoint>,
    ) -> Transfer {
        Transfer {
            idx: x.idx,
            batch_transfer_idx: td.batch_transfer_idx,
            created_at: td.created_at,
            updated_at: td.updated_at,
            status: td.status,
            requested_assignment: x.requested_assignment.clone(),
            assignments: td.assignments,
            kind: td.kind,
            txid: td.txid,
            recipient_id: x.recipient_id.clone(),
            receive_utxo: td.receive_utxo,
            change_utxo: td.change_utxo,
            expiration: td.expiration,
            transport_endpoints,
            invoice_string: x.invoice_string.clone(),
            consignment_path: td.consignment_path,
        }
    }
}

/// An RGB transport endpoint for a transfer.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct TransferTransportEndpoint {
    /// Endpoint address
    pub endpoint: String,
    /// Endpoint transport type
    pub transport_type: TransportType,
    /// Whether the endpoint has been used
    pub used: bool,
}

impl TransferTransportEndpoint {
    fn from_db_transfer_transport_endpoint(
        x: &DbTransferTransportEndpoint,
        ce: &DbTransportEndpoint,
    ) -> TransferTransportEndpoint {
        TransferTransportEndpoint {
            endpoint: ce.endpoint.clone(),
            transport_type: ce.transport_type,
            used: x.used,
        }
    }
}

/// The type of an RGB transfer.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum TransferKind {
    /// A transfer that issued the asset
    Issuance,
    /// An incoming transfer via blinded UTXO
    ReceiveBlind,
    /// An incoming transfer via a Bitcoin script (witness TX)
    ReceiveWitness,
    /// An outgoing transfer
    Send,
    /// An inflation transfer
    Inflation,
}

/// A wallet unspent.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Unspent {
    /// Bitcoin UTXO
    pub utxo: Utxo,
    /// RGB allocations on the UTXO
    pub rgb_allocations: Vec<RgbAllocation>,
    /// Number of pending blind receive operations
    pub pending_blinded: u32,
}

impl From<LocalUnspent> for Unspent {
    fn from(x: LocalUnspent) -> Unspent {
        Unspent {
            utxo: Utxo::from(x.utxo),
            rgb_allocations: x
                .rgb_allocations
                .into_iter()
                .map(RgbAllocation::from)
                .collect::<Vec<RgbAllocation>>(),
            pending_blinded: x.pending_blinded,
        }
    }
}

impl From<LocalOutput> for Unspent {
    fn from(x: LocalOutput) -> Unspent {
        Unspent {
            utxo: Utxo::from(x),
            rgb_allocations: vec![],
            pending_blinded: 0,
        }
    }
}

/// A Bitcoin unspent transaction output.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Utxo {
    /// UTXO outpoint
    pub outpoint: Outpoint,
    /// Amount (in sats)
    pub btc_amount: u64,
    /// Defines if the UTXO can have RGB allocations
    pub colorable: bool,
    /// Defines if the UTXO already exists (TX that creates it has been broadcasted)
    pub exists: bool,
}

impl From<DbTxo> for Utxo {
    fn from(x: DbTxo) -> Utxo {
        Utxo {
            outpoint: x.outpoint(),
            btc_amount: x
                .btc_amount
                .parse::<u64>()
                .expect("DB should contain a valid u64 value"),
            colorable: true,
            exists: x.exists,
        }
    }
}

impl From<LocalOutput> for Utxo {
    fn from(x: LocalOutput) -> Utxo {
        Utxo {
            outpoint: Outpoint::from(x.outpoint),
            btc_amount: x.txout.value.to_sat(),
            colorable: false,
            exists: true,
        }
    }
}

/// Data that defines a [`Wallet`].
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct WalletData {
    /// Directory where the wallet directory is stored
    pub data_dir: String,
    /// Bitcoin network for the wallet
    pub bitcoin_network: BitcoinNetwork,
    /// Database type for the wallet
    pub database_type: DatabaseType,
    /// The max number of RGB allocations allowed per UTXO
    #[serde(deserialize_with = "from_str_or_number_mandatory")]
    pub max_allocations_per_utxo: u32,
    /// Wallet account-level xPub for the vanilla-side of the wallet
    pub account_xpub_vanilla: String,
    /// Wallet account-level xPub for the colored-side of the wallet
    pub account_xpub_colored: String,
    /// Wallet mnemonic phrase
    pub mnemonic: Option<String>,
    /// Wallet master fingerprint
    pub master_fingerprint: String,
    /// Keychain index for the vanilla-side of the wallet (default: 0)
    #[serde(deserialize_with = "from_str_or_number_optional")]
    pub vanilla_keychain: Option<u8>,
    /// List of schemas the wallet should support (when issuing, sending and receiving). Empty list
    /// means the wallet should support all the schemas rgb-lib supports.
    pub supported_schemas: Vec<AssetSchema>,
    /// When `true`, address generation returns a pinned address instead of creating new ones.
    /// Default: `false`
    #[serde(default)]
    pub reuse_addresses: bool,
}

pub(crate) type BdkPersister = super::memory_store::MemoryStore<ChangeSet>;

pub(crate) type LoggerGuard = ();

pub(crate) type RgbLibDb = crate::database::memory_db::InMemoryDb;

/// An RGB wallet.
///
/// This should not be manually constructed but should be obtained from the [`Wallet::new`]
/// method.
pub struct Wallet {
    pub(crate) wallet_data: WalletData,
    pub(crate) logger: Logger,
    pub(crate) _logger_guard: LoggerGuard,
    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) watch_only: bool,
    pub(crate) database: Arc<RgbLibDb>,
    pub(crate) wallet_dir: PathBuf,
    pub(crate) bdk_wallet: PersistedWallet<BdkPersister>,
    pub(crate) bdk_database: BdkPersister,
    /// Pinned derivation index per keychain for address reuse.
    pub(crate) reuse_address_index: HashMap<KeychainKind, u32>,
    #[cfg(feature = "esplora")]
    pub(crate) wasm_proxy_client: crate::api::proxy::WasmProxyClient,
    #[cfg(feature = "esplora")]
    pub(crate) online_data: Option<OnlineData>,
    /// In-memory storage for transfer artifacts (replaces filesystem on wasm32).
    /// Key: txid
    #[cfg(feature = "esplora")]
    pub(crate) transfer_artifacts: HashMap<String, super::online::TransferArtifacts>,
    /// In-memory storage for received consignment bytes (replaces filesystem on wasm32).
    /// Key: recipient_id
    #[cfg(feature = "esplora")]
    pub(crate) received_consignments: HashMap<String, Vec<u8>>,
    pub(crate) idb_save_in_progress: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub(crate) idb_sequence: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Persistent in-memory RGB stock.
    pub(crate) rgb_stock: std::rc::Rc<std::cell::RefCell<Option<rgbstd::persistence::Stock>>>,
    pub(crate) vss_client: Option<super::vss::VssBackupClient>,
    pub(crate) last_vss_backup_error: std::cell::RefCell<Option<String>>,
}

impl Wallet {
    /// Create a wallet and restore its latest durable IndexedDB snapshot, if one exists.
    ///
    /// The returned wallet is ready for use only after the IndexedDB read and snapshot
    /// restoration complete. IndexedDB and snapshot decoding failures are returned to the caller.
    pub async fn restore(wallet_data: WalletData) -> Result<Self, Error> {
        let mut wallet = Self::new(wallet_data)?;
        let snapshot = super::idb_store::load_snapshot(&wallet.idb_key())
            .await
            .map_err(|details| Error::Persistence { details })?;
        if let Some(snapshot) = snapshot {
            if snapshot.stock_stash_b64.is_none()
                || snapshot.stock_state_b64.is_none()
                || snapshot.stock_index_b64.is_none()
            {
                return Err(Error::Persistence {
                    details: s!("persisted wallet snapshot is missing RGB stock state"),
                });
            }
            wallet
                .idb_sequence
                .store(snapshot.sequence, std::sync::atomic::Ordering::SeqCst);
            wallet.restore_from_snapshot(snapshot)?;
        }
        Ok(wallet)
    }

    /// Create a new RGB wallet based on the provided [`WalletData`].
    #[allow(clippy::arc_with_non_send_sync)] // wasm32 is single-threaded; Arc matches native API
    pub fn new(wallet_data: WalletData) -> Result<Self, Error> {
        let wdata = wallet_data.clone();

        // wallet account xPubs
        let bdk_network = BdkNetwork::from(wdata.bitcoin_network);
        let xpub_rgb = str_to_xpub(&wdata.account_xpub_colored, bdk_network)?;
        let xpub_btc = str_to_xpub(&wdata.account_xpub_vanilla, bdk_network)?;

        // wallet directory and file logging setup
        let data_dir_path = PathBuf::from(&wdata.data_dir);
        if let Some(mnemonic) = &wdata.mnemonic {
            // check master fingerprint derived from mnemonic matches provided one
            let mnemonic = Mnemonic::parse_in(Language::English, mnemonic)?;
            let master_xprv =
                Xpriv::new_master(wdata.bitcoin_network, &mnemonic.to_seed("")).unwrap();
            let master_xpub = Xpub::from_priv(&Secp256k1::new(), &master_xprv);
            let master_fingerprint = master_xpub.fingerprint();
            if master_fingerprint
                != Fingerprint::from_str(&wdata.master_fingerprint)
                    .map_err(|_| Error::InvalidFingerprint)?
            {
                return Err(Error::FingerprintMismatch);
            }
        }
        let wallet_dir = data_dir_path.join(&wdata.master_fingerprint);
        let (logger, _logger_guard) = setup_logger(&wallet_dir, None)?;
        info!(logger.clone(), "New wallet in '{:?}'", wallet_dir);
        let panic_logger = logger.clone();
        let prev_hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            error!(panic_logger.clone(), "PANIC: {:?}", info);
            prev_hook(info);
        }));

        // BDK setup
        let (desc_colored, desc_vanilla, watch_only) = if let Some(mnemonic) = wdata.mnemonic {
            let (desc_colored, desc_vanilla) = get_descriptors(
                wdata.bitcoin_network,
                &mnemonic,
                wdata.vanilla_keychain,
                xpub_btc,
                xpub_rgb,
            )?;
            (desc_colored, desc_vanilla, false)
        } else {
            let (desc_colored, desc_vanilla) = get_descriptors_from_xpubs(
                wdata.bitcoin_network,
                &wdata.master_fingerprint,
                xpub_rgb,
                xpub_btc,
                wdata.vanilla_keychain,
            )?;
            (desc_colored, desc_vanilla, true)
        };
        let chain_net: ChainNet = wdata.bitcoin_network.into();
        let mut wallet_params = BdkWallet::load()
            .descriptor(KeychainKind::External, Some(desc_colored.clone()))
            .descriptor(KeychainKind::Internal, Some(desc_vanilla.clone()))
            .check_genesis_hash(BlockHash::from_byte_array(
                chain_net.chain_hash().to_bytes(),
            ));
        if !watch_only {
            wallet_params = wallet_params.extract_keys();
        }
        let mut bdk_database: BdkPersister = super::memory_store::MemoryStore::new();
        let bdk_wallet = match wallet_params.load_wallet(&mut bdk_database)? {
            Some(wallet) => wallet,
            None => BdkWallet::create(desc_colored, desc_vanilla)
                .network(bdk_network)
                .create_wallet(&mut bdk_database)?,
        };

        // RGB setup
        let supported_schemas = wdata.supported_schemas;
        if supported_schemas.is_empty() {
            return Err(Error::NoSupportedSchemas);
        }
        if wdata.bitcoin_network == BitcoinNetwork::Mainnet
            && supported_schemas.contains(&AssetSchema::Ifa)
        {
            return Err(Error::CannotUseIfaOnMainnet);
        }
        let mut runtime = load_rgb_runtime(wallet_dir.clone())?;
        let known_schemas = runtime.schemata()?;
        if known_schemas.len() < NUM_KNOWN_SCHEMAS {
            let known: HashSet<_> = known_schemas.iter().map(|s| s.id).collect();
            for schema in supported_schemas {
                if !known.contains(&SchemaId::from(schema)) {
                    schema.import_kit(&mut runtime)?;
                }
            }
        }

        // RGB-LIB setup
        let database = crate::database::memory_db::InMemoryDb::new();
        #[cfg(feature = "esplora")]
        let wasm_proxy_client = crate::api::proxy::WasmProxyClient::new()?;

        info!(logger, "New wallet completed");
        Ok(Wallet {
            wallet_data,
            logger,
            _logger_guard,
            watch_only,
            database: Arc::new(database),
            wallet_dir,
            bdk_wallet,
            bdk_database,
            reuse_address_index: HashMap::new(),
            #[cfg(feature = "esplora")]
            wasm_proxy_client,
            #[cfg(feature = "esplora")]
            online_data: None,
            #[cfg(feature = "esplora")]
            transfer_artifacts: HashMap::new(),
            #[cfg(feature = "esplora")]
            received_consignments: HashMap::new(),
            idb_save_in_progress: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            idb_sequence: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            rgb_stock: {
                let stock =
                    std::mem::replace(&mut runtime.stock, rgbstd::persistence::Stock::in_memory());
                std::rc::Rc::new(std::cell::RefCell::new(Some(stock)))
            },
            vss_client: None,
            last_vss_backup_error: std::cell::RefCell::new(None),
        })
    }

    pub(crate) fn bitcoin_network(&self) -> BitcoinNetwork {
        self.wallet_data.bitcoin_network
    }

    pub(crate) fn chain_net(&self) -> ChainNet {
        self.bitcoin_network().into()
    }

    pub(crate) fn rgb_runtime(&self) -> Result<RgbRuntime, Error> {
        let stock = self
            .rgb_stock
            .try_borrow_mut()
            .map_err(|_| Error::Internal {
                details: s!("RGB runtime already in use"),
            })?
            .take()
            .unwrap_or_else(rgbstd::persistence::Stock::in_memory);
        Ok(RgbRuntime::with_shared_stock(
            stock,
            self.wallet_dir.clone(),
            self.rgb_stock.clone(),
        ))
    }

    /// Return the data that defines the wallet.
    pub fn get_wallet_data(&self) -> WalletData {
        self.wallet_data.clone()
    }

    /// Return the wallet directory.
    pub fn get_wallet_dir(&self) -> PathBuf {
        self.wallet_dir.clone()
    }

    /// Return the media directory.
    pub fn get_media_dir(&self) -> PathBuf {
        self.wallet_dir.join(MEDIA_DIR)
    }

    pub(crate) fn get_transfers_dir(&self) -> PathBuf {
        self.wallet_dir.join(TRANSFERS_DIR)
    }

    pub(crate) fn get_transfer_dir(&self, transfer_id: &str) -> PathBuf {
        self.get_transfers_dir().join(transfer_id)
    }

    pub(crate) fn get_asset_transfer_dir<P: AsRef<Path>>(
        &self,
        transfer_dir: P,
        asset_id: &str,
    ) -> PathBuf {
        let asset_id_no_prefix = asset_id.replace(ASSET_ID_PREFIX, "");
        transfer_dir.as_ref().join(&asset_id_no_prefix)
    }

    pub(crate) fn max_allocations_per_utxo(&self) -> u32 {
        self.wallet_data.max_allocations_per_utxo
    }

    pub(crate) fn supports_schema(&self, asset_schema: &AssetSchema) -> bool {
        self.wallet_data.supported_schemas.contains(asset_schema)
    }

    pub(crate) fn check_schema_support(&self, asset_schema: &AssetSchema) -> Result<(), Error> {
        if !self.supports_schema(asset_schema) {
            return Err(Error::UnsupportedSchema {
                asset_schema: *asset_schema,
            });
        }
        Ok(())
    }

    pub(crate) fn check_transport_endpoints(
        &self,
        transport_endpoints: &[String],
    ) -> Result<(), Error> {
        if transport_endpoints.is_empty() {
            return Err(Error::InvalidTransportEndpoints {
                details: s!("must provide at least a transport endpoint"),
            });
        }
        if transport_endpoints.len() > MAX_TRANSPORT_ENDPOINTS {
            return Err(Error::InvalidTransportEndpoints {
                details: format!(
                    "library supports at max {MAX_TRANSPORT_ENDPOINTS} transport endpoints"
                ),
            });
        }

        Ok(())
    }

    pub(crate) fn filter_unspents(
        &self,
        keychain: KeychainKind,
    ) -> impl Iterator<Item = LocalOutput> + '_ {
        self.bdk_wallet
            .list_unspent()
            .filter(move |u| u.keychain == keychain)
    }

    pub(crate) fn internal_unspents(&self) -> impl Iterator<Item = LocalOutput> + '_ {
        self.filter_unspents(KeychainKind::Internal)
    }

    pub(crate) fn get_uncolorable_btc_sum(&self) -> Result<u64, Error> {
        Ok(self
            .internal_unspents()
            .map(|u| u.txout.value.to_sat())
            .sum())
    }

    pub(crate) fn get_available_allocations<T>(
        &self,
        unspents: T,
        exclude_utxos: &[Outpoint],
        max_allocations: Option<u32>,
    ) -> Result<Vec<LocalUnspent>, Error>
    where
        T: Into<Vec<LocalUnspent>>,
    {
        let mut mut_unspents = unspents.into();
        mut_unspents
            .iter_mut()
            .for_each(|u| u.rgb_allocations.retain(|a| !a.status.failed()));
        let max_allocs = max_allocations.unwrap_or(self.max_allocations_per_utxo() - 1);
        Ok(mut_unspents
            .iter()
            .filter(|u| u.utxo.exists)
            .filter(|u| !u.utxo.pending_witness)
            .filter(|u| !exclude_utxos.contains(&u.utxo.outpoint()))
            .filter(|u| {
                (u.rgb_allocations.len() as u32) + u.pending_blinded <= max_allocs
                    && !u
                        .rgb_allocations
                        .iter()
                        .any(|a| !a.incoming && a.status.waiting_counterparty())
            })
            .cloned()
            .collect())
    }

    pub(crate) fn detect_btc_unspendable_err(&self) -> Result<Error, Error> {
        let available = self.get_uncolorable_btc_sum()?;
        Ok(if available < MIN_BTC_REQUIRED {
            Error::InsufficientBitcoins {
                needed: MIN_BTC_REQUIRED,
                available,
            }
        } else {
            Error::InsufficientAllocationSlots
        })
    }
}

impl Wallet {
    pub(crate) fn get_utxo(
        &self,
        exclude_utxos: &[Outpoint],
        unspents: Option<&[LocalUnspent]>,
        pending_operation: bool,
        max_allocations: Option<u32>,
    ) -> Result<DbTxo, Error> {
        let rgb_allocations = if unspents.is_none() {
            let unspent_txos = self.database.get_unspent_txos(vec![])?;
            Some(
                self.database
                    .get_rgb_allocations(unspent_txos, None, None, None, None)?,
            )
        } else {
            None
        };
        let unspents: &[LocalUnspent] = match unspents {
            Some(u) => u,
            None => rgb_allocations.as_deref().unwrap(),
        };

        let mut allocatable =
            self.get_available_allocations(unspents, exclude_utxos, max_allocations)?;
        allocatable.sort_by_key(|t| t.rgb_allocations.len() + t.pending_blinded as usize);
        match allocatable.first() {
            Some(mut selected) => {
                if allocatable.len() > 1 && !selected.rgb_allocations.is_empty() {
                    let filtered_allocatable: Vec<&LocalUnspent> = if pending_operation {
                        allocatable
                            .iter()
                            .filter(|t| t.rgb_allocations.iter().any(|a| a.future()))
                            .collect()
                    } else {
                        allocatable
                            .iter()
                            .filter(|t| t.rgb_allocations.iter().all(|a| !a.future()))
                            .collect()
                    };
                    if let Some(other) = filtered_allocatable.first() {
                        selected = other;
                    }
                }
                Ok(selected.clone().utxo)
            }
            None => Err(self.detect_btc_unspendable_err()?),
        }
    }

    pub(crate) fn save_transfer_transport_endpoint(
        &self,
        transfer_idx: i32,
        transport_endpoint: &LocalTransportEndpoint,
    ) -> Result<(), Error> {
        let transport_endpoint_idx = match self
            .database
            .get_transport_endpoint(transport_endpoint.endpoint.clone())?
        {
            Some(ce) => ce.idx,
            None => self
                .database
                .set_transport_endpoint(DbTransportEndpointActMod {
                    transport_type: ActiveValue::Set(transport_endpoint.transport_type),
                    endpoint: ActiveValue::Set(transport_endpoint.endpoint.clone()),
                    ..Default::default()
                })?,
        };

        self.database
            .set_transfer_transport_endpoint(DbTransferTransportEndpointActMod {
                transfer_idx: ActiveValue::Set(transfer_idx),
                transport_endpoint_idx: ActiveValue::Set(transport_endpoint_idx),
                used: ActiveValue::Set(transport_endpoint.used),
                ..Default::default()
            })?;

        Ok(())
    }

    pub(crate) fn check_details(&self, details: String) -> Result<Details, Error> {
        if details.is_empty() {
            return Err(Error::InvalidDetails {
                details: s!("ident must contain at least one character"),
            });
        }
        Details::from_str(&details).map_err(|e| Error::InvalidDetails {
            details: e.to_string(),
        })
    }

    fn _check_name(&self, name: String) -> Result<Name, Error> {
        Name::try_from(name).map_err(|e| Error::InvalidName {
            details: e.to_string(),
        })
    }

    fn _check_precision(&self, precision: u8) -> Result<Precision, Error> {
        Precision::try_from(precision).map_err(|_| Error::InvalidPrecision {
            details: s!("precision is too high"),
        })
    }

    fn _check_reject_list_url(&self, opid_reject_url: String) -> Result<RejectListUrl, Error> {
        RejectListUrl::try_from(opid_reject_url).map_err(|e| Error::InvalidRejectListUrl {
            details: e.to_string(),
        })
    }

    fn _check_ticker(&self, ticker: String) -> Result<Ticker, Error> {
        if ticker.to_ascii_uppercase() != *ticker {
            return Err(Error::InvalidTicker {
                details: s!("ticker needs to be all uppercase"),
            });
        }
        Ticker::try_from(ticker).map_err(|e| Error::InvalidTicker {
            details: e.to_string(),
        })
    }

    fn _get_total_issue_amount(&self, amounts: &[u64], allow_empty: bool) -> Result<u64, Error> {
        if amounts.is_empty() && !allow_empty {
            return Err(Error::NoIssuanceAmounts);
        }
        amounts.iter().try_fold(0u64, |acc, x| {
            if *x == 0 {
                return Err(Error::InvalidAmountZero);
            }
            acc.checked_add(*x).ok_or(Error::TooHighIssuanceAmounts)
        })
    }

    pub(crate) fn get_total_inflation_amount(
        &self,
        inflation_amounts: &[u64],
        issued_supply: u64,
    ) -> Result<u64, Error> {
        let total_inflation = inflation_amounts.iter().try_fold(0u64, |acc, x| {
            if *x == 0 {
                return Err(Error::InvalidAmountZero);
            }
            acc.checked_add(*x).ok_or(Error::TooHighInflationAmounts)
        })?;

        if inflation_amounts.is_empty() {
            return Ok(0);
        }

        issued_supply
            .checked_add(total_inflation)
            .ok_or(Error::TooHighInflationAmounts)?;

        Ok(total_inflation)
    }

    pub(crate) fn new_asset_terms(
        &self,
        text: RicardianContract,
        media: Option<Attachment>,
    ) -> ContractTerms {
        ContractTerms { text, media }
    }

    pub(crate) fn get_blind_seal(&self, outpoint: impl Into<OutPoint>) -> BlindSeal<RgbTxid> {
        let outpoint = outpoint.into();
        BlindSeal::new_random(outpoint.txid, outpoint.vout)
    }

    pub(crate) fn get_builder_seal(
        &self,
        outpoint: impl Into<OutPoint>,
    ) -> BuilderSeal<BlindSeal<RgbTxid>> {
        BuilderSeal::from(self.get_blind_seal(outpoint))
    }

    fn _get_issue_consignment_path(&self, asset_id: &str) -> PathBuf {
        self.wallet_dir.join(ASSETS_DIR).join(asset_id)
    }

    pub(crate) fn get_or_insert_media(&self, digest: String, mime: String) -> Result<i32, Error> {
        Ok(match self.database.get_media_by_digest(digest.clone())? {
            Some(media) => media.idx,
            None => self.database.set_media(DbMediaActMod {
                digest: ActiveValue::Set(digest),
                mime: ActiveValue::Set(mime),
                ..Default::default()
            })?,
        })
    }

    pub(crate) fn add_asset_to_db(
        &self,
        asset_id: String,
        schema: &AssetSchema,
        added_at: Option<i64>,
        timestamp: i64,
        asset_data: LocalAssetData,
    ) -> Result<DbAsset, Error> {
        let added_at = added_at.unwrap_or_else(|| now().unix_timestamp());
        let mut db_asset = DbAssetActMod {
            idx: ActiveValue::NotSet,
            media_idx: ActiveValue::Set(asset_data.media_idx),
            id: ActiveValue::Set(asset_id),
            schema: ActiveValue::Set(*schema),
            added_at: ActiveValue::Set(added_at),
            details: ActiveValue::Set(asset_data.details),
            initial_supply: ActiveValue::Set(asset_data.initial_supply.to_string()),
            max_supply: ActiveValue::Set(asset_data.max_supply.map(|s| s.to_string())),
            known_circulating_supply: ActiveValue::Set(
                asset_data.known_circulating_supply.map(|s| s.to_string()),
            ),
            name: ActiveValue::Set(asset_data.name),
            precision: ActiveValue::Set(asset_data.precision),
            ticker: ActiveValue::Set(asset_data.ticker),
            timestamp: ActiveValue::Set(timestamp),
            reject_list_url: ActiveValue::Set(asset_data.reject_list_url),
        };
        let idx = self.database.set_asset(db_asset.clone())?;
        db_asset.idx = ActiveValue::Set(idx);
        Ok(db_asset.try_into_model().unwrap())
    }

    fn _get_btc_balance(&self, keychain: KeychainKind) -> Result<Balance, Error> {
        let chain = self.bdk_wallet.local_chain();
        let chain_tip = self.bdk_wallet.latest_checkpoint().block_id();
        let outpoints = self.filter_unspents(keychain).map(|lo| ((), lo.outpoint));
        let balance = self.bdk_wallet.as_ref().balance(
            chain,
            chain_tip,
            CanonicalizationParams::default(),
            outpoints,
            |_, _| false,
        );

        let future = balance.total();
        Ok(Balance {
            settled: balance.confirmed.to_sat(),
            future: future.to_sat(),
            spendable: future.to_sat() - balance.immature.to_sat(),
        })
    }

    /// Issue a new RGB NIA asset with the provided `ticker`, `name`, `precision` and `amounts`,
    /// then return it.
    ///
    /// At least 1 amount needs to be provided and the sum of all amounts cannot exceed the maximum
    /// `u64` value.
    ///
    /// If `amounts` contains more than 1 element, each one will be issued as a separate allocation
    /// for the same asset (on a separate UTXO that needs to be already available).
    pub fn issue_asset_nia(
        &self,
        ticker: String,
        name: String,
        precision: u8,
        amounts: Vec<u64>,
    ) -> Result<AssetNIA, Error> {
        info!(
            self.logger,
            "Issuing NIA asset with ticker '{}' name '{}' precision '{}' amounts '{:?}'...",
            ticker,
            name,
            precision,
            amounts
        );

        let asset_schema = &AssetSchema::Nia;

        self.check_schema_support(asset_schema)?;

        let settled = self._get_total_issue_amount(&amounts, false)?;

        let db_data = self.database.get_db_data(false)?;

        let mut unspents: Vec<LocalUnspent> = self.database.get_rgb_allocations(
            self.database.get_unspent_txos(db_data.txos.clone())?,
            None,
            None,
            None,
            None,
        )?;
        unspents.retain(|u| {
            !(u.rgb_allocations
                .iter()
                .any(|a| !a.incoming && a.status.waiting_counterparty()))
        });

        let created_at = now().unix_timestamp();
        let text = RicardianContract::default();
        let terms = self.new_asset_terms(text, None);
        let details = None;
        let spec = AssetSpec {
            ticker: self._check_ticker(ticker.clone())?,
            name: self._check_name(name.clone())?,
            details,
            precision: self._check_precision(precision)?,
        };

        let mut runtime = self.rgb_runtime()?;
        let mut builder = ContractBuilder::with(
            Identity::default(),
            NonInflatableAsset::schema(),
            NonInflatableAsset::types(),
            NonInflatableAsset::scripts(),
            self.chain_net(),
        )
        .add_global_state("spec", spec.clone())
        .expect("invalid spec")
        .add_global_state("terms", terms)
        .expect("invalid terms")
        .add_global_state(RGB_GLOBAL_ISSUED_SUPPLY, Amount::from(settled))
        .expect("invalid issuedSupply");

        let mut issue_utxos: HashMap<DbTxo, u64> = HashMap::new();
        let mut exclude_outpoints = vec![];
        for amount in &amounts {
            let utxo = self.get_utxo(&exclude_outpoints, Some(&unspents), false, None)?;
            exclude_outpoints.push(utxo.outpoint());
            issue_utxos.insert(utxo.clone(), *amount);

            builder = builder
                .add_fungible_state(RGB_STATE_ASSET_OWNER, self.get_builder_seal(utxo), *amount)
                .expect("invalid global state data");
        }
        debug!(self.logger, "Issuing on UTXOs: {issue_utxos:?}");

        let validated_contract = builder.issue_contract().expect("failure issuing contract");
        let asset_id = validated_contract.contract_id().to_string();
        runtime
            .import_contract(validated_contract.clone(), &DumbResolver)
            .expect("failure importing issued contract");
        // Drop runtime so Stock returns to RefCell before auto-save
        drop(runtime);

        let asset_data = LocalAssetData {
            name,
            precision,
            ticker: Some(ticker),
            details: spec.details().map(|d| d.to_string()),
            media_idx: None,
            initial_supply: settled,
            max_supply: None,
            known_circulating_supply: None,
            reject_list_url: None,
        };
        let asset = self.add_asset_to_db(
            asset_id.clone(),
            asset_schema,
            Some(created_at),
            created_at,
            asset_data,
        )?;
        let batch_transfer = DbBatchTransferActMod {
            status: ActiveValue::Set(TransferStatus::Settled),
            expiration: ActiveValue::Set(None),
            created_at: ActiveValue::Set(created_at),
            min_confirmations: ActiveValue::Set(0),
            ..Default::default()
        };
        let batch_transfer_idx = self.database.set_batch_transfer(batch_transfer)?;
        let asset_transfer = DbAssetTransferActMod {
            user_driven: ActiveValue::Set(true),
            batch_transfer_idx: ActiveValue::Set(batch_transfer_idx),
            asset_id: ActiveValue::Set(Some(asset_id)),
            ..Default::default()
        };
        let asset_transfer_idx = self.database.set_asset_transfer(asset_transfer)?;
        let transfer = DbTransferActMod {
            asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
            incoming: ActiveValue::Set(true),
            ..Default::default()
        };
        self.database.set_transfer(transfer)?;
        for (utxo, amount) in issue_utxos {
            let db_coloring = DbColoringActMod {
                txo_idx: ActiveValue::Set(utxo.idx),
                asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                r#type: ActiveValue::Set(ColoringType::Issue),
                assignment: ActiveValue::Set(Assignment::Fungible(amount)),
                ..Default::default()
            };
            self.database.set_coloring(db_coloring)?;
        }

        let asset = AssetNIA::get_asset_details(self, &asset, None, None, None, None, None, None)?;

        self.update_backup_info(false)?;
        self.trigger_auto_backup();

        info!(self.logger, "Issue asset NIA completed");
        Ok(asset)
    }

    /// Issue a new RGB IFA asset with the provided `ticker`, `name`, `precision`, `amounts`,
    /// and `inflation_amounts`, then return it.
    ///
    /// At least 1 amount needs to be provided and the sum of all amounts cannot exceed the maximum
    /// `u64` value.
    ///
    /// If `amounts` contains more than 1 element, each one will be issued as a separate allocation
    /// for the same asset (on a separate UTXO that needs to be already available).
    ///
    /// The `inflation_amounts` can be empty. If provided the sum of its elements plus the sum of
    /// `amounts` cannot exceed the maximum `u64` value.
    pub fn issue_asset_ifa(
        &self,
        ticker: String,
        name: String,
        precision: u8,
        amounts: Vec<u64>,
        inflation_amounts: Vec<u64>,
        reject_list_url: Option<String>,
    ) -> Result<AssetIFA, Error> {
        info!(
            self.logger,
            "Issuing IFA asset with ticker '{}' name '{}' precision '{}' amounts '{:?}' inflation amounts {:?}...",
            ticker,
            name,
            precision,
            amounts,
            inflation_amounts,
        );

        let asset_schema = &AssetSchema::Ifa;

        self.check_schema_support(asset_schema)?;

        let settled = self._get_total_issue_amount(&amounts, true)?;
        let inflation_amt = self.get_total_inflation_amount(&inflation_amounts, settled)?;
        if settled == 0 && inflation_amt == 0 {
            return Err(Error::NoIssuanceAmounts);
        }
        let max_supply = settled + inflation_amt;

        let db_data = self.database.get_db_data(false)?;

        let mut unspents: Vec<LocalUnspent> = self.database.get_rgb_allocations(
            self.database.get_unspent_txos(db_data.txos.clone())?,
            None,
            None,
            None,
            None,
        )?;
        unspents.retain(|u| {
            !(u.rgb_allocations
                .iter()
                .any(|a| !a.incoming && a.status.waiting_counterparty()))
        });

        let created_at = now().unix_timestamp();
        let text = RicardianContract::default();
        let terms = self.new_asset_terms(text, None);
        let details = None;
        let spec = AssetSpec {
            ticker: self._check_ticker(ticker.clone())?,
            name: self._check_name(name.clone())?,
            details,
            precision: self._check_precision(precision)?,
        };

        let mut runtime = self.rgb_runtime()?;
        let mut builder = ContractBuilder::with(
            Identity::default(),
            InflatableFungibleAsset::schema(),
            InflatableFungibleAsset::types(),
            InflatableFungibleAsset::scripts(),
            self.chain_net(),
        )
        .add_global_state("spec", spec.clone())
        .expect("invalid spec")
        .add_global_state("terms", terms)
        .expect("invalid terms")
        .add_global_state(RGB_GLOBAL_ISSUED_SUPPLY, Amount::from(settled))
        .expect("invalid issuedSupply")
        .add_global_state("maxSupply", Amount::from(max_supply))
        .expect("invalid maxSupply");
        if let Some(reject_list_url) = &reject_list_url {
            builder = builder
                .add_global_state(
                    RGB_GLOBAL_REJECT_LIST_URL,
                    self._check_reject_list_url(reject_list_url.clone())?,
                )
                .expect("invalid rejectListUrl");
        }

        let mut issue_utxos: HashMap<DbTxo, u64> = HashMap::new();
        let mut exclude_outpoints: Vec<Outpoint> = vec![];
        for amount in &amounts {
            let utxo = self.get_utxo(&exclude_outpoints, Some(&unspents), false, None)?;
            exclude_outpoints.push(utxo.outpoint());
            issue_utxos.insert(utxo.clone(), *amount);

            builder = builder
                .add_fungible_state(RGB_STATE_ASSET_OWNER, self.get_builder_seal(utxo), *amount)
                .expect("invalid global state data");
        }
        debug!(self.logger, "Issuing on UTXOs: {issue_utxos:?}");

        let mut inflation_utxos: HashMap<DbTxo, u64> = HashMap::new();
        for amount in &inflation_amounts {
            let utxo = self.get_utxo(&exclude_outpoints, Some(&unspents), false, Some(0))?;
            exclude_outpoints.push(utxo.outpoint());
            inflation_utxos.insert(utxo.clone(), *amount);

            builder = builder
                .add_fungible_state(
                    RGB_STATE_INFLATION_ALLOWANCE,
                    self.get_builder_seal(utxo),
                    *amount,
                )
                .expect("invalid global state data");
        }
        debug!(
            self.logger,
            "Assigning inflation rights: {inflation_utxos:?}"
        );

        let validated_contract = builder.issue_contract().expect("failure issuing contract");
        let asset_id = validated_contract.contract_id().to_string();
        runtime
            .import_contract(validated_contract.clone(), &DumbResolver)
            .expect("failure importing issued contract");
        // Drop runtime so Stock returns to RefCell before auto-save
        drop(runtime);

        let asset_data = LocalAssetData {
            name,
            precision,
            ticker: Some(ticker),
            details: spec.details().map(|d| d.to_string()),
            media_idx: None,
            initial_supply: settled,
            max_supply: Some(max_supply),
            known_circulating_supply: Some(settled),
            reject_list_url,
        };
        let asset = self.add_asset_to_db(
            asset_id.clone(),
            asset_schema,
            Some(created_at),
            created_at,
            asset_data,
        )?;
        let batch_transfer = DbBatchTransferActMod {
            status: ActiveValue::Set(TransferStatus::Settled),
            expiration: ActiveValue::Set(None),
            created_at: ActiveValue::Set(created_at),
            min_confirmations: ActiveValue::Set(0),
            ..Default::default()
        };
        let batch_transfer_idx = self.database.set_batch_transfer(batch_transfer)?;
        let asset_transfer = DbAssetTransferActMod {
            user_driven: ActiveValue::Set(true),
            batch_transfer_idx: ActiveValue::Set(batch_transfer_idx),
            asset_id: ActiveValue::Set(Some(asset_id)),
            ..Default::default()
        };
        let asset_transfer_idx = self.database.set_asset_transfer(asset_transfer)?;
        let transfer = DbTransferActMod {
            asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
            incoming: ActiveValue::Set(true),
            ..Default::default()
        };
        self.database.set_transfer(transfer)?;
        for (utxo, amount) in issue_utxos {
            let db_coloring = DbColoringActMod {
                txo_idx: ActiveValue::Set(utxo.idx),
                asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                r#type: ActiveValue::Set(ColoringType::Issue),
                assignment: ActiveValue::Set(Assignment::Fungible(amount)),
                ..Default::default()
            };
            self.database.set_coloring(db_coloring)?;
        }
        for (utxo, amount) in inflation_utxos {
            let db_coloring = DbColoringActMod {
                txo_idx: ActiveValue::Set(utxo.idx),
                asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                r#type: ActiveValue::Set(ColoringType::Issue),
                assignment: ActiveValue::Set(Assignment::InflationRight(amount)),
                ..Default::default()
            };
            self.database.set_coloring(db_coloring)?;
        }

        let asset = AssetIFA::get_asset_details(self, &asset, None, None, None, None, None, None)?;

        self.update_backup_info(false)?;
        self.trigger_auto_backup();

        info!(self.logger, "Issue asset IFA completed");
        Ok(asset)
    }

    /// Finalize a PSBT, optionally providing BDK sign options to tweak the behavior of the
    /// finalizer.
    pub fn finalize_psbt(
        &self,
        signed_psbt: String,
        sign_options: Option<SignOptions>,
    ) -> Result<String, Error> {
        info!(self.logger, "Finalizing PSBT...");
        let mut psbt = Psbt::from_str(&signed_psbt)?;
        let sign_options = sign_options.unwrap_or_default();
        if !self
            .bdk_wallet
            .finalize_psbt(&mut psbt, sign_options)
            .map_err(InternalError::from)?
        {
            return Err(Error::CannotFinalizePsbt);
        }
        info!(self.logger, "Finalize PSBT completed");
        Ok(psbt.to_string())
    }

    fn _sign_psbt(&self, psbt: &mut Psbt, sign_options: Option<SignOptions>) -> Result<(), Error> {
        let sign_options = sign_options.unwrap_or_default();
        self.bdk_wallet
            .sign(psbt, sign_options)
            .map_err(InternalError::from)?;
        Ok(())
    }

    /// Sign a PSBT, optionally providing BDK sign options.
    pub fn sign_psbt(
        &self,
        unsigned_psbt: String,
        sign_options: Option<SignOptions>,
    ) -> Result<String, Error> {
        info!(self.logger, "Signing PSBT...");
        let mut psbt = Psbt::from_str(&unsigned_psbt)?;
        self._sign_psbt(&mut psbt, sign_options)?;
        info!(self.logger, "Sign PSBT completed");
        Ok(psbt.to_string())
    }

    fn _delete_batch_transfer(
        &self,
        batch_transfer: &DbBatchTransfer,
        asset_transfers: &Vec<DbAssetTransfer>,
        colorings: &[DbColoring],
        txos: &[DbTxo],
    ) -> Result<(), Error> {
        let mut txos_to_delete = HashSet::new();
        for asset_transfer in asset_transfers {
            self.database.del_coloring(asset_transfer.idx)?;
            colorings
                .iter()
                .filter(|c| c.asset_transfer_idx == asset_transfer.idx)
                .for_each(|c| {
                    if let Some(txo) = txos.iter().find(|t| !t.exists && t.idx == c.txo_idx) {
                        txos_to_delete.insert(txo.idx);
                    }
                });
        }
        for txo in txos_to_delete {
            self.database.del_txo(txo)?;
        }
        Ok(self.database.del_batch_transfer(batch_transfer)?)
    }

    /// Delete eligible transfers from the database and return true if any transfer has been
    /// deleted.
    ///
    /// An optional `batch_transfer_idx` can be provided to operate on a single batch transfer.
    ///
    /// If a `batch_transfer_idx` is provided and `no_asset_only` is true, transfers with an
    /// associated asset ID will not be deleted and instead return an error.
    ///
    /// If no `batch_transfer_idx` is provided, all failed transfers will be deleted, and if
    /// `no_asset_only` is true transfers with an associated asset ID will be skipped.
    ///
    /// Eligible transfers are the ones in status [`TransferStatus::Failed`].
    pub fn delete_transfers(
        &self,
        batch_transfer_idx: Option<i32>,
        no_asset_only: bool,
    ) -> Result<bool, Error> {
        info!(
            self.logger,
            "Deleting batch transfer with idx {:?}...", batch_transfer_idx
        );

        let db_data = self.database.get_db_data(false)?;
        let mut transfers_changed = false;

        if let Some(batch_transfer_idx) = batch_transfer_idx {
            let batch_transfer = &self
                .database
                .get_batch_transfer_or_fail(batch_transfer_idx, &db_data.batch_transfers)?;

            if !batch_transfer.failed() {
                return Err(Error::CannotDeleteBatchTransfer);
            }

            let asset_transfers = batch_transfer.get_asset_transfers(&db_data.asset_transfers)?;

            if no_asset_only {
                let connected_assets = asset_transfers.iter().any(|t| t.asset_id.is_some());
                if connected_assets {
                    return Err(Error::CannotDeleteBatchTransfer);
                }
            }

            transfers_changed = true;
            self._delete_batch_transfer(
                batch_transfer,
                &asset_transfers,
                &db_data.colorings,
                &db_data.txos,
            )?
        } else {
            // delete all failed transfers
            let mut batch_transfers: Vec<DbBatchTransfer> = db_data
                .batch_transfers
                .clone()
                .into_iter()
                .filter(|t| t.failed())
                .collect();
            for batch_transfer in batch_transfers.iter_mut() {
                let asset_transfers =
                    batch_transfer.get_asset_transfers(&db_data.asset_transfers)?;
                if no_asset_only {
                    let connected_assets = asset_transfers.iter().any(|t| t.asset_id.is_some());
                    if connected_assets {
                        continue;
                    }
                }
                transfers_changed = true;
                self._delete_batch_transfer(
                    batch_transfer,
                    &asset_transfers,
                    &db_data.colorings,
                    &db_data.txos,
                )?
            }
        }

        if transfers_changed {
            self.update_backup_info(false)?;
            self.trigger_auto_backup();
        }

        info!(self.logger, "Delete transfer completed");
        Ok(transfers_changed)
    }

    /// Return the [`Metadata`] of the RGB asset with the given ID.
    pub fn get_asset_metadata(&self, asset_id: String) -> Result<Metadata, Error> {
        info!(self.logger, "Getting metadata for asset '{}'...", asset_id);
        let asset = self.database.check_asset_exists(asset_id)?;

        let initial_supply = asset.initial_supply.parse::<u64>().unwrap();
        let max_supply = if let Some(max_supply) = asset.max_supply {
            max_supply.parse::<u64>().unwrap()
        } else {
            initial_supply
        };
        let known_circulating_supply =
            if let Some(known_circulating_supply) = asset.known_circulating_supply {
                known_circulating_supply.parse::<u64>().unwrap()
            } else {
                initial_supply
            };

        info!(self.logger, "Get asset metadata completed");
        Ok(Metadata {
            asset_schema: asset.schema,
            initial_supply,
            max_supply,
            known_circulating_supply,
            timestamp: asset.timestamp,
            name: asset.name,
            precision: asset.precision,
            ticker: asset.ticker,
            details: asset.details,
            reject_list_url: asset.reject_list_url,
        })
    }

    /// Return the [`Balance`] for the RGB asset with the given ID.
    pub fn get_asset_balance(&self, asset_id: String) -> Result<Balance, Error> {
        info!(self.logger, "Getting balance for asset '{}'...", asset_id);
        self.database.check_asset_exists(asset_id.clone())?;
        let balance = self
            .database
            .get_asset_balance(asset_id, None, None, None, None, None);
        info!(self.logger, "Get asset balance completed");
        balance
    }

    /// Return the [`BtcBalance`] of the internal Bitcoin wallets.
    pub fn get_btc_balance(
        &mut self,
        online: Option<Online>,
        skip_sync: bool,
    ) -> Result<BtcBalance, Error> {
        info!(self.logger, "Getting BTC balance...");

        self.sync_if_requested(online, skip_sync)?;

        let vanilla = self._get_btc_balance(KeychainKind::Internal)?;
        let colored = self._get_btc_balance(KeychainKind::External)?;

        let balance = BtcBalance { vanilla, colored };

        info!(self.logger, "Get BTC balance completed");
        Ok(balance)
    }

    /// List the known RGB assets.
    ///
    /// Providing an empty `filter_asset_schemas` will list assets for all schemas, otherwise only
    /// assets for the provided schemas will be listed.
    ///
    /// The returned `Assets` will have fields set to `None` for schemas that have not been
    /// requested.
    pub fn list_assets(&self, mut filter_asset_schemas: Vec<AssetSchema>) -> Result<Assets, Error> {
        info!(self.logger, "Listing assets...");
        if filter_asset_schemas.is_empty() {
            filter_asset_schemas = AssetSchema::VALUES.to_vec()
        }

        let batch_transfers = Some(self.database.iter_batch_transfers()?);
        let colorings = Some(self.database.iter_colorings()?);
        let txos = Some(self.database.iter_txos()?);
        let asset_transfers = Some(self.database.iter_asset_transfers()?);
        let transfers = Some(self.database.iter_transfers()?);
        let medias = Some(self.database.iter_media()?);

        let assets = self.database.iter_assets()?;
        let mut nia = None;
        let mut ifa = None;
        for schema in filter_asset_schemas {
            match schema {
                AssetSchema::Nia => {
                    nia = Some(
                        assets
                            .iter()
                            .filter(|a| a.schema == schema)
                            .map(|a| {
                                AssetNIA::get_asset_details(
                                    self,
                                    a,
                                    transfers.clone(),
                                    asset_transfers.clone(),
                                    batch_transfers.clone(),
                                    colorings.clone(),
                                    txos.clone(),
                                    medias.clone(),
                                )
                            })
                            .collect::<Result<Vec<AssetNIA>, Error>>()?,
                    );
                }
                AssetSchema::Ifa => {
                    ifa = Some(
                        assets
                            .iter()
                            .filter(|a| a.schema == schema)
                            .map(|a| {
                                AssetIFA::get_asset_details(
                                    self,
                                    a,
                                    transfers.clone(),
                                    asset_transfers.clone(),
                                    batch_transfers.clone(),
                                    colorings.clone(),
                                    txos.clone(),
                                    medias.clone(),
                                )
                            })
                            .collect::<Result<Vec<AssetIFA>, Error>>()?,
                    );
                }
            }
        }

        info!(self.logger, "List assets completed");
        Ok(Assets { nia, ifa })
    }

    pub(crate) fn sync_if_requested(
        &mut self,
        #[cfg_attr(not(feature = "esplora"), allow(unused_variables))] online: Option<Online>,
        skip_sync: bool,
    ) -> Result<(), Error> {
        if !skip_sync {
            #[cfg(not(feature = "esplora"))]
            return Err(Error::Offline);
            #[cfg(feature = "esplora")]
            {
                if let Some(online) = online {
                    self.check_online(online)?;
                } else {
                    return Err(Error::OnlineNeeded);
                }
                return Err(Error::Offline);
            }
        }
        Ok(())
    }

    /// List the Bitcoin [`Transaction`]s known to the wallet.
    pub fn list_transactions(
        &mut self,
        online: Option<Online>,
        skip_sync: bool,
    ) -> Result<Vec<Transaction>, Error> {
        info!(self.logger, "Listing transactions...");

        self.sync_if_requested(online, skip_sync)?;

        let mut create_utxos_txids = vec![];
        let mut drain_txids = vec![];
        let wallet_transactions = self.database.iter_wallet_transactions()?;
        for tx in wallet_transactions {
            match tx.r#type {
                WalletTransactionType::CreateUtxos => create_utxos_txids.push(tx.txid),
                WalletTransactionType::Drain => drain_txids.push(tx.txid),
            }
        }
        let rgb_send_txids: Vec<String> = self
            .database
            .iter_batch_transfers()?
            .into_iter()
            .filter_map(|t| t.txid)
            .collect();
        let transactions = self
            .bdk_wallet
            .transactions()
            .map(|t| {
                let txid = t.tx_node.txid.to_string();
                let transaction_type = if drain_txids.contains(&txid) {
                    TransactionType::Drain
                } else if create_utxos_txids.contains(&txid) {
                    TransactionType::CreateUtxos
                } else if rgb_send_txids.contains(&txid) {
                    TransactionType::RgbSend
                } else {
                    TransactionType::User
                };
                let confirmation_time = match t.chain_position {
                    ChainPosition::Confirmed { anchor, .. } => Some(BlockTime {
                        height: anchor.block_id.height,
                        timestamp: anchor.confirmation_time,
                    }),
                    _ => None,
                };
                let (sent, received) = self.bdk_wallet.sent_and_received(&t.tx_node);
                let fee = self.bdk_wallet.calculate_fee(&t.tx_node).unwrap();
                Transaction {
                    transaction_type,
                    txid,
                    received: received.to_sat(),
                    sent: sent.to_sat(),
                    fee: fee.to_sat(),
                    confirmation_time,
                }
            })
            .collect();
        info!(self.logger, "List transactions completed");
        Ok(transactions)
    }

    pub(crate) fn normalize_recipient_id(&self, recipient_id: &str) -> String {
        recipient_id.replace(":", "_")
    }

    pub(crate) fn get_receive_consignment_path(&self, recipient_id: &str) -> PathBuf {
        self.get_transfers_dir()
            .join(self.normalize_recipient_id(recipient_id))
            .join(CONSIGNMENT_RCV_FILE)
    }

    pub(crate) fn get_transfer_data(
        &self,
        transfer: &DbTransfer,
        asset_transfer: &DbAssetTransfer,
        batch_transfer: &DbBatchTransfer,
        txos: &[DbTxo],
        colorings: &[DbColoring],
    ) -> Result<TransferData, Error> {
        let filtered_coloring = colorings
            .iter()
            .filter(|&c| c.asset_transfer_idx == asset_transfer.idx)
            .cloned();

        let assignments = filtered_coloring
            .clone()
            .filter(|c| c.r#type != ColoringType::Input)
            .map(|c| c.assignment)
            .collect();

        let kind = if transfer.incoming {
            if filtered_coloring.clone().count() > 0
                && filtered_coloring
                    .clone()
                    .all(|c| c.r#type == ColoringType::Issue)
            {
                TransferKind::Issuance
            } else {
                match transfer.recipient_type.as_ref().unwrap() {
                    RecipientTypeFull::Blind { .. } => TransferKind::ReceiveBlind,
                    RecipientTypeFull::Witness { .. } => TransferKind::ReceiveWitness,
                }
            }
        } else if filtered_coloring.clone().count() > 0
            && filtered_coloring
                .clone()
                .any(|c| c.r#type == ColoringType::Issue)
        {
            // inflation transfer is outgoing and connected to issue colorings
            TransferKind::Inflation
        } else {
            TransferKind::Send
        };

        let txo_ids: Vec<i32> = filtered_coloring.clone().map(|c| c.txo_idx).collect();
        let transfer_txos: Vec<DbTxo> = txos
            .iter()
            .filter(|&t| txo_ids.contains(&t.idx))
            .cloned()
            .collect();
        let receive_utxo = match &transfer.recipient_type {
            Some(RecipientTypeFull::Blind { unblinded_utxo }) => Some(unblinded_utxo.clone()),
            Some(RecipientTypeFull::Witness { vout }) => {
                let received_txo_idx: Vec<i32> = filtered_coloring
                    .clone()
                    // issue coloring from inflation is considered as received
                    .filter(|c| [ColoringType::Receive, ColoringType::Issue].contains(&c.r#type))
                    .map(|c| c.txo_idx)
                    .collect();
                transfer_txos
                    .clone()
                    .into_iter()
                    .filter(|t| received_txo_idx.contains(&t.idx) && t.vout == vout.unwrap())
                    .map(|t| t.outpoint())
                    .collect::<Vec<Outpoint>>()
                    .first()
                    .cloned()
            }
            _ => None,
        };
        let change_utxo = match kind {
            TransferKind::ReceiveBlind | TransferKind::ReceiveWitness => None,
            TransferKind::Send | TransferKind::Inflation => {
                let change_txo_idx: Vec<i32> = filtered_coloring
                    .filter(|c| c.r#type == ColoringType::Change)
                    .map(|c| c.txo_idx)
                    .collect();
                transfer_txos
                    .into_iter()
                    .filter(|t| change_txo_idx.contains(&t.idx))
                    .map(|t| t.outpoint())
                    .collect::<Vec<Outpoint>>()
                    .first()
                    .cloned()
            }
            TransferKind::Issuance => None,
        };

        let consignment_path = match (&kind, batch_transfer.status) {
            (TransferKind::Send | TransferKind::Inflation, _) => {
                Some(self.get_send_consignment_path(
                    &asset_transfer.asset_id.clone().unwrap(),
                    &batch_transfer.txid.clone().unwrap(),
                ))
            }
            (
                TransferKind::ReceiveBlind | TransferKind::ReceiveWitness,
                TransferStatus::WaitingCounterparty,
            ) => None,
            (TransferKind::ReceiveBlind | TransferKind::ReceiveWitness, _) => {
                Some(self.get_receive_consignment_path(&transfer.recipient_id.clone().unwrap()))
            }
            (TransferKind::Issuance, _) => {
                Some(self._get_issue_consignment_path(&asset_transfer.asset_id.clone().unwrap()))
            }
        }
        .map(|p| p.to_string_lossy().to_string());

        Ok(TransferData {
            kind,
            status: batch_transfer.status,
            batch_transfer_idx: batch_transfer.idx,
            assignments,
            txid: batch_transfer.txid.clone(),
            receive_utxo,
            change_utxo,
            created_at: batch_transfer.created_at,
            updated_at: batch_transfer.updated_at,
            expiration: batch_transfer.expiration,
            consignment_path,
        })
    }

    /// List the RGB [`Transfer`]s known to the wallet.
    ///
    /// `filter` selects transfers by asset. When a `txid` is provided, restrict the result to the
    /// transfers committed by the on-chain transaction with that ID; an unknown `txid` yields an
    /// empty list.
    pub fn list_transfers(
        &self,
        filter: AssetFilter,
        txid: Option<String>,
    ) -> Result<Vec<Transfer>, Error> {
        info!(
            self.logger,
            "Listing transfers for filter '{:?}' and txid '{:?}'...", filter, txid
        );
        if let AssetFilter::Id(asset_id) = &filter {
            self.database.check_asset_exists(asset_id.clone())?;
        }
        let db_data = self.database.get_db_data(false)?;
        let batch_transfer_idx = match &txid {
            Some(txid) => match db_data
                .batch_transfers
                .iter()
                .find(|b| b.txid.as_deref() == Some(txid))
            {
                Some(batch_transfer) => Some(batch_transfer.idx),
                None => return Ok(vec![]),
            },
            None => None,
        };
        let asset_transfer_ids: Vec<i32> = db_data
            .asset_transfers
            .iter()
            .filter(|t| match &filter {
                AssetFilter::Any => true,
                AssetFilter::NoAsset => t.asset_id.is_none(),
                AssetFilter::Id(asset_id) => t.asset_id.as_ref() == Some(asset_id),
            })
            .filter(|t| batch_transfer_idx.is_none_or(|idx| t.batch_transfer_idx == idx))
            .filter(|t| t.user_driven)
            .map(|t| t.idx)
            .collect();
        let transfers: Vec<Transfer> = db_data
            .transfers
            .into_iter()
            .filter(|t| asset_transfer_ids.contains(&t.asset_transfer_idx))
            .map(|t| {
                let (asset_transfer, batch_transfer) =
                    t.related_transfers(&db_data.asset_transfers, &db_data.batch_transfers)?;
                let tte_data = self.database.get_transfer_transport_endpoints_data(t.idx)?;
                Ok(Transfer::from_db_transfer(
                    &t,
                    self.get_transfer_data(
                        &t,
                        &asset_transfer,
                        &batch_transfer,
                        &db_data.txos,
                        &db_data.colorings,
                    )?,
                    tte_data
                        .iter()
                        .map(|(tte, ce)| {
                            TransferTransportEndpoint::from_db_transfer_transport_endpoint(tte, ce)
                        })
                        .collect(),
                ))
            })
            .collect::<Result<Vec<Transfer>, Error>>()?;

        info!(self.logger, "List transfers completed");
        Ok(transfers)
    }

    /// List the [`Unspent`]s known to the wallet.
    ///
    /// If `settled` is true only show settled RGB allocations, if false also show pending RGB
    /// allocations.
    pub fn list_unspents(
        &mut self,
        online: Option<Online>,
        settled_only: bool,
        skip_sync: bool,
    ) -> Result<Vec<Unspent>, Error> {
        info!(self.logger, "Listing unspents...");

        self.sync_if_requested(online, skip_sync)?;

        let db_data = self.database.get_db_data(false)?;

        let mut allocation_txos = self.database.get_unspent_txos(db_data.txos.clone())?;
        let spent_txos_ids: Vec<i32> = db_data
            .txos
            .clone()
            .into_iter()
            .filter(|t| t.spent)
            .map(|u| u.idx)
            .collect();
        let waiting_confs_batch_transfer_ids: Vec<i32> = db_data
            .batch_transfers
            .clone()
            .into_iter()
            .filter(|t| t.waiting_confirmations())
            .map(|t| t.idx)
            .collect();
        let waiting_confs_transfer_ids: Vec<i32> = db_data
            .asset_transfers
            .clone()
            .into_iter()
            .filter(|t| waiting_confs_batch_transfer_ids.contains(&t.batch_transfer_idx))
            .map(|t| t.idx)
            .collect();
        let almost_spent_txos_ids: Vec<i32> = db_data
            .colorings
            .clone()
            .into_iter()
            .filter(|c| {
                waiting_confs_transfer_ids.contains(&c.asset_transfer_idx)
                    && spent_txos_ids.contains(&c.txo_idx)
            })
            .map(|c| c.txo_idx)
            .collect();
        let mut spent_txos = db_data
            .txos
            .into_iter()
            .filter(|t| almost_spent_txos_ids.contains(&t.idx))
            .collect();
        allocation_txos.append(&mut spent_txos);

        let mut txos_allocations = self.database.get_rgb_allocations(
            allocation_txos,
            Some(db_data.colorings),
            Some(db_data.batch_transfers),
            Some(db_data.asset_transfers),
            Some(db_data.transfers),
        )?;

        txos_allocations
            .iter_mut()
            .for_each(|t| t.rgb_allocations.retain(|a| a.settled() || a.future()));

        txos_allocations.retain(|t| !(t.rgb_allocations.is_empty() && t.utxo.spent));

        let mut unspents: Vec<Unspent> = txos_allocations.into_iter().map(Unspent::from).collect();

        if settled_only {
            unspents
                .iter_mut()
                .for_each(|u| u.rgb_allocations.retain(|a| a.settled));
        }

        let mut internal_unspents: Vec<Unspent> =
            self.internal_unspents().map(Unspent::from).collect();

        unspents.append(&mut internal_unspents);

        info!(self.logger, "List unspents completed");
        Ok(unspents)
    }

    fn _receive(
        &self,
        asset_id: Option<String>,
        assignment: Assignment,
        duration_seconds: Option<u32>,
        transport_endpoints: Vec<String>,
        min_confirmations: u8,
        beneficiary: Beneficiary,
        recipient_type: RecipientTypeFull,
    ) -> Result<(String, String, Option<i64>, i32), Error> {
        let network: ChainNet = self.bitcoin_network().into();

        let beneficiary = XChainNet::with(network, beneficiary);
        let recipient_id = beneficiary.to_string();
        debug!(self.logger, "Recipient ID: {recipient_id}");
        let (schema, contract_id) = if let Some(aid) = asset_id.clone() {
            let asset = self.database.check_asset_exists(aid.clone())?;
            let contract_id = ContractId::from_str(&aid).expect("invalid contract ID");
            (Some(asset.schema), Some(contract_id))
        } else {
            (None, None)
        };

        self.check_transport_endpoints(&transport_endpoints)?;
        let mut transport_endpoints_dedup = transport_endpoints.clone();
        transport_endpoints_dedup.sort();
        transport_endpoints_dedup.dedup();
        if transport_endpoints_dedup.len() != transport_endpoints.len() {
            return Err(Error::InvalidTransportEndpoints {
                details: s!("no duplicate transport endpoints allowed"),
            });
        }
        let mut endpoints: Vec<String> = vec![];
        for endpoint_str in &transport_endpoints {
            let rgb_transport = RgbTransport::from_str(endpoint_str)?;
            match &rgb_transport {
                RgbTransport::JsonRpc { .. } => {
                    endpoints.push(
                        TransportEndpoint::try_from(rgb_transport)
                            .unwrap()
                            .endpoint
                            .clone(),
                    );
                }
                _ => {
                    return Err(Error::UnsupportedTransportType);
                }
            }
        }

        let mut invoice_builder = RgbInvoiceBuilder::new(beneficiary);
        if let Some(schema) = schema {
            invoice_builder = invoice_builder.set_schema(schema.into());
        }
        if let Some(contract_id) = contract_id {
            invoice_builder = invoice_builder.set_contract(contract_id);
        }
        let transports: Vec<&str> = transport_endpoints.iter().map(AsRef::as_ref).collect();
        invoice_builder = invoice_builder.add_transports(transports).unwrap();
        let detected_assignment = match (&assignment, schema) {
            (Assignment::Fungible(amt), Some(AssetSchema::Nia) | Some(AssetSchema::Ifa) | None) => {
                invoice_builder = invoice_builder.set_amount_raw(*amt);
                invoice_builder = invoice_builder.set_assignment_name(RGB_STATE_ASSET_OWNER);
                assignment
            }
            (Assignment::Any, Some(AssetSchema::Nia)) => {
                invoice_builder = invoice_builder.set_assignment_name(RGB_STATE_ASSET_OWNER);
                Assignment::Fungible(0)
            }
            (Assignment::InflationRight(amt), Some(AssetSchema::Ifa)) => {
                invoice_builder = invoice_builder.set_amount_raw(*amt);
                invoice_builder =
                    invoice_builder.set_assignment_name(RGB_STATE_INFLATION_ALLOWANCE);
                assignment
            }
            (Assignment::Any, _) => Assignment::Any,
            _ => return Err(Error::InvalidAssignment),
        };
        let created_at = now().unix_timestamp();
        let expiry = if duration_seconds == Some(0) {
            None
        } else {
            let duration_seconds = duration_seconds.unwrap_or(DURATION_RCV_TRANSFER) as i64;
            let expiry = created_at + duration_seconds;
            invoice_builder = invoice_builder.set_expiry_timestamp(expiry);
            Some(expiry)
        };

        let invoice = invoice_builder.finish();
        let invoice_string = invoice.to_string();

        let batch_transfer = DbBatchTransferActMod {
            status: ActiveValue::Set(TransferStatus::WaitingCounterparty),
            expiration: ActiveValue::Set(expiry),
            created_at: ActiveValue::Set(created_at),
            min_confirmations: ActiveValue::Set(min_confirmations),
            ..Default::default()
        };
        let batch_transfer_idx = self.database.set_batch_transfer(batch_transfer)?;
        let asset_transfer = DbAssetTransferActMod {
            user_driven: ActiveValue::Set(true),
            batch_transfer_idx: ActiveValue::Set(batch_transfer_idx),
            asset_id: ActiveValue::Set(asset_id),
            ..Default::default()
        };
        let asset_transfer_idx = self.database.set_asset_transfer(asset_transfer)?;
        let transfer = DbTransferActMod {
            asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
            requested_assignment: ActiveValue::Set(Some(detected_assignment)),
            incoming: ActiveValue::Set(true),
            recipient_id: ActiveValue::Set(Some(recipient_id.clone())),
            recipient_type: ActiveValue::Set(Some(recipient_type)),
            invoice_string: ActiveValue::Set(Some(invoice_string.clone())),
            ..Default::default()
        };
        let transfer_idx = self.database.set_transfer(transfer)?;
        for endpoint in endpoints {
            self.save_transfer_transport_endpoint(
                transfer_idx,
                &LocalTransportEndpoint {
                    endpoint,
                    transport_type: TransportType::JsonRpc,
                    used: false,
                    usable: true,
                },
            )?;
        }

        Ok((recipient_id, invoice_string, expiry, batch_transfer_idx))
    }

    /// Blind an UTXO to receive RGB assets and return the resulting [`ReceiveData`].
    ///
    /// An optional asset ID can be specified, which will be embedded in the invoice, resulting in
    /// the refusal of the transfer is the asset doesn't match.
    ///
    /// An optional amount can be specified, which will be embedded in the invoice. It will not be
    /// checked when accepting the transfer.
    ///
    /// An optional duration (in seconds) can be specified, which will set the expiration of the
    /// invoice. A duration of 0 seconds means no expiration.
    ///
    /// Each endpoint in the provided `transport_endpoints` list will be used as RGB data exchange
    /// medium. The list needs to contain at least 1 endpoint and a maximum of 3. Strings
    /// specifying invalid endpoints and duplicate ones will cause an error to be raised. A valid
    /// endpoint string encodes an
    /// [`RgbTransport`](https://docs.rs/rgb-invoicing/latest/rgbinvoice/enum.RgbTransport.html).
    /// At the moment the only supported variant is JsonRpc (e.g. `rpc://127.0.0.1` or
    /// `rpcs://example.com`).
    ///
    /// The `min_confirmations` number determines the minimum number of confirmations needed for
    /// the transaction anchoring the transfer for it to be considered final and move (while
    /// refreshing) to the [`TransferStatus::Settled`] status.
    pub fn blind_receive(
        &self,
        asset_id: Option<String>,
        assignment: Assignment,
        duration_seconds: Option<u32>,
        transport_endpoints: Vec<String>,
        min_confirmations: u8,
    ) -> Result<ReceiveData, Error> {
        info!(
            self.logger,
            "Receiving via blinded UTXO for asset '{:?}' with duration '{:?}'...",
            asset_id,
            duration_seconds
        );

        let mut unspents: Vec<LocalUnspent> = self.database.get_rgb_allocations(
            self.database.get_unspent_txos(vec![])?,
            None,
            None,
            None,
            None,
        )?;
        unspents.retain(|u| {
            !(u.rgb_allocations
                .iter()
                .any(|a| !a.incoming && a.status.waiting_counterparty()))
        });
        let utxo = self.get_utxo(&[], Some(&unspents), true, None)?;
        let unblinded_utxo = utxo.outpoint();
        debug!(
            self.logger,
            "Blinding outpoint '{}'",
            unblinded_utxo.to_string()
        );
        let blind_seal = self.get_blind_seal(utxo.clone()).transmutate();
        let beneficiary = Beneficiary::BlindedSeal(blind_seal.conceal());

        let (recipient_id, invoice, expiration_timestamp, batch_transfer_idx) = self._receive(
            asset_id,
            assignment,
            duration_seconds,
            transport_endpoints,
            min_confirmations,
            beneficiary,
            RecipientTypeFull::Blind { unblinded_utxo },
        )?;

        let mut runtime = self.rgb_runtime()?;
        runtime.store_secret_seal(blind_seal)?;
        // Drop runtime so Stock returns to RefCell before auto-save
        drop(runtime);

        self.update_backup_info(false)?;
        self.trigger_auto_backup();

        info!(self.logger, "Blind receive completed");
        Ok(ReceiveData {
            invoice,
            recipient_id,
            expiration_timestamp,
            batch_transfer_idx,
        })
    }

    /// Create an address to receive RGB assets and return the resulting [`ReceiveData`].
    ///
    /// An optional asset ID can be specified, which will be embedded in the invoice, resulting in
    /// the refusal of the transfer is the asset doesn't match.
    ///
    /// An optional amount can be specified, which will be embedded in the invoice. It will not be
    /// checked when accepting the transfer.
    ///
    /// An optional duration (in seconds) can be specified, which will set the expiration of the
    /// invoice. A duration of 0 seconds means no expiration.
    ///
    /// Each endpoint in the provided `transport_endpoints` list will be used as RGB data exchange
    /// medium. The list needs to contain at least 1 endpoint and a maximum of 3. Strings
    /// specifying invalid endpoints and duplicate ones will cause an error to be raised. A valid
    /// endpoint string encodes an
    /// [`RgbTransport`](https://docs.rs/rgb-invoicing/latest/rgbinvoice/enum.RgbTransport.html).
    /// At the moment the only supported variant is JsonRpc (e.g. `rpc://127.0.0.1` or
    /// `rpcs://example.com`).
    ///
    /// The `min_confirmations` number determines the minimum number of confirmations needed for
    /// the transaction anchoring the transfer for it to be considered final and move (while
    /// refreshing) to the [`TransferStatus::Settled`] status.
    pub fn witness_receive(
        &mut self,
        asset_id: Option<String>,
        assignment: Assignment,
        duration_seconds: Option<u32>,
        transport_endpoints: Vec<String>,
        min_confirmations: u8,
    ) -> Result<ReceiveData, Error> {
        info!(
            self.logger,
            "Receiving via witness TX for asset '{:?}' with duration '{:?}'...",
            asset_id,
            duration_seconds
        );

        let script_pubkey = self.get_new_address()?.script_pubkey();
        let beneficiary = beneficiary_from_script_buf(script_pubkey.clone());

        let (recipient_id, invoice, expiration_timestamp, batch_transfer_idx) = self._receive(
            asset_id,
            assignment,
            duration_seconds,
            transport_endpoints,
            min_confirmations,
            beneficiary,
            RecipientTypeFull::Witness { vout: None },
        )?;

        self.database
            .set_pending_witness_script(DbPendingWitnessScriptActMod {
                script: ActiveValue::Set(script_pubkey.to_hex_string()),
                ..Default::default()
            })?;

        self.update_backup_info(false)?;
        self.trigger_auto_backup();

        info!(self.logger, "Witness receive completed");
        Ok(ReceiveData {
            invoice,
            recipient_id,
            expiration_timestamp,
            batch_transfer_idx,
        })
    }

    pub(crate) fn _get_new_address(&mut self, keychain: KeychainKind) -> Result<BdkAddress, Error> {
        if self.wallet_data.reuse_addresses {
            let index = self
                .reuse_address_index
                .get(&keychain)
                .copied()
                .unwrap_or(0);
            // reveal_addresses_to ensures BDK's sync engine tracks this address
            let _ = self.bdk_wallet.reveal_addresses_to(keychain, index);
            self.bdk_wallet.persist(&mut self.bdk_database)?;
            let address = self.bdk_wallet.peek_address(keychain, index).address;
            return Ok(address);
        }
        let address = self.bdk_wallet.reveal_next_address(keychain).address;
        self.bdk_wallet.persist(&mut self.bdk_database)?;
        Ok(address)
    }

    pub(crate) fn get_new_address(&mut self) -> Result<BdkAddress, Error> {
        self._get_new_address(KeychainKind::External)
    }

    /// Return a new Bitcoin address from the vanilla wallet.
    pub fn get_address(&mut self) -> Result<String, Error> {
        info!(self.logger, "Getting address...");
        let address = self._get_new_address(KeychainKind::Internal)?;

        if !self.wallet_data.reuse_addresses {
            self.update_backup_info(false)?;
            self.trigger_auto_backup();
        }

        info!(self.logger, "Get address completed");
        Ok(address.to_string())
    }

    /// Rotate the pinned address for the given keychain.
    ///
    /// Only meaningful when `reuse_addresses` is `true`. Increments the pinned derivation
    /// index so subsequent address generation returns a fresh address.
    pub fn rotate_address(&mut self, keychain: KeychainKind) -> Result<String, Error> {
        if !self.wallet_data.reuse_addresses {
            return Err(Error::AddressReuseDisabled);
        }
        let index = self
            .reuse_address_index
            .get(&keychain)
            .copied()
            .unwrap_or(0);
        let new_index = index + 1;
        self.reuse_address_index.insert(keychain, new_index);
        let _ = self.bdk_wallet.reveal_addresses_to(keychain, new_index);
        self.bdk_wallet.persist(&mut self.bdk_database)?;
        let address = self.bdk_wallet.peek_address(keychain, new_index).address;

        self.trigger_auto_backup();

        Ok(address.to_string())
    }
}

/// IndexedDB persistence: save wallet state after mutations.
impl Wallet {
    pub(crate) fn update_backup_info(
        &self,
        doing_backup: bool,
    ) -> Result<Option<DbBackupInfo>, Error> {
        use crate::database::memory_db::{ActiveValue, DbBackupInfoActMod};

        let now_str = {
            let ms = js_sys::Date::now() as i64;
            let secs = ms / 1000;
            let nanos = ((ms % 1000) * 1_000_000) as u32;
            let dt = time::OffsetDateTime::from_unix_timestamp(secs)
                .unwrap()
                .replace_nanosecond(nanos)
                .unwrap();
            dt.format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default()
        };

        let existing = self.database.get_backup_info()?;
        if doing_backup {
            if let Some(_info) = existing {
                let mut act = DbBackupInfoActMod {
                    last_backup_timestamp: ActiveValue::Set(now_str),
                    ..Default::default()
                };
                let updated = self.database.update_backup_info(&mut act)?;
                Ok(Some(updated))
            } else {
                let act = DbBackupInfoActMod {
                    last_backup_timestamp: ActiveValue::Set(now_str.clone()),
                    last_operation_timestamp: ActiveValue::Set(now_str),
                    ..Default::default()
                };
                self.database.set_backup_info(act)?;
                Ok(self.database.get_backup_info()?)
            }
        } else if let Some(_info) = existing {
            let mut act = DbBackupInfoActMod {
                last_operation_timestamp: ActiveValue::Set(now_str),
                ..Default::default()
            };
            let updated = self.database.update_backup_info(&mut act)?;
            Ok(Some(updated))
        } else {
            let act = DbBackupInfoActMod {
                last_backup_timestamp: ActiveValue::Set(String::new()),
                last_operation_timestamp: ActiveValue::Set(now_str),
                ..Default::default()
            };
            self.database.set_backup_info(act)?;
            Ok(self.database.get_backup_info()?)
        }
    }

    /// Returns the IndexedDB key for this wallet (derived from wallet_dir).
    pub fn idb_key(&self) -> String {
        self.wallet_dir.to_string_lossy().to_string()
    }

    /// Trigger an async save of wallet state to IndexedDB.
    pub(crate) fn trigger_auto_backup(&self) {
        self.save_to_idb();
    }

    fn snapshot(&self) -> Result<super::idb_store::WalletSnapshot, Error> {
        let stock = self
            .rgb_stock
            .try_borrow()
            .map_err(|_| Error::Persistence {
                details: s!("RGB runtime is active; wallet state cannot be snapshotted"),
            })?;
        let stock = stock.as_ref().ok_or_else(|| Error::Persistence {
            details: s!("RGB runtime is active; wallet state cannot be snapshotted"),
        })?;
        let (stock_stash_b64, stock_state_b64, stock_index_b64) =
            super::idb_store::serialize_stock(stock).ok_or_else(|| Error::Persistence {
                details: s!("failed to serialize RGB stock"),
            })?;

        Ok(super::idb_store::WalletSnapshot {
            sequence: self
                .idb_sequence
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1,
            db: self.database.as_ref().clone(),
            bdk_changeset: self.bdk_database.get_data().clone(),
            transfer_artifacts: self.transfer_artifacts.clone(),
            received_consignments: self.received_consignments.clone(),
            stock_stash_b64: Some(stock_stash_b64),
            stock_state_b64: Some(stock_state_b64),
            stock_index_b64: Some(stock_index_b64),
            reuse_address_index: self.reuse_address_index.clone(),
        })
    }

    /// Durably persist the current wallet state to IndexedDB.
    ///
    /// This method resolves only after the IndexedDB read-write transaction commits. Callers must
    /// not persist dependent application state until this method succeeds. A failure leaves the
    /// current in-memory wallet state intact and may be retried.
    pub async fn flush(&self) -> Result<(), Error> {
        let snapshot = self.snapshot()?;
        super::idb_store::save_snapshot(&self.idb_key(), &snapshot)
            .await
            .map_err(|details| Error::Persistence { details })
    }

    /// Save current wallet state to IndexedDB asynchronously via spawn_local.
    fn save_to_idb(&self) {
        use std::sync::atomic::Ordering;

        // Debounce: skip if a save is already in progress
        if self
            .idb_save_in_progress
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }

        let snapshot = match self.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                web_sys::console::error_1(&error.to_string().into());
                self.idb_save_in_progress.store(false, Ordering::SeqCst);
                return;
            }
        };
        let key = self.idb_key();
        let flag = self.idb_save_in_progress.clone();

        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = super::idb_store::save_snapshot(&key, &snapshot).await {
                web_sys::console::error_1(&format!("IDB save error: {e}").into());
            }
            flag.store(false, Ordering::SeqCst);
        });
    }

    /// Restore wallet state from an IndexedDB snapshot.
    #[allow(clippy::arc_with_non_send_sync)] // wasm32 is single-threaded; Arc matches native API
    pub fn restore_from_snapshot(
        &mut self,
        snapshot: super::idb_store::WalletSnapshot,
    ) -> Result<(), Error> {
        // Restore the in-memory database
        self.database = Arc::new(snapshot.db);

        // Restore BDK changeset and re-load the BDK wallet
        if let Some(changeset) = snapshot.bdk_changeset {
            self.bdk_database.set_data(Some(changeset));

            // Reconstruct descriptors (same logic as Wallet::new)
            let wdata = &self.wallet_data;
            let bdk_network = BdkNetwork::from(wdata.bitcoin_network);
            let xpub_rgb = str_to_xpub(&wdata.account_xpub_colored, bdk_network)?;
            let xpub_btc = str_to_xpub(&wdata.account_xpub_vanilla, bdk_network)?;

            let (desc_colored, desc_vanilla, watch_only) = if let Some(mnemonic) = &wdata.mnemonic {
                let (dc, dv) = get_descriptors(
                    wdata.bitcoin_network,
                    mnemonic,
                    wdata.vanilla_keychain,
                    xpub_btc,
                    xpub_rgb,
                )?;
                (dc, dv, false)
            } else {
                let (dc, dv) = get_descriptors_from_xpubs(
                    wdata.bitcoin_network,
                    &wdata.master_fingerprint,
                    xpub_rgb,
                    xpub_btc,
                    wdata.vanilla_keychain,
                )?;
                (dc, dv, true)
            };

            let chain_net: ChainNet = wdata.bitcoin_network.into();
            let mut wallet_params = BdkWallet::load()
                .descriptor(KeychainKind::External, Some(desc_colored.clone()))
                .descriptor(KeychainKind::Internal, Some(desc_vanilla.clone()))
                .check_genesis_hash(BlockHash::from_byte_array(
                    chain_net.chain_hash().to_bytes(),
                ));
            if !watch_only {
                wallet_params = wallet_params.extract_keys();
            }

            match wallet_params.load_wallet(&mut self.bdk_database) {
                Ok(Some(wallet)) => {
                    self.bdk_wallet = wallet;
                }
                Ok(None) => {
                    return Err(Error::Persistence {
                        details: s!("persisted BDK changeset did not contain a loadable wallet"),
                    });
                }
                Err(error) => {
                    return Err(Error::Persistence {
                        details: format!("failed to load persisted BDK wallet: {error}"),
                    });
                }
            }
        }

        // Restore complete pending transfers so send_end can resume after a page reload.
        self.transfer_artifacts = snapshot.transfer_artifacts;

        // Restore received consignments
        self.received_consignments = snapshot.received_consignments;

        // Restore address reuse pinned indices
        self.reuse_address_index = snapshot.reuse_address_index;

        // Restore RGB Stock from strict-encoded components
        if let (Some(stash_b64), Some(state_b64), Some(index_b64)) = (
            snapshot.stock_stash_b64,
            snapshot.stock_state_b64,
            snapshot.stock_index_b64,
        ) {
            let stock = super::idb_store::deserialize_stock(&stash_b64, &state_b64, &index_b64)
                .map_err(InternalError::StockError)?;
            *self.rgb_stock.borrow_mut() = Some(stock);
        }

        Ok(())
    }
}

// ─── On-chain swap ─────────────────────────────────────────────────────────

pub(crate) const SWAP_DEFAULT_RGB_OUTPUT_SAT: u64 = 1_000;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum SwapDirection {
    RgbForBtc,
    BtcForRgb,
    RgbForRgb,
}

pub(crate) fn swap_invalid(details: impl Into<String>) -> Error {
    Error::InvalidDetails {
        details: details.into(),
    }
}

pub(crate) fn swap_random_id() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("system randomness should be available");
    bytes
        .iter()
        .map(|b| char::from(ALPHABET[*b as usize % ALPHABET.len()]))
        .collect()
}

pub(crate) fn swap_random_blinding() -> u64 {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).expect("system randomness should be available");
    std::cmp::Ord::max(u64::from_be_bytes(bytes), 1)
}

/// Derive a deterministic MPC entropy for a swap from its identifier. Both parties compute the
/// same value so the MPC commitment they each (re)produce is identical.

pub(crate) fn swap_mpc_entropy(swap_id: &str) -> u64 {
    let hash = hash_bytes_hex(swap_id.as_bytes());
    let mut bytes = [0u8; 8];
    let src = hash.as_bytes();
    let take = src.len().min(16);
    let hex_slice = &hash[..take];
    if let Ok(parsed) = u64::from_str_radix(hex_slice, 16) {
        return std::cmp::Ord::max(parsed, 1);
    }
    bytes.copy_from_slice(&src[..8]);
    std::cmp::Ord::max(u64::from_be_bytes(bytes), 1)
}

pub(crate) fn swap_ensure_not_expired(offer: &OnchainSwapOffer) -> Result<(), Error> {
    if let Some(expiration_timestamp) = offer.expiration_timestamp {
        let current_timestamp = u64::try_from(now().unix_timestamp()).unwrap_or_default();
        if expiration_timestamp <= current_timestamp {
            return Err(swap_invalid("swap offer expired"));
        }
    }
    Ok(())
}

pub(crate) fn swap_validate_leg(leg: &OnchainSwapLeg) -> Result<(), Error> {
    if leg.amount == 0 {
        return Err(Error::InvalidAmountZero);
    }
    match leg.kind {
        OnchainSwapLegKind::Btc => {
            if leg.asset_id.is_some() {
                return Err(swap_invalid("BTC swap legs must not include an asset ID"));
            }
        }
        OnchainSwapLegKind::Rgb => {
            let asset_id = leg
                .asset_id
                .as_ref()
                .ok_or_else(|| swap_invalid("RGB swap legs require an asset ID"))?;
            ContractId::from_str(asset_id)
                .map_err(|e| swap_invalid(format!("invalid RGB asset ID: {e}")))?;
        }
    }
    Ok(())
}

pub(crate) fn swap_validate_legs(
    gives: &OnchainSwapLeg,
    receives: &OnchainSwapLeg,
) -> Result<SwapDirection, Error> {
    swap_validate_leg(gives)?;
    swap_validate_leg(receives)?;
    if gives == receives {
        return Err(swap_invalid("swap legs cannot be identical"));
    }
    if matches!(gives.kind, OnchainSwapLegKind::Btc)
        && matches!(receives.kind, OnchainSwapLegKind::Btc)
    {
        return Err(swap_invalid("BTC-for-BTC swaps are not supported"));
    }
    if matches!(gives.kind, OnchainSwapLegKind::Rgb)
        && matches!(receives.kind, OnchainSwapLegKind::Rgb)
        && gives.asset_id == receives.asset_id
    {
        return Err(swap_invalid(
            "RGB-for-RGB swaps require different asset IDs",
        ));
    }
    Ok(match (gives.kind, receives.kind) {
        (OnchainSwapLegKind::Rgb, OnchainSwapLegKind::Btc) => SwapDirection::RgbForBtc,
        (OnchainSwapLegKind::Btc, OnchainSwapLegKind::Rgb) => SwapDirection::BtcForRgb,
        (OnchainSwapLegKind::Rgb, OnchainSwapLegKind::Rgb) => SwapDirection::RgbForRgb,
        (OnchainSwapLegKind::Btc, OnchainSwapLegKind::Btc) => unreachable!("BTC-for-BTC rejected"),
    })
}

pub(crate) fn swap_parse_script(script_hex: &str) -> Result<ScriptBuf, Error> {
    ScriptBuf::from_hex(script_hex).map_err(|e| swap_invalid(format!("invalid script: {e}")))
}

pub(crate) fn swap_input_to_outpoint(input: &OnchainSwapInput) -> BdkOutPoint {
    BdkOutPoint::from(input.outpoint.clone())
}

pub(crate) fn swap_input_txout(input: &OnchainSwapInput) -> Result<TxOut, Error> {
    Ok(TxOut {
        value: BdkAmount::from_sat(input.amount_sat),
        script_pubkey: swap_parse_script(&input.script_pubkey_hex)?,
    })
}

pub(crate) fn swap_build_input(local_output: &LocalOutput) -> OnchainSwapInput {
    OnchainSwapInput {
        outpoint: local_output.outpoint.into(),
        amount_sat: local_output.txout.value.to_sat(),
        script_pubkey_hex: local_output.txout.script_pubkey.to_hex_string(),
    }
}

pub(crate) fn swap_selected_inputs_total(inputs: &[OnchainSwapInput]) -> u64 {
    inputs.iter().map(|i| i.amount_sat).sum()
}

pub(crate) fn swap_require_rgb_destination(
    leg: &OnchainSwapLeg,
    script_hex: &Option<String>,
    blinding: &Option<u64>,
) -> Result<(), Error> {
    if matches!(leg.kind, OnchainSwapLegKind::Rgb) && (script_hex.is_none() || blinding.is_none()) {
        return Err(swap_invalid("missing RGB receive data"));
    }
    Ok(())
}

pub(crate) fn swap_ensure_state_matches<T: PartialEq>(
    expected: &T,
    actual: &T,
    state_name: &str,
) -> Result<(), Error> {
    if expected != actual {
        return Err(swap_invalid(format!(
            "swap {state_name} does not match local state"
        )));
    }
    Ok(())
}

pub(crate) fn swap_history_recipient_id(swap_id: &str, asset_id: &str) -> String {
    let asset_id_hash = hash_bytes_hex(asset_id.as_bytes());
    format!("swap-history-{swap_id}-{}", &asset_id_hash[..16])
}

pub(crate) fn swap_proxy_transport_endpoint(proxy_url: &str) -> Result<String, Error> {
    let endpoint = if let Some(rest) = proxy_url.strip_prefix("http://") {
        format!("rpc://{rest}")
    } else if let Some(rest) = proxy_url.strip_prefix("https://") {
        format!("rpcs://{rest}")
    } else {
        proxy_url.to_string()
    };
    let transport = RgbTransport::from_str(&endpoint)?;
    TransportEndpoint::try_from(transport)?;
    Ok(endpoint)
}

pub(crate) fn swap_validate_proxy_url(proxy_url: &Option<String>) -> Result<(), Error> {
    let proxy_url = proxy_url
        .as_deref()
        .ok_or_else(|| Error::InvalidTransportEndpoints {
            details: s!("on-chain RGB swaps require a proxy URL"),
        })?;
    swap_proxy_transport_endpoint(proxy_url)?;
    Ok(())
}

/// Build the coloring for a swap RGB leg: `leg.amount` goes to `recipient_vout` and everything
/// else the inputs carry (`available`, per contract) goes back to `change_vout`. Vouts are final
/// PSBT indices; the returned flag tells whether the map was shifted for an OP_RETURN-first PSBT.

pub(crate) fn swap_rgb_leg_coloring_info(
    leg: &OnchainSwapLeg,
    psbt: &Psbt,
    recipient_vout: u32,
    change_vout: Option<u32>,
    blinding: u64,
    available: &BTreeMap<ContractId, u64>,
) -> Result<(ContractId, rust_only::ColoringInfo, bool), Error> {
    let asset_id = leg.asset_id.clone().expect("RGB leg has asset ID");
    let contract_id = ContractId::from_str(&asset_id)
        .map_err(|e| swap_invalid(format!("invalid RGB asset ID: {e}")))?;
    let outputs = &psbt.unsigned_tx.output;
    let shift = outputs
        .first()
        .is_some_and(|o| o.script_pubkey.is_op_return())
        && outputs.iter().any(|o| o.script_pubkey.is_p2tr());
    let map_key = |vout: u32| -> Result<u32, Error> {
        let output = outputs
            .get(vout as usize)
            .ok_or_else(|| swap_invalid(format!("swap vout {vout} does not exist in the PSBT")))?;
        if output.script_pubkey.is_op_return() {
            return Err(swap_invalid(format!(
                "swap vout {vout} is the OP_RETURN output"
            )));
        }
        vout.checked_sub(u32::from(shift))
            .ok_or_else(|| swap_invalid("invalid RGB swap vout"))
    };
    let swap_available = available.get(&contract_id).copied().unwrap_or(0);
    if swap_available < leg.amount {
        return Err(swap_invalid(format!(
            "swap inputs carry {swap_available} of {asset_id}, {} required",
            leg.amount
        )));
    }
    let mut asset_info_map = HashMap::new();
    for (cid, amount) in available {
        let mut output_map = HashMap::new();
        let change = if *cid == contract_id {
            output_map.insert(map_key(recipient_vout)?, leg.amount);
            amount - leg.amount
        } else {
            *amount
        };
        if change > 0 {
            let change_vout = change_vout.ok_or_else(|| {
                swap_invalid("swap inputs carry RGB change but the swap has no change output")
            })?;
            if change_vout == recipient_vout {
                return Err(swap_invalid("swap change vout is the recipient vout"));
            }
            output_map.insert(map_key(change_vout)?, change);
        }
        asset_info_map.insert(
            *cid,
            rust_only::AssetColoringInfo {
                output_map,
                static_blinding: (*cid == contract_id).then_some(blinding),
            },
        );
    }
    let coloring_info = rust_only::ColoringInfo {
        asset_info_map,
        static_blinding: Some(blinding),
        nonce: None,
    };
    Ok((contract_id, coloring_info, shift))
}

pub(crate) fn swap_side_btc_payment(leg: &OnchainSwapLeg) -> u64 {
    if matches!(leg.kind, OnchainSwapLegKind::Btc) {
        leg.amount
    } else {
        0
    }
}

pub(crate) fn swap_side_rgb_output_cost(receives: &OnchainSwapLeg, rgb_output_sat: u64) -> u64 {
    if matches!(receives.kind, OnchainSwapLegKind::Rgb) {
        rgb_output_sat
    } else {
        0
    }
}

pub(crate) fn swap_append_side_change(
    outputs: &mut Vec<TxOut>,
    inputs: &[OnchainSwapInput],
    gives: &OnchainSwapLeg,
    receives: &OnchainSwapLeg,
    rgb_output_sat: u64,
    fee_sat: u64,
    change_script_hex: &str,
) -> Result<(), Error> {
    let input_total = swap_selected_inputs_total(inputs);
    let required = swap_side_btc_payment(gives)
        .checked_add(swap_side_rgb_output_cost(receives, rgb_output_sat))
        .and_then(|v| v.checked_add(fee_sat))
        .ok_or_else(|| swap_invalid("swap amounts overflow"))?;
    if input_total < required {
        return Err(Error::InsufficientBitcoins {
            needed: required,
            available: input_total,
        });
    }
    let change = input_total - required;
    if change > 0 {
        outputs.push(TxOut {
            value: BdkAmount::from_sat(change),
            script_pubkey: swap_parse_script(change_script_hex)?,
        });
    }
    Ok(())
}

pub(crate) fn swap_build_psbt(
    proposal: &OnchainSwapProposal,
) -> Result<(Psbt, Option<u32>, Option<u32>), Error> {
    let offer = &proposal.request.offer;
    let maker_inputs = &proposal.maker_inputs;
    let taker_inputs = &proposal.request.taker_inputs;
    let maker_gives = &offer.maker_gives;
    let maker_receives = &offer.maker_receives;
    let taker_gives = maker_receives;
    let taker_receives = maker_gives;

    let mut tx_inputs = vec![];
    for input in maker_inputs.iter().chain(taker_inputs.iter()) {
        tx_inputs.push(TxIn {
            previous_output: swap_input_to_outpoint(input),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::new(),
        });
    }

    let mut outputs = vec![TxOut {
        value: BdkAmount::ZERO,
        script_pubkey: ScriptBuf::new_op_return([]),
    }];
    let mut maker_rgb_vout = None;
    let mut taker_rgb_vout = None;

    if matches!(maker_receives.kind, OnchainSwapLegKind::Btc) {
        let address = offer
            .maker_btc_address
            .as_ref()
            .ok_or_else(|| swap_invalid("missing maker BTC receive address"))?;
        outputs.push(TxOut {
            value: BdkAmount::from_sat(maker_receives.amount),
            script_pubkey: parse_address_str(address, offer.bitcoin_network)?.script_pubkey(),
        });
    }
    if matches!(taker_receives.kind, OnchainSwapLegKind::Btc) {
        let address = proposal
            .request
            .taker_btc_address
            .as_ref()
            .ok_or_else(|| swap_invalid("missing taker BTC receive address"))?;
        outputs.push(TxOut {
            value: BdkAmount::from_sat(taker_receives.amount),
            script_pubkey: parse_address_str(address, offer.bitcoin_network)?.script_pubkey(),
        });
    }
    if matches!(maker_receives.kind, OnchainSwapLegKind::Rgb) {
        maker_rgb_vout = Some(outputs.len() as u32);
        outputs.push(TxOut {
            value: BdkAmount::from_sat(offer.rgb_output_sat),
            script_pubkey: swap_parse_script(
                offer
                    .maker_rgb_script_pubkey_hex
                    .as_ref()
                    .ok_or_else(|| swap_invalid("missing maker RGB receive script"))?,
            )?,
        });
    }
    if matches!(taker_receives.kind, OnchainSwapLegKind::Rgb) {
        taker_rgb_vout = Some(outputs.len() as u32);
        outputs.push(TxOut {
            value: BdkAmount::from_sat(offer.rgb_output_sat),
            script_pubkey: swap_parse_script(
                proposal
                    .request
                    .taker_rgb_script_pubkey_hex
                    .as_ref()
                    .ok_or_else(|| swap_invalid("missing taker RGB receive script"))?,
            )?,
        });
    }

    if offer.platform_fee_sat > 0 {
        outputs.push(TxOut {
            value: BdkAmount::from_sat(offer.platform_fee_sat),
            script_pubkey: swap_parse_script(
                offer
                    .fee_recipient_script_pubkey_hex
                    .as_ref()
                    .ok_or_else(|| swap_invalid("missing fee recipient script"))?,
            )?,
        });
    }

    // The taker funds the network fee and the platform fee, same convention as
    // rgb-onchain-swaps-atomic's earlier PSBT-level design: the platform fee is charged
    // alongside the miner fee, both funded by whoever is buying/taking the offer.
    let taker_fee_sat = offer
        .network_fee_sat
        .checked_add(offer.platform_fee_sat)
        .ok_or_else(|| swap_invalid("swap amounts overflow"))?;
    swap_append_side_change(
        &mut outputs,
        maker_inputs,
        maker_gives,
        maker_receives,
        offer.rgb_output_sat,
        0,
        &proposal.maker_change_script_pubkey_hex,
    )?;
    swap_append_side_change(
        &mut outputs,
        taker_inputs,
        taker_gives,
        taker_receives,
        offer.rgb_output_sat,
        taker_fee_sat,
        &proposal.request.taker_change_script_pubkey_hex,
    )?;

    let mut psbt = Psbt::from_unsigned_tx(BdkTransaction {
        version: TxVersion::TWO,
        lock_time: LockTime::ZERO,
        input: tx_inputs,
        output: outputs,
    })
    .map_err(|e| Error::InvalidPsbt {
        details: e.to_string(),
    })?;
    let all_inputs = maker_inputs
        .iter()
        .chain(taker_inputs.iter())
        .cloned()
        .collect::<Vec<_>>();
    swap_restore_input_metadata(&mut psbt, &all_inputs)?;
    Ok((psbt, maker_rgb_vout, taker_rgb_vout))
}

pub(crate) fn swap_restore_input_metadata(
    psbt: &mut Psbt,
    inputs: &[OnchainSwapInput],
) -> Result<(), Error> {
    for (idx, input) in inputs.iter().enumerate() {
        let psbt_input = psbt
            .inputs
            .get_mut(idx)
            .ok_or_else(|| swap_invalid("PSBT input metadata mismatch"))?;
        psbt_input.witness_utxo = Some(swap_input_txout(input)?);
    }
    Ok(())
}

pub(crate) fn swap_validate_proposal_psbt(
    proposal: &OnchainSwapProposal,
    psbt: &Psbt,
) -> Result<(), Error> {
    let (expected_psbt, _, _) = swap_build_psbt(proposal)?;
    let actual_tx = &psbt.unsigned_tx;
    let expected_tx = &expected_psbt.unsigned_tx;

    if actual_tx.version != expected_tx.version || actual_tx.lock_time != expected_tx.lock_time {
        return Err(swap_invalid("swap PSBT transaction header mismatch"));
    }
    if actual_tx.input != expected_tx.input {
        return Err(swap_invalid("swap PSBT inputs mismatch"));
    }
    if actual_tx.output.len() != expected_tx.output.len() {
        return Err(swap_invalid("swap PSBT output count mismatch"));
    }
    if psbt.inputs.len() != expected_psbt.inputs.len() {
        return Err(swap_invalid("swap PSBT input metadata count mismatch"));
    }
    for (idx, (actual, expected)) in psbt
        .inputs
        .iter()
        .zip(expected_psbt.inputs.iter())
        .enumerate()
    {
        if actual.witness_utxo != expected.witness_utxo {
            return Err(swap_invalid(format!(
                "swap PSBT input {idx} metadata mismatch"
            )));
        }
    }
    if actual_tx
        .output
        .first()
        .is_none_or(|o| o.value != BdkAmount::ZERO || !o.script_pubkey.is_op_return())
    {
        return Err(swap_invalid("swap PSBT missing RGB OP_RETURN host"));
    }
    for (idx, (actual, expected)) in actual_tx
        .output
        .iter()
        .zip(expected_tx.output.iter())
        .enumerate()
        .skip(1)
    {
        if actual != expected {
            return Err(swap_invalid(format!("swap PSBT output {idx} mismatch")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod swap_unit_tests {
    use super::*;

    fn btc(amount: u64) -> OnchainSwapLeg {
        OnchainSwapLeg {
            kind: OnchainSwapLegKind::Btc,
            asset_id: None,
            amount,
        }
    }

    fn rgb(asset_id: &str, amount: u64) -> OnchainSwapLeg {
        OnchainSwapLeg {
            kind: OnchainSwapLegKind::Rgb,
            asset_id: Some(asset_id.to_string()),
            amount,
        }
    }

    const ASSET_1: &str = "rgb:Ar4ouaLv-b7f7Dc_-z5EMvtu-FA5KNh1-nlae~jk-8xMBo7E";
    const ASSET_2: &str = "rgb:4bBH~Lrb-rx8sB_n-WAJLPcn-X5tFL9q-dFDGbSz-8yApPws";

    fn swap_input(txid: &str, vout: u32, amount_sat: u64) -> OnchainSwapInput {
        OnchainSwapInput {
            outpoint: Outpoint {
                txid: txid.to_string(),
                vout,
            },
            amount_sat,
            script_pubkey_hex: "51".to_string(),
        }
    }

    fn rgb_rgb_proposal() -> OnchainSwapProposal {
        let offer = OnchainSwapOffer {
            swap_id: "test-swap".to_string(),
            maker_gives: rgb(ASSET_1, 10),
            maker_receives: rgb(ASSET_2, 20),
            bitcoin_network: BitcoinNetwork::Regtest,
            network_fee_sat: 100,
            rgb_output_sat: SWAP_DEFAULT_RGB_OUTPUT_SAT,
            expiration_timestamp: None,
            maker_btc_address: None,
            maker_rgb_recipient_id: Some("maker-rgb-recipient".to_string()),
            maker_rgb_script_pubkey_hex: Some("51".to_string()),
            maker_rgb_blinding: Some(1),
            proxy_url: Some("rpc://127.0.0.1:3000".to_string()),
            platform_fee_sat: 0,
            fee_recipient_script_pubkey_hex: None,
        };
        OnchainSwapProposal {
            request: OnchainSwapRequest {
                offer,
                taker_inputs: vec![swap_input(
                    "1111111111111111111111111111111111111111111111111111111111111111",
                    0,
                    2_100,
                )],
                taker_btc_address: None,
                taker_rgb_recipient_id: Some("taker-rgb-recipient".to_string()),
                taker_rgb_script_pubkey_hex: Some("51".to_string()),
                taker_rgb_blinding: Some(2),
                taker_change_script_pubkey_hex: "51".to_string(),
            },
            maker_inputs: vec![swap_input(
                "2222222222222222222222222222222222222222222222222222222222222222",
                0,
                1_000,
            )],
            maker_change_script_pubkey_hex: "51".to_string(),
            psbt: String::new(),
            txid: String::new(),
            consignments: vec![],
            maker_history: None,
        }
    }

    #[test]
    fn validates_supported_leg_pairs() {
        assert_eq!(
            swap_validate_legs(&rgb(ASSET_1, 1), &btc(1_000)).unwrap(),
            SwapDirection::RgbForBtc
        );
        assert_eq!(
            swap_validate_legs(&btc(1_000), &rgb(ASSET_1, 1)).unwrap(),
            SwapDirection::BtcForRgb
        );
        assert_eq!(
            swap_validate_legs(&rgb(ASSET_1, 1), &rgb(ASSET_2, 1)).unwrap(),
            SwapDirection::RgbForRgb
        );
    }

    #[test]
    fn rejects_invalid_leg_pairs() {
        assert!(matches!(
            swap_validate_legs(&btc(0), &rgb(ASSET_1, 1)),
            Err(Error::InvalidAmountZero)
        ));
        assert!(swap_validate_legs(&btc(1), &btc(2)).is_err());
        assert!(swap_validate_legs(&rgb(ASSET_1, 1), &rgb(ASSET_1, 2)).is_err());
        assert!(swap_validate_legs(&rgb("invalid", 1), &btc(1)).is_err());
    }

    #[test]
    fn rejects_missing_or_extra_asset_ids() {
        assert!(
            swap_validate_legs(
                &OnchainSwapLeg {
                    kind: OnchainSwapLegKind::Rgb,
                    asset_id: None,
                    amount: 1,
                },
                &btc(1_000),
            )
            .is_err()
        );
        assert!(
            swap_validate_legs(
                &OnchainSwapLeg {
                    kind: OnchainSwapLegKind::Btc,
                    asset_id: Some(ASSET_1.to_string()),
                    amount: 1_000,
                },
                &rgb(ASSET_2, 1),
            )
            .is_err()
        );
    }

    #[test]
    fn validates_swap_proxy_transport() {
        assert_eq!(
            swap_proxy_transport_endpoint("http://127.0.0.1:3000").unwrap(),
            "rpc://127.0.0.1:3000"
        );
        assert_eq!(
            swap_proxy_transport_endpoint("https://example.com/json-rpc").unwrap(),
            "rpcs://example.com/json-rpc"
        );
        assert!(swap_validate_proxy_url(&Some("rpc://127.0.0.1:3000".to_string())).is_ok());
        assert!(swap_validate_proxy_url(&None).is_err());
        assert!(swap_validate_proxy_url(&Some("ws://127.0.0.1:3000".to_string())).is_err());
    }

    #[test]
    fn validates_expected_swap_psbt_shape() {
        let mut proposal = rgb_rgb_proposal();
        let (psbt, _, _) = swap_build_psbt(&proposal).unwrap();
        proposal.txid = psbt.unsigned_tx.compute_txid().to_string();
        proposal.psbt = psbt.to_string();
        swap_validate_proposal_psbt(&proposal, &psbt).unwrap();
    }

    #[test]
    fn rejects_tampered_swap_psbt_output() {
        let mut proposal = rgb_rgb_proposal();
        let (mut psbt, _, _) = swap_build_psbt(&proposal).unwrap();
        proposal.txid = psbt.unsigned_tx.compute_txid().to_string();
        proposal.psbt = psbt.to_string();
        psbt.unsigned_tx.output[1].value = BdkAmount::from_sat(42);
        assert!(swap_validate_proposal_psbt(&proposal, &psbt).is_err());
    }

    #[test]
    fn swap_coloring_carries_change_and_other_assets() {
        let proposal = rgb_rgb_proposal();
        let (psbt, _, taker_rgb_vout) = swap_build_psbt(&proposal).unwrap();
        let recipient_vout = taker_rgb_vout.unwrap();
        let change_vout = psbt.unsigned_tx.output.len() as u32 - 1;
        let asset_1 = ContractId::from_str(ASSET_1).unwrap();
        let asset_2 = ContractId::from_str(ASSET_2).unwrap();
        let available = BTreeMap::from_iter([(asset_1, 15), (asset_2, 7)]);
        let (contract_id, coloring_info, shift) = swap_rgb_leg_coloring_info(
            &rgb(ASSET_1, 10),
            &psbt,
            recipient_vout,
            Some(change_vout),
            42,
            &available,
        )
        .unwrap();
        assert_eq!(contract_id, asset_1);
        assert!(!shift);
        let swap_info = &coloring_info.asset_info_map[&asset_1];
        assert_eq!(
            swap_info.output_map,
            HashMap::from_iter([(recipient_vout, 10), (change_vout, 5)])
        );
        assert_eq!(swap_info.static_blinding, Some(42));
        assert_eq!(
            coloring_info.asset_info_map[&asset_2].output_map,
            HashMap::from_iter([(change_vout, 7)])
        );
    }

    #[test]
    fn swap_coloring_fails_closed() {
        let proposal = rgb_rgb_proposal();
        let (psbt, _, taker_rgb_vout) = swap_build_psbt(&proposal).unwrap();
        let recipient_vout = taker_rgb_vout.unwrap();
        let outputs = psbt.unsigned_tx.output.len() as u32;
        let asset_1 = ContractId::from_str(ASSET_1).unwrap();
        let exact = BTreeMap::from_iter([(asset_1, 10)]);
        let leftover = BTreeMap::from_iter([(asset_1, 11)]);
        let leg = rgb(ASSET_1, 10);
        // leftover without a change output would be burned
        assert!(
            swap_rgb_leg_coloring_info(&leg, &psbt, recipient_vout, None, 1, &leftover).is_err()
        );
        // not enough on the inputs
        let short = BTreeMap::from_iter([(asset_1, 9)]);
        assert!(swap_rgb_leg_coloring_info(&leg, &psbt, recipient_vout, None, 1, &short).is_err());
        // out-of-range and OP_RETURN vouts
        assert!(swap_rgb_leg_coloring_info(&leg, &psbt, outputs, None, 1, &exact).is_err());
        assert!(swap_rgb_leg_coloring_info(&leg, &psbt, 0, None, 1, &exact).is_err());
        assert!(
            swap_rgb_leg_coloring_info(&leg, &psbt, recipient_vout, Some(outputs), 1, &leftover)
                .is_err()
        );
        assert!(swap_rgb_leg_coloring_info(&leg, &psbt, recipient_vout, None, 1, &exact).is_ok());
    }

    #[test]
    #[ignore = "reads the clock through js_sys, which only exists in a browser"]
    fn rejects_expired_offer() {
        let mut proposal = rgb_rgb_proposal();
        proposal.request.offer.expiration_timestamp = Some(1);
        assert!(swap_ensure_not_expired(&proposal.request.offer).is_err());
    }
}

#[cfg(test)]
mod address_reuse_tests {
    use super::*;
    use crate::keys::generate_keys;
    use crate::utils::{get_descriptors, str_to_xpub};

    /// Helper: build a minimal Wallet for native tests.
    ///
    /// Bypasses `Wallet::new` (which requires RGB schema import / js-sys)
    /// by constructing only the BDK wallet and the fields that
    /// `_get_new_address` touches.
    fn make_test_wallet(reuse: bool) -> Wallet {
        let keys = generate_keys(BitcoinNetwork::Regtest);
        let bdk_network = BdkNetwork::from(BitcoinNetwork::Regtest);
        let xpub_rgb = str_to_xpub(&keys.account_xpub_colored, bdk_network).unwrap();
        let xpub_btc = str_to_xpub(&keys.account_xpub_vanilla, bdk_network).unwrap();
        let (desc_colored, desc_vanilla) = get_descriptors(
            BitcoinNetwork::Regtest,
            &keys.mnemonic,
            None,
            xpub_btc,
            xpub_rgb,
        )
        .unwrap();
        let mut bdk_database: BdkPersister = super::super::memory_store::MemoryStore::new();
        let bdk_wallet = BdkWallet::create(desc_colored, desc_vanilla)
            .network(bdk_network)
            .create_wallet(&mut bdk_database)
            .unwrap();

        let wd = WalletData {
            data_dir: ":memory:".to_string(),
            bitcoin_network: BitcoinNetwork::Regtest,
            database_type: DatabaseType::Sqlite,
            max_allocations_per_utxo: 5,
            account_xpub_vanilla: keys.account_xpub_vanilla.clone(),
            account_xpub_colored: keys.account_xpub_colored.clone(),
            mnemonic: Some(keys.mnemonic.clone()),
            master_fingerprint: keys.master_fingerprint.clone(),
            vanilla_keychain: None,
            supported_schemas: vec![AssetSchema::Nia],
            reuse_addresses: reuse,
        };

        let drain = slog::Discard;
        let logger = Logger::root(drain, o!());

        Wallet {
            wallet_data: wd,
            logger,
            _logger_guard: (),
            watch_only: false,
            database: Arc::new(crate::database::memory_db::InMemoryDb::new()),
            wallet_dir: PathBuf::from(":memory:"),
            bdk_wallet,
            bdk_database,
            reuse_address_index: HashMap::new(),
            #[cfg(feature = "esplora")]
            wasm_proxy_client: crate::api::proxy::WasmProxyClient::new().unwrap(),
            #[cfg(feature = "esplora")]
            online_data: None,
            #[cfg(feature = "esplora")]
            transfer_artifacts: HashMap::new(),
            #[cfg(feature = "esplora")]
            received_consignments: HashMap::new(),
            idb_save_in_progress: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            idb_sequence: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            rgb_stock: std::rc::Rc::new(std::cell::RefCell::new(None)),
            vss_client: None,
            last_vss_backup_error: std::cell::RefCell::new(None),
        }
    }

    #[test]
    fn reuse_returns_same_address() {
        let mut wallet = make_test_wallet(true);
        let addr1 = wallet._get_new_address(KeychainKind::Internal).unwrap();
        let addr2 = wallet._get_new_address(KeychainKind::Internal).unwrap();
        assert_eq!(addr1, addr2, "reuse enabled: same address expected");
    }

    #[test]
    fn no_reuse_returns_different_addresses() {
        let mut wallet = make_test_wallet(false);
        let addr1 = wallet._get_new_address(KeychainKind::Internal).unwrap();
        let addr2 = wallet._get_new_address(KeychainKind::Internal).unwrap();
        assert_ne!(addr1, addr2, "reuse disabled: different addresses expected");
    }

    #[test]
    fn reuse_index_changes_address() {
        let mut wallet = make_test_wallet(true);
        let old_addr = wallet._get_new_address(KeychainKind::Internal).unwrap();

        // Simulate what rotate_address does internally
        wallet.reuse_address_index.insert(KeychainKind::Internal, 1);
        let new_addr = wallet._get_new_address(KeychainKind::Internal).unwrap();

        assert_ne!(
            old_addr, new_addr,
            "bumping index should change the pinned address"
        );
    }

    #[test]
    fn keychains_have_independent_indices() {
        let mut wallet = make_test_wallet(true);
        let internal = wallet._get_new_address(KeychainKind::Internal).unwrap();
        let external = wallet._get_new_address(KeychainKind::External).unwrap();
        assert_ne!(
            internal.to_string(),
            external.to_string(),
            "different keychains"
        );

        // Bump External index — should not affect Internal
        wallet.reuse_address_index.insert(KeychainKind::External, 1);
        let internal_after = wallet._get_new_address(KeychainKind::Internal).unwrap();
        assert_eq!(
            internal.to_string(),
            internal_after.to_string(),
            "Internal unchanged after External index bump"
        );
    }
}
