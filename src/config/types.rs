use std::sync::{Arc, Mutex};

use rusqlite::Connection;

pub type Database = Arc<Mutex<Connection>>;
