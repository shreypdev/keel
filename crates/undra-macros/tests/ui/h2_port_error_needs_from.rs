//! H2/E0033: a port method that returns `Result<T, E>` reports an unavailable port, a cancelled
//! call and an undecodable reply as `E`, so `E` needs `From<PortError>`. A method without an
//! error channel needs nothing (it panics with a message that says how to bind the port).
#![allow(unused)]

use undra::prelude::Bytes;
use undra_macros as k;

#[k::error]
#[derive(Clone)]
pub enum StoreError {
    #[error("offline")]
    Offline,
}

#[k::port]
pub trait Store {
    async fn load(&self, key: String) -> Result<Bytes, StoreError>;
    async fn save(&self, key: String, value: Bytes) -> Result<(), StoreError>;
    // No error channel: nothing to implement for this one.
    async fn exists(&self, key: String) -> bool;
}

fn main() {}
