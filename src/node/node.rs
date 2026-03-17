use tokio::sync::broadcast;

use crate::network::manager::{ManagerEvent, PeerManager, PeerManagerHandle};

#[derive(Debug)]
pub struct Node {
    // evm: // coming for simulations
    // pools // unimplemented
    manager: PeerManager,
    mgr_handler: PeerManagerHandle,
    peer_events: broadcast::Receiver<ManagerEvent>,
}

impl Node {
    pub fn new(
        manager: PeerManager,
        mgr_handler: PeerManagerHandle,
        peer_events: broadcast::Receiver<ManagerEvent>,
    ) -> Self {
        Self {
            manager,
            mgr_handler,
            peer_events,
        }
    }

    pub async fn start(self) {
        tokio::spawn(self.manager.run());

        let mut evts = self.peer_events;
        loop {
            match evts.recv().await {
                Ok(ManagerEvent::PeerConnected(info)) => {
                    let num_peers = self.mgr_handler.connected_peers().await.unwrap_or_default();
                    println!(
                        "✓ peer connected | addr={} eth={:?} client={:?} connected_peers={}",
                        info.remote_addr,
                        info.eth_version,
                        info.client_version,
                        num_peers.len()
                    );

                    // Kick off historical sync
                    // tokio::spawn(historical_sync(mgr_handle.clone(), pools.clone()));
                }

                Ok(ManagerEvent::PeerDisconnected(info, reason)) => {
                    let num_peers = self.mgr_handler.connected_peers().await.unwrap_or_default();
                    println!(
                        "✗ peer disconnected | addr={} eth={:?} client={:?} connected_peers={} reason={}",
                        info.remote_addr,
                        info.eth_version,
                        info.client_version,
                        num_peers.len(),
                        reason.unwrap_or_default()
                    );
                }

                // The broadcast channel will return Lagged if this consumer
                // falls behind — just log and continue rather than crashing.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    eprintln!("warn: event receiver lagged, skipped {n} events");
                }

                // Sender dropped — manager task exited, nothing left to do.
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    eprintln!("error: peer manager stopped unexpectedly");
                    break;
                }
            }
        }
    }
}
