//! `ctx.http()`, `ctx.kv()`, ...: the typed accessors of SPEC 5.3 as methods on [`Ctx`].

use std::sync::Arc;

use undra_runtime::Ctx;

use crate::{Clock, Fs, Http, Kv, Log, Rng, SecureStore, Timer};

/// The standard ports as methods on [`Ctx`] (SPEC 5.3).
///
/// Each method is the accessor `#[undra::port]` generated for the trait (`undra_ports::http(ctx)`,
/// ...): the Rust binding if one is installed (a fake), otherwise a proxy to the platform.
///
/// ```
/// use undra_ports::{Clock, CtxPorts, fakes};
/// use undra_runtime::testing::TestRuntime;
///
/// let t = TestRuntime::new();
/// let fakes = fakes::install(&t);
/// fakes.clock.set_now_ms(7);
/// assert_eq!(t.ctx().clock().now_ms(), 7);
/// ```
pub trait CtxPorts {
    /// The `Clock` port.
    fn clock(&self) -> Arc<dyn Clock>;
    /// The `Rng` port.
    fn rng(&self) -> Arc<dyn Rng>;
    /// The `Log` port.
    fn log(&self) -> Arc<dyn Log>;
    /// The `Http` port.
    fn http(&self) -> Arc<dyn Http>;
    /// The `Kv` port.
    fn kv(&self) -> Arc<dyn Kv>;
    /// The `SecureStore` port.
    fn secure_store(&self) -> Arc<dyn SecureStore>;
    /// The `Fs` port.
    fn fs(&self) -> Arc<dyn Fs>;
    /// The `Timer` port.
    fn timer(&self) -> Arc<dyn Timer>;
    /// The `WebSocket` port (feature `websocket`; prefer [`WsConnection`](crate::ws::WsConnection)).
    #[cfg(feature = "websocket")]
    fn web_socket(&self) -> Arc<dyn crate::WebSocket>;
    /// The `Sse` port (feature `sse`; prefer [`sse::subscribe`](crate::sse::subscribe)).
    #[cfg(feature = "sse")]
    fn sse(&self) -> Arc<dyn crate::Sse>;
    /// The `Db` port (feature `db`; prefer [`Database`](crate::db::Database)).
    #[cfg(feature = "db")]
    fn db(&self) -> Arc<dyn crate::Db>;
}

impl CtxPorts for Ctx {
    fn clock(&self) -> Arc<dyn Clock> {
        crate::clock(self)
    }

    fn rng(&self) -> Arc<dyn Rng> {
        crate::rng(self)
    }

    fn log(&self) -> Arc<dyn Log> {
        crate::log(self)
    }

    fn http(&self) -> Arc<dyn Http> {
        crate::http(self)
    }

    fn kv(&self) -> Arc<dyn Kv> {
        crate::kv(self)
    }

    fn secure_store(&self) -> Arc<dyn SecureStore> {
        crate::secure_store(self)
    }

    fn fs(&self) -> Arc<dyn Fs> {
        crate::fs(self)
    }

    fn timer(&self) -> Arc<dyn Timer> {
        crate::timer(self)
    }

    #[cfg(feature = "websocket")]
    fn web_socket(&self) -> Arc<dyn crate::WebSocket> {
        crate::web_socket(self)
    }

    #[cfg(feature = "sse")]
    fn sse(&self) -> Arc<dyn crate::Sse> {
        crate::sse(self)
    }

    #[cfg(feature = "db")]
    fn db(&self) -> Arc<dyn crate::Db> {
        crate::db(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes;
    use undra_runtime::testing::TestRuntime;
    use undra_wire::Bytes;

    #[test]
    fn every_method_reaches_its_fake() {
        let t = TestRuntime::new();
        let fakes = fakes::install(&t);
        let ctx = t.ctx();
        fakes.clock.set_now_ms(11);
        assert_eq!(ctx.clock().now_ms(), 11);
        assert_eq!(ctx.rng().fill(4), fakes::SeededRng::default().fill(4));
        ctx.log().log(2, "t".into(), "m".into());
        assert_eq!(fakes.log.messages(), ["m"]);
        fakes.http.respond(
            fakes::Matcher::any(),
            crate::HttpResponse::new(204, Vec::new()),
        );
        let response = t
            .run_until(ctx.http().request(crate::HttpRequest::get("u")))
            .unwrap();
        assert_eq!(response.status, 204);
        t.run_until(ctx.kv().set("k".into(), Bytes(vec![1])))
            .unwrap();
        t.run_until(ctx.secure_store().set("s".into(), Bytes(vec![2])))
            .unwrap();
        t.run_until(ctx.fs().write("f".into(), Bytes(vec![3])))
            .unwrap();
        ctx.timer().set(1, 5);
        assert_eq!(fakes.kv.value("k"), Some(vec![1]));
        assert_eq!(fakes.secure_store.value("s"), Some(vec![2]));
        assert_eq!(fakes.fs.contents("f"), Some(vec![3]));
        assert_eq!(fakes.clock.pending_timer_ids(), [1]);
    }

    #[test]
    fn without_fakes_the_methods_return_proxies_to_the_platform() {
        let t = TestRuntime::new();
        t.host().script_port_ok(
            <dyn Clock as undra_runtime::Port>::PORT_ID,
            undra_meta::ids::port_method_id("Clock", "now_ms"),
            9_i64.to_le_bytes().to_vec(),
        );
        assert_eq!(t.ctx().clock().now_ms(), 9);
    }
}
