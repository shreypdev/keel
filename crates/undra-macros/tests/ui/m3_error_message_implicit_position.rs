//! M3: `{}` without a position or a name is rejected: the schema keeps the message text and the
//! platforms resolve placeholders by index or name.
#![allow(unused)]

use undra_macros as k;

#[k::error]
pub enum Failure {
    #[error("pair {} and {}")]
    Pair(u8, u8),
    #[error("one {0}")]
    One(u8),
}

fn main() {}
