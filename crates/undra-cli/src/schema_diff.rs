//! The difference between two schemas, in the words of a reviewer (`undra schema diff`, ADR-062).
//!
//! The generated bindings are an artifact of the schema, so the schema is what is reviewed and
//! versioned. [`diff`] compares two of them and returns one [`Change`] per thing an app can notice,
//! each marked [`Severity::Breaking`] or [`Severity::Additive`] by the rules of `docs/SPEC.md` 2.6:
//! the question a line answers is whether code written against the old bindings still compiles and
//! means the same against the new ones, in every one of the three languages. A removed or changed
//! signature is breaking; an added function, method, store, object or record is additive; an added
//! record field is breaking even with a default, since a TypeScript object literal must name it.
//!
//! The comparison is by name, never by position in a list (every list of a schema is unordered but
//! for the ones that are part of the wire layout), and the output order is fixed: types, objects and
//! stores, functions, ports, callbacks, queries; by name within a group; the members of one item in
//! the order its definition lists them. The same two schemas always print the same text.
//!
//! Ids are not compared (all of them but a signal's position derive from a name, SPEC 1.1) and
//! neither are doc comments, so a schema exported with and without `--docs` has no difference.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use undra_bindgen::stdlib;
use undra_meta::{
    EnumDef, FieldDef, FunctionDef, GenericOf, MethodDef, ObjectDef, ParamDef, PortDef, PortKind,
    QueryDef, QueryKind, RecordDef, Schema, SignalDef, TypeRef, VariantDef,
};

/// How a change affects code written against the old bindings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Severity {
    /// Compatible: nothing an app wrote against the old schema stops compiling or changes its types.
    /// Includes a behaviour change that no signature shows (a query's stale time).
    Additive,
    /// Code that compiled against the old bindings can stop compiling (or change what it means).
    Breaking,
}

impl Severity {
    /// The word a line starts with: `breaking` or `additive` (both eight letters, so lines align).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Severity::Additive => "additive",
            Severity::Breaking => "breaking",
        }
    }
}

/// One difference between two schemas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Whether it breaks code written against the old schema.
    pub severity: Severity,
    /// What kind of thing changed: `record`, `field`, `case`, `store`, `signal`, `method`,
    /// `function`, `query`, `port`, `callback`, ...
    pub noun: &'static str,
    /// Where: `Todo`, `Todo.title`, `Todos.add`, `fetch_todos`.
    pub path: String,
    /// What happened, as a clause: `added (u8, without a default)`, `type changed from A to B`.
    pub text: String,
}

impl Change {
    /// The line without its severity label: `field Todo.title: type changed from String to ...`.
    #[must_use]
    pub fn describe(&self) -> String {
        format!("{} {}: {}", self.noun, self.path, self.text)
    }

    /// The whole line as printed without colour: the label, two spaces, then [`Change::describe`].
    #[must_use]
    pub fn line(&self) -> String {
        format!("{}  {}", self.severity.label(), self.describe())
    }
}

/// Every difference between an old and a new schema, in the fixed order of the module's docs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaDiff {
    /// The differences.
    pub changes: Vec<Change>,
    /// The old schema's hash (SPEC 2.3).
    pub old_hash: u64,
    /// The new schema's hash.
    pub new_hash: u64,
}

impl SchemaDiff {
    /// How many changes are breaking.
    #[must_use]
    pub fn breaking(&self) -> usize {
        self.count(Severity::Breaking)
    }

    /// How many changes are additive.
    #[must_use]
    pub fn additive(&self) -> usize {
        self.count(Severity::Additive)
    }

    /// Whether any change is breaking: what `--exit-code` turns into an exit status.
    #[must_use]
    pub fn has_breaking(&self) -> bool {
        self.breaking() > 0
    }

    fn count(&self, severity: Severity) -> usize {
        self.changes
            .iter()
            .filter(|c| c.severity == severity)
            .count()
    }

    /// The closing line: how many changes of each kind, or that there are none (and, when the two
    /// hashes differ though no rule found a difference, that something outside the API moved).
    #[must_use]
    pub fn summary(&self) -> String {
        if !self.changes.is_empty() {
            return format!(
                "{} breaking, {} additive.",
                self.breaking(),
                self.additive()
            );
        }
        if self.old_hash == self.new_hash {
            format!(
                "No changes: the public API is the same (schema {:#018x}).",
                self.new_hash
            )
        } else {
            format!(
                "No change to the public API, though the schema hashes differ ({:#018x} and {:#018x}): \
                 a wire id differs, which no signature shows.",
                self.old_hash, self.new_hash
            )
        }
    }

    /// The whole report as plain text, without colour: see [`SchemaDiff::render_with`].
    #[must_use]
    pub fn render(&self, old_label: &str, new_label: &str) -> String {
        self.render_with(old_label, new_label, |_, label| label.to_owned())
    }

    /// The whole report: a header naming both sides and their schema hashes, a blank line, one line
    /// per change, a blank line (when there are changes) and the [`summary`](SchemaDiff::summary).
    /// `paint` styles the severity word of each line (the command colours it on a terminal).
    #[must_use]
    pub fn render_with(
        &self,
        old_label: &str,
        new_label: &str,
        paint: impl Fn(Severity, &str) -> String,
    ) -> String {
        let mut out = format!(
            "Public API: {old_label} (schema {:#018x}) -> {new_label} (schema {:#018x})\n\n",
            self.old_hash, self.new_hash
        );
        for change in &self.changes {
            let _ = writeln!(
                out,
                "{}  {}",
                paint(change.severity, change.severity.label()),
                change.describe()
            );
        }
        if !self.changes.is_empty() {
            out.push('\n');
        }
        let _ = writeln!(out, "{}", self.summary());
        out
    }
}

/// Compares two schemas by the compatibility rules of SPEC 2.6.
///
/// ```
/// use undra_meta::Schema;
///
/// let before = Schema::new("demo-core");
/// let after = Schema::new("demo-core");
/// assert!(undra_cli::schema_diff::diff(&before, &after).changes.is_empty());
/// ```
#[must_use]
pub fn diff(old: &Schema, new: &Schema) -> SchemaDiff {
    let mut out = Out::default();
    types(&mut out, old, new);
    objects(&mut out, old, new);
    functions(&mut out, old, new);
    ports(&mut out, old, new, false);
    ports(&mut out, old, new, true);
    queries(&mut out, old, new);
    SchemaDiff {
        changes: out.changes,
        old_hash: old.hash(),
        new_hash: new.hash(),
    }
}

// ----- plumbing -----------------------------------------------------------------------------

#[derive(Default)]
struct Out {
    changes: Vec<Change>,
}

impl Out {
    fn push(
        &mut self,
        severity: Severity,
        noun: &'static str,
        path: impl Into<String>,
        text: impl Into<String>,
    ) {
        self.changes.push(Change {
            severity,
            noun,
            path: path.into(),
            text: text.into(),
        });
    }

    fn breaking(&mut self, noun: &'static str, path: &str, text: impl Into<String>) {
        self.push(Severity::Breaking, noun, path, text);
    }

    fn additive(&mut self, noun: &'static str, path: &str, text: impl Into<String>) {
        self.push(Severity::Additive, noun, path, text);
    }
}

/// `items` by name, for a comparison that does not depend on the order of the lists.
fn by_name<T>(items: &[T], name: impl Fn(&T) -> &str) -> BTreeMap<&str, &T> {
    items.iter().map(|item| (name(item), item)).collect()
}

/// The names of both maps, each once, sorted.
fn union<'a, A, B>(a: &BTreeMap<&'a str, A>, b: &BTreeMap<&'a str, B>) -> Vec<&'a str> {
    a.keys()
        .chain(b.keys())
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// A type as Rust spells it: `Vec<Header>`, `Option<Bytes>`, `Arc<Mailbox>`, `()`.
fn ty(t: &TypeRef) -> String {
    let leaf = |name: &str| name.to_owned();
    match t {
        TypeRef::Bool => leaf("bool"),
        TypeRef::I8 => leaf("i8"),
        TypeRef::I16 => leaf("i16"),
        TypeRef::I32 => leaf("i32"),
        TypeRef::I64 => leaf("i64"),
        TypeRef::U8 => leaf("u8"),
        TypeRef::U16 => leaf("u16"),
        TypeRef::U32 => leaf("u32"),
        TypeRef::U64 => leaf("u64"),
        TypeRef::F32 => leaf("f32"),
        TypeRef::F64 => leaf("f64"),
        TypeRef::String => leaf("String"),
        TypeRef::Bytes => leaf("Bytes"),
        TypeRef::Unit => leaf("()"),
        TypeRef::Duration => leaf("Duration"),
        TypeRef::Timestamp => leaf("Timestamp"),
        TypeRef::Uuid => leaf("Uuid"),
        TypeRef::Decimal => leaf("Decimal"),
        TypeRef::Option(inner) => format!("Option<{}>", ty(inner)),
        TypeRef::Vec(inner) => format!("Vec<{}>", ty(inner)),
        TypeRef::Lazy(inner) => format!("Lazy<{}>", ty(inner)),
        TypeRef::Stream(inner) => format!("Stream<{}>", ty(inner)),
        TypeRef::Map(key, value) => format!("Map<{}, {}>", ty(key), ty(value)),
        TypeRef::Result(ok, err) => format!("Result<{}, {}>", ty(ok), ty(err)),
        TypeRef::Named(name) => name.clone(),
        TypeRef::Object(name) => format!("Arc<{name}>"),
        TypeRef::Callback(name) => format!("Arc<dyn {name}>"),
    }
}

/// `a: A, b: B`.
fn params_text(params: &[ParamDef]) -> String {
    params
        .iter()
        .map(|p| format!("{}: {}", p.name, ty(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `fn name(a: A) -> R`, with `async` first when it is, and no `-> ()`.
fn signature(name: &str, params: &[ParamDef], returns: &TypeRef, is_async: bool) -> String {
    let mut text = String::new();
    if is_async {
        text.push_str("async ");
    }
    let _ = write!(text, "fn {name}({})", params_text(params));
    if *returns != TypeRef::Unit {
        let _ = write!(text, " -> {}", ty(returns));
    }
    text
}

/// What a function, a method, a constructor and a port method have in common.
struct Callable<'a> {
    name: &'a str,
    params: &'a [ParamDef],
    returns: &'a TypeRef,
    is_async: bool,
    takes_ctx: bool,
    generic: Option<&'a GenericOf>,
}

impl<'a> From<&'a MethodDef> for Callable<'a> {
    fn from(m: &'a MethodDef) -> Self {
        Callable {
            name: &m.name,
            params: &m.params,
            returns: &m.returns,
            is_async: m.is_async,
            takes_ctx: m.takes_ctx,
            generic: m.generic.as_ref(),
        }
    }
}

impl<'a> From<&'a FunctionDef> for Callable<'a> {
    fn from(f: &'a FunctionDef) -> Self {
        Callable {
            name: &f.name,
            params: &f.params,
            returns: &f.returns,
            is_async: f.is_async,
            takes_ctx: f.takes_ctx,
            generic: f.generic.as_ref(),
        }
    }
}

impl Callable<'_> {
    fn signature(&self) -> String {
        signature(self.name, self.params, self.returns, self.is_async)
    }
}

/// Why `old` and `new` are not the same parameter list: each reason is a breaking change (a call
/// site must change). Added and removed parameters, a changed type, a rename (Swift labels and
/// Kotlin named arguments are the names) and a reorder, in that order.
fn param_changes(old: &[ParamDef], new: &[ParamDef]) -> Vec<String> {
    let mut reasons = Vec::new();
    let old_names: BTreeSet<&str> = old.iter().map(|p| p.name.as_str()).collect();
    let new_names: BTreeSet<&str> = new.iter().map(|p| p.name.as_str()).collect();
    // A parameter in the same place with the same type and another name is a rename, not a removal
    // and an addition.
    let mut renamed: BTreeMap<&str, &str> = BTreeMap::new();
    for (o, n) in old.iter().zip(new) {
        if o.name != n.name
            && o.ty == n.ty
            && !new_names.contains(o.name.as_str())
            && !old_names.contains(n.name.as_str())
        {
            renamed.insert(o.name.as_str(), n.name.as_str());
        }
    }
    let renamed_to: BTreeSet<&str> = renamed.values().copied().collect();
    for p in new {
        if !old_names.contains(p.name.as_str()) && !renamed_to.contains(p.name.as_str()) {
            reasons.push(format!("parameter `{}: {}` added", p.name, ty(&p.ty)));
        }
    }
    for p in old {
        if let Some(to) = renamed.get(p.name.as_str()) {
            reasons.push(format!("parameter `{}` renamed to `{to}`", p.name));
        } else if !new_names.contains(p.name.as_str()) {
            reasons.push(format!("parameter `{}: {}` removed", p.name, ty(&p.ty)));
        }
    }
    for p in old {
        if let Some(q) = new.iter().find(|q| q.name == p.name)
            && q.ty != p.ty
        {
            reasons.push(format!(
                "parameter `{}`: type changed from {} to {}",
                p.name,
                ty(&p.ty),
                ty(&q.ty)
            ));
        }
    }
    // The order of the parameters both lists have.
    let order = |list: &[ParamDef]| -> Vec<String> {
        list.iter()
            .map(|p| p.name.clone())
            .filter(|n| old_names.contains(n.as_str()) && new_names.contains(n.as_str()))
            .collect()
    };
    let (before, after) = (order(old), order(new));
    if before != after {
        reasons.push(format!(
            "parameters reordered ({} -> {})",
            before.join(", "),
            after.join(", ")
        ));
    }
    reasons
}

/// The changes between two callables of one name: parameters, return type, `async` and generic
/// label are breaking; `Ctx` is additive (no generated signature shows it).
fn callable_changes(out: &mut Out, noun: &'static str, path: &str, old: &Callable, new: &Callable) {
    for reason in param_changes(old.params, new.params) {
        out.breaking(noun, path, reason);
    }
    if old.returns != new.returns {
        out.breaking(
            noun,
            path,
            format!(
                "return type changed from {} to {}",
                ty(old.returns),
                ty(new.returns)
            ),
        );
    }
    if old.is_async != new.is_async {
        let text = if new.is_async {
            "now `async`"
        } else {
            "no longer `async`"
        };
        out.breaking(noun, path, text);
    }
    // `Ctx` is not a wire parameter (SPEC 2.2) and no generator reads `takes_ctx`: every generated
    // callable takes the core the same way, so this is visible inside the core only.
    if old.takes_ctx != new.takes_ctx {
        let text = if new.takes_ctx {
            "now takes a `Ctx` (inside the core: no generated signature shows it)"
        } else {
            "no longer takes a `Ctx` (inside the core: no generated signature shows it)"
        };
        out.additive(noun, path, text);
    }
    if old.generic != new.generic {
        let label = |g: Option<&GenericOf>| g.map_or_else(|| "none".to_owned(), GenericOf::name);
        out.breaking(
            noun,
            path,
            format!(
                "generic instantiation changed from {} to {}",
                label(old.generic),
                label(new.generic)
            ),
        );
    }
}

/// What a callable added to a list means for the app.
#[derive(Clone, Copy)]
enum Added {
    /// One more thing it may call (a function, an object's method, an event port's method).
    Plain,
    /// One more thing it may call, and why that is worth saying.
    Additive(&'static str),
    /// Something it implements and must now add (a sync or async port's method, a callback's), and why.
    Breaking(&'static str),
}

/// Compares the callables of two lists by name. `noun` names them; `path` is the owner (`Todos`) or
/// empty for a free function; what an added one means is `added`.
fn callables(
    out: &mut Out,
    noun: &'static str,
    owner: &str,
    old: &BTreeMap<&str, Callable>,
    new: &BTreeMap<&str, Callable>,
    added: Added,
) {
    for name in union(old, new) {
        let path = if owner.is_empty() {
            name.to_owned()
        } else {
            format!("{owner}.{name}")
        };
        match (old.get(name), new.get(name)) {
            (None, Some(n)) => {
                let text = format!("added ({})", n.signature());
                match added {
                    Added::Plain => out.additive(noun, &path, text),
                    Added::Additive(why) => out.additive(noun, &path, format!("{text}; {why}")),
                    Added::Breaking(why) => out.breaking(noun, &path, format!("{text}; {why}")),
                }
            }
            (Some(o), None) => {
                out.breaking(noun, &path, format!("removed (was {})", o.signature()))
            }
            (Some(o), Some(n)) => callable_changes(out, noun, &path, o, n),
            (None, None) => {}
        }
    }
}

// ----- types ----------------------------------------------------------------------------------

fn types(out: &mut Out, old: &Schema, new: &Schema) {
    let (old_records, new_records) = (
        by_name(&old.records, |r| &r.name),
        by_name(&new.records, |r| &r.name),
    );
    let (old_enums, new_enums) = (
        by_name(&old.enums, |e| &e.name),
        by_name(&new.enums, |e| &e.name),
    );
    let names: BTreeSet<&str> = old_records
        .keys()
        .chain(new_records.keys())
        .chain(old_enums.keys())
        .chain(new_enums.keys())
        .copied()
        .collect();
    for name in names {
        match (old_records.get(name), new_records.get(name)) {
            (None, Some(_)) => out.additive("record", name, "added"),
            (Some(_), None) => out.breaking("record", name, "removed"),
            (Some(o), Some(n)) => record(out, o, n),
            (None, None) => {}
        }
        match (old_enums.get(name), new_enums.get(name)) {
            (None, Some(n)) => out.additive(enum_noun(n), name, "added"),
            (Some(o), None) => out.breaking(enum_noun(o), name, "removed"),
            (Some(o), Some(n)) => enumeration(out, o, n),
            (None, None) => {}
        }
    }
}

fn enum_noun(e: &EnumDef) -> &'static str {
    if e.is_error { "error" } else { "enum" }
}

fn record(out: &mut Out, old: &RecordDef, new: &RecordDef) {
    let name = &new.name;
    if old.transparent != new.transparent {
        let text = if new.transparent {
            "is now a newtype"
        } else {
            "is no longer a newtype"
        };
        out.breaking("record", name, text);
    }
    fields(out, name, &old.fields, &new.fields);
}

/// Whether the generated Swift initializer and Kotlin constructor give a `#[undra(default)]` field
/// of type `t` a default value: they spell the zero of a primitive, a string, bytes, a time, a
/// UUID, a decimal, an optional and a collection, and nothing for a record or an enum (Rust's
/// `Default` of a named type is not something they can know). TypeScript never does: a record is
/// an interface whose every member is required. A test generates all three and checks this.
fn has_generated_default(t: &TypeRef) -> bool {
    matches!(
        t,
        TypeRef::Bool
            | TypeRef::I8
            | TypeRef::I16
            | TypeRef::I32
            | TypeRef::I64
            | TypeRef::U8
            | TypeRef::U16
            | TypeRef::U32
            | TypeRef::U64
            | TypeRef::F32
            | TypeRef::F64
            | TypeRef::String
            | TypeRef::Bytes
            | TypeRef::Duration
            | TypeRef::Timestamp
            | TypeRef::Uuid
            | TypeRef::Decimal
            | TypeRef::Option(_)
            | TypeRef::Vec(_)
            | TypeRef::Map(..)
    )
}

/// The fields of the record `owner`, by name (a case's payload is compared whole, in `case`).
///
/// A field added is breaking whatever its default: a TypeScript record is an interface, and an
/// object literal that builds one must name every member. The text says why for the other two: a
/// field without a default (or with one no initializer can spell) is required by Swift and Kotlin
/// too, and one inserted before an existing field shifts Kotlin's positional arguments and
/// `componentN` destructuring.
fn fields(out: &mut Out, owner: &str, old: &[FieldDef], new: &[FieldDef]) {
    let (old_by, new_by) = (by_name(old, |f| &f.name), by_name(new, |f| &f.name));
    // The new definition's order first (it is what a reader sees), then what only the old one had.
    for (at, f) in new.iter().enumerate() {
        let path = format!("{owner}.{}", f.name);
        let Some(o) = old_by.get(f.name.as_str()) else {
            let shown = ty(&f.ty);
            // The first field of the old definition that now comes after this one, if any.
            let before = new[at + 1..]
                .iter()
                .find(|g| old_by.contains_key(g.name.as_str()));
            let text = match (f.default && has_generated_default(&f.ty), before) {
                _ if !f.default => format!(
                    "added without a default ({shown}): everything that builds a `{owner}` must supply it"
                ),
                (false, _) => format!(
                    "added ({shown}, with a default no generated initializer spells): everything that builds a \
                     `{owner}` must supply it"
                ),
                (true, Some(next)) => format!(
                    "added ({shown}, with a default) before `{}`: Kotlin's positional arguments and destructuring \
                     of a `{owner}` shift, and a TypeScript object literal that builds one must name it",
                    next.name
                ),
                (true, None) => format!(
                    "added ({shown}, with a default): Swift and Kotlin initializers default it, but a TypeScript \
                     object literal that builds a `{owner}` must name it"
                ),
            };
            out.breaking("field", &path, text);
            continue;
        };
        field(out, &path, o, f);
    }
    for f in old {
        if !new_by.contains_key(f.name.as_str()) {
            out.breaking(
                "field",
                &format!("{owner}.{}", f.name),
                format!("removed (was {})", ty(&f.ty)),
            );
        }
    }
    let common = |list: &[FieldDef], other: &BTreeMap<&str, &FieldDef>| -> Vec<String> {
        list.iter()
            .filter(|f| other.contains_key(f.name.as_str()))
            .map(|f| f.name.clone())
            .collect()
    };
    let (before, after) = (common(old, &new_by), common(new, &old_by));
    if before != after {
        out.breaking(
            "record",
            owner,
            format!(
                "fields reordered ({} -> {}): the wire layout and positional initializers moved",
                before.join(", "),
                after.join(", ")
            ),
        );
    }
}

fn field(out: &mut Out, path: &str, old: &FieldDef, new: &FieldDef) {
    if old.ty != new.ty {
        out.breaking(
            "field",
            path,
            format!("type changed from {} to {}", ty(&old.ty), ty(&new.ty)),
        );
    }
    match (old.default, new.default) {
        (false, true) => out.additive("field", path, "gained a default"),
        // Only an initializer that spelled the default can lose it.
        (true, false) if has_generated_default(&old.ty) => {
            out.breaking("field", path, "lost its default");
        }
        (true, false) => out.additive(
            "field",
            path,
            "lost its default (no generated initializer spelled it)",
        ),
        _ => {}
    }
}

fn enumeration(out: &mut Out, old: &EnumDef, new: &EnumDef) {
    let name = &new.name;
    let noun = enum_noun(new);
    if old.is_error != new.is_error {
        let text = if new.is_error {
            "is now an error enum"
        } else {
            "is no longer an error enum"
        };
        out.breaking(noun, name, text);
    }
    let (old_by, new_by) = (
        by_name(&old.variants, |v| &v.name),
        by_name(&new.variants, |v| &v.name),
    );
    for v in &new.variants {
        let path = format!("{name}.{}", v.name);
        match old_by.get(v.name.as_str()) {
            None => out.breaking(
                "case",
                &path,
                "added (an exhaustive `switch` or `when` over it stops compiling)",
            ),
            Some(o) => case(out, &path, o, v),
        }
    }
    for v in &old.variants {
        if !new_by.contains_key(v.name.as_str()) {
            out.breaking("case", &format!("{name}.{}", v.name), "removed");
        }
    }
}

/// `(String, u8)` for a tuple case, `{ at: Timestamp }` for a struct case, nothing for a unit case.
fn payload(v: &VariantDef) -> String {
    if v.fields.is_empty() {
        return "no payload".to_owned();
    }
    if v.tuple {
        let items: Vec<String> = v.fields.iter().map(|f| ty(&f.ty)).collect();
        format!("({})", items.join(", "))
    } else {
        let items: Vec<String> = v
            .fields
            .iter()
            .map(|f| format!("{}: {}", f.name, ty(&f.ty)))
            .collect();
        format!("{{ {} }}", items.join(", "))
    }
}

fn case(out: &mut Out, path: &str, old: &VariantDef, new: &VariantDef) {
    if old.index != new.index {
        out.breaking(
            "case",
            path,
            format!(
                "index changed from {} to {} (its wire value moved)",
                old.index, new.index
            ),
        );
    }
    /// What a case's payload is made of, to compare: tuple or struct, and each field's name and type.
    fn shape(v: &VariantDef) -> (bool, Vec<(&str, &TypeRef)>) {
        (
            v.tuple,
            v.fields.iter().map(|f| (f.name.as_str(), &f.ty)).collect(),
        )
    }
    if shape(old) != shape(new) {
        out.breaking(
            "case",
            path,
            format!("payload changed from {} to {}", payload(old), payload(new)),
        );
    } else {
        // The same payload: a field's default is all that can differ, and no generator spells one
        // in a case (only a migration reads it, SPEC 5.9), but it moves the hash, so it is a line.
        for (o, n) in old.fields.iter().zip(&new.fields) {
            if o.default != n.default {
                out.additive(
                    "case",
                    path,
                    format!(
                        "field `{}` {} (only a migration reads it; no generated case spells one)",
                        n.name,
                        if n.default {
                            "gained a default"
                        } else {
                            "lost its default"
                        }
                    ),
                );
            }
        }
    }
    if old.message != new.message {
        out.additive("case", path, "message changed");
    }
}

// ----- objects and stores ---------------------------------------------------------------------

fn object_noun(o: &ObjectDef) -> &'static str {
    if o.store.is_some() { "store" } else { "object" }
}

fn objects(out: &mut Out, old: &Schema, new: &Schema) {
    let (old_by, new_by) = (
        by_name(&old.objects, |o| &o.name),
        by_name(&new.objects, |o| &o.name),
    );
    for name in union(&old_by, &new_by) {
        match (old_by.get(name), new_by.get(name)) {
            (None, Some(n)) => out.additive(object_noun(n), name, "added"),
            (Some(o), None) => out.breaking(object_noun(o), name, "removed"),
            (Some(o), Some(n)) => object(out, o, n),
            (None, None) => {}
        }
    }
}

fn object(out: &mut Out, old: &ObjectDef, new: &ObjectDef) {
    let name = new.name.as_str();
    match (&old.store, &new.store) {
        (None, Some(_)) => out.breaking(
            "store",
            name,
            "was an object and is now a store (its class gains signals)",
        ),
        (Some(_), None) => out.breaking(
            "object",
            name,
            "was a store and is now an object (its signals are gone)",
        ),
        _ => {}
    }
    for (noun, old_list, new_list) in [
        ("constructor", &old.constructors, &new.constructors),
        ("method", &old.methods, &new.methods),
    ] {
        callables(
            out,
            noun,
            name,
            &method_callables(old_list),
            &method_callables(new_list),
            Added::Plain,
        );
    }
    if let (Some(o), Some(n)) = (&old.store, &new.store) {
        signals(out, name, &o.signals, &n.signals);
    }
}

/// The methods of an object or a port by name.
fn method_callables(list: &[MethodDef]) -> BTreeMap<&str, Callable<'_>> {
    list.iter()
        .map(|m| (m.name.as_str(), Callable::from(m)))
        .collect()
}

fn signals(out: &mut Out, store: &str, old: &[SignalDef], new: &[SignalDef]) {
    let (old_by, new_by) = (by_name(old, |s| &s.name), by_name(new, |s| &s.name));
    for s in new {
        let path = format!("{store}.{}", s.name);
        match old_by.get(s.name.as_str()) {
            None => out.additive(
                "signal",
                &path,
                format!(
                    "added ({}{})",
                    ty(&s.ty),
                    if s.computed { ", computed" } else { "" }
                ),
            ),
            Some(o) => signal(out, &path, o, s),
        }
    }
    for s in old {
        if !new_by.contains_key(s.name.as_str()) {
            out.breaking(
                "signal",
                &format!("{store}.{}", s.name),
                format!("removed (was {})", ty(&s.ty)),
            );
        }
    }
    let common = |list: &[SignalDef], other: &BTreeMap<&str, &SignalDef>| -> Vec<String> {
        list.iter()
            .filter(|s| other.contains_key(s.name.as_str()))
            .map(|s| s.name.clone())
            .collect()
    };
    let (before, after) = (common(old, &new_by), common(new, &old_by));
    if before != after {
        out.additive(
            "store",
            store,
            format!(
                "signals reordered ({} -> {}): their wire ids moved",
                before.join(", "),
                after.join(", ")
            ),
        );
    }
}

fn signal(out: &mut Out, path: &str, old: &SignalDef, new: &SignalDef) {
    if old.ty != new.ty {
        out.breaking(
            "signal",
            path,
            format!("type changed from {} to {}", ty(&old.ty), ty(&new.ty)),
        );
    }
    // Every signal is a read-only property on the platforms, computed or not, and a key only decides
    // whether a list's changes arrive as keyed patches: the generated declaration is the same.
    if old.computed != new.computed {
        let text = if new.computed {
            "is now computed (the platforms read it as before: every signal is read-only there)"
        } else {
            "is no longer computed (the platforms read it as before: every signal is read-only there)"
        };
        out.additive("signal", path, text);
    }
    if old.key != new.key {
        let key = |k: &Option<String>| {
            k.as_ref()
                .map_or_else(|| "none".to_owned(), |k| format!("`{k}`"))
        };
        out.additive(
            "signal",
            path,
            format!(
                "key changed from {} to {} (how its changes travel; the property is the same)",
                key(&old.key),
                key(&new.key)
            ),
        );
    }
    if old.no_coalesce != new.no_coalesce {
        let text = if new.no_coalesce {
            "is now `no_coalesce`: every change reaches the UI"
        } else {
            "is no longer `no_coalesce`: changes in one frame fold"
        };
        out.additive("signal", path, text);
    }
    if old.default != new.default {
        let text = if new.default {
            "gained a default"
        } else {
            "lost its default"
        };
        out.additive("signal", path, text);
    }
}

// ----- functions ------------------------------------------------------------------------------

/// The free functions of a schema by name.
fn function_callables(list: &[FunctionDef]) -> BTreeMap<&str, Callable<'_>> {
    list.iter()
        .map(|f| (f.name.as_str(), Callable::from(f)))
        .collect()
}

fn functions(out: &mut Out, old: &Schema, new: &Schema) {
    callables(
        out,
        "function",
        "",
        &function_callables(&old.functions),
        &function_callables(&new.functions),
        Added::Plain,
    );
}

// ----- ports and callbacks --------------------------------------------------------------------

fn kind_text(kind: PortKind) -> &'static str {
    match kind {
        PortKind::Sync => "sync",
        PortKind::Async => "async",
        PortKind::Event => "event",
        PortKind::Callback => "callback",
    }
}

/// Ports (`callbacks` false) or callback interfaces (`callbacks` true). The app implements a
/// callback and a sync or async port, so a method added to one breaks the implementer; it calls an
/// event port (the generated `<Port>Events` sends host-to-core events), so there a method added is
/// one more thing it may send.
///
/// A standard port (SPEC 8, exactly as `undra-ports` declares it: the same test the generators use
/// to leave it out, `stdlib::covered`) is implemented by the runtimes, and every runtime registers
/// the ones of SPEC 8 by default; the opt-in ones of 8.1 a web app registers itself
/// (`LoadOptions.ports`), so a core that enables one needs the web app to change.
fn ports(out: &mut Out, old: &Schema, new: &Schema, callbacks: bool) {
    let is_callback = |p: &PortDef| p.kind == PortKind::Callback;
    let (old_by, new_by) = (
        by_name(&old.ports, |p| &p.name),
        by_name(&new.ports, |p| &p.name),
    );
    let standard = stdlib::covered(new).ports;
    let opt_in = |name: &str| {
        stdlib::PORTS[stdlib::CORE_PORT_COUNT..]
            .iter()
            .any(|p| p.name == name)
    };
    for name in union(&old_by, &new_by) {
        let (o, n) = (old_by.get(name), new_by.get(name));
        // An item belongs to the group of its new kind (its old one when it is gone); one that
        // moved from a port to a callback is reported with the callbacks.
        let owner = n
            .or(o)
            .copied()
            .expect("the name came from one of the maps");
        if is_callback(owner) != callbacks {
            continue;
        }
        let noun = if callbacks { "callback" } else { "port" };
        match (o, n) {
            (None, Some(_)) if callbacks => out.additive(noun, name, "added"),
            (None, Some(_)) if standard.contains(name) && opt_in(name) => out.breaking(
                noun,
                name,
                "added (an opt-in standard port: the Swift and Kotlin platform defaults register its adapter, \
                 a web app must pass one in `LoadOptions.ports` or its calls fail as unavailable)",
            ),
            (None, Some(_)) if standard.contains(name) => out.additive(
                noun,
                name,
                "added (a standard port: the runtimes ship and register its adapter)",
            ),
            (None, Some(n)) if n.kind == PortKind::Event => out.additive(
                noun,
                name,
                "added (an event port: the app sends its events when it has them, and implements nothing)",
            ),
            (None, Some(_)) => out.breaking(
                noun,
                name,
                "added (the app must supply an adapter, or its calls fail with E0062)",
            ),
            (Some(_), None) => out.breaking(noun, name, "removed"),
            (Some(o), Some(n)) => port(out, noun, name, o, n),
            (None, None) => {}
        }
    }
}

fn port(out: &mut Out, noun: &'static str, name: &str, old: &PortDef, new: &PortDef) {
    if old.kind != new.kind {
        out.breaking(
            noun,
            name,
            format!(
                "kind changed from {} to {}",
                kind_text(old.kind),
                kind_text(new.kind)
            ),
        );
    }
    // Where the calls arrive is part of the contract with the implementation: Swift spells it as the
    // protocol's isolation (`@MainActor` unless `background`), and code that touches the UI from a
    // call is right on one thread and wrong on the other in every language.
    if old.background != new.background {
        let text = if new.background {
            "is now `background`: its calls arrive off the main thread (Swift's protocol is no longer `@MainActor`), \
             so an implementation that touches the UI must move to it"
        } else {
            "is no longer `background`: its calls arrive on the main thread (Swift's protocol becomes `@MainActor`), \
             so a nonisolated implementation no longer conforms as it did"
        };
        out.breaking(noun, name, text);
    }
    // The app implements a sync, async or callback port's methods and calls an event port's.
    let added = if new.kind == PortKind::Event {
        Added::Additive("the app sends it when it has one")
    } else {
        Added::Breaking("the app implements it, so its implementation must add it")
    };
    callables(
        out,
        "method",
        name,
        &method_callables(&old.methods),
        &method_callables(&new.methods),
        added,
    );
    let old_by = by_name(&old.methods, |m| &m.name);
    for m in &new.methods {
        if let Some(prev) = old_by.get(m.name.as_str())
            && prev.coalesce != m.coalesce
        {
            let text = if m.coalesce {
                "is now `coalesce`: the host delivers only the newest pending call"
            } else {
                "is no longer `coalesce`: every call is delivered"
            };
            out.additive("method", &format!("{name}.{}", m.name), text);
        }
    }
}

// ----- queries and mutations ------------------------------------------------------------------

fn query_noun(q: &QueryDef) -> &'static str {
    match q.kind {
        QueryKind::Query => "query",
        QueryKind::Mutation => "mutation",
    }
}

fn query_signature(q: &QueryDef) -> String {
    signature(&q.name, &q.params, &q.returns, true)
}

fn queries(out: &mut Out, old: &Schema, new: &Schema) {
    let (old_by, new_by) = (
        by_name(&old.queries, |q| &q.name),
        by_name(&new.queries, |q| &q.name),
    );
    for name in union(&old_by, &new_by) {
        match (old_by.get(name), new_by.get(name)) {
            (None, Some(n)) => out.additive(
                query_noun(n),
                name,
                format!("added ({})", query_signature(n)),
            ),
            (Some(o), None) => out.breaking(
                query_noun(o),
                name,
                format!("removed (was {})", query_signature(o)),
            ),
            (Some(o), Some(n)) => query(out, o, n),
            (None, None) => {}
        }
    }
}

fn query(out: &mut Out, old: &QueryDef, new: &QueryDef) {
    let noun = query_noun(new);
    let name = new.name.as_str();
    if old.kind != new.kind {
        out.breaking(
            noun,
            name,
            format!("was a {} and is now a {noun}", query_noun(old)),
        );
    }
    for reason in param_changes(&old.params, &new.params) {
        out.breaking(noun, name, reason);
    }
    if old.returns != new.returns {
        out.breaking(
            noun,
            name,
            format!(
                "return type changed from {} to {}",
                ty(&old.returns),
                ty(&new.returns)
            ),
        );
    }
    match (&old.infinite, &new.infinite) {
        (None, Some(_)) => out.breaking(noun, name, "is now an infinite (paged) query"),
        (Some(_), None) => out.breaking(noun, name, "is no longer an infinite (paged) query"),
        (Some(o), Some(n)) => {
            // The cursor stays in the core: the handle's `fetchNextPage()` takes none.
            if o.cursor != n.cursor {
                out.additive(
                    noun,
                    name,
                    format!(
                        "cursor type changed from {} to {} (the core pages with it; no generated declaration shows it)",
                        ty(&o.cursor),
                        ty(&n.cursor)
                    ),
                );
            }
            // Swift makes the row type `Identifiable` when the key is `id` (ADR-043); any other key
            // only decides how pages merge into the keyed list.
            if o.item_key != n.item_key {
                let changed = format!("item key changed from `{}` to `{}`", o.item_key, n.item_key);
                if o.item_key == "id" || n.item_key == "id" {
                    out.breaking(
                        noun,
                        name,
                        format!(
                            "{changed} (Swift's row type is `Identifiable` only when the key is `id`)"
                        ),
                    );
                } else {
                    out.additive(
                        noun,
                        name,
                        format!("{changed} (how pages merge; the handle is the same)"),
                    );
                }
            }
        }
        (None, None) => {}
    }
    // Behaviour no signature shows: compatible.
    if old.key != new.key {
        out.additive(
            noun,
            name,
            format!("cache key changed from `{}` to `{}`", old.key, new.key),
        );
    }
    let millis = |m: Option<u64>| m.map_or_else(|| "none".to_owned(), |m| format!("{m} ms"));
    if old.stale_ms != new.stale_ms {
        out.additive(
            noun,
            name,
            format!(
                "stale time changed from {} to {}",
                millis(old.stale_ms),
                millis(new.stale_ms)
            ),
        );
    }
    if old.interval_ms != new.interval_ms {
        out.additive(
            noun,
            name,
            format!(
                "poll interval changed from {} to {}",
                millis(old.interval_ms),
                millis(new.interval_ms)
            ),
        );
    }
    for (what, before, after) in [
        ("`persist`", old.persist, new.persist),
        ("`idempotent`", old.idempotent, new.idempotent),
        (
            "`poll_in_background`",
            old.poll_in_background,
            new.poll_in_background,
        ),
    ] {
        if before != after {
            out.additive(
                noun,
                name,
                format!("{what} changed from {before} to {after}"),
            );
        }
    }
}

#[cfg(test)]
#[path = "schema_diff_tests.rs"]
mod tests;
