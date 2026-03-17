use clap::Parser;

use rusqlite::Connection;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use v3_node::config::load_pools;
use v3_node::config::types::Database;
use v3_node::db::schema;
use v3_node::network::manager::PeerManager;
use v3_node::network::manager::start_network;
use v3_node::node::node::Node;

#[derive(Parser)]
#[command(
    name = "v3-node",
    about = "Listens to EVM P2P networks for Uniswap V3 style pool state changes"
)]
struct Cli {
    #[arg(long, default_value = "data/v3_node.db")]
    db: PathBuf,

    #[arg(long, default_value = "data/pools.json")]
    pools: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let pools = load_pools(&cli.pools).unwrap();
    println!("Loaded {} pool(s)", pools.len());

    let conn = Connection::open(&cli.db)?;
    schema::run_migrations(&conn)?;

    let db: Database = Arc::new(Mutex::new(conn));

    let network_handle = start_network().await?;
    let (manager, mgr_handle, events) = PeerManager::new(network_handle);
    let node = Node::new(manager, mgr_handle, events, db, pools);

    node.start().await;

    Ok(())
}
