//! The conformance file: what every implementation of the deterministic fakes must answer.
//!
//! `undra::ports::fakes` is the reference. The Swift, Kotlin and TypeScript testing kits each
//! carry their own fakes (a core behind the C ABI cannot have Rust fakes installed in it), and
//! this module runs scripted scenarios against the Rust ones and writes what they answered to
//! `testkit/conformance/fakes.json`. Each kit replays that file against its fakes in its own test
//! suite, so the four cannot drift apart. [`generate`] makes the text; the test of this crate
//! fails when the checked-in file differs, and `UNDRA_BLESS=1 cargo test -p undra-testkit
//! conformance` rewrites it.
//!
//! The file is one JSON document with five sections (`rng`, `clock`, `store`, `fs`, `http`), each
//! a list of cases whose `steps` carry the inputs and the `result` the Rust fakes gave. Bytes are
//! hex, errors are `{"error": "<kind>", "message": ..}`.

use core::time::Duration;
use std::fmt::Write as _;

use undra_ports::fakes::{FakeClock, Fakes, MemFs, MemKv, SeededRng};
use undra_ports::{Clock, Fs, FsError, Http, HttpError, HttpMethod, HttpRequest, Kv, Rng, Timer};
use undra_runtime::testing::TestRuntime;
use undra_wire::Bytes;

use crate::hex;
use crate::seed::Seed;

/// The version of the conformance file.
pub const CONFORMANCE_VERSION: u32 = 1;

fn q(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

fn list(items: impl IntoIterator<Item = String>) -> String {
    format!("[{}]", items.into_iter().collect::<Vec<_>>().join(","))
}

fn quoted_list(items: &[String]) -> String {
    list(items.iter().map(|s| q(s)))
}

fn obj(fields: &[(&str, String)]) -> String {
    let body: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("{}:{v}", q(k)))
        .collect();
    format!("{{{}}}", body.join(","))
}

fn fs_result(r: Result<String, FsError>) -> String {
    match r {
        Ok(v) => v,
        Err(FsError::NotFound) => obj(&[("error", q("not_found"))]),
        Err(FsError::Denied) => obj(&[("error", q("denied"))]),
        Err(FsError::Io(m)) => obj(&[("error", q("io")), ("message", q(&m))]),
    }
}

fn section(out: &mut String, name: &str, cases: &[String], last: bool) {
    let _ = writeln!(out, "  {}: [", q(name));
    for (i, case) in cases.iter().enumerate() {
        let comma = if i + 1 < cases.len() { "," } else { "" };
        let _ = writeln!(out, "    {case}{comma}");
    }
    let _ = writeln!(out, "  ]{}", if last { "" } else { "," });
}

fn rng_cases() -> Vec<String> {
    let seeds: [(&str, u64); 3] = [
        ("42", 42),
        ("0", 0),
        ("\"0x4b45454c5f524e47\"", SeededRng::DEFAULT_SEED),
    ];
    seeds
        .iter()
        .map(|(json, seed)| {
            let rng = SeededRng::new(*seed);
            let fills = [8_u32, 3, 0, 16, 9, 1];
            let results: Vec<String> = fills
                .iter()
                .map(|n| q(&hex::encode(&rng.fill(*n).0)))
                .collect();
            obj(&[
                ("seed", (*json).to_owned()),
                ("fills", list(fills.iter().map(u32::to_string))),
                ("results", list(results)),
            ])
        })
        .collect()
}

fn clock_cases() -> Vec<String> {
    let clock = FakeClock::new();
    let state = |c: &FakeClock| {
        obj(&[
            ("now_ms", c.now_ms().to_string()),
            ("monotonic_ns", c.monotonic_ns().to_string()),
        ])
    };
    let mut steps = Vec::new();
    for (id, delay) in [(7_u32, 500_u64), (8, 100), (9, 100)] {
        clock.set(id, delay);
        steps.push(obj(&[
            ("op", q("timer")),
            ("id", id.to_string()),
            ("delay_ms", delay.to_string()),
        ]));
    }
    let advance = |ms: u64, steps: &mut Vec<String>| {
        let fired = clock.advance(Duration::from_millis(ms));
        steps.push(obj(&[
            ("op", q("advance")),
            ("ms", ms.to_string()),
            ("fired", list(fired.iter().map(u32::to_string))),
            ("state", state(&clock)),
        ]));
    };
    advance(50, &mut steps);
    advance(50, &mut steps);
    clock.set_now_ms(5_000);
    steps.push(obj(&[
        ("op", q("set_now")),
        ("ms", "5000".to_owned()),
        ("state", state(&clock)),
    ]));
    advance(500, &mut steps);
    clock.set(10, 0);
    steps.push(obj(&[
        ("op", q("timer")),
        ("id", "10".to_owned()),
        ("delay_ms", "0".to_owned()),
    ]));
    advance(0, &mut steps);
    vec![obj(&[
        ("now_ms", FakeClock::DEFAULT_NOW_MS.to_string()),
        ("steps", list(steps)),
    ])]
}

fn store_cases() -> Vec<String> {
    let t = TestRuntime::new();
    let kv = MemKv::new();
    let mut steps = Vec::new();
    let set = |key: &str, value: &[u8], steps: &mut Vec<String>| {
        t.run_until(kv.set(key.to_owned(), Bytes(value.to_vec())));
        steps.push(obj(&[
            ("op", q("set")),
            ("key", q(key)),
            ("value", q(&hex::encode(value))),
        ]));
    };
    set("b", b"2", &mut steps);
    set("a", b"1", &mut steps);
    set("a/1", b"x", &mut steps);
    // UTF-8 byte order, not UTF-16: U+FF5E sorts before U+1F600 here, and after it in UTF-16.
    set("\u{1f600}", b"e", &mut steps);
    set("\u{ff5e}", b"w", &mut steps);
    for prefix in ["", "a", "a/", "\u{ff}"] {
        let keys = t.run_until(kv.list(prefix.to_owned()));
        steps.push(obj(&[
            ("op", q("list")),
            ("prefix", q(prefix)),
            ("result", quoted_list(&keys)),
        ]));
    }
    for key in ["a", "zz"] {
        let value = t.run_until(kv.get(key.to_owned()));
        steps.push(obj(&[
            ("op", q("get")),
            ("key", q(key)),
            (
                "result",
                value.map_or("null".to_owned(), |v| q(&hex::encode(&v.0))),
            ),
        ]));
    }
    for key in ["a", "missing"] {
        t.run_until(kv.delete(key.to_owned()));
        steps.push(obj(&[("op", q("delete")), ("key", q(key))]));
    }
    let after = t.run_until(kv.get("a".to_owned()));
    steps.push(obj(&[
        ("op", q("get")),
        ("key", q("a")),
        (
            "result",
            after.map_or("null".to_owned(), |v| q(&hex::encode(&v.0))),
        ),
    ]));
    vec![obj(&[("steps", list(steps))])]
}

fn fs_cases() -> Vec<String> {
    let t = TestRuntime::new();
    let fs = MemFs::new();
    let mut steps = Vec::new();
    let write = |path: &str, data: &[u8], steps: &mut Vec<String>| {
        let r = t.run_until(fs.write(path.to_owned(), Bytes(data.to_vec())));
        steps.push(obj(&[
            ("op", q("write")),
            ("path", q(path)),
            ("data", q(&hex::encode(data))),
            ("result", fs_result(r.map(|()| "null".to_owned()))),
        ]));
    };
    write("docs/notes/a.txt", b"hi", &mut steps);
    write("docs/b.txt", b"b", &mut steps);
    write("docs", b"x", &mut steps);
    write("", b"x", &mut steps);
    write("docs/b.txt/x", b"x", &mut steps);
    write("../x", b"x", &mut steps);
    write("./docs//c.txt", b"c", &mut steps);
    let read = |path: &str, steps: &mut Vec<String>| {
        let r = t.run_until(fs.read(path.to_owned()));
        steps.push(obj(&[
            ("op", q("read")),
            ("path", q(path)),
            ("result", fs_result(r.map(|b| q(&hex::encode(&b.0))))),
        ]));
    };
    read("docs/notes/a.txt", &mut steps);
    read("docs//b.txt", &mut steps);
    read("docs/missing", &mut steps);
    read("docs", &mut steps);
    read("", &mut steps);
    read("a/../b", &mut steps);
    let ls = |dir: &str, steps: &mut Vec<String>| {
        let r = t.run_until(fs.list(dir.to_owned()));
        steps.push(obj(&[
            ("op", q("list")),
            ("dir", q(dir)),
            ("result", fs_result(r.map(|names| quoted_list(&names)))),
        ]));
    };
    ls("docs", &mut steps);
    ls("", &mut steps);
    ls("docs/b.txt", &mut steps);
    ls("nope", &mut steps);
    let delete = |path: &str, steps: &mut Vec<String>| {
        let r = t.run_until(fs.delete(path.to_owned()));
        steps.push(obj(&[
            ("op", q("delete")),
            ("path", q(path)),
            ("result", fs_result(r.map(|()| "null".to_owned()))),
        ]));
    };
    delete("docs/notes", &mut steps);
    delete("nope", &mut steps);
    delete("", &mut steps);
    read("docs/notes/a.txt", &mut steps);
    ls("docs", &mut steps);
    vec![obj(&[("steps", list(steps))])]
}

fn method_name(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "get",
        HttpMethod::Post => "post",
        HttpMethod::Put => "put",
        HttpMethod::Delete => "delete",
        HttpMethod::Patch => "patch",
        HttpMethod::Head => "head",
        HttpMethod::Options => "options",
    }
}

fn http_cases() -> Vec<String> {
    const RULES: &str = r#"[{"url":"https://x.test/a","method":"get","status":201,"headers":[["content-type","text/plain"],["x-n","1"],["x-n","2"]],"body":"ok"},{"url":"https://x.test/a","status":405},{"url_prefix":"https://x.test/slow","error":"timeout"},{"url_prefix":"https://x.test/gone","error":{"network":"down"}},{"url_prefix":"https://x.test/bad","error":{"invalid_url":"nope"}},{"url_prefix":"https://x.test/","method":"post","status":202,"body":{"hex":"00ff"}},{"method":"delete","error":"cancelled"}]"#;
    let seed = Seed::from_json(&format!("{{\"http\":{RULES}}}")).unwrap_or_default();
    let fakes = Fakes::new();
    let _ = seed.apply(&fakes);
    let t = TestRuntime::new();
    let requests = [
        (HttpMethod::Get, "https://x.test/a"),
        (HttpMethod::Post, "https://x.test/a"),
        (HttpMethod::Get, "https://x.test/slow/1"),
        (HttpMethod::Get, "https://x.test/gone"),
        (HttpMethod::Get, "https://x.test/bad"),
        (HttpMethod::Post, "https://x.test/other"),
        (HttpMethod::Delete, "https://y.test/z"),
        (HttpMethod::Get, "https://y.test/z"),
        (HttpMethod::Put, "https://x.test/other"),
    ];
    let steps: Vec<String> = requests
        .iter()
        .map(|(method, url)| {
            let r = t.run_until(fakes.http.request(HttpRequest::new(*method, *url)));
            let result = match r {
                Ok(resp) => obj(&[
                    ("status", resp.status.to_string()),
                    (
                        "headers",
                        list(resp.headers.iter().map(|h| list([q(&h.name), q(&h.value)]))),
                    ),
                    ("body", q(&hex::encode(&resp.body.0))),
                ]),
                Err(HttpError::Network(m)) => obj(&[("error", q("network")), ("message", q(&m))]),
                Err(HttpError::Timeout) => obj(&[("error", q("timeout"))]),
                Err(HttpError::Cancelled) => obj(&[("error", q("cancelled"))]),
                Err(HttpError::InvalidUrl(u)) => {
                    obj(&[("error", q("invalid_url")), ("message", q(&u))])
                }
            };
            obj(&[
                ("method", q(method_name(*method))),
                ("url", q(url)),
                ("result", result),
            ])
        })
        .collect();
    vec![obj(&[
        ("rules", RULES.to_owned()),
        ("requests", list(steps)),
    ])]
}

/// The text of `testkit/conformance/fakes.json`: the scenarios, run against the Rust fakes.
pub fn generate() -> String {
    let mut out = String::new();
    out.push_str("{\n");
    let _ = writeln!(out, "  \"version\": {CONFORMANCE_VERSION},");
    section(&mut out, "rng", &rng_cases(), false);
    section(&mut out, "clock", &clock_cases(), false);
    section(&mut out, "store", &store_cases(), false);
    section(&mut out, "fs", &fs_cases(), false);
    section(&mut out, "http", &http_cases(), true);
    out.push_str("}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generated_text_is_json_and_stable() {
        let text = generate();
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["version"], 1);
        for section in ["rng", "clock", "store", "fs", "http"] {
            assert!(
                doc[section].as_array().is_some_and(|a| !a.is_empty()),
                "{section}"
            );
        }
        assert_eq!(generate(), text, "generation is deterministic");
    }
}
