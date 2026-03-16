use clap::Parser;
use v3_node::config::load_pools;
use std::path::PathBuf;

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
    tracing_subscriber::fmt().with_env_filter("info").init();

    let cli = Cli::parse();

    let pools = load_pools(&cli.pools).unwrap();
    println!("Loaded {} pool(s)", pools.len());

    Ok(())
}
