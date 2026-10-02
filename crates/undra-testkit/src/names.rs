//! The names of the standard ports and their methods, for the informational `name` of a recorded
//! port call. Ids stay authoritative; a name is only there for the person reading the file.

use undra_meta::ids::{port_id, port_method_id};

/// `(trait, methods)` of the ten standard ports (SPEC 8).
const STANDARD: &[(&str, &[&str])] = &[
    ("Clock", &["now_ms", "monotonic_ns"]),
    ("Rng", &["fill"]),
    ("Log", &["log"]),
    ("Http", &["request"]),
    ("Kv", &["get", "set", "delete", "list"]),
    ("SecureStore", &["get", "set", "delete", "list"]),
    ("Fs", &["read", "write", "delete", "list"]),
    ("Timer", &["set"]),
    ("Connectivity", &["changed"]),
    ("Lifecycle", &["changed"]),
];

/// `"Http.request"` for the standard port method `(port, method)`, `None` for anything else.
pub fn standard_name(port: u32, method: u32) -> Option<String> {
    STANDARD
        .iter()
        .filter(|(port_name, _)| port_id(port_name) == port)
        .flat_map(|(port_name, methods)| methods.iter().map(move |m| (*port_name, *m)))
        .find(|(port_name, m)| port_method_id(port_name, m) == method)
        .map(|(port_name, m)| format!("{port_name}.{m}"))
}

/// The id of the standard port named `name` (`"Http"`), if there is one.
pub fn standard_port(name: &str) -> Option<u32> {
    STANDARD
        .iter()
        .find(|(port_name, _)| *port_name == name)
        .map(|(port_name, _)| port_id(port_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_standard_methods_and_nothing_else() {
        let http = standard_port("Http").unwrap();
        assert_eq!(
            standard_name(http, port_method_id("Http", "request")).as_deref(),
            Some("Http.request")
        );
        assert_eq!(standard_name(http, 7), None);
        assert_eq!(standard_name(1, 2), None);
        assert_eq!(standard_port("Nope"), None);
        // Kv and SecureStore share method names but not ids.
        let secure = standard_port("SecureStore").unwrap();
        assert_eq!(
            standard_name(secure, port_method_id("SecureStore", "get")).as_deref(),
            Some("SecureStore.get")
        );
    }
}
