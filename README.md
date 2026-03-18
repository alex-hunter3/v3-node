# V3 Node

**V3 Node** is a lightweight Rust-based node that connects directly to **EVM peer-to-peer networks** to monitor and track **Uniswap V3–style decentralised exchange pools**. It performs historical synchronisation from the contract creation block, and maintains an up-to-date local state database using event logs.

The node continuously listens for new state updates and relays them as they occur, making it suitable for analytics pipelines, trading infrastructure, and on-chain monitoring systems.

---

## Features

- **Direct EVM P2P connectivity**  
  Connects directly to Ethereum-compatible networks without relying on centralised indexers.

- **Automatic pool discovery**  
  Reads a configuration file containing Uniswap V3–style DEX pool metadata and discovers the corresponding on-chain data.

- **Historical synchronisation**  
  Detects the pool creation block and synchronises historical state by scanning event receipts.

- **SQLite state storage**  
  All indexed events and derived state are persisted in a local SQLite database for efficient querying.

- **Real-time updates**  
  After synchronisation, the node listens for and relays new pool state updates as they occur.

- **Mempool access**  
  When listening to a pool, receive updates about swaps that are to take place in the future on the watched pools.   

- **Rust performance and safety**  
  Built with Rust for high performance, memory safety, and reliability.

---

## Architecture

        +----------------------+
        |      Config File     |
        |  (DEX Pool Metadata) |
        +----------+-----------+
                   |
                   v
        +----------------------+
        |    Poll Peers for    |
        |     Pool Address     |
        +----------+-----------+
                   |
                   v
        +----------------------+
        |  Historical Sync     |
        |  Event Receipt Scan  |
        +----------+-----------+
                   |
                   v
        +----------------------+
        |      SQLite DB       |
        |  Indexed Pool State  |
        +----------+-----------+
                   |
                   v
        +----------------------+
        |   Real-time Relay    |
        |  New State Updates   |
        +----------------------+

---

---

## File Structure

```
v3-node/
├── Cargo.toml
├── README.MD
├── .gitignore
│
└── src/
├── lib.rs # Public library root — re-exports the public API
├── main.rs # Binary entry point — thin, just wires config + calls lib
│
├── config/
│ ├── mod.rs
│ └── types.rs # Config structs (PoolConfig, NodeConfig, etc.)
│
├── network/
│ ├── mod.rs
│ ├── peer.rs # P2P peer discovery & connection logic
│ └── manager.rs # Manages all peer events after connection
│
├── node/
│ ├── mod.rs
│ └── node.rs # High-level orchestrator — coordinates network, sync, pools, and DB
│
├── sync/
│ ├── mod.rs
│ ├── historical.rs # Block-range event receipt scanning
│ └── live.rs # New-block subscription & real-time relay
│
├── pool/
│ ├── mod.rs
│ ├── events.rs # Swap / Mint / Burn / Initialise event decoding
│ ├── state.rs # Derived pool state (price, liquidity, tick, etc.)
│ └── math.rs # V3 tick/price/liquidity math (sqrt price, etc.)
│
├── db/
│ ├── mod.rs
│ ├── schema.rs # SQLite table definitions / migrations
│ ├── queries.rs # Read/write query helpers
│ └── models.rs # Rust structs that map to DB rows
│
└── error.rs # Unified error type (thiserror)
```

---

## How It Works

1. **Load pools**  
   The node reads a file containing all the Uniswap V3–style pools to be monitored along with their contract creation block.

2. **Historical sync**  
   Event receipts are fetched from the creation block up to the latest block.

3. **State persistence**  
   Events and derived state are stored in a local SQLite database.

4. **Live monitoring**  
   The node subscribes to new blocks and processes pool events in real time.

---

## Installation

### Prerequisites

- Rust (latest stable)
- Cargo

### Build

```bash
git clone https://github.com/alex-hunter3/v3-node.git
cd v3-node
cargo build --release
```
