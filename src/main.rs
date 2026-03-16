use clap::Parser;
use std::path::PathBuf;
use v3_node::config::load_pools;
use v3_node::network::{
    manager::{ManagerEvent, PeerManager},
    peer::start_network,
};

#[derive(Parser)]
#[command(
    name = "v3-node",
    about = "Listens to EVM P2P networks for Uniswap V3 style pool state changes"
)]
struct Cli {
    #[arg(long)]
    pools: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let pools = load_pools(&cli.pools).unwrap();
    println!("Loaded {} pool(s)", pools.len());

    // 1. Spin up the reth NetworkManager and get a handle to it.
    //    This opens the TCP listener and starts discv4 peer discovery
    //    in a background task automatically.
    let network_handle = start_network().await?;

    // 2. Build the PeerManager, which subscribes to raw network events
    //    before the first poll so no session is missed.
    //    - `manager`     — the actor, must be spawned
    //    - `mgr_handle`  — cheap to clone, use this everywhere else
    //    - `events`      — broadcast receiver for peer lifecycle events
    let (manager, _mgr_handle, mut events) = PeerManager::new(network_handle);

    // 3. Spawn the manager's event loop as a background task.
    tokio::spawn(manager.run());

    // 4. Log peer connections / disconnections as they arrive.
    //    In a real node you'd fan this out to your sync and pool
    //    monitor tasks instead.
    loop {
        match events.recv().await {
            Ok(ManagerEvent::PeerConnected(info)) => {
                println!(
                    "✓ peer connected | id={} addr={} eth={:?} client",
                    info.id, info.remote_addr, info.eth_version,
                );

                // Kick off historical sync
                // tokio::spawn(historical_sync(mgr_handle.clone(), pools.clone()));
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

            _ => {}
        }
    }

    Ok(())
}
