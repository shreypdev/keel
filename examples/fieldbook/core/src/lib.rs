//! The core of Fieldbook, the sample app of Undra: field notes with photos.
//!
//! A small but real app, written the way an application writes its core, with nothing but the
//! public `undra` API (constitution R10), and shared by the web, iOS and Android apps in
//! `examples/fieldbook/`. It puts the pieces of the cookbook (`examples/cookbook`) together:
//!
//! | Module | What it shows |
//! |---|---|
//! | [`net`] | where the server is; typed network errors |
//! | [`auth`] | a session store, tokens in `SecureStore`, one re-auth on a 401, a sign-out that refuses to lose queued writes |
//! | [`notes`] | a local-first notebook: a keyed list, a derived view (filter, pinned first, newest first), a tag set, photos in `Fs`, every change a queued idempotent mutation, and an update (`#[undra(default)]`, `#[undra::migrate]`) |
//! | `presence` | who else is in the notebook, over a WebSocket that reconnects in the core (feature `presence`: it needs the opt-in `WebSocket` port) |
//!
//! The core reads no clock and no random source and starts no thread (R12): time is the `Clock`
//! and `Timer` ports, the network the `Http` port, secrets the `SecureStore` port, notes the `Kv`
//! port and photos the `Fs` port, so every test in this crate runs against `undra::ports::fakes`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod auth;
pub mod net;
pub mod notes;
#[cfg(feature = "presence")]
pub mod presence;

pub use auth::{Auth, AuthError, Session, authed};
pub use net::{NetError, ServerConfig, configure_server};
pub use notes::{
    DeleteRemoteNoteMutation, Filter, MAX_TITLE, Note, NoteError, Notebook, Outbox,
    PushNoteMutation, PushPhotoMutation, delete_remote_note, outbox, push_note, push_photo,
    read_photo,
};
#[cfg(feature = "presence")]
pub use presence::{Link, Member, Presence};
