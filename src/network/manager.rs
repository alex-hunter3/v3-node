//! Peer lifecycle management and message routing.
//!
//! # Generic parameter note
//!
//! `NetworkHandle::event_listener()` returns
//! `EventStream<NetworkEvent<PeerRequest<EthNetworkPrimitives>>>`.
//! The `NetworkEvent<N>` generic is `PeerRequest<N>`, **not** the bare
//! primitives type.  This flows through to `ActivePeerSession.messages` which
//! is `PeerRequestSender<PeerRequest<EthNetworkPrimitives>>`, meaning
//! `try_send` correctly accepts `PeerRequest<EthNetworkPrimitives>` values.
//!
//! # Usage
//!
//! ```rust,ignore
//! let handle = start_network().await?;
//! let (manager, mgr_handle, mut events) = PeerManager::new(handle);
//!
//! tokio::spawn(manager.run());
//!
//! while let Ok(ev) = events.recv().await {
//!     match ev {
//!         ManagerEvent::PeerConnected(info)  => { /* ... */ }
//!         ManagerEvent::PeerDisconnected(id) => { /* ... */ }
//!     }
//! }
//! ```

use std::collections::HashMap;

use alloy::primitives::B256;
use futures::StreamExt;
use reth_network::{
    DisconnectReason, EthNetworkPrimitives, NetworkEventListenerProvider, NetworkHandle,
    events::{NetworkEvent, PeerEvent, PeerRequest, PeerRequestSender, SessionInfo},
};
// use reth_network_peers::AnyNode::PeerId;
use reth_network_peers::PeerId;
use reth_primitives::Receipt;
use reth_tokio_util::EventStream;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::{
    error::AppError,
    network::peer::{Peer, PeerError, PeerInfo},
};

// ─── Public event type ───────────────────────────────────────────────────────

/// Events broadcast to downstream consumers whenever the set of live peers
/// changes or an important protocol event occurs.
#[derive(Debug, Clone)]
pub enum ManagerEvent {
    /// A new peer session has been established and registered.
    PeerConnected(PeerInfo),
    /// A peer has been removed (session closed or connection dropped).
    PeerDisconnected(PeerInfo, Option<DisconnectReason>),
}

// ─── Internal command type ───────────────────────────────────────────────────

/// Commands sent *to* the [`PeerManager`] task over the command channel.
enum Command {
    FetchReceipts {
        peer_id: Option<PeerId>,
        block_hashes: Vec<B256>,
        reply: oneshot::Sender<Result<Vec<Vec<Receipt>>, PeerError>>,
    },
    ConnectedPeers {
        reply: oneshot::Sender<Vec<PeerInfo>>,
    },
}

// ─── PeerManagerHandle ───────────────────────────────────────────────────────

/// Handle to the background [`PeerManager`] task. Cheap to clone.
#[derive(Debug, Clone)]
pub struct PeerManagerHandle {
    cmd_tx: mpsc::Sender<Command>,
}

impl PeerManagerHandle {
    /// Fetch receipts from a *specific* peer.
    pub async fn fetch_receipts_from(
        &self,
        peer_id: PeerId,
        block_hashes: Vec<B256>,
    ) -> Result<Vec<Vec<Receipt>>, PeerError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(Command::FetchReceipts {
                peer_id: Some(peer_id),
                block_hashes,
                reply: tx,
            })
            .await
            .map_err(|_| PeerError::Disconnected(peer_id))?;

        rx.await
            .map_err(|_| PeerError::ResponseChannelClosed(peer_id))?
    }

    /// Fetch receipts from *any* available peer (round-robin with fallback).
    pub async fn fetch_receipts(
        &self,
        block_hashes: Vec<B256>,
    ) -> Result<Vec<Vec<Receipt>>, AppError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(Command::FetchReceipts {
                peer_id: None,
                block_hashes,
                reply: tx,
            })
            .await
            .map_err(|_| AppError::Network("manager task is gone".into()))?;

        rx.await
            .map_err(|_| AppError::Network("receipt reply channel dropped".into()))?
            .map_err(|e| AppError::Network(e.to_string()))
    }

    /// Return a snapshot of metadata for every currently connected peer.
    pub async fn connected_peers(&self) -> Result<Vec<PeerInfo>, AppError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(Command::ConnectedPeers { reply: tx })
            .await
            .map_err(|_| AppError::Network("manager task is gone".into()))?;
        rx.await
            .map_err(|_| AppError::Network("peer list reply channel dropped".into()))
    }

    /// Number of currently connected peers.
    pub async fn peer_count(&self) -> Result<usize, AppError> {
        Ok(self.connected_peers().await?.len())
    }
}

// ─── PeerManager (actor) ─────────────────────────────────────────────────────
#[derive(Debug)]
pub struct PeerManager {
    peers: HashMap<PeerId, Peer>,
    rr_index: usize,
    events: EventStream<NetworkEvent<PeerRequest<EthNetworkPrimitives>>>,
    cmd_rx: mpsc::Receiver<Command>,
    event_tx: broadcast::Sender<ManagerEvent>,
}

impl PeerManager {
    /// Create a new [`PeerManager`] bound to `handle`.
    ///
    /// Returns the manager (to be spawned), a handle for callers, and the
    /// first broadcast receiver for lifecycle events.
    pub fn new(
        handle: NetworkHandle,
    ) -> (Self, PeerManagerHandle, broadcast::Receiver<ManagerEvent>) {
        let (cmd_tx, cmd_rx) = mpsc::channel(128);
        let (event_tx, event_rx) = broadcast::channel(256);

        // Subscribe before the first poll — actual type is
        // EventStream<NetworkEvent<PeerRequest<EthNetworkPrimitives>>>.
        let events = handle.event_listener();

        let manager = Self {
            peers: HashMap::new(),
            rr_index: 0,
            events,
            cmd_rx,
            event_tx,
        };

        (manager, PeerManagerHandle { cmd_tx }, event_rx)
    }

    /// Clone the broadcast sender for additional downstream subscribers.
    pub fn event_tx(&self) -> broadcast::Sender<ManagerEvent> {
        self.event_tx.clone()
    }

    // ── Main event loop ───────────────────────────────────────────────────

    pub async fn run(mut self) {
        println!("Connecting to peers...");
        loop {
            tokio::select! {
                // EventStream implements Stream — poll with StreamExt::next().
                maybe_event = self.events.next() => {
                    match maybe_event {
                        Some(event) => self.handle_network_event(event),
                        None => {
                            break;
                        }
                    }
                }

                maybe_cmd = self.cmd_rx.recv() => {
                    match maybe_cmd {
                        Some(cmd) => self.handle_command(cmd).await,
                        None => {
                            break;
                        }
                    }
                }
            }
        }
    }

    // ── Network event dispatch ────────────────────────────────────────────

    fn handle_network_event(&mut self, event: NetworkEvent<PeerRequest<EthNetworkPrimitives>>) {
        match event {
            NetworkEvent::ActivePeerSession { info, messages } => {
                self.on_peer_connected(info, messages);
            }

            NetworkEvent::Peer(evt) => match evt {
                PeerEvent::SessionClosed { peer_id, reason } => {
                    if self.peers.contains_key(&peer_id) {
                        // let info = self.peers.get(&peer_id).unwrap().info();
                        // println!(
                        //     "✗ peer disconnected | id={} addr={} eth={:?} reason={}",
                        //     info.id,
                        //     info.remote_addr,
                        //     info.eth_version,
                        //     reason.unwrap_or_default()
                        // );
                        self.on_peer_removed(peer_id, reason);
                    }
                }
                PeerEvent::PeerRemoved(peer_id) => {
                    if self.peers.contains_key(&peer_id) {
                        // let info = self.peers.get(&peer_id).unwrap().info();
                        // println!(
                        //     "✗ peer removed | id={} addr={} eth={:?}",
                        //     info.id, info.remote_addr, info.eth_version
                        // );
                        self.on_peer_removed(peer_id, None);
                    }
                }
                PeerEvent::PeerAdded(_peer_id) => {
                    // println!("id: {} peer added (awaiting handshake)", peer_id);
                }
                _ => {}
            },
        }
    }

    fn on_peer_connected(
        &mut self,
        info: SessionInfo,
        sender: PeerRequestSender<PeerRequest<EthNetworkPrimitives>>,
    ) {
        let peer = Peer::from_session(info, sender);
        let peer_info = peer.info();
        let peer_id = peer.id();

        self.peers.insert(peer_id, peer);
        let _ = self.event_tx.send(ManagerEvent::PeerConnected(peer_info));
    }

    fn on_peer_removed(&mut self, peer_id: PeerId, reason: Option<DisconnectReason>) {
        let info = self.peers.get(&peer_id).unwrap().info();
        if self.peers.remove(&peer_id).is_some() {
            let _ = self.event_tx.send(ManagerEvent::PeerDisconnected(info, reason));
        }
    }

    // ── Command dispatch ──────────────────────────────────────────────────

    async fn handle_command(&mut self, cmd: Command) {
        match cmd {
            Command::FetchReceipts {
                peer_id,
                block_hashes,
                reply,
            } => {
                let result = self.do_fetch_receipts(peer_id, block_hashes).await;
                let _ = reply.send(result);
            }
            Command::ConnectedPeers { reply } => {
                let _ = reply.send(self.peers.values().map(|p| p.info()).collect());
            }
        }
    }

    // ── Receipt dispatch ──────────────────────────────────────────────────

    async fn do_fetch_receipts(
        &mut self,
        target: Option<PeerId>,
        block_hashes: Vec<B256>,
    ) -> Result<Vec<Vec<Receipt>>, PeerError> {
        if self.peers.is_empty() {
            return Err(PeerError::Disconnected(PeerId::default()));
        }

        match target {
            Some(peer_id) => match self.peers.get(&peer_id) {
                Some(peer) => peer.fetch_receipts(block_hashes).await,
                None => Err(PeerError::Disconnected(peer_id)),
            },

            None => {
                let peer_ids: Vec<PeerId> = self.peers.keys().copied().collect();
                let total = peer_ids.len();
                self.rr_index = self.rr_index % total;

                let mut last_err = PeerError::Disconnected(PeerId::default());
                let mut to_evict: Vec<PeerId> = Vec::new();

                for attempt in 0..total {
                    let idx = (self.rr_index + attempt) % total;
                    let peer_id = peer_ids[idx];

                    if let Some(peer) = self.peers.get(&peer_id) {
                        match peer.fetch_receipts(block_hashes.clone()).await {
                            Ok(receipts) => {
                                self.rr_index = (idx + 1) % total;
                                for id in to_evict {
                                    self.on_peer_removed(id, None);
                                }
                                return Ok(receipts);
                            }
                            Err(e) => {
                                // warn!(
                                //     target: "network::manager",
                                //     %peer_id,
                                //     error = %e,
                                //     "receipt request failed, trying next peer",
                                // );
                                if matches!(e, PeerError::Disconnected(_)) {
                                    to_evict.push(peer_id);
                                }
                                last_err = e;
                            }
                        }
                    }
                }

                for id in to_evict {
                    self.on_peer_removed(id, None);
                }

                Err(last_err)
            }
        }
    }

    // new function
    pub async fn peer_info(&self, peer_id: PeerId) -> Result<&Peer, anyhow::Error> {
        let result = self
            .peers
            .get(&peer_id)
            .ok_or_else(|| AppError::Network(format!("peer {peer_id} not found")))?;

        Ok(result)
    }
}
