//! The workshop: objects as parameters and returns (ADR-040) and host callbacks (ADR-041).
//!
//! A [`Workshop`] is a store that hands out child objects: [`Shelf`]es, which are stores of their
//! own, so a platform gets a `Shelf` it can observe from a method of the `Workshop`
//! ([`Workshop::shelf`] returns the same shelf every time, one handle and one wrapper). The
//! platforms hand shelves back as arguments ([`Workshop::merge`], [`Workshop::total`]).
//!
//! The app also implements a [`Reporter`]: the core calls it back with progress, notes and a
//! question ([`Workshop::run`], [`Workshop::announce`], [`Workshop::burst`]). A reporter
//! passed to [`Workshop::watch`] is kept until the returned [`Watch`] is closed, the
//! subscription-object pattern of ADR-041: closing it drops the core's proxy, which gives the
//! host's reference back.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};

use undra::prelude::*;
use undra::runtime::{PortError, Stream};

/// What a reporter's `confirm` can fail with.
#[undra::error]
#[derive(Clone, Debug, PartialEq)]
pub enum ReportError {
    /// The user answered no.
    #[error("the user declined")]
    Declined,
    /// The app's reporter is not there or failed.
    #[error("the reporter is not available: {0}")]
    Unavailable(String),
}

impl From<PortError> for ReportError {
    fn from(error: PortError) -> Self {
        ReportError::Unavailable(error.to_string())
    }
}

/// What can go wrong opening a shelf.
#[undra::error]
#[derive(Clone, Debug, PartialEq)]
pub enum WorkshopError {
    /// The name is empty.
    #[error("a shelf needs a name")]
    NoName,
}

/// The subscribed reporters, by subscription number.
type Watchers = Mutex<Vec<(u32, Arc<dyn Reporter>)>>;

/// What the app implements to hear from the core.
#[undra::callback]
pub trait Reporter {
    /// How far a job is: `done` of `total`. Only the newest report of a burst matters, so the
    /// platforms deliver just that one to a listener that has fallen behind.
    #[undra(coalesce)]
    fn progress(&self, done: u32, total: u32);
    /// A line to show.
    fn note(&self, line: String);
    /// Asks whether to go on; the app may answer later, or not at all.
    async fn confirm(&self, question: String) -> Result<bool, ReportError>;
}

/// A shelf: a child store of the [`Workshop`], with a label and a count of the items on it.
#[undra::store]
pub struct Shelf {
    label: Signal<String>,
    items: Signal<u32>,
}

#[undra::api(store)]
impl Shelf {
    /// An empty shelf with no label. A platform gets shelves from [`Workshop::shelf`], not from
    /// here, so that one name is one shelf.
    pub fn new() -> Self {
        Shelf {
            label: Signal::new(String::new()),
            items: Signal::new(0),
        }
    }

    /// Puts `count` more items on the shelf.
    pub fn stock(&self, count: u32) {
        self.items
            .update(|items| *items = items.saturating_add(count));
    }

    /// Takes every item off the shelf.
    pub fn clear(&self) {
        self.items.set(0);
    }
}

impl Default for Shelf {
    fn default() -> Self {
        Shelf::new()
    }
}

/// A subscription: while a platform holds it, the reporter given to [`Workshop::watch`] is told
/// what [`Workshop::announce`] says. Closing it (or letting it go) stops that.
pub struct Watch {
    id: u32,
    watchers: Weak<Watchers>,
}

#[undra::api]
impl Watch {
    /// The subscription's number, in the order they were made.
    pub fn id(&self) -> u32 {
        self.id
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(watchers) = self.watchers.upgrade() {
            // Dropped outside the lock: a proxy's drop calls the host.
            let gone = {
                let mut watchers = watchers.lock().unwrap_or_else(|e| e.into_inner());
                watchers
                    .iter()
                    .position(|(id, _)| *id == self.id)
                    .map(|at| watchers.remove(at))
            };
            drop(gone);
        }
    }
}

/// A workshop that owns shelves and tells subscribers what happens in it.
#[undra::store]
pub struct Workshop {
    /// How many jobs have run (a signal, so a platform can show it).
    jobs: Signal<u32>,
    /// How many notes were announced.
    notes: Signal<u32>,
    shelves: Mutex<BTreeMap<String, Arc<Shelf>>>,
    watchers: Arc<Watchers>,
    next_watch: Mutex<u32>,
    ctx: WeakCtx,
}

/// The state a restore cannot rebuild is empty after one: the shelves and subscribers of the old
/// workshop are gone with it (ADR-040 decision 8), and `shelf` makes them again.
#[undra::api(store)]
impl Workshop {
    /// An empty workshop.
    pub fn new(ctx: Ctx) -> Self {
        Workshop {
            jobs: Signal::new(0),
            notes: Signal::new(0),
            shelves: Mutex::new(BTreeMap::new()),
            watchers: Arc::new(Mutex::new(Vec::new())),
            next_watch: Mutex::new(1),
            ctx: ctx.downgrade(),
        }
    }

    /// The shelf called `name`, made on first use: the same shelf every time, so a platform that
    /// asks twice gets one object (and one reference to give back, not two).
    pub fn shelf(&self, name: String) -> Arc<Shelf> {
        let mut shelves = self.shelves.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(shelf) = shelves.get(&name) {
            return Arc::clone(shelf);
        }
        let shelf = Arc::new(Shelf::new());
        shelf.label.set(name.clone());
        shelves.insert(name, Arc::clone(&shelf));
        shelf
    }

    /// The shelf called `name`, if there is one.
    pub fn find(&self, name: String) -> Option<Arc<Shelf>> {
        self.shelves
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&name)
            .cloned()
    }

    /// Every shelf, in name order.
    pub fn shelves(&self) -> Vec<Arc<Shelf>> {
        self.shelves
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// Opens a shelf after `delay_ms` milliseconds (a call a platform can cancel while it
    /// waits: then no shelf was handed out and nothing is owed).
    pub async fn open(&self, name: String, delay_ms: u32) -> Result<Arc<Shelf>, WorkshopError> {
        if name.is_empty() {
            return Err(WorkshopError::NoName);
        }
        if let Ok(ctx) = self.ctx.upgrade() {
            ctx.sleep(std::time::Duration::from_millis(u64::from(delay_ms)))
                .await;
        }
        Ok(self.shelf(name))
    }

    /// Moves every item of `from` onto `onto`, in one transaction (a method that takes two
    /// stores).
    pub fn merge(&self, from: &Shelf, onto: &Shelf) {
        txn(|| {
            let moved = from.items.get();
            from.items.set(0);
            onto.items
                .update(|items| *items = items.saturating_add(moved));
        });
    }

    /// How many items there are on the shelves given.
    pub fn total(&self, shelves: Vec<Arc<Shelf>>) -> u32 {
        shelves.iter().map(|shelf| shelf.items.get()).sum()
    }

    /// The label of `shelf`, or `none` for no shelf at all.
    pub fn describe(&self, shelf: Option<Arc<Shelf>>) -> String {
        shelf.map_or_else(|| "none".to_owned(), |shelf| shelf.label.get())
    }

    /// Runs a job of `steps` steps, reporting to the app: one progress report and one note per
    /// step, then a question whose answer is the result (`steps` if the app says go on, else
    /// `Declined`). The job counts as run whatever the answer.
    pub async fn run(&self, steps: u32, reporter: Arc<dyn Reporter>) -> Result<u32, ReportError> {
        for step in 1..=steps {
            reporter.progress(step, steps);
            reporter.note(format!("step {step} of {steps}"));
        }
        self.jobs.update(|jobs| *jobs += 1);
        if reporter
            .confirm(format!("ran {steps} steps; go on?"))
            .await?
        {
            Ok(steps)
        } else {
            Err(ReportError::Declined)
        }
    }

    /// Reports `steps` times in a row without waiting: a burst, of which the app that is slow to
    /// react gets only the newest `progress` (the others are coalesced), but every `note`.
    pub fn burst(&self, steps: u32, reporter: Arc<dyn Reporter>) {
        for step in 1..=steps {
            reporter.progress(step, steps);
            reporter.note(format!("burst {step}"));
        }
    }

    /// Keeps `reporter` until the returned subscription is closed; [`Workshop::announce`] tells it.
    pub fn watch(&self, reporter: Arc<dyn Reporter>) -> Arc<Watch> {
        let id = {
            let mut next = self.next_watch.lock().unwrap_or_else(|e| e.into_inner());
            let id = *next;
            *next += 1;
            id
        };
        self.watchers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((id, reporter));
        Arc::new(Watch {
            id,
            watchers: Arc::downgrade(&self.watchers),
        })
    }

    /// Counts a note (a signal commit, which reaches the platform first) and tells every
    /// subscribed reporter `line`: the platform sees the new count before it hears the note.
    pub fn announce(&self, line: String) -> u32 {
        self.notes.update(|notes| *notes += 1);
        let watchers: Vec<Arc<dyn Reporter>> = self
            .watchers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(_, reporter)| Arc::clone(reporter))
            .collect();
        for reporter in &watchers {
            reporter.note(line.clone());
        }
        u32::try_from(watchers.len()).unwrap_or(u32::MAX)
    }

    /// Reads how many items `shelf` holds, once per step for `steps` steps: a stream that takes an
    /// object, which the core holds (borrowed from the platform) while the stream runs.
    pub fn tally(&self, shelf: Arc<Shelf>, steps: u32) -> impl Stream<Item = u32> + Send + 'static {
        Tally { shelf, left: steps }
    }

    /// Walks `steps` steps, telling `reporter` a note at each one and yielding the step number: a
    /// stream that takes a callback, whose reference is the core's until the stream ends or is dropped.
    pub fn walk(
        &self,
        steps: u32,
        reporter: Arc<dyn Reporter>,
    ) -> impl Stream<Item = u32> + Send + 'static {
        Walk {
            step: 0,
            steps,
            reporter,
        }
    }

    /// How many reporters are subscribed.
    pub fn watching(&self) -> u32 {
        u32::try_from(
            self.watchers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .len(),
        )
        .unwrap_or(u32::MAX)
    }
}

/// The stream of [`Workshop::tally`].
struct Tally {
    shelf: Arc<Shelf>,
    left: u32,
}

impl Stream for Tally {
    type Item = u32;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<u32>> {
        if self.left == 0 {
            return std::task::Poll::Ready(None);
        }
        self.left -= 1;
        std::task::Poll::Ready(Some(self.shelf.items.get()))
    }
}

/// The stream of [`Workshop::walk`].
struct Walk {
    step: u32,
    steps: u32,
    reporter: Arc<dyn Reporter>,
}

impl Stream for Walk {
    type Item = u32;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<u32>> {
        if self.step >= self.steps {
            return std::task::Poll::Ready(None);
        }
        self.step += 1;
        let (step, steps) = (self.step, self.steps);
        self.reporter.note(format!("walk {step} of {steps}"));
        std::task::Poll::Ready(Some(step))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use undra::runtime::testing::TestRuntime;

    use super::*;

    /// A reporter written in Rust: what a test passes where an app passes its own.
    #[derive(Default)]
    struct Recorder {
        lines: Mutex<Vec<String>>,
        progress: Mutex<Vec<(u32, u32)>>,
        answer: Mutex<bool>,
    }

    #[undra::port]
    impl Reporter for Recorder {
        fn progress(&self, done: u32, total: u32) {
            self.progress.lock().unwrap().push((done, total));
        }

        fn note(&self, line: String) {
            self.lines.lock().unwrap().push(line);
        }

        async fn confirm(&self, _question: String) -> Result<bool, ReportError> {
            Ok(*self.answer.lock().unwrap())
        }
    }

    fn workshop(t: &TestRuntime) -> Workshop {
        Workshop::new(t.ctx())
    }

    #[test]
    fn a_shelf_is_made_once_per_name() {
        let t = TestRuntime::new();
        let w = workshop(&t);
        let a = w.shelf("a".to_owned());
        assert!(Arc::ptr_eq(&a, &w.shelf("a".to_owned())));
        assert!(!Arc::ptr_eq(&a, &w.shelf("b".to_owned())));
        assert_eq!(w.shelves().len(), 2);
        assert!(w.find("c".to_owned()).is_none());
        assert_eq!(w.describe(Some(a)), "a");
        assert_eq!(w.describe(None), "none");
    }

    #[test]
    fn merge_moves_the_items_and_total_adds_them_up() {
        let t = TestRuntime::new();
        let w = workshop(&t);
        let (a, b) = (w.shelf("a".to_owned()), w.shelf("b".to_owned()));
        a.stock(3);
        b.stock(4);
        assert_eq!(w.total(vec![Arc::clone(&a), Arc::clone(&b)]), 7);
        w.merge(&a, &b);
        assert_eq!(w.total(vec![a, b]), 7);
    }

    #[test]
    fn a_job_reports_every_step_and_the_answer_decides_the_result() {
        let t = TestRuntime::new();
        let w = workshop(&t);
        let reporter = Arc::new(Recorder::default());
        *reporter.answer.lock().unwrap() = true;
        let done = t.run_until(w.run(3, reporter.clone()));
        assert_eq!(done, Ok(3));
        assert_eq!(*reporter.progress.lock().unwrap(), [(1, 3), (2, 3), (3, 3)]);
        assert_eq!(reporter.lines.lock().unwrap().len(), 3);
        *reporter.answer.lock().unwrap() = false;
        assert_eq!(t.run_until(w.run(1, reporter)), Err(ReportError::Declined));
    }

    #[test]
    fn a_subscription_keeps_the_reporter_until_it_is_dropped() {
        let t = TestRuntime::new();
        let w = workshop(&t);
        let reporter = Arc::new(Recorder::default());
        let watch = w.watch(reporter.clone());
        assert_eq!((w.watching(), watch.id()), (1, 1));
        assert_eq!(w.announce("hello".to_owned()), 1);
        assert_eq!(*reporter.lines.lock().unwrap(), ["hello"]);
        drop(watch);
        assert_eq!(w.watching(), 0);
        assert_eq!(w.announce("again".to_owned()), 0);
        assert_eq!(reporter.lines.lock().unwrap().len(), 1);
    }

    /// Drains a stream on this thread (nothing here waits for another).
    fn drain<S: Stream<Item = u32>>(stream: S) -> Vec<u32> {
        let mut stream = Box::pin(stream);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let mut items = Vec::new();
        while let std::task::Poll::Ready(Some(item)) = stream.as_mut().poll_next(&mut cx) {
            items.push(item);
        }
        items
    }

    #[test]
    fn a_stream_can_hold_a_shelf_and_a_reporter() {
        let t = TestRuntime::new();
        let w = workshop(&t);
        let shelf = w.shelf("a".to_owned());
        shelf.stock(4);
        assert_eq!(drain(w.tally(Arc::clone(&shelf), 3)), [4, 4, 4]);
        let reporter = Arc::new(Recorder::default());
        assert_eq!(drain(w.walk(2, reporter.clone())), [1, 2]);
        assert_eq!(
            *reporter.lines.lock().unwrap(),
            ["walk 1 of 2", "walk 2 of 2"]
        );
    }

    #[test]
    fn opening_a_shelf_without_a_name_is_an_error() {
        let t = TestRuntime::new();
        let w = workshop(&t);
        assert_eq!(
            t.run_until(w.open(String::new(), 0)).err(),
            Some(WorkshopError::NoName)
        );
        let shelf = t.run_until(w.open("x".to_owned(), 0)).unwrap();
        assert!(Arc::ptr_eq(&shelf, &w.shelf("x".to_owned())));
    }
}
