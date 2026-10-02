//! ADR-042 decision 4.3: a type that only `chrono` or `time` has, spelled without the feature of
//! `undra`, is ONE error that names the feature (not the three the compiler adds for a type that
//! implements neither `Encode`, `Decode` nor `WireLeaf`). The types here are stand-ins: the macro
//! reads the spelling.
#![allow(unused)]

use undra_macros as k;

pub struct Utc;
pub struct DateTime<Tz>(pub Tz);
pub struct OffsetDateTime;
pub struct UtcDateTime;
pub struct TimeDelta;

#[k::api]
pub struct Event {
    pub at: DateTime<Utc>,
    pub opened: OffsetDateTime,
    pub closed: UtcDateTime,
    pub took: TimeDelta,
}

fn main() {}
