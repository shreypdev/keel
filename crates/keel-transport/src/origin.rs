//! Which web pages may open a session.
//!
//! WebSockets are not subject to the browser's same-origin policy: any page the developer has
//! open in a browser can connect to `ws://127.0.0.1:<port>` and, once connected, call anything
//! the core exposes. The upgrade request tells the server which page is asking (the `Origin`
//! header, which a page cannot forge), so that is where this is stopped.
//!
//! Native clients (URLSession, the JDK, Node) send no `Origin` and are always accepted.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Which `Origin` headers a [`Server`](crate::Server) accepts on the WebSocket upgrade.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum OriginPolicy {
    /// No `Origin` at all (every native runtime), or an origin whose host is this machine or
    /// a private network: `localhost`, `*.localhost`, `*.local`, loopback, RFC 1918, link-local,
    /// unique-local IPv6 and the CGNAT range Tailscale uses. A page on the public internet is
    /// refused with `403`. The default.
    #[default]
    LocalNetwork,
    /// [`LocalNetwork`](OriginPolicy::LocalNetwork), plus these exact origins
    /// (`"https://dev.example.com"`), for a web app served from a tunnel or a real host name.
    LocalNetworkAnd(Vec<String>),
    /// Any origin. Only for a machine nobody else's pages can reach the server from.
    Any,
}

impl OriginPolicy {
    /// Whether an upgrade whose `Origin` header is `origin` (`None`: no header) is accepted.
    pub fn allows(&self, origin: Option<&str>) -> bool {
        let Some(origin) = origin else {
            return true;
        };
        match self {
            OriginPolicy::Any => true,
            OriginPolicy::LocalNetwork => is_local_network(origin),
            OriginPolicy::LocalNetworkAnd(extra) => {
                is_local_network(origin) || extra.iter().any(|allowed| allowed == origin)
            }
        }
    }
}

/// Whether `origin` (`scheme://host[:port]`) names a host on this machine or a private network.
fn is_local_network(origin: &str) -> bool {
    let Some((_, rest)) = origin.split_once("://") else {
        return false; // "null", or not an origin at all
    };
    let authority = rest.split('/').next().unwrap_or_default();
    // An Origin has no userinfo; if one is present the real host is what follows the `@`.
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => match authority.rsplit_once(':') {
            Some((host, port)) if port.bytes().all(|b| b.is_ascii_digit()) => host,
            _ => authority,
        },
    };
    let host = host.to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => is_local_v4(ip),
        Ok(IpAddr::V6(ip)) => is_local_v6(ip),
        Err(_) => false,
    }
}

fn is_local_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || (a == 100 && (64..128).contains(&b)) // 100.64.0.0/10, carrier-grade NAT (Tailscale)
}

fn is_local_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    ip.is_loopback()
        || (first & 0xfe00) == 0xfc00 // fc00::/7 unique local
        || (first & 0xffc0) == 0xfe80 // fe80::/10 link local
        || ip.to_ipv4_mapped().is_some_and(is_local_v4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(origin: &str) -> bool {
        OriginPolicy::LocalNetwork.allows(Some(origin))
    }

    #[test]
    fn native_clients_send_no_origin_and_are_accepted() {
        for policy in [
            OriginPolicy::LocalNetwork,
            OriginPolicy::LocalNetworkAnd(vec![]),
            OriginPolicy::Any,
        ] {
            assert!(policy.allows(None));
        }
    }

    #[test]
    fn pages_served_from_this_machine_or_a_private_network_are_accepted() {
        for origin in [
            "http://localhost",
            "http://localhost:5173",
            "https://LocalHost:3000",
            "http://app.localhost:8080",
            "http://127.0.0.1:5173",
            "http://127.1.2.3",
            "http://[::1]:5173",
            "http://192.168.1.20:5173",
            "http://10.0.0.7",
            "http://172.20.1.1:80",
            "http://169.254.10.10",
            "http://100.101.102.103:3000",
            "http://macbook.local:5173",
            "capacitor://localhost",
            "http://[fd12:3456::1]:80",
            "http://[fe80::1]",
            "http://[::ffff:192.168.0.1]",
        ] {
            assert!(local(origin), "{origin}");
        }
    }

    #[test]
    fn pages_from_anywhere_else_are_refused() {
        for origin in [
            "https://evil.example",
            "https://evil.example:443",
            "http://localhost.evil.example",
            "http://127.0.0.1.evil.example",
            "http://8.8.8.8",
            "http://172.32.0.1",
            "http://100.128.0.1",
            "http://[2001:db8::1]",
            "http://192.168.1.1@evil.example",
            "chrome-extension://abcdefghijklmnop",
            "null",
            "",
            "localhost",
        ] {
            assert!(!local(origin), "{origin}");
        }
    }

    #[test]
    fn extra_origins_are_exact_and_any_allows_everything() {
        let policy = OriginPolicy::LocalNetworkAnd(vec!["https://dev.example.com".into()]);
        assert!(policy.allows(Some("https://dev.example.com")));
        assert!(policy.allows(Some("http://localhost:1")));
        assert!(!policy.allows(Some("https://dev.example.com:444")));
        assert!(!policy.allows(Some("https://other.example.com")));
        assert!(OriginPolicy::Any.allows(Some("https://evil.example")));
    }
}
