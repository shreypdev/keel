//! ADR-042 decision 4.2: only `DateTime<Utc>` crosses. A local time or an offset is E0001 ("convert
//! to `Utc`; offsets do not cross") whether or not the `chrono` feature is on.
#![allow(unused)]

use undra_macros as k;

pub struct Local;
pub struct FixedOffset;
pub struct DateTime<Tz>(pub Tz);

#[k::api]
pub struct Event {
    pub local: DateTime<Local>,
    pub offset: DateTime<FixedOffset>,
}

fn main() {}
