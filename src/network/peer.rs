//! Peer connection and discovery logic.

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

use crate::error::AppError;

// ─── Error type ─────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    #[error("peer {0} is disconnected; failed to send request")]
    Disconnected(PeerId),
    #[error("peer {0} returned a request error: {1}")]
    RequestFailed(PeerId, String),
    #[error("peer {0} response channel closed unexpectedly")]
    ResponseChannelClosed(PeerId),
}

// ─── PeerInfo ───────────────────────────────────────────────────────────────

/// Immutable metadata snapshot for a connected peer. Cheap to clone.
#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub id: PeerId,
    pub remote_addr: SocketAddr,
    pub eth_version: EthVersion,
    pub client_version: Arc<str>,
    pub capabilities: Arc<Capabilities>,
    pub connected_at: Instant,
}

// ─── Peer ───────────────────────────────────────────────────────────────────

/// A live, authenticated P2P session with a remote Ethereum peer.
#[derive(Debug, Clone)]
pub struct Peer {
    info: PeerInfo,
    /// `ActivePeerSession.messages` is `PeerRequestSender<PeerRequest<N>>` —
    /// the `PeerRequest<N>` wrapper is present, so `try_send` accepts
    /// `PeerRequest<EthNetworkPrimitives>` values directly.
    sender: PeerRequestSender<PeerRequest<EthNetworkPrimitives>>,
}

impl Peer {
    pub fn from_session(
        info: SessionInfo,
        sender: PeerRequestSender<PeerRequest<EthNetworkPrimitives>>,
    ) -> Self {
        let peer_info = PeerInfo {
            id: info.peer_id,
            remote_addr: info.remote_addr,
            eth_version: info.version,
            client_version: info.client_version,
            capabilities: info.capabilities,
            connected_at: Instant::now(),
        };

        Self {
            info: peer_info,
            sender,
        }
    }

    pub fn id(&self) -> PeerId {
        self.info.id
    }

    pub fn remote_addr(&self) -> SocketAddr {
        self.info.remote_addr
    }

    pub fn eth_version(&self) -> EthVersion {
        self.info.eth_version
    }

    pub fn info(&self) -> PeerInfo {
        self.info.clone()
    }

    // ── Receipt fetching ──────────────────────────────────────────────────

    /// Request transaction receipts for one or more blocks from this peer.
    ///
    /// Dispatches to the correct [`PeerRequest`] variant for the negotiated
    /// ETH version and normalises the response to `Vec<Vec<Receipt>>`.
    ///
    /// ETH version  Response wrapper                         Bloom present?
    /// ────────────────────────────────────────────────────────────────────
    ///  eth/66-68   Receipts<T>   = Vec<Vec<ReceiptWithBloom<T>>>   yes
    ///  eth/69      Receipts69<T> = Vec<Vec<T>>                      no
    ///  eth/70      Receipts70<T> → Into<Receipts<T>>                yes
    pub async fn fetch_receipts(
        &self,
        block_hashes: Vec<B256>,
    ) -> Result<Vec<Vec<Receipt>>, PeerError> {
        let peer_id = self.info.id;

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

/// Spin up a [`NetworkManager`] connected to mainnet and return a
/// [`NetworkHandle`].
pub async fn start_network() -> Result<NetworkHandle, AppError> {
    let secret_key = rng_secret_key();

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

/// Dial a specific peer by [`NodeRecord`].
pub fn add_peer(handle: &NetworkHandle, node: NodeRecord) {
    handle.add_peer(node.id, node.tcp_addr());
}
