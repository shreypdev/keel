#![allow(unused)]

use keel_macros as k;

#[k::api]
pub struct Profile {
    pub name: &str,
    pub visits: usize,
}

pub struct Session;

#[k::api]
impl Session {
    pub fn touch(&self, ctx: &Ctx) {}
}

fn main() {}
