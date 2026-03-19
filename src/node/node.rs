use std::sync::Arc;

use alloy::consensus::{Transaction, transaction::SignerRecoverable};
use reth_eth_wire::PooledTransactions;
use reth_network::{EthNetworkPrimitives, transactions::NetworkTransactionEvent};
use reth_network_peers::PeerId;
use tokio::{
    sync::{
        broadcast::{self, error::RecvError},
        mpsc::UnboundedReceiver,
    },
    task,
};

use crate::{
    config::{PoolConfig, types::Database},
    error::AppError,
    network::{
        manager::{ManagerEvent, PeerManagerHandle},
        peer::PeerInfo,
    },
    pool::decoder::decode_pool_calldata,
};

#[derive(Debug)]
pub struct Node {
    // evm: // coming for simulations
    mgr_handler: PeerManagerHandle,
    peer_events: broadcast::Receiver<ManagerEvent>,
    pool_config: Vec<PoolConfig>,
    _db: Database,
}

impl Node {
    pub fn new(
        mgr_handler: PeerManagerHandle,
        peer_events: broadcast::Receiver<ManagerEvent>,
        db: Database,
        pool_config: Vec<PoolConfig>,
    ) -> Arc<Self> {
        Arc::new(Self {
            mgr_handler,
            peer_events,
            pool_config,
            _db: db,
        })
    }

    async fn handle_pool_sync(&self) {}

    async fn connected_peers(&self) -> Result<Vec<PeerInfo>, AppError> {
        self.mgr_handler.connected_peers().await
    }

    async fn handle_tx_event(&self, event: NetworkTransactionEvent<EthNetworkPrimitives>) {
        match event {
            NetworkTransactionEvent::IncomingTransactions { peer_id: _, msg } => {
                // Full transactions gossiped directly — scan for pool interactions
                for mut tx in msg.0 {
                    // Check if this tx is directed at a watched pool
                    let Some(to) = tx.to() else { continue };

                    if let Some(pool) = self
                        .pool_config
                        .iter()
                        .find(|p| p.address == to.to_string())
                    {
                        println!(
                            "🎯 pool interaction | pool={} hash={} from={:?}",
                            pool.address,
                            tx.hash(),
                            tx.recover_signer(),
                        );

                        if let Some(decoded) = decode_pool_calldata(tx.input_mut()) {
                            println!("   └─ {:?}", decoded);
                        }
                    }
                }
            }

            NetworkTransactionEvent::IncomingPooledTransactionHashes { peer_id, msg } => {
                // Peer announced tx hashes — you can request full txs if interested
                println!("📋 {} pooled tx hashes from {}", msg.len(), short_id(&peer_id));
                // TODO: filter for txs to watched pools, request full txs
            }

            NetworkTransactionEvent::GetPooledTransactions {
                peer_id: _,
                request: _,
                response,
            } => {
                // Peer is asking us for txs — you don't have a pool so just drop
                let _ = response.send(Ok(PooledTransactions(vec![])));
            }

            NetworkTransactionEvent::GetTransactionsHandle(_) => {}
        }
    }

    async fn handle_peer_event(
        &self,
        evt: Result<ManagerEvent, tokio::sync::broadcast::error::RecvError>,
    ) {
        match evt {
            Ok(ManagerEvent::PeerConnected(peer)) => {
                let num_peers = match self.connected_peers().await {
                    Ok(peers) => peers.len(),
                    Err(_) => return,
                };
                println!(
                    "✓ peer connected | id={} eth={:?} client={:?} connected_peers={}",
                    short_id(&peer.id), peer.eth_version, peer.client_version, num_peers
                );

                self.handle_pool_sync().await;
            }

            Ok(ManagerEvent::PeerDisconnected(peer, reason)) => {
                let num_peers = match self.connected_peers().await {
                    Ok(peers) => peers.len(),
                    Err(_) => return,
                };
                println!(
                    "✗ peer disconnected | id={} eth={:?} client={:?} connected_peers={} reason={}",
                    short_id(&peer.id),
                    peer.eth_version,
                    peer.client_version,
                    num_peers,
                    reason.unwrap_or_default()
                );
            }

            Err(RecvError::Lagged(n)) => {
                eprintln!("warn: peer events receiver lagged, skipped {n} events");
            }

            Err(RecvError::Closed) => {
                eprintln!("error: peer manager stopped unexpectedly");
                // break;
            }

            _ => {}
        }
    }

    pub async fn start(
        self: Arc<Self>,
        mut tx_events: UnboundedReceiver<NetworkTransactionEvent<EthNetworkPrimitives>>,
    ) {
        let mut peer_evts = self.peer_events.resubscribe();

        loop {
            tokio::select! {
                peer_evt = peer_evts.recv() => {
                    let this = Arc::clone(&self);
                    task::spawn(async move {
                        this.handle_peer_event(peer_evt).await;
                    });
                }

                tx_evt = tx_events.recv() => {
                    if let Some(tx_evt) = tx_evt {
                        let this = Arc::clone(&self);
                        task::spawn(async move {
                            this.handle_tx_event(tx_evt).await;
                        });
                    }
                }
            }
        }
    }
}

fn short_id(id: &PeerId) -> String {
    let s = format!("{id:x}");
    format!("{}…{}", &s[..8], &s[s.len() - 8..])
}
