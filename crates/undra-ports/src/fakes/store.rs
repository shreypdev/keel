//! [`MemKv`] and [`MemSecureStore`]: in-memory key-value stores.

use core::ops::Deref;
use std::collections::BTreeMap;

use parking_lot::Mutex;
use undra_wire::Bytes;

use crate::{Kv, SecureStore};

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

#[derive(Default)]
struct Inner {
    map: BTreeMap<String, Vec<u8>>,
    ops: Vec<StoreOp>,
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

    fn port_get(&self, key: String) -> Option<Bytes> {
        let mut inner = self.inner.lock();
        let value = inner.map.get(&key).cloned().map(Bytes);
        inner.ops.push(StoreOp::Get(key));
        value
    }

    fn port_set(&self, key: String, value: Bytes) {
        let mut inner = self.inner.lock();
        inner.map.insert(key.clone(), value.0);
        inner.ops.push(StoreOp::Set(key));
    }

    fn port_delete(&self, key: String) {
        let mut inner = self.inner.lock();
        inner.map.remove(&key);
        inner.ops.push(StoreOp::Delete(key));
    }

    fn port_list(&self, prefix: String) -> Vec<String> {
        let mut inner = self.inner.lock();
        let keys = inner
            .map
            .range(prefix.clone()..)
            .map(|(key, _)| key)
            .take_while(|key| key.starts_with(&prefix))
            .cloned()
            .collect();
        inner.ops.push(StoreOp::List(prefix));
        keys
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
            async fn get(&self, key: String) -> Option<Bytes> {
                self.0.port_get(key)
            }

            async fn set(&self, key: String, value: Bytes) {
                self.0.port_set(key, value);
            }

            async fn delete(&self, key: String) {
                self.0.port_delete(key);
            }

            async fn list(&self, prefix: String) -> Vec<String> {
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
    /// assert_eq!(t.run_until(kv.get("greeting".into())), Some(Bytes(b"hi".to_vec())));
    /// t.run_until(kv.set("a/1".into(), Bytes(vec![1])));
    /// assert_eq!(t.run_until(kv.list("a/".into())), ["a/1"]);
    /// assert_eq!(kv.value("a/1"), Some(vec![1]));
    /// ```
    MemKv, Kv
}

mem_port! {
    /// An in-memory [`SecureStore`] port: the same behaviour as [`MemKv`], under its own port id.
    MemSecureStore, SecureStore
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::testing::block_on;

    #[test]
    fn get_set_delete_round_trip() {
        let kv = MemKv::new();
        assert_eq!(block_on(kv.get("k".into())), None);
        block_on(kv.set("k".into(), Bytes(vec![1, 2])));
        assert_eq!(block_on(kv.get("k".into())), Some(Bytes(vec![1, 2])));
        block_on(kv.set("k".into(), Bytes(vec![3])));
        assert_eq!(
            block_on(kv.get("k".into())),
            Some(Bytes(vec![3])),
            "set replaces"
        );
        block_on(kv.delete("k".into()));
        block_on(kv.delete("missing".into()));
        assert_eq!(block_on(kv.get("k".into())), None);
        assert!(kv.is_empty());
    }

    #[test]
    fn list_is_prefix_filtered_and_ascending() {
        let kv = MemKv::new();
        for key in ["b/2", "a/2", "a/1", "ab", "b/1", "", "a"] {
            block_on(kv.set(key.into(), Bytes(vec![])));
        }
        assert_eq!(block_on(kv.list("a/".into())), ["a/1", "a/2"]);
        assert_eq!(block_on(kv.list("a".into())), ["a", "a/1", "a/2", "ab"]);
        assert_eq!(
            block_on(kv.list("".into())),
            ["", "a", "a/1", "a/2", "ab", "b/1", "b/2"]
        );
        assert_eq!(block_on(kv.list("zzz".into())), Vec::<String>::new());
    }

    #[test]
    fn empty_values_and_unicode_keys() {
        let kv = MemKv::new();
        block_on(kv.set("caf\u{e9}/\u{1F30A}".into(), Bytes(vec![])));
        assert_eq!(
            block_on(kv.get("caf\u{e9}/\u{1F30A}".into())),
            Some(Bytes(vec![])),
            "an empty value is not a missing key"
        );
        assert_eq!(block_on(kv.list("caf\u{e9}".into())).len(), 1);
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
        block_on(kv.set("a".into(), Bytes(vec![])));
        block_on(kv.get("a".into()));
        block_on(kv.list("".into()));
        block_on(kv.delete("a".into()));
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
        block_on(secrets.set("token".into(), Bytes(vec![7])));
        assert_eq!(block_on(secrets.get("token".into())), Some(Bytes(vec![7])));
        assert_eq!(block_on(kv.get("token".into())), None);
        assert_eq!(block_on(secrets.list("t".into())), ["token"]);
        block_on(secrets.delete("token".into()));
        assert!(secrets.is_empty());
    }
}
