//! E0071 (ADR-041): the methods of a callback interface report or are async with a `Result`.
#![allow(unused)]

use undra_macros as k;

#[k::error]
pub enum Oops {
    #[error("oops")]
    Oops,
}

impl From<undra::runtime::PortError> for Oops {
    fn from(_: undra::runtime::PortError) -> Self {
        Oops::Oops
    }
}

#[k::callback]
pub trait Provider {
    fn token(&self) -> String;
    async fn ask(&self);
    async fn count(&self) -> u32;
    #[undra(coalesce)]
    async fn confirm(&self) -> Result<bool, Oops>;
    fn __release(&self);
    #[undra(coalesce)]
    fn fine(&self, progress: u32);
}

fn main() {}
