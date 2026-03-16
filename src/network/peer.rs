//! Peer connection and discovery logic.
//!
//! A [`Peer`] is a thin wrapper around a live, authenticated ETH-wire session.
//! [`NetworkManager`] emits a [`NetworkEvent::ActivePeerSession`] for every new
//! session; `manager.rs` converts that event into a [`Peer`] and tracks it.
//! Each [`Peer`] can then be polled independently for per-block transaction
//! receipts via the ETH wire [`PeerRequest::GetReceipts`] family.
//!
//! # Lifecycle
//!
//! ```text
//!  start_network()
//!       │
//!       ▼
//!  NetworkManager  ──(discv4)──▶  discovers peers
//!       │
//!       ▼ NetworkEvent::ActivePeerSession { info, messages }
//!  manager.rs  ──▶  Peer::from_session(info, messages)
//!       │
//!       ▼
//!  peer.fetch_receipts(block_hashes)   ←── called by historical / live sync
//! ```

use std::{net::SocketAddr, sync::Arc, time::Instant};

use alloy::primitives::B256;
use reth_eth_wire::{
    Capabilities, EthVersion, GetReceipts, GetReceipts70, Receipts, Receipts69, Receipts70,
};
use reth_network::{
    EthNetworkPrimitives, NetworkConfig, NetworkHandle, NetworkManager, Peers,
    config::rng_secret_key,
    events::{PeerRequest, PeerRequestSender, SessionInfo},
};
use reth_network_p2p::error::RequestError;
use reth_network_peers::{NodeRecord, PeerId, mainnet_nodes};
use reth_primitives::Receipt;
use reth_storage_api::noop::NoopProvider;
use tokio::sync::oneshot;
use tracing::{debug, trace};

use crate::error::AppError;

// ─── Error type ─────────────────────────────────────────────────────────────

/// Errors that can occur when interacting with a single peer.
#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    /// The request could not be sent — the peer's session channel is closed.
    #[error("peer {0} is disconnected; failed to send request")]
    Disconnected(PeerId),

    /// The request was delivered but the peer returned a protocol-level error.
    #[error("peer {0} returned a request error: {1}")]
    RequestFailed(PeerId, String),

    /// The response oneshot was dropped before a reply arrived.
    #[error("peer {0} response channel closed unexpectedly")]
    ResponseChannelClosed(PeerId),
}

// ─── PeerInfo ───────────────────────────────────────────────────────────────

/// Immutable metadata snapshot for a connected peer.
///
/// Cheap to clone; used by the manager to answer informational queries without
/// borrowing the full [`Peer`].
#[derive(Debug, Clone)]
pub struct PeerInfo {
    /// The peer's secp256k1 node identity.
    pub id: PeerId,
    /// The TCP address of the remote end.
    pub remote_addr: SocketAddr,
    /// Negotiated ETH sub-protocol version (e.g. `Eth68`).
    pub eth_version: EthVersion,
    /// The peer's self-reported client name and version string.
    pub client_version: Arc<str>,
    /// All protocol capabilities announced in the ETH handshake.
    pub capabilities: Arc<Capabilities>,
    /// When this session was established.
    pub connected_at: Instant,
}

// ─── Peer ───────────────────────────────────────────────────────────────────

/// A live, authenticated P2P session with a remote Ethereum peer.
///
/// Constructed from a [`NetworkEvent::ActivePeerSession`] emitted by
/// [`NetworkManager`]. Owns the [`PeerRequestSender`] that is the *only* way
/// to address that specific session — dropping a [`Peer`] cleanly prevents any
/// further requests to it.
#[derive(Debug)]
pub struct Peer {
    info: PeerInfo,
    /// Direct channel into the spawned session task for this peer.
    ///
    /// `PeerRequestSender` uses its default generic `R = PeerRequest`, which
    /// resolves to `PeerRequest<EthNetworkPrimitives>` — the same type that
    /// `NetworkEvent::ActivePeerSession` delivers.
    sender: PeerRequestSender,
}

impl Peer {
    // ── Constructor ───────────────────────────────────────────────────────

    /// Build a [`Peer`] from the data delivered by
    /// [`NetworkEvent::ActivePeerSession`].
    ///
    /// * `info`   — [`SessionInfo`] from the network event.
    /// * `sender` — [`PeerRequestSender`] from the same event; exclusively
    ///              addresses this session.
    pub fn from_session(info: SessionInfo, sender: PeerRequestSender) -> Self {
        let peer_info = PeerInfo {
            id: info.peer_id,
            remote_addr: info.remote_addr,
            eth_version: info.version,
            client_version: info.client_version,
            capabilities: info.capabilities,
            connected_at: Instant::now(),
        };

        debug!(
            target: "net::peer",
            peer_id = %peer_info.id,
            addr    = %peer_info.remote_addr,
            eth_ver = ?peer_info.eth_version,
            client  = %peer_info.client_version,
            "peer session established",
        );

        Self {
            info: peer_info,
            sender,
        }
    }

    // ── Accessors ─────────────────────────────────────────────────────────

    /// The peer's node identity (public key).
    pub fn id(&self) -> PeerId {
        self.info.id
    }

    /// TCP address of the remote end.
    pub fn remote_addr(&self) -> SocketAddr {
        self.info.remote_addr
    }

    /// Negotiated ETH sub-protocol version.
    pub fn eth_version(&self) -> EthVersion {
        self.info.eth_version
    }

    /// Cheap clone of the peer's metadata.
    pub fn info(&self) -> PeerInfo {
        self.info.clone()
    }

    // ── Receipt fetching ──────────────────────────────────────────────────

    /// Request the transaction receipts for one or more blocks from this peer.
    ///
    /// Sends the appropriate [`PeerRequest`] variant for the negotiated ETH
    /// version and waits for the peer to respond.
    ///
    /// Returns one inner `Vec<Receipt>` per requested block, in the same order
    /// as `block_hashes`. Returns a [`PeerError`] if the peer is disconnected,
    /// the request fails at the protocol level, or the response channel is
    /// dropped.
    pub async fn fetch_receipts(
        &self,
        block_hashes: Vec<B256>,
    ) -> Result<Vec<Vec<Receipt>>, PeerError> {
        let peer_id = self.info.id;

        trace!(
            target: "net::peer",
            %peer_id,
            blocks = block_hashes.len(),
            "requesting receipts",
        );

        // Dispatch to the correct ETH-version handler.
        //
        // Each arm:
        //   1. Creates a oneshot channel whose Sender type exactly matches the
        //      `response` field of the corresponding `PeerRequest` variant.
        //   2. Builds and sends the request via `try_send`.
        //   3. Awaits the response and normalises it to `Vec<Vec<Receipt>>`.
        //
        // ETH version  Response wrapper           Bloom present?
        // ──────────────────────────────────────────────────────
        //  eth/66-68   Receipts<T>    = Vec<Vec<ReceiptWithBloom<T>>>   yes
        //  eth/69      Receipts69<T>  = Vec<Vec<T>>                     no
        //  eth/70      Receipts70<T>  → Into<Receipts<T>>               yes

        match self.info.eth_version {
            EthVersion::Eth70 => {
                let (tx, rx) = oneshot::channel::<Result<Receipts70<Receipt>, RequestError>>();

                self.sender
                    .try_send(PeerRequest::GetReceipts70 {
                        request: GetReceipts70 {
                            first_block_receipt_index: 0,
                            block_hashes,
                        },
                        response: tx,
                    })
                    .map_err(|_| PeerError::Disconnected(peer_id))?;

                let receipts70 = rx
                    .await
                    .map_err(|_| PeerError::ResponseChannelClosed(peer_id))?
                    .map_err(|e| PeerError::RequestFailed(peer_id, e.to_string()))?;

                // Receipts70<T> → Into → Receipts<T>(Vec<Vec<ReceiptWithBloom<T>>>)
                let receipts: Receipts<Receipt> = receipts70.into();
                Ok(receipts
                    .0
                    .into_iter()
                    .map(|block| block.into_iter().map(|rwb| rwb.receipt).collect())
                    .collect())
            }

            EthVersion::Eth69 => {
                let (tx, rx) = oneshot::channel::<Result<Receipts69<Receipt>, RequestError>>();

                self.sender
                    .try_send(PeerRequest::GetReceipts69 {
                        request: GetReceipts(block_hashes),
                        response: tx,
                    })
                    .map_err(|_| PeerError::Disconnected(peer_id))?;

                let receipts69 = rx
                    .await
                    .map_err(|_| PeerError::ResponseChannelClosed(peer_id))?
                    .map_err(|e| PeerError::RequestFailed(peer_id, e.to_string()))?;

                // Receipts69<T> wraps Vec<Vec<T>> directly — no bloom to strip.
                Ok(receipts69.0)
            }

            _ => {
                // eth/66, eth/67, eth/68 — receipts include bloom filter.
                let (tx, rx) = oneshot::channel::<Result<Receipts<Receipt>, RequestError>>();

                self.sender
                    .try_send(PeerRequest::GetReceipts {
                        request: GetReceipts(block_hashes),
                        response: tx,
                    })
                    .map_err(|_| PeerError::Disconnected(peer_id))?;

                let receipts = rx
                    .await
                    .map_err(|_| PeerError::ResponseChannelClosed(peer_id))?
                    .map_err(|e| PeerError::RequestFailed(peer_id, e.to_string()))?;

                // Receipts<T> wraps Vec<Vec<ReceiptWithBloom<T>>>; strip bloom.
                Ok(receipts
                    .0
                    .into_iter()
                    .map(|block| block.into_iter().map(|rwb| rwb.receipt).collect())
                    .collect())
            }
        }
    }
}

// ─── Network bootstrap ──────────────────────────────────────────────────────

/// Spin up a [`NetworkManager`] connected to mainnet and return a shareable
/// [`NetworkHandle`].
///
/// The caller (i.e. `manager.rs`) should subscribe to peer events by calling
/// `handle.event_listener()` *before* the first tick, then match on
/// [`NetworkEvent::ActivePeerSession`] and call [`Peer::from_session`] to
/// build a tracked [`Peer`].
///
/// # Errors
///
/// Propagates any I/O or configuration error from [`NetworkManager::new`].
pub async fn start_network() -> Result<NetworkHandle, AppError> {
    // Generate a fresh ephemeral node identity.
    // In production this should be persisted so that the node keeps the same
    // PeerId across restarts (improves peer reputation continuity).
    let secret_key = rng_secret_key();

    // `NoopProvider` satisfies the `BlockReader` bound required by
    // `NetworkConfig`. This node is a pure consumer — it never serves blocks
    // to peers, so a no-op implementation is correct.
    let config = NetworkConfig::<_, EthNetworkPrimitives>::builder(secret_key)
        .boot_nodes(mainnet_nodes())
        .build(NoopProvider::default());

    let manager = NetworkManager::new(config)
        .await
        .map_err(|e| AppError::Network(e.to_string()))?;

    let handle = manager.handle().clone();

    tokio::spawn(manager);

    Ok(handle)
}

// ─── Peer management helpers ─────────────────────────────────────────────────

/// Attempt an outbound connection to a specific peer by [`NodeRecord`].
///
/// The [`NetworkManager`] will immediately dial the peer's TCP address.  If
/// the ETH handshake succeeds a [`NetworkEvent::ActivePeerSession`] will be
/// emitted on the event stream — the manager should then call
/// [`Peer::from_session`] to register it.
///
/// The [`Peers`] trait is imported in this module so the method is in scope.
pub fn add_peer(handle: &NetworkHandle, node: NodeRecord) {
    debug!(
        target: "net::peer",
        peer_id = %node.id,
        addr    = %node.tcp_addr(),
        "adding peer to network",
    );
    handle.add_peer(node.id, node.tcp_addr());
}
