use clap::Parser;
use std::path::PathBuf;

use v3_node::config::load_pools;

#[derive(Parser)]
#[command(
    name = "v3-node",
    about = "Listens to EVM P2P networks for Uniswap V3 pool state changes"
)]
struct Cli {
    /// Path to the JSON file containing pool configurations
    #[arg(long)]
    pools: PathBuf,
}

fn main() {
    let cli = Cli::parse();

    let pools = load_pools(&cli.pools).expect("Failed to load pools config");
    println!("Loaded {} pool(s)", pools.len());

    // v3_node::run(pools) -- will wire up once core logic exists
}
