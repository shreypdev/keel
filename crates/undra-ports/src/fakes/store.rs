//! [`MemKv`] and [`MemSecureStore`]: in-memory key-value stores with injectable failures, and
//! [`FailingKv`]: a store that cannot store anything.

use core::ops::Deref;
use std::collections::BTreeMap;

use parking_lot::Mutex;
use undra_wire::Bytes;

use crate::{Kv, SecureStore, StorageError};

/// One operation a [`MemStore`] served through its port, for asserting on persistence
/// behaviour ("written once", "never read").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreOp {
    /// `get(key)`.
    Get(String),
    /// `set(key, ..)`.
    Set(String),
    /// `delete(key)`.
    Delete(String),
    /// `list(prefix)`.
    List(String),
}

/// Which operations an injected failure applies to (see [`MemStore::fail`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FailOn {
    /// `get`.
    Get,
    /// `set`.
    Set,
    /// `delete`.
    Delete,
    /// `list`.
    List,
    /// Every operation.
    Any,
}

impl FailOn {
    fn matches(self, op: &StoreOp) -> bool {
        matches!(
            (self, op),
            (FailOn::Any, _)
                | (FailOn::Get, StoreOp::Get(_))
                | (FailOn::Set, StoreOp::Set(_))
                | (FailOn::Delete, StoreOp::Delete(_))
                | (FailOn::List, StoreOp::List(_))
        )
    }
}

/// One injected failure: which operations, which error, and how many more times (`None`: until
/// [`MemStore::heal`]).
#[derive(Clone, Debug)]
struct Injected {
    on: FailOn,
    error: StorageError,
    remaining: Option<u32>,
}

#[derive(Default)]
struct Inner {
    map: BTreeMap<String, Vec<u8>>,
    ops: Vec<StoreOp>,
    failures: Vec<Injected>,
    failed: usize,
}

/// The storage and bookkeeping behind [`MemKv`] and [`MemSecureStore`].
///
/// Keys are ordered bytewise, so `list` answers in ascending order like the platform
/// adapters. The inspection methods here ([`value`](MemStore::value),
/// [`entries`](MemStore::entries), ...) do not count as operations: they read the store
/// without going through the port.
#[derive(Debug, Default)]
pub struct MemStore {
    inner: Mutex<Inner>,
}

impl core::fmt::Debug for Inner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Inner")
            .field("entries", &self.map.len())
            .field("ops", &self.ops.len())
            .finish()
    }
}

impl MemStore {
    /// An empty store.
    pub fn new() -> MemStore {
        MemStore::default()
    }

    /// Puts `value` under `key` without recording an operation: seeds a test.
    pub fn insert(&self, key: impl Into<String>, value: impl Into<Vec<u8>>) {
        self.inner.lock().map.insert(key.into(), value.into());
    }

    /// The value under `key`, read directly (not recorded as an operation).
    pub fn value(&self, key: &str) -> Option<Vec<u8>> {
        self.inner.lock().map.get(key).cloned()
    }

    /// Whether `key` is present.
    pub fn contains_key(&self, key: &str) -> bool {
        self.inner.lock().map.contains_key(key)
    }

    /// How many keys are stored.
    pub fn len(&self) -> usize {
        self.inner.lock().map.len()
    }

    /// Whether the store holds no keys.
    pub fn is_empty(&self) -> bool {
        self.inner.lock().map.is_empty()
    }

    /// Every key, ascending.
    pub fn keys(&self) -> Vec<String> {
        self.inner.lock().map.keys().cloned().collect()
    }

    /// A copy of the whole store.
    pub fn entries(&self) -> BTreeMap<String, Vec<u8>> {
        self.inner.lock().map.clone()
    }

    /// Removes every key (recorded operations are kept).
    pub fn clear(&self) {
        self.inner.lock().map.clear();
    }

    /// The operations served through the port so far, oldest first.
    pub fn ops(&self) -> Vec<StoreOp> {
        self.inner.lock().ops.clone()
    }

    /// Removes and returns the recorded operations.
    pub fn take_ops(&self) -> Vec<StoreOp> {
        std::mem::take(&mut self.inner.lock().ops)
    }

    /// Makes every later operation that matches `on` fail with `error`, until [`heal`](Self::heal).
    /// The operation is still recorded (as attempted) and the stored data is left untouched.
    ///
    /// ```
    /// use undra_ports::fakes::{FailOn, MemKv};
    /// use undra_ports::{Kv, StorageError};
    /// use undra_runtime::testing::TestRuntime;
    /// use undra_wire::Bytes;
    ///
    /// let kv = MemKv::new();
    /// kv.fail(FailOn::Set, StorageError::Full);
    /// let t = TestRuntime::new();
    /// assert_eq!(t.run_until(kv.set("k".into(), Bytes(vec![1]))), Err(StorageError::Full));
    /// assert_eq!(kv.value("k"), None);
    /// kv.heal();
    /// assert_eq!(t.run_until(kv.set("k".into(), Bytes(vec![1]))), Ok(()));
    /// ```
    pub fn fail(&self, on: FailOn, error: StorageError) {
        self.inner.lock().failures.push(Injected {
            on,
            error,
            remaining: None,
        });
    }

    /// Makes the next `times` operations that match `on` fail with `error`; later ones succeed.
    pub fn fail_times(&self, on: FailOn, error: StorageError, times: u32) {
        if times == 0 {
            return;
        }
        self.inner.lock().failures.push(Injected {
            on,
            error,
            remaining: Some(times),
        });
    }

    /// Removes every injected failure.
    pub fn heal(&self) {
        self.inner.lock().failures.clear();
    }

    /// How many operations failed because of an injected failure so far.
    pub fn failed_ops(&self) -> usize {
        self.inner.lock().failed
    }

    /// Records `op` and answers the injected failure it meets, if any (the first matching one;
    /// a counted one is used up).
    fn attempt(inner: &mut Inner, op: StoreOp) -> Result<(), StorageError> {
        let position = inner.failures.iter().position(|f| f.on.matches(&op));
        inner.ops.push(op);
        let Some(at) = position else {
            return Ok(());
        };
        let error = inner.failures[at].error.clone();
        if let Some(remaining) = &mut inner.failures[at].remaining {
            *remaining -= 1;
            if *remaining == 0 {
                inner.failures.remove(at);
            }
        }
        inner.failed += 1;
        Err(error)
    }

    fn port_get(&self, key: String) -> Result<Option<Bytes>, StorageError> {
        let mut inner = self.inner.lock();
        let value = inner.map.get(&key).cloned().map(Bytes);
        MemStore::attempt(&mut inner, StoreOp::Get(key))?;
        Ok(value)
    }

    fn port_set(&self, key: String, value: Bytes) -> Result<(), StorageError> {
        let mut inner = self.inner.lock();
        MemStore::attempt(&mut inner, StoreOp::Set(key.clone()))?;
        inner.map.insert(key, value.0);
        Ok(())
    }

    fn port_delete(&self, key: String) -> Result<(), StorageError> {
        let mut inner = self.inner.lock();
        MemStore::attempt(&mut inner, StoreOp::Delete(key.clone()))?;
        inner.map.remove(&key);
        Ok(())
    }

    fn port_list(&self, prefix: String) -> Result<Vec<String>, StorageError> {
        let mut inner = self.inner.lock();
        let keys = inner
            .map
            .range(prefix.clone()..)
            .map(|(key, _)| key)
            .take_while(|key| key.starts_with(&prefix))
            .cloned()
            .collect();
        MemStore::attempt(&mut inner, StoreOp::List(prefix))?;
        Ok(keys)
    }
}

/// Defines a newtype over [`MemStore`] and implements a key-value port trait for it.
macro_rules! mem_port {
    ($(#[$doc:meta])* $name:ident, $port:ident) => {
        $(#[$doc])*
        #[derive(Debug, Default)]
        pub struct $name(MemStore);

        impl $name {
            /// An empty store.
            pub fn new() -> $name {
                $name::default()
            }
        }

        impl Deref for $name {
            type Target = MemStore;

            fn deref(&self) -> &MemStore {
                &self.0
            }
        }

        #[undra_macros::port]
        impl $port for $name {
            async fn get(&self, key: String) -> Result<Option<Bytes>, StorageError> {
                self.0.port_get(key)
            }

            async fn set(&self, key: String, value: Bytes) -> Result<(), StorageError> {
                self.0.port_set(key, value)
            }

            async fn delete(&self, key: String) -> Result<(), StorageError> {
                self.0.port_delete(key)
            }

            async fn list(&self, prefix: String) -> Result<Vec<String>, StorageError> {
                self.0.port_list(prefix)
            }
        }
    };
}

mem_port! {
    /// An in-memory [`Kv`] port. Inspect it through [`MemStore`] (it derefs to one).
    ///
    /// ```
    /// use undra_ports::Kv;
    /// use undra_ports::fakes::MemKv;
    /// use undra_runtime::testing::TestRuntime;
    /// use undra_wire::Bytes;
    ///
    /// let kv = MemKv::new();
    /// kv.insert("greeting", b"hi".to_vec()); // seed without going through the port
    /// let t = TestRuntime::new();
    /// assert_eq!(t.run_until(kv.get("greeting".into())), Ok(Some(Bytes(b"hi".to_vec()))));
    /// t.run_until(kv.set("a/1".into(), Bytes(vec![1]))).unwrap();
    /// assert_eq!(t.run_until(kv.list("a/".into())), Ok(vec!["a/1".to_owned()]));
    /// assert_eq!(kv.value("a/1"), Some(vec![1]));
    /// ```
    MemKv, Kv
}

mem_port! {
    /// An in-memory [`SecureStore`] port: the same behaviour as [`MemKv`], under its own port id.
    MemSecureStore, SecureStore
}

/// A [`Kv`] that stores nothing: every operation fails with the error it was made with. For
/// tests of what a core does when storage is gone for good (an iOS app before first unlock, a
/// browser without IndexedDB).
///
/// ```
/// use undra_ports::fakes::FailingKv;
/// use undra_ports::{Kv, StorageError};
/// use undra_runtime::testing::TestRuntime;
///
/// let kv = FailingKv::new(StorageError::Locked);
/// let t = TestRuntime::new();
/// assert_eq!(t.run_until(kv.get("k".into())), Err(StorageError::Locked));
/// assert_eq!(kv.calls(), 1);
/// ```
#[derive(Debug)]
pub struct FailingKv {
    error: StorageError,
    calls: Mutex<usize>,
}

impl FailingKv {
    /// A store whose every operation fails with `error`.
    pub fn new(error: StorageError) -> FailingKv {
        FailingKv {
            error,
            calls: Mutex::new(0),
        }
    }

    /// How many operations were attempted.
    pub fn calls(&self) -> usize {
        *self.calls.lock()
    }

    fn refuse<T>(&self) -> Result<T, StorageError> {
        *self.calls.lock() += 1;
        Err(self.error.clone())
    }
}

#[undra_macros::port]
impl Kv for FailingKv {
    async fn get(&self, _key: String) -> Result<Option<Bytes>, StorageError> {
        self.refuse()
    }

    async fn set(&self, _key: String, _value: Bytes) -> Result<(), StorageError> {
        self.refuse()
    }

    async fn delete(&self, _key: String) -> Result<(), StorageError> {
        self.refuse()
    }

    async fn list(&self, _prefix: String) -> Result<Vec<String>, StorageError> {
        self.refuse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::testing::block_on;

    #[test]
    fn get_set_delete_round_trip() {
        let kv = MemKv::new();
        assert_eq!(block_on(kv.get("k".into())), Ok(None));
        block_on(kv.set("k".into(), Bytes(vec![1, 2]))).unwrap();
        assert_eq!(block_on(kv.get("k".into())), Ok(Some(Bytes(vec![1, 2]))));
        block_on(kv.set("k".into(), Bytes(vec![3]))).unwrap();
        assert_eq!(
            block_on(kv.get("k".into())),
            Ok(Some(Bytes(vec![3]))),
            "set replaces"
        );
        block_on(kv.delete("k".into())).unwrap();
        block_on(kv.delete("missing".into())).unwrap();
        assert_eq!(block_on(kv.get("k".into())), Ok(None));
        assert!(kv.is_empty());
    }

    #[test]
    fn list_is_prefix_filtered_and_ascending() {
        let kv = MemKv::new();
        for key in ["b/2", "a/2", "a/1", "ab", "b/1", "", "a"] {
            block_on(kv.set(key.into(), Bytes(vec![]))).unwrap();
        }
        assert_eq!(block_on(kv.list("a/".into())).unwrap(), ["a/1", "a/2"]);
        assert_eq!(
            block_on(kv.list("a".into())).unwrap(),
            ["a", "a/1", "a/2", "ab"]
        );
        assert_eq!(
            block_on(kv.list("".into())).unwrap(),
            ["", "a", "a/1", "a/2", "ab", "b/1", "b/2"]
        );
        assert_eq!(
            block_on(kv.list("zzz".into())).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn empty_values_and_unicode_keys() {
        let kv = MemKv::new();
        block_on(kv.set("caf\u{e9}/\u{1F30A}".into(), Bytes(vec![]))).unwrap();
        assert_eq!(
            block_on(kv.get("caf\u{e9}/\u{1F30A}".into())),
            Ok(Some(Bytes(vec![]))),
            "an empty value is not a missing key"
        );
        assert_eq!(block_on(kv.list("caf\u{e9}".into())).unwrap().len(), 1);
    }

    #[test]
    fn operations_are_recorded_and_inspection_is_not() {
        let kv = MemKv::new();
        kv.insert("seed", vec![1]);
        assert_eq!(kv.value("seed"), Some(vec![1]));
        assert!(kv.contains_key("seed"));
        assert!(
            kv.ops().is_empty(),
            "seeding and peeking are not operations"
        );
        block_on(kv.set("a".into(), Bytes(vec![]))).unwrap();
        block_on(kv.get("a".into())).unwrap();
        block_on(kv.list("".into())).unwrap();
        block_on(kv.delete("a".into())).unwrap();
        assert_eq!(
            kv.take_ops(),
            [
                StoreOp::Set("a".into()),
                StoreOp::Get("a".into()),
                StoreOp::List("".into()),
                StoreOp::Delete("a".into()),
            ]
        );
        assert!(kv.ops().is_empty());
        assert_eq!(kv.keys(), ["seed"]);
        assert_eq!(kv.entries().len(), 1);
        assert_eq!(kv.len(), 1);
        kv.clear();
        assert!(kv.is_empty());
    }

    #[test]
    fn secure_store_is_an_independent_store() {
        let kv = MemKv::new();
        let secrets = MemSecureStore::new();
        block_on(secrets.set("token".into(), Bytes(vec![7]))).unwrap();
        assert_eq!(
            block_on(secrets.get("token".into())),
            Ok(Some(Bytes(vec![7])))
        );
        assert_eq!(block_on(kv.get("token".into())), Ok(None));
        assert_eq!(block_on(secrets.list("t".into())).unwrap(), ["token"]);
        block_on(secrets.delete("token".into())).unwrap();
        assert!(secrets.is_empty());
    }

    #[test]
    fn an_injected_failure_leaves_the_data_alone_until_healed() {
        let kv = MemKv::new();
        kv.insert("k", vec![1]);
        kv.fail(FailOn::Any, StorageError::Locked);
        assert_eq!(block_on(kv.get("k".into())), Err(StorageError::Locked));
        assert_eq!(
            block_on(kv.set("k".into(), Bytes(vec![2]))),
            Err(StorageError::Locked)
        );
        assert_eq!(block_on(kv.delete("k".into())), Err(StorageError::Locked));
        assert_eq!(block_on(kv.list("".into())), Err(StorageError::Locked));
        assert_eq!(kv.value("k"), Some(vec![1]), "nothing was changed");
        assert_eq!(kv.failed_ops(), 4);
        assert_eq!(kv.ops().len(), 4, "attempts are recorded");
        kv.heal();
        assert_eq!(block_on(kv.get("k".into())), Ok(Some(Bytes(vec![1]))));
    }

    #[test]
    fn a_counted_failure_is_used_up_and_matches_only_its_operation() {
        let kv = MemKv::new();
        kv.fail_times(FailOn::Set, StorageError::Full, 2);
        assert_eq!(block_on(kv.get("k".into())), Ok(None), "a get is not a set");
        assert_eq!(
            block_on(kv.set("k".into(), Bytes(vec![1]))),
            Err(StorageError::Full)
        );
        assert_eq!(
            block_on(kv.set("k".into(), Bytes(vec![1]))),
            Err(StorageError::Full)
        );
        assert_eq!(block_on(kv.set("k".into(), Bytes(vec![1]))), Ok(()));
        assert_eq!(kv.value("k"), Some(vec![1]));
        kv.fail_times(FailOn::Delete, StorageError::Io("x".into()), 0);
        assert_eq!(
            block_on(kv.delete("k".into())),
            Ok(()),
            "zero times is none"
        );
    }

    #[test]
    fn a_failing_kv_fails_everything() {
        let kv = FailingKv::new(StorageError::Unavailable("gone".into()));
        let gone = StorageError::Unavailable("gone".into());
        assert_eq!(block_on(kv.get("k".into())), Err(gone.clone()));
        assert_eq!(
            block_on(kv.set("k".into(), Bytes(vec![]))),
            Err(gone.clone())
        );
        assert_eq!(block_on(kv.delete("k".into())), Err(gone.clone()));
        assert_eq!(block_on(kv.list("k".into())), Err(gone));
        assert_eq!(kv.calls(), 4);
    }
}
