//! Objects as parameters and returns (ADR-040): the generated dispatchers hand out one owned
//! host reference per handle in a reply, intern an object to one handle, resolve object
//! parameters before the method runs and refuse stale or wrongly typed ones with a reason.
#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex};

use undra::meta::{PortKind, TypeRef, collect_schema, ids};
use undra::runtime::{Ctx, Handle, WeakCtx};
use undra::signals::Signal;
use undra::wire::{Decode, Encode, Reader, Writer};
use undra_macros as k;

mod support;
use support::Runtime;

fn args(encode: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    encode(&mut w);
    w.into_vec()
}

fn handle_of(bytes: &[u8]) -> u64 {
    u64::decode_exact(bytes).expect("a handle")
}

#[k::error]
#[derive(Clone, PartialEq)]
pub enum MailError {
    #[error("no such thread")]
    NoThread,
}

/// A folder.
pub struct Mailbox {
    name: String,
}

#[k::api]
impl Mailbox {
    pub fn name(&self) -> String {
        self.name.clone()
    }
}

/// A conversation.
pub struct Thread {
    id: u32,
}

#[k::api]
impl Thread {
    pub fn id(&self) -> u32 {
        self.id
    }
}

/// A child store: it has signals the platform mirrors.
#[k::store]
pub struct ChatStore {
    pub messages: Signal<u32>,
    peer: u32,
}

#[k::api(store)]
impl ChatStore {
    pub fn new(_ctx: Ctx) -> Self {
        ChatStore {
            messages: Signal::new(0),
            peer: 0,
        }
    }

    pub fn send(&self) {
        self.messages.update(|n| *n += 1);
    }

    pub fn peer(&self) -> u32 {
        self.peer
    }
}

/// An account that hands out children.
pub struct Account {
    inbox: Arc<Mailbox>,
    sent: Arc<Mailbox>,
    chat: Arc<ChatStore>,
    moved: Mutex<Vec<String>>,
    _ctx: WeakCtx,
}

#[k::api]
impl Account {
    pub fn new(ctx: Ctx) -> Self {
        Account {
            inbox: Arc::new(Mailbox { name: "inbox".into() }),
            sent: Arc::new(Mailbox { name: "sent".into() }),
            chat: Arc::new(ChatStore::new(ctx.clone())),
            moved: Mutex::new(Vec::new()),
            _ctx: ctx.downgrade(),
        }
    }

    /// The mailbox for `folder`.
    pub fn mailbox(&self, folder: String) -> Arc<Mailbox> {
        if folder == "sent" {
            Arc::clone(&self.sent)
        } else {
            Arc::clone(&self.inbox)
        }
    }

    pub fn drafts(&self, exists: bool) -> Option<Arc<Mailbox>> {
        exists.then(|| Arc::clone(&self.inbox))
    }

    pub fn mailboxes(&self) -> Vec<Arc<Mailbox>> {
        vec![Arc::clone(&self.inbox), Arc::clone(&self.sent), Arc::clone(&self.inbox)]
    }

    pub async fn open_thread(&self, id: u32) -> Result<Arc<Thread>, MailError> {
        std::future::ready(()).await;
        if id == 0 {
            Err(MailError::NoThread)
        } else {
            Ok(Arc::new(Thread { id }))
        }
    }

    pub fn chat(&self) -> Arc<ChatStore> {
        Arc::clone(&self.chat)
    }

    pub fn move_to(&self, message: u32, target: &Mailbox) {
        self.moved.lock().unwrap().push(format!("{message}->{}", target.name));
    }

    pub fn move_to_shared(&self, target: Arc<Mailbox>) -> String {
        target.name.clone()
    }

    pub fn move_maybe(&self, target: Option<&Mailbox>) -> String {
        target.map_or_else(|| "none".to_owned(), |t| t.name.clone())
    }

    pub fn move_maybe_shared(&self, target: Option<Arc<Mailbox>>) -> String {
        target.map_or_else(|| "none".to_owned(), |t| t.name.clone())
    }

    pub fn names(&self, boxes: Vec<Arc<Mailbox>>) -> String {
        boxes.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(",")
    }

    pub async fn rename_later(&self, target: &Mailbox) -> String {
        std::future::ready(()).await;
        format!("{}!", target.name)
    }

    pub fn moved(&self) -> Vec<String> {
        self.moved.lock().unwrap().clone()
    }
}

/// A process-wide singleton object, constructed through `Arc<Self>`.
pub struct Registry;

static REGISTRY: std::sync::OnceLock<Arc<Registry>> = std::sync::OnceLock::new();

#[k::api]
impl Registry {
    pub fn shared() -> Arc<Self> {
        Arc::clone(REGISTRY.get_or_init(|| Arc::new(Registry)))
    }

    pub fn id(&self) -> u32 {
        1
    }
}

/// A free function that takes and returns objects.
#[k::api]
pub fn same_box(target: Arc<Mailbox>) -> Arc<Mailbox> {
    target
}

fn refs(rt: &Runtime, handle: u64) -> Option<u32> {
    rt.real().objects().host_refs_of(Handle(handle))
}

fn account(rt: &Runtime) -> u64 {
    handle_of(&rt.call_object("Account", "new", 0, &[]).sync_ok())
}

#[test]
fn the_schema_names_objects_by_object_references() {
    let schema = collect_schema("test");
    let account = schema.objects.iter().find(|o| o.name == "Account").unwrap();
    let method = |name: &str| account.methods.iter().find(|m| m.name == name).unwrap();
    let mailbox = TypeRef::object("Mailbox");
    assert_eq!(method("mailbox").returns, mailbox);
    assert_eq!(method("drafts").returns, TypeRef::option(mailbox.clone()));
    assert_eq!(method("mailboxes").returns, TypeRef::vec(mailbox.clone()));
    assert_eq!(
        method("open_thread").returns,
        TypeRef::result(TypeRef::object("Thread"), TypeRef::named("MailError"))
    );
    assert_eq!(method("chat").returns, TypeRef::object("ChatStore"));
    for (name, ty) in [
        ("move_to", mailbox.clone()),
        ("move_to_shared", mailbox.clone()),
        ("move_maybe", TypeRef::option(mailbox.clone())),
        ("move_maybe_shared", TypeRef::option(mailbox.clone())),
        ("names", TypeRef::vec(mailbox.clone())),
    ] {
        let params = &method(name).params;
        assert_eq!(params.last().unwrap().ty, ty, "{name}");
    }
    // A constructor still returns its own object as `Named` (hash stability), `Arc<Self>` too.
    let registry = schema.objects.iter().find(|o| o.name == "Registry").unwrap();
    assert_eq!(registry.constructors[0].returns, TypeRef::named("Registry"));
    let account_ctor = &account.constructors[0];
    assert_eq!(account_ctor.returns, TypeRef::named("Account"));
    let function = schema.functions.iter().find(|f| f.name == "same_box").unwrap();
    assert_eq!(function.params[0].ty, mailbox);
    assert_eq!(function.returns, mailbox);
    assert_eq!(schema.validate(), Ok(()));
    assert!(schema.ports.iter().all(|p| p.kind != PortKind::Callback));
}

#[test]
fn a_returned_object_is_one_owned_host_reference_and_one_handle() {
    let rt = Runtime::new();
    let acct = account(&rt);
    let before = rt.real().objects().host_refs();

    let first = handle_of(&rt.call_object("Account", "mailbox", acct, &args(|w| "inbox".encode(w))).sync_ok());
    assert_eq!(refs(&rt, first), Some(1));
    let again = handle_of(&rt.call_object("Account", "mailbox", acct, &args(|w| "inbox".encode(w))).sync_ok());
    assert_eq!(again, first, "an object has at most one live handle");
    assert_eq!(refs(&rt, first), Some(2));
    assert_eq!(rt.real().objects().host_refs(), before + 2);

    // The mailbox answers calls through its handle.
    let name = rt.call_object("Mailbox", "name", first, &[]).sync_ok();
    assert_eq!(String::decode_exact(&name).unwrap(), "inbox");

    // Giving one reference back keeps the handle; the last removes it.
    rt.real().release(first);
    assert_eq!(refs(&rt, first), Some(1));
    rt.real().release(first);
    assert_eq!(refs(&rt, first), None);
    assert!(rt.call_object("Mailbox", "name", first, &[]).bad_request().contains("stale handle"));

    // Returned again after the host let go, it is a fresh handle.
    let fresh = handle_of(&rt.call_object("Account", "mailbox", acct, &args(|w| "inbox".encode(w))).sync_ok());
    assert_ne!(fresh, first);
    assert_eq!(Handle(fresh).index(), Handle(first).index(), "the slot is reused");
    assert!(Handle(fresh).generation() > Handle(first).generation());
}

#[test]
fn option_and_vec_returns_issue_one_reference_per_handle() {
    let rt = Runtime::new();
    let acct = account(&rt);

    let none = rt.call_object("Account", "drafts", acct, &args(|w| false.encode(w))).sync_ok();
    assert_eq!(none, [0]);
    let some = rt.call_object("Account", "drafts", acct, &args(|w| true.encode(w))).sync_ok();
    let some = Option::<u64>::decode_exact(&some).unwrap().unwrap();
    assert_eq!(refs(&rt, some), Some(1));

    let all = rt.call_object("Account", "mailboxes", acct, &[]).sync_ok();
    let all = Vec::<u64>::decode_exact(&all).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0], all[2], "the same Arc twice is one handle");
    assert_eq!(all[0], some, "and the same handle as before");
    assert_eq!(refs(&rt, all[0]), Some(3), "one from drafts, two from the list");
    assert_eq!(refs(&rt, all[1]), Some(1));
}

#[test]
fn object_parameters_resolve_before_the_method_runs() {
    let rt = Runtime::new();
    let acct = account(&rt);
    let sent = handle_of(&rt.call_object("Account", "mailbox", acct, &args(|w| "sent".encode(w))).sync_ok());
    let inbox = handle_of(&rt.call_object("Account", "mailbox", acct, &args(|w| "inbox".encode(w))).sync_ok());

    rt.call_object("Account", "move_to", acct, &args(|w| {
        3_u32.encode(w);
        sent.encode(w);
    }))
    .sync_ok();
    let moved = rt.call_object("Account", "moved", acct, &[]).sync_ok();
    assert_eq!(Vec::<String>::decode_exact(&moved).unwrap(), ["3->sent"]);
    // Borrowed: the call gave nothing and took nothing.
    assert_eq!(refs(&rt, sent), Some(1));

    let by_arc = rt.call_object("Account", "move_to_shared", acct, &args(|w| inbox.encode(w))).sync_ok();
    assert_eq!(String::decode_exact(&by_arc).unwrap(), "inbox");
    let opt = |h: Option<u64>| args(|w| h.encode(w));
    for (method, expect_some) in [("move_maybe", true), ("move_maybe_shared", true)] {
        let got = rt.call_object("Account", method, acct, &opt(Some(sent))).sync_ok();
        assert_eq!(String::decode_exact(&got).unwrap(), "sent", "{method} {expect_some}");
        let got = rt.call_object("Account", method, acct, &opt(None)).sync_ok();
        assert_eq!(String::decode_exact(&got).unwrap(), "none", "{method}");
    }
    let names = rt.call_object("Account", "names", acct, &args(|w| vec![sent, inbox, sent].encode(w))).sync_ok();
    assert_eq!(String::decode_exact(&names).unwrap(), "sent,inbox,sent");

    // A free function takes and returns an object: the same handle back, one more reference.
    let back = handle_of(&rt.call_function("same_box", &args(|w| sent.encode(w))).sync_ok());
    assert_eq!(back, sent);
    assert_eq!(refs(&rt, sent), Some(2));
}

#[test]
fn a_stale_or_wrongly_typed_object_parameter_is_a_bad_request_naming_it() {
    let rt = Runtime::new();
    let acct = account(&rt);
    let sent = handle_of(&rt.call_object("Account", "mailbox", acct, &args(|w| "sent".encode(w))).sync_ok());
    rt.real().release(sent);

    let reason = rt
        .call_object("Account", "move_to", acct, &args(|w| {
            3_u32.encode(w);
            sent.encode(w);
        }))
        .bad_request();
    assert!(reason.contains("argument `target` of `Account.move_to`"), "{reason}");
    assert!(reason.contains("stale handle"), "{reason}");

    // The account's own handle is not a mailbox.
    let reason = rt
        .call_object("Account", "move_to", acct, &args(|w| {
            3_u32.encode(w);
            acct.encode(w);
        }))
        .bad_request();
    assert!(reason.contains("argument `target` of `Account.move_to`"), "{reason}");
    assert!(reason.contains("not a"), "{reason}");

    // The null handle is not an object either.
    let reason = rt
        .call_object("Account", "move_to", acct, &args(|w| {
            3_u32.encode(w);
            0_u64.encode(w);
        }))
        .bad_request();
    assert!(reason.contains("null handle"), "{reason}");

    // One bad handle in a list refuses the call.
    let inbox = handle_of(&rt.call_object("Account", "mailbox", acct, &args(|w| "inbox".encode(w))).sync_ok());
    let reason = rt
        .call_object("Account", "names", acct, &args(|w| vec![inbox, sent].encode(w)))
        .bad_request();
    assert!(reason.contains("argument `boxes` of `Account.names`"), "{reason}");
}

#[test]
fn an_async_method_issues_in_its_last_poll_and_an_error_issues_nothing() {
    let rt = Runtime::new();
    let acct = account(&rt);
    let before = rt.real().objects().host_refs();

    let ok = rt
        .call_object("Account", "open_thread", acct, &args(|w| 5_u32.encode(w)))
        .run_async()
        .expect("a thread");
    let thread = handle_of(&ok);
    assert_eq!(refs(&rt, thread), Some(1));
    let id = rt.call_object("Thread", "id", thread, &[]).sync_ok();
    assert_eq!(u32::decode_exact(&id).unwrap(), 5);

    let err = rt
        .call_object("Account", "open_thread", acct, &args(|w| 0_u32.encode(w)))
        .run_async()
        .expect_err("a typed error");
    assert_eq!(MailError::decode_exact(&err).unwrap(), MailError::NoThread);
    assert_eq!(rt.real().objects().host_refs(), before + 1, "the error issued nothing");

    // A call dropped before it finishes (a cancelled call) never got to issue.
    let dispatched = rt.call_object("Account", "open_thread", acct, &args(|w| 6_u32.encode(w)));
    drop(dispatched);
    assert_eq!(rt.real().objects().host_refs(), before + 1);

    // An object parameter is held for the whole call: released while it runs, it still resolves.
    let inbox = handle_of(&rt.call_object("Account", "mailbox", acct, &args(|w| "inbox".encode(w))).sync_ok());
    let pending = rt.call_object("Account", "rename_later", acct, &args(|w| inbox.encode(w)));
    rt.real().release(inbox);
    let out = pending.run_async().expect("the call kept its Arc");
    assert_eq!(String::decode_exact(&out).unwrap(), "inbox!");
}

#[test]
fn a_returned_store_is_attached_transient_and_observable() {
    let rt = Runtime::new();
    let acct = account(&rt);
    let chat = handle_of(&rt.call_object("Account", "chat", acct, &[]).sync_ok());
    assert_eq!(refs(&rt, chat), Some(1));
    // Returned twice: one handle, one cell, two references.
    let again = handle_of(&rt.call_object("Account", "chat", acct, &[]).sync_ok());
    assert_eq!(again, chat);
    assert_eq!(refs(&rt, chat), Some(2));

    // It is a store: observing it delivers its current signals.
    rt.real().observe(chat, undra::meta::ids::ALL_SIGNALS, true);
    let delivered = rt.change_sets();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].entries[0].handle, Handle(chat));

    rt.call_object("ChatStore", "send", chat, &[]).sync_ok();
    let delivered = rt.change_sets();
    assert_eq!(delivered.len(), 1, "a change-set per commit");

    // A snapshot leaves a derived store out (ADR-040 decision 8) ...
    let snapshot = rt.real().snapshot();
    let decoded = undra::wire::payload::Snapshot::decode(&mut Reader::new(&snapshot)).unwrap();
    assert!(decoded.stores.is_empty(), "derived handles are transient");
    // ... so a restore makes its handle stale, and the method returns a fresh one.
    rt.real().restore(&snapshot).unwrap();
    let reason = rt.call_object("ChatStore", "peer", chat, &[]).bad_request();
    assert!(reason.contains("stale handle"), "{reason}");
    // (The account is not a store: its handle is stale after the restore too.)
    let acct = account(&rt);
    let fresh = handle_of(&rt.call_object("Account", "chat", acct, &[]).sync_ok());
    assert_ne!(fresh, chat);

    // At the last release the cell is unobserved and forgets its handle, so a later issue
    // starts clean: the new mirror gets the initial values.
    rt.real().release(fresh);
    let third = handle_of(&rt.call_object("Account", "chat", acct, &[]).sync_ok());
    rt.real().observe(third, undra::meta::ids::ALL_SIGNALS, true);
    let delivered = rt.change_sets();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].entries[0].handle, Handle(third));
}

#[test]
fn a_constructor_returning_arc_self_interns() {
    let rt = Runtime::new();
    let first = handle_of(&rt.call_object("Registry", "shared", 0, &[]).sync_ok());
    let second = handle_of(&rt.call_object("Registry", "shared", 0, &[]).sync_ok());
    assert_eq!(first, second);
    assert_eq!(refs(&rt, first), Some(2));
    rt.real().release(first);
    rt.real().release(second);
    assert_eq!(refs(&rt, first), None);
    let third = handle_of(&rt.call_object("Registry", "shared", 0, &[]).sync_ok());
    assert_ne!(third, first);
}

#[test]
fn origins_hold_what_their_calls_returned_until_released() {
    let rt = Runtime::new();
    let acct = account(&rt);
    let payload = |call_id: u32, target: undra::wire::payload::CallTarget, a: &[u8]| {
        undra::runtime::testing::call_payload(target, call_id, a)
    };
    let mailbox_call = payload(
        10,
        undra::wire::payload::CallTarget::Method {
            handle: Handle(acct),
            method_id: ids::method_id("Account", "mailbox"),
        },
        &args(|w| "inbox".encode(w)),
    );
    // Session 7 asks for the mailbox twice, session 8 once.
    assert_eq!(rt.real().call_from(7, &mailbox_call), 0);
    let first = rt.real().call_from(8, &payload(
        11,
        undra::wire::payload::CallTarget::Method {
            handle: Handle(acct),
            method_id: ids::method_id("Account", "mailbox"),
        },
        &args(|w| "inbox".encode(w)),
    ));
    assert_eq!(first, 0);
    let inbox = rt.call_object("Account", "mailbox", acct, &args(|w| "inbox".encode(w)));
    let inbox = handle_of(&inbox.sync_ok());
    assert_eq!(refs(&rt, inbox), Some(3));

    // Session 7 gives one back by itself, and disconnects holding nothing more.
    rt.real().release_from(7, inbox);
    assert_eq!(refs(&rt, inbox), Some(2));
    assert_eq!(rt.real().release_origin(7), 0);
    assert_eq!(refs(&rt, inbox), Some(2));
    // Session 8 disconnects without releasing: its reference is given back.
    assert_eq!(rt.real().release_origin(8), 1);
    assert_eq!(refs(&rt, inbox), Some(1));
    assert_eq!(rt.real().release_origin(8), 0, "once");
}
