//! Host callback interfaces (ADR-041): `#[undra::callback]` traits are port instances. The
//! generated dispatchers turn an instance handle into a proxy after every argument has decoded,
//! intern it, give references back with `__release`, cancel with `__cancel`, and never panic when
//! the host is gone.
#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex};

use undra::meta::{PortKind, TypeRef, collect_schema, ids};
use undra::runtime::{Ctx, Port, PortError};
use undra::wire::{Decode, Encode, Handle, Writer};
use undra_macros as k;

mod support;
use support::Runtime;

fn args(encode: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    encode(&mut w);
    w.into_vec()
}

#[k::error]
#[derive(Clone, PartialEq)]
pub enum PromptError {
    #[error("the user said no")]
    Declined,
    #[error("the host is not available: {0}")]
    Unavailable(String),
}

impl From<PortError> for PromptError {
    fn from(e: PortError) -> Self {
        PromptError::Unavailable(e.to_string())
    }
}

/// Reports upload progress and asks questions.
#[k::callback]
pub trait UploadListener {
    /// Bytes sent so far.
    #[undra(coalesce)]
    fn progress(&self, sent: u64, total: u64);
    /// Tells the host the upload is over.
    fn finished(&self, name: String);
    /// Asks the user whether to replace an existing file.
    async fn confirm_replace(&self, name: String) -> Result<bool, PromptError>;
}

/// A callback that runs off the main thread.
#[k::callback(background)]
pub trait TokenProvider {
    async fn token(&self) -> Result<String, PromptError>;
}

pub struct Watch;

#[k::api]
impl Watch {
    pub fn id(&self) -> u32 {
        9
    }
}

#[derive(Default)]
pub struct Uploader {
    kept: Mutex<Vec<Arc<dyn UploadListener>>>,
}

#[k::api]
impl Uploader {
    pub fn new() -> Self {
        Uploader {
            kept: Mutex::new(Vec::new()),
        }
    }

    pub async fn upload(
        &self,
        name: String,
        listener: Arc<dyn UploadListener>,
    ) -> Result<bool, PromptError> {
        listener.progress(0, 10);
        let replace = listener.confirm_replace(name.clone()).await?;
        listener.progress(10, 10);
        listener.finished(name);
        Ok(replace)
    }

    /// Keeps the listener (so its proxy outlives the call) and tells whether it is the same
    /// proxy as the one kept before.
    pub fn keep(&self, listener: Arc<dyn UploadListener>) -> bool {
        let mut kept = self.kept.lock().unwrap();
        let same = kept.iter().any(|k| Arc::ptr_eq(k, &listener));
        kept.push(listener);
        same
    }

    pub fn clear(&self) {
        self.kept.lock().unwrap().clear();
    }

    pub fn ping_all(&self) {
        for listener in self.kept.lock().unwrap().iter() {
            listener.progress(1, 2);
        }
    }

    pub fn maybe(&self, listener: Option<Arc<dyn UploadListener>>) -> bool {
        listener.is_some()
    }

    pub fn watch(&self, listener: Arc<dyn UploadListener>) -> Arc<Watch> {
        self.kept.lock().unwrap().push(listener);
        Arc::new(Watch)
    }

    pub fn with_token(&self, provider: Arc<dyn TokenProvider>, other: &Watch) -> u32 {
        let _ = (provider, other);
        1
    }
}

/// A store whose constructor takes a callback and hands out a signal that already belongs to
/// another store the second time it runs: it fails after its body, with the proxy made.
#[k::store]
pub struct Gauge {
    level: undra::signals::Signal<u32>,
}

static GAUGE_SIGNAL: std::sync::OnceLock<undra::signals::Signal<u32>> = std::sync::OnceLock::new();

#[k::api(store)]
impl Gauge {
    pub fn new(listener: Arc<dyn UploadListener>) -> Self {
        // The proxy is made before the body runs and dropped when it returns: released by the core.
        let _ = listener;
        Gauge {
            level: GAUGE_SIGNAL
                .get_or_init(|| undra::signals::Signal::new(0))
                .clone(),
        }
    }
}

const PORT: u32 = ids::port_id("UploadListener");

fn method(name: &str) -> u32 {
    ids::port_method_id("UploadListener", name)
}

fn uploader(rt: &Runtime) -> u64 {
    u64::decode_exact(&rt.call_object("Uploader", "new", 0, &[]).sync_ok()).unwrap()
}

fn is_release(call: &support::PortCallRecord, instance: u64) -> bool {
    call.method_id == ids::callback_release_id("UploadListener")
        && call.args == instance.to_le_bytes()
        && call.port_call_id == 0
}

#[test]
fn the_schema_describes_a_callback_port_and_callback_parameters() {
    let schema = collect_schema("test");
    let port = schema
        .ports
        .iter()
        .find(|p| p.name == "UploadListener")
        .unwrap();
    assert_eq!(port.kind, PortKind::Callback);
    assert!(!port.background);
    assert_eq!(port.port_id, ids::port_id("UploadListener"));
    let find = |n: &str| port.methods.iter().find(|m| m.name == n).unwrap();
    assert_eq!(find("progress").returns, TypeRef::Unit);
    assert!(find("progress").coalesce);
    assert!(!find("finished").coalesce);
    assert!(find("confirm_replace").is_async);
    assert_eq!(
        find("confirm_replace").returns,
        TypeRef::result(TypeRef::Bool, TypeRef::named("PromptError"))
    );
    let provider = schema
        .ports
        .iter()
        .find(|p| p.name == "TokenProvider")
        .unwrap();
    assert!(provider.background);

    let uploader = schema
        .objects
        .iter()
        .find(|o| o.name == "Uploader")
        .unwrap();
    let upload = uploader
        .methods
        .iter()
        .find(|m| m.name == "upload")
        .unwrap();
    assert_eq!(upload.params[1].ty, TypeRef::callback("UploadListener"));
    let maybe = uploader.methods.iter().find(|m| m.name == "maybe").unwrap();
    assert_eq!(
        maybe.params[0].ty,
        TypeRef::option(TypeRef::callback("UploadListener"))
    );
    assert_eq!(schema.validate(), Ok(()));
    assert_eq!(<dyn UploadListener as Port>::KIND, PortKind::Callback);
}

#[test]
fn an_instance_becomes_a_proxy_that_calls_the_host_with_the_instance_first() {
    let rt = Runtime::new();
    rt.bind_foreign(PORT, |method_id, args| {
        if method_id == method("confirm_replace") {
            // instance u64, name String
            let mut r = undra::wire::Reader::new(args);
            let instance = r.read_u64().unwrap();
            assert_eq!(instance, 42);
            Ok(args_of(|w| true.encode(w)))
        } else {
            Ok(Vec::new())
        }
    });
    let up = uploader(&rt);
    let out = rt
        .call_object(
            "Uploader",
            "upload",
            up,
            &args(|w| {
                "a.txt".encode(w);
                42_u64.encode(w);
            }),
        )
        .run_async()
        .unwrap();
    assert!(bool::decode_exact(&out).unwrap());

    let calls = rt.port_calls();
    // progress(0, 10) (fire-and-forget), confirm_replace (a real call), progress(10, 10),
    // finished (fire-and-forget), and the proxy's release when the call's `Arc` drops.
    let methods: Vec<u32> = calls.iter().map(|c| c.method_id).collect();
    assert_eq!(
        methods,
        [
            method("progress"),
            method("confirm_replace"),
            method("progress"),
            method("finished"),
            ids::callback_release_id("UploadListener"),
        ]
    );
    assert_eq!(calls[0].port_call_id, 0, "fire-and-forget");
    assert_eq!(
        calls[0].args,
        args(|w| {
            42_u64.encode(w);
            0_u64.encode(w);
            10_u64.encode(w);
        })
    );
    assert_ne!(calls[1].port_call_id, 0, "an async method is a real call");
    assert_eq!(
        calls[1].args,
        args(|w| {
            42_u64.encode(w);
            "a.txt".encode(w);
        })
    );
    assert!(
        is_release(&calls[4], 42),
        "the proxy gave its reference back: {:?}",
        calls[4]
    );
}

fn args_of(encode: impl FnOnce(&mut Writer)) -> Vec<u8> {
    args(encode)
}

#[test]
fn the_same_instance_twice_is_one_proxy_and_the_duplicate_reference_goes_back() {
    let rt = Runtime::new();
    let up = uploader(&rt);
    let keep = |instance: u64| {
        let out = rt
            .call_object("Uploader", "keep", up, &args(|w| instance.encode(w)))
            .sync_ok();
        bool::decode_exact(&out).unwrap()
    };
    assert!(!keep(7), "the first crossing makes a proxy");
    assert!(rt.port_calls().is_empty(), "and sends nothing");
    assert!(
        keep(7),
        "the second finds the live proxy: Arc::ptr_eq holds"
    );
    let calls = rt.port_calls();
    assert_eq!(calls.len(), 1);
    assert!(
        is_release(&calls[0], 7),
        "the duplicate reference went back at once: {calls:?}"
    );
    assert!(!keep(8), "another instance is another proxy");
    rt.port_calls();

    // Dropping the proxies gives one reference each back: instance 7 was kept twice (the same
    // proxy, so one reference), instance 8 once.
    rt.call_object("Uploader", "clear", up, &[]).sync_ok();
    let released: Vec<u64> = rt
        .port_calls()
        .iter()
        .map(|c| {
            assert_eq!(c.method_id, ids::callback_release_id("UploadListener"));
            u64::decode_exact(&c.args).unwrap()
        })
        .collect();
    assert_eq!(
        released.len(),
        2,
        "the two Arcs of instance 7 are one proxy: one reference"
    );
    assert!(released.contains(&7) && released.contains(&8));
}

#[test]
fn a_refused_call_transfers_nothing() {
    let rt = Runtime::new();
    let up = uploader(&rt);
    rt.real().release(up); // the receiver is stale now
    let reason = rt
        .call_object("Uploader", "keep", up, &args(|w| 11_u64.encode(w)))
        .bad_request();
    assert!(reason.contains("stale handle"), "{reason}");
    assert!(
        rt.port_calls().is_empty(),
        "no proxy was made, so nothing is released by the core"
    );
    assert!(rt.real().stats_json().contains("\"live_callbacks\":0"));

    // The null instance and undecodable bytes are refused before any proxy is made.
    let up = uploader(&rt);
    let reason = rt
        .call_object("Uploader", "keep", up, &args(|w| 0_u64.encode(w)))
        .bad_request();
    assert!(
        reason.contains("argument `listener` of `Uploader.keep`"),
        "{reason}"
    );
    assert!(reason.contains("null instance"), "{reason}");
    let reason = rt
        .call_object("Uploader", "keep", up, &[1, 2])
        .bad_request();
    assert!(
        reason.contains("cannot decode argument `listener`"),
        "{reason}"
    );
    assert!(rt.port_calls().is_empty());

    // An optional callback: none transfers nothing, some transfers one reference.
    let none = rt
        .call_object(
            "Uploader",
            "maybe",
            up,
            &args(|w| Option::<u64>::None.encode(w)),
        )
        .sync_ok();
    assert!(!bool::decode_exact(&none).unwrap());
    let some = rt
        .call_object("Uploader", "maybe", up, &args(|w| Some(5_u64).encode(w)))
        .sync_ok();
    assert!(bool::decode_exact(&some).unwrap());
    let calls = rt.port_calls();
    assert_eq!(
        calls.len(),
        1,
        "the unused proxy dropped at the end of the call"
    );
    assert!(is_release(&calls[0], 5));
    let reason = rt
        .call_object("Uploader", "maybe", up, &args(|w| Some(0_u64).encode(w)))
        .bad_request();
    assert!(reason.contains("null instance"), "{reason}");
}

#[test]
fn dropping_the_future_of_an_async_callback_sends_cancel() {
    let rt = Runtime::new();
    // The host takes the question and will answer later.
    rt.bind_foreign(PORT, |method_id, _| {
        if method_id == method("confirm_replace") {
            Err(PortError::Cancelled)
        } else {
            Ok(Vec::new())
        }
    });
    let up = uploader(&rt);
    let pending = rt.call_object(
        "Uploader",
        "upload",
        up,
        &args(|w| {
            "b.txt".encode(w);
            43_u64.encode(w);
        }),
    );
    // Poll it once so the proxy has made its call, then drop it: the call is cancelled.
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let undra::runtime::DispatchResult::Async(mut future) = pending.0 else {
        panic!("an async method")
    };
    assert!(future.as_mut().poll(&mut cx).is_pending());
    let calls = rt.port_calls();
    let question = calls
        .iter()
        .find(|c| c.method_id == method("confirm_replace"))
        .unwrap()
        .clone();
    assert!(question.port_call_id != 0);
    drop(future);
    let calls = rt.port_calls();
    let cancel = calls
        .iter()
        .find(|c| c.method_id == ids::callback_cancel_id("UploadListener"))
        .unwrap_or_else(|| panic!("no __cancel in {calls:?}"));
    assert_eq!(cancel.port_call_id, 0, "__cancel is fire-and-forget");
    assert_eq!(
        cancel.args,
        args(|w| {
            43_u64.encode(w);
            question.port_call_id.encode(w);
        })
    );
    // The proxy that went with the future gave its reference back as well.
    assert!(calls.iter().any(|c| is_release(c, 43)), "{calls:?}");
    // A late answer is discarded, as for any port call.
    rt.real()
        .port_reply(&undra::runtime::testing::port_reply_ok(
            question.port_call_id,
            &[1],
        ));
}

#[test]
fn an_answered_async_callback_sends_no_cancel() {
    let rt = Runtime::new();
    rt.bind_foreign(PORT, |method_id, _| {
        if method_id == method("confirm_replace") {
            Ok(args_of(|w| false.encode(w)))
        } else {
            Ok(Vec::new())
        }
    });
    let up = uploader(&rt);
    rt.call_object(
        "Uploader",
        "upload",
        up,
        &args(|w| {
            "c.txt".encode(w);
            44_u64.encode(w);
        }),
    )
    .run_async()
    .unwrap();
    assert!(
        rt.port_calls()
            .iter()
            .all(|c| c.method_id != ids::callback_cancel_id("UploadListener"))
    );
}

#[test]
fn a_host_that_fails_or_is_gone_is_a_typed_error_not_a_panic() {
    let rt = Runtime::new();
    // The host reports the typed error of the method (status 1).
    rt.bind_foreign(PORT, |method_id, _| {
        if method_id == method("confirm_replace") {
            Err(PortError::Failed(args_of(|w| {
                PromptError::Declined.encode(w)
            })))
        } else {
            Ok(Vec::new())
        }
    });
    let up = uploader(&rt);
    let err = rt
        .call_object(
            "Uploader",
            "upload",
            up,
            &args(|w| {
                "d.txt".encode(w);
                45_u64.encode(w);
            }),
        )
        .run_async()
        .expect_err("the host's own error");
    assert_eq!(
        PromptError::decode_exact(&err).unwrap(),
        PromptError::Declined
    );

    // No host binding at all: unavailable, which the error type represents.
    let rt = Runtime::new();
    let up = uploader(&rt);
    let err = rt
        .call_object(
            "Uploader",
            "upload",
            up,
            &args(|w| {
                "e.txt".encode(w);
                46_u64.encode(w);
            }),
        )
        .run_async()
        .expect_err("unavailable");
    assert!(matches!(
        PromptError::decode_exact(&err).unwrap(),
        PromptError::Unavailable(_)
    ));

    // After shutdown a kept proxy is silent and its async method is `Unavailable(cancelled)`.
    let rt = Runtime::new();
    let up = uploader(&rt);
    let _ = rt
        .call_object("Uploader", "keep", up, &args(|w| 47_u64.encode(w)))
        .sync_ok();
    rt.port_calls();
    let ctx: Ctx = rt.ctx();
    let kept = ctx.runtime().callback::<dyn UploadListener>(47);
    rt.port_calls();
    rt.real().shutdown();
    kept.progress(1, 1);
    let answer = support::testing::block_on(kept.confirm_replace("f".to_owned()));
    assert!(matches!(answer, Err(PromptError::Unavailable(_))));
    assert!(rt.port_calls().is_empty(), "nothing is sent after shutdown");
    drop(kept);
    assert!(rt.port_calls().is_empty());
}

#[test]
fn objects_and_callbacks_mix_in_one_signature() {
    let rt = Runtime::new();
    let up = uploader(&rt);
    let watch = u64::decode_exact(
        &rt.call_object("Uploader", "watch", up, &args(|w| 50_u64.encode(w)))
            .sync_ok(),
    )
    .unwrap();
    assert!(Handle(watch).generation() > 0);
    let one = rt
        .call_object(
            "Uploader",
            "with_token",
            up,
            &args(|w| {
                51_u64.encode(w);
                watch.encode(w);
            }),
        )
        .sync_ok();
    assert_eq!(u32::decode_exact(&one).unwrap(), 1);
    // A stale object parameter refuses the call before the callback proxy is made.
    rt.real().release(watch);
    rt.port_calls();
    let reason = rt
        .call_object(
            "Uploader",
            "with_token",
            up,
            &args(|w| {
                52_u64.encode(w);
                watch.encode(w);
            }),
        )
        .bad_request();
    assert!(
        reason.contains("argument `other` of `Uploader.with_token`"),
        "{reason}"
    );
    assert!(
        rt.port_calls().iter().all(|c| !is_release(c, 52)),
        "no reference to give back"
    );
}

/// Objects-followups O6: a constructor that took a callback and then failed after its body made the
/// proxy, which is released with the unpublished value. Status 5 would say "refused: owns nothing"
/// and make the host give its reference back as well (a double release); a failed call keeps it.
#[test]
fn a_constructor_that_fails_after_making_its_proxies_is_not_a_refusal() {
    let rt = Runtime::new();
    let first = rt
        .call_object("Gauge", "new", 0, &args(|w| 31_u64.encode(w)))
        .sync_ok();
    assert!(u64::decode_exact(&first).unwrap() > 0, "the first Gauge is published");
    rt.port_calls();

    // The second one reuses the first one's signal, which belongs to a store already.
    let reason = rt
        .call_object("Gauge", "new", 0, &args(|w| 32_u64.encode(w)))
        .failed();
    assert!(
        reason.contains("store `Gauge` could not attach its signals") && reason.contains("already attached"),
        "{reason}"
    );
    let calls = rt.port_calls();
    assert_eq!(
        calls.len(),
        1,
        "the core released the proxy it made, once: {calls:?}"
    );
    assert!(is_release(&calls[0], 32), "{:?}", calls[0]);
}
