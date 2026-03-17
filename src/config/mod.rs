pub mod types;

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenConfig {
    pub decimals: u8,
    pub address: String,
    pub symbol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolConfig {
    pub fee_tier: u32,
    pub address: String,
    pub token0: TokenConfig,
    pub token1: TokenConfig,
    pub router: String,
    pub exchange: String,
    pub creation_block: u64,
}

pub fn load_pools(path: &Path) -> Result<Vec<PoolConfig>, Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(path)?;
    let pools: Vec<PoolConfig> = serde_json::from_str(&contents)?;
    Ok(pools)
}
