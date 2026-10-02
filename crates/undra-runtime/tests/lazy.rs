//! The runtime's page server (ADR-043): any `LazySource` in the object table answers `LazyPage`
//! calls under the panic guard, a page is read at one version, and a hostile call never makes the
//! core allocate more than the cap or fall over.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use undra_runtime::testing::{TestRuntime, call_payload, decode_reply};
use undra_runtime::{LazyList, MAX_PAGE_ITEMS};
use undra_signals::LazySource;
use undra_wire::payload::{CallTarget, LazyPage, ReplyStatus};
use undra_wire::{Handle, Reader, Writer};

fn page(
    t: &TestRuntime,
    handle: Handle,
    offset: u32,
    limit: u32,
) -> undra_runtime::testing::ReplyRecord {
    let payload = call_payload(
        CallTarget::LazyPage {
            handle,
            offset,
            limit,
        },
        9,
        &[],
    );
    t.runtime().call_sync_with(&payload, decode_reply)
}

/// A source whose encoder panics while armed: user code (an item's `Encode`, a view's pipeline).
struct Bomb {
    armed: AtomicBool,
}

impl LazySource for Bomb {
    fn stamp(&self) -> (usize, u64) {
        (3, 1)
    }

    fn encode_page(&self, _offset: u32, _limit: u32, out: &mut Writer) -> LazyPage {
        assert!(
            !self.armed.load(Ordering::SeqCst),
            "the item encoder failed"
        );
        out.write_u32(7);
        LazyPage {
            version: 1,
            total: 3,
            count: 1,
        }
    }
}

#[test]
fn a_panicking_source_is_a_status_2_reply_and_the_runtime_serves_on() {
    let t = TestRuntime::new();
    let bomb = Arc::new(Bomb {
        armed: AtomicBool::new(true),
    });
    let handle = t.runtime().insert_lazy_source(bomb.clone());
    let reply = page(&t, handle, 0, 10);
    assert_eq!(reply.status, ReplyStatus::Panic, "{reply:?}");
    // The panic message travels in the reply body; the runtime counted it.
    assert!(String::from_utf8_lossy(&reply.body).contains("the item encoder failed"));
    assert!(t.runtime().stats_json().contains("\"panics\":1"));

    // Disarmed, the same page server answers: nothing was poisoned for good.
    bomb.armed.store(false, Ordering::SeqCst);
    let reply = page(&t, handle, 0, 10);
    assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
    let mut r = Reader::new(&reply.body);
    let header = LazyPage::decode(&mut r).unwrap();
    assert_eq!((header.version, header.total, header.count), (1, 3, 1));
    assert_eq!(r.read_u32().unwrap(), 7);
    r.finish().unwrap();
}

#[test]
fn a_page_is_read_at_one_version_while_the_list_changes() {
    let t = TestRuntime::new();
    let list = LazyList::new();
    let handle = t.runtime().insert_lazy_list(&list);
    let writer = {
        let list = list.clone();
        std::thread::spawn(move || {
            for n in 0..2_000_u32 {
                list.push_encoded(&n);
            }
        })
    };
    let mut last = 0;
    while !writer.is_finished() {
        let reply = page(&t, handle, 0, 8);
        assert_eq!(reply.status, ReplyStatus::Ok);
        let mut r = Reader::new(&reply.body);
        let header = LazyPage::decode(&mut r).unwrap();
        // Every push is one version and one item: a page that read the version and the length at
        // different instants would disagree.
        assert_eq!(u64::from(header.total), header.version, "{header:?}");
        assert!(header.version >= last, "the version never goes back");
        last = header.version;
        assert_eq!(header.count, header.total.min(8));
        for n in 0..header.count {
            assert_eq!(r.read_u32().unwrap(), n);
        }
        r.finish().unwrap();
    }
    writer.join().unwrap();
    let reply = page(&t, handle, 1_990, 100);
    let mut r = Reader::new(&reply.body);
    let header = LazyPage::decode(&mut r).unwrap();
    assert_eq!(
        (header.version, header.total, header.count),
        (2_000, 2_000, 10)
    );
}

#[test]
fn hostile_arguments_never_make_the_core_encode_more_than_the_cap() {
    let t = TestRuntime::new();
    let list = LazyList::from_items((0..MAX_PAGE_ITEMS * 3).map(|_| vec![0; 64]).collect());
    let handle = t.runtime().insert_lazy_list(&list);
    for (offset, limit) in [
        (0, u32::MAX),
        (1, u32::MAX),
        (MAX_PAGE_ITEMS, u32::MAX),
        (u32::MAX, u32::MAX),
        (u32::MAX, 0),
        (0, 0),
        (MAX_PAGE_ITEMS * 3 - 1, 5),
    ] {
        let reply = page(&t, handle, offset, limit);
        assert_eq!(reply.status, ReplyStatus::Ok, "({offset}, {limit})");
        let header = LazyPage::decode(&mut Reader::new(&reply.body)).unwrap();
        assert!(
            header.count <= MAX_PAGE_ITEMS,
            "({offset}, {limit}): {header:?}"
        );
        assert!(header.count <= limit);
        assert_eq!(reply.body.len(), 16 + 64 * header.count as usize);
        assert_eq!(header.total, MAX_PAGE_ITEMS * 3);
    }
}

#[test]
fn a_lazy_source_handle_is_an_ordinary_host_reference() {
    let t = TestRuntime::new();
    let list = LazyList::from_items(vec![vec![1], vec![2]]);
    let handle = t.runtime().insert_lazy_list(&list);
    assert_eq!(page(&t, handle, 0, 5).status, ReplyStatus::Ok);
    // The caller of `insert_lazy_source` owns the reference, as for any object it constructs.
    t.runtime().release(handle.0);
    assert_eq!(page(&t, handle, 0, 5).status, ReplyStatus::BadRequest);
}
