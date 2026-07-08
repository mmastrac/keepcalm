//! Runtime lock-order ("lockdep") deadlock detection.
//!
//! When the `deadlock_detection` feature is enabled, every *blocking* lock
//! acquisition made through [`crate::Shared`], [`crate::SharedMut`],
//! [`crate::SharedGlobal`] or [`crate::SharedGlobalMut`] is checked against a
//! process-wide graph of previously-observed lock orderings:
//!
//!  * Acquiring a lock while holding another records the edge `held -> acquired`.
//!  * If a new edge would close a cycle in that graph, two code paths acquire
//!    the same locks in opposite orders, and this `panic!`s with a report that
//!    includes a backtrace of where the opposite ordering was first observed.
//!  * Re-acquiring a lock the current thread already holds panics immediately
//!    (a `Mutex` re-lock always deadlocks; a `RwLock` re-lock may deadlock
//!    whenever a writer is waiting, as `parking_lot` read locks are fair and
//!    not recursive-safe).
//!
//! Because the check happens *before* the acquisition blocks, an ordering
//! violation is reported the first time the inverted order is ever exercised —
//! the threads do not have to actually race into the deadlock.
//!
//! Notes on what is (and is not) tracked:
//!
//!  * Projections share their root container's lock, so they are naturally
//!    tracked as the root lock. Reading a projection while holding its root is
//!    reported as a re-entrant acquisition.
//!  * Plain [`std::sync::Arc`]-backed and RCU-backed containers never block
//!    while held, so they are exempt.
//!  * `try_read`/`try_write` cannot block and are therefore never reported,
//!    but a successfully try-acquired lock *is* recorded as held, so blocking
//!    acquisitions made while it is held are still checked.
//!  * Guards are `Send`; a guard dropped on another thread is correctly
//!    released from the acquiring thread's held-set.

use parking_lot::Mutex;
use std::backtrace::Backtrace;
use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::ThreadId;

use crate::implementation::LockMetadata;
use crate::synchronizer::{SynchronizerReadLock, SynchronizerType, SynchronizerWriteLock};

/// How a tracked lock is being acquired. Only used to make reports clearer;
/// cycle detection is conservative and treats both modes identically.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    Shared,
    Exclusive,
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mode::Shared => f.write_str("shared (read)"),
            Mode::Exclusive => f.write_str("exclusive (write)"),
        }
    }
}

/// A lazily-assigned, process-unique identity for a lock. Lives in
/// [`LockMetadata`] so that all clones and projections of a container resolve
/// to the same id. Zero means "not yet assigned" (this keeps construction
/// `const`-compatible for globals).
pub(crate) struct LockId(AtomicU64);

static NEXT_LOCK_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_HELD_SEQ: AtomicU64 = AtomicU64::new(1);

impl LockId {
    pub(crate) const fn unassigned() -> Self {
        LockId(AtomicU64::new(0))
    }

    fn get(&self) -> u64 {
        match self.0.load(Ordering::Relaxed) {
            0 => {
                let new = NEXT_LOCK_ID.fetch_add(1, Ordering::Relaxed);
                match self
                    .0
                    .compare_exchange(0, new, Ordering::Relaxed, Ordering::Relaxed)
                {
                    Ok(_) => new,
                    Err(existing) => existing,
                }
            }
            id => id,
        }
    }
}

/// When a lock is dropped (its last container reference goes away), purge everything the detector
/// recorded about it. Ids are never reused, so this reclaims memory without losing any actionable
/// ordering information: a re-created lock gets a fresh id and cannot alias the dropped one.
///
/// This is what keeps the detector's memory bounded by the set of *live* locks rather than the set
/// of locks ever created -- important for long-running processes that churn through short-lived
/// locks (the recorded backtraces are the heavy part).
impl Drop for LockId {
    fn drop(&mut self) {
        let id = *self.0.get_mut();
        if id != 0 {
            forget_lock(id);
        }
    }
}

/// Remove all state associated with a now-dead lock id.
fn forget_lock(id: u64) {
    let mut guard = STATE.lock();
    let Some(state) = guard.as_mut() else {
        return;
    };
    state.names.remove(&id);
    // Outgoing edges from this lock.
    state.edges.remove(&id);
    // Incoming edges into this lock, dropping any source whose edge set becomes empty.
    state.edges.retain(|_, targets| {
        targets.remove(&id);
        !targets.is_empty()
    });
    // A lock cannot be dropped while a guard on it is held (the guard keeps the container alive),
    // so `held` needs no cleanup here.
}

/// Proof that a lock acquisition was registered in the held-set. Stored in the
/// guard; returned to [`release_held`] on drop. The token remembers which
/// thread acquired the lock so that guards dropped on a different thread
/// (guards are `Send`) still clean up the correct entry.
pub(crate) struct HeldToken {
    thread: ThreadId,
    seq: u64,
}

struct HeldEntry {
    id: u64,
    seq: u64,
    mode: Mode,
}

#[derive(Default)]
struct DetectorState {
    /// Locks currently held, per acquiring thread.
    held: HashMap<ThreadId, Vec<HeldEntry>>,
    /// Observed ordering edges: `from` was held while `to` was acquired.
    /// Each edge stores a backtrace of where it was first observed.
    edges: HashMap<u64, HashMap<u64, Backtrace>>,
    /// The `type_name` of each lock's contained type, recorded on first acquire,
    /// to make reports human-readable.
    names: HashMap<u64, &'static str>,
}

static STATE: Mutex<Option<DetectorState>> = Mutex::new(None);

/// Test-only: `(edge source nodes, total edges, named locks)`, for asserting the detector's memory
/// stays bounded as locks are created and dropped.
#[cfg(test)]
fn state_sizes() -> (usize, usize, usize) {
    let guard = STATE.lock();
    match guard.as_ref() {
        Some(state) => (
            state.edges.len(),
            state.edges.values().map(|t| t.len()).sum(),
            state.names.len(),
        ),
        None => (0, 0, 0),
    }
}

thread_local! {
    /// Set while a lockdep-triggered panic is unwinding this thread. Guards drop
    /// during that unwind but their data is *not* inconsistent (the detector
    /// aborted *before* acquiring the second lock), so they must not poison. The
    /// flag is cleared whenever the thread next acquires a lock, which is proof
    /// the earlier unwind has been caught and we are back on a normal path.
    static ABORTING: Cell<bool> = const { Cell::new(false) };
}

fn set_aborting() {
    ABORTING.with(|c| c.set(true));
}

fn clear_aborting() {
    ABORTING.with(|c| c.set(false));
}

/// Whether a lockdep panic is currently unwinding this thread. Guards consult
/// this to decide whether a panic-time drop should poison the lock.
pub(crate) fn is_aborting() -> bool {
    ABORTING.with(Cell::get)
}

fn tracked_mode(sync_type: SynchronizerType, write: bool) -> Option<Mode> {
    match sync_type {
        // Arc reads and RCU reads/writes never block while held.
        SynchronizerType::Arc | SynchronizerType::Rcu => None,
        SynchronizerType::RwLock => Some(if write { Mode::Exclusive } else { Mode::Shared }),
        // A mutex "read" serializes with everything else.
        SynchronizerType::Mutex => Some(Mode::Exclusive),
    }
}

/// Check a blocking acquisition before it happens. Panics if it would
/// re-acquire a lock this thread already holds, or if the implied ordering
/// edge closes a cycle in the global order graph.
pub(crate) fn check_blocking_acquire(
    metadata: &LockMetadata,
    sync_type: SynchronizerType,
    write: bool,
    name: &'static str,
) {
    // Reaching here means we are acquiring a lock on a normal path, so any prior
    // lockdep-abort unwind has been caught: clear the flag.
    clear_aborting();
    let Some(mode) = tracked_mode(sync_type, write) else {
        return;
    };
    let id = metadata.lock_id.get();
    let thread = std::thread::current().id();

    let mut guard = STATE.lock();
    let state = guard.get_or_insert_with(Default::default);
    state.names.entry(id).or_insert(name);

    // Snapshot the held ids (deduplicated, oldest first) so we can mutate the
    // edge map below without fighting the borrow checker.
    let mut held_ids: Vec<u64> = Vec::new();
    let mut relock: Option<Mode> = None;
    if let Some(held) = state.held.get(&thread) {
        for entry in held {
            if entry.id == id {
                relock = Some(entry.mode);
            }
            if !held_ids.contains(&entry.id) {
                held_ids.push(entry.id);
            }
        }
    }

    if let Some(held_mode) = relock {
        let msg = relock_message(state, id, held_mode, mode);
        drop(guard);
        // The detector aborts before the second acquisition, so held locks are
        // consistent; suppress poisoning as this panic unwinds.
        set_aborting();
        panic!("{}", msg);
    }

    for from in held_ids {
        if let Some(targets) = state.edges.get(&from) {
            if targets.contains_key(&id) {
                // Ordering already known to be consistent.
                continue;
            }
        }
        // Would the new edge `from -> id` close a cycle? It does iff `from` is
        // already reachable from `id`.
        if let Some(path) = find_path(&state.edges, id, from) {
            let msg = cycle_message(state, from, id, mode, &path);
            drop(guard);
            set_aborting();
            panic!("{}", msg);
        }
        state
            .edges
            .entry(from)
            .or_default()
            .insert(id, Backtrace::force_capture());
    }
}

/// Register a lock as held by the current thread. Called after the acquisition
/// succeeds (for both blocking and `try_` acquisitions).
fn register_held(metadata: &LockMetadata, mode: Mode) -> HeldToken {
    // Successfully acquiring a lock (including via `try_`) proves we are not
    // mid-abort-unwind, so clear the flag.
    clear_aborting();
    let id = metadata.lock_id.get();
    let thread = std::thread::current().id();
    let seq = NEXT_HELD_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut guard = STATE.lock();
    let state = guard.get_or_insert_with(Default::default);
    state
        .held
        .entry(thread)
        .or_default()
        .push(HeldEntry { id, seq, mode });
    HeldToken { thread, seq }
}

/// Remove a held-set entry when its guard drops. Uses the token's origin
/// thread, not the current thread, so cross-thread guard drops stay balanced.
pub(crate) fn release_held(token: HeldToken) {
    let mut guard = STATE.lock();
    let Some(state) = guard.as_mut() else {
        return;
    };
    if let Some(entries) = state.held.get_mut(&token.thread) {
        if let Some(pos) = entries.iter().rposition(|e| e.seq == token.seq) {
            entries.remove(pos);
        }
        if entries.is_empty() {
            state.held.remove(&token.thread);
        }
    }
}

/// Compute the held-token for a freshly-acquired read guard, if its
/// synchronizer is a tracked (blocking) kind.
pub(crate) fn read_guard_token<T: ?Sized>(
    lock: &SynchronizerReadLock<LockMetadata, T>,
) -> Option<HeldToken> {
    match lock {
        SynchronizerReadLock::Arc(..) | SynchronizerReadLock::ReadCopyUpdate(..) => None,
        SynchronizerReadLock::RwLock(metadata, _) => Some(register_held(metadata, Mode::Shared)),
        SynchronizerReadLock::Mutex(metadata, _) => Some(register_held(metadata, Mode::Exclusive)),
    }
}

/// Compute the held-token for a freshly-acquired write guard, if its
/// synchronizer is a tracked (blocking) kind.
pub(crate) fn write_guard_token<T: ?Sized>(
    lock: &SynchronizerWriteLock<LockMetadata, T>,
) -> Option<HeldToken> {
    match lock {
        SynchronizerWriteLock::Arc(..) | SynchronizerWriteLock::ReadCopyUpdate(..) => None,
        SynchronizerWriteLock::RwLock(metadata, _) | SynchronizerWriteLock::Mutex(metadata, _) => {
            Some(register_held(metadata, Mode::Exclusive))
        }
    }
}

/// Breadth-first search for a path `from -> ... -> to` in the order graph.
fn find_path(
    edges: &HashMap<u64, HashMap<u64, Backtrace>>,
    from: u64,
    to: u64,
) -> Option<Vec<u64>> {
    let mut parents: HashMap<u64, u64> = HashMap::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(from);
    while let Some(node) = queue.pop_front() {
        if node == to {
            let mut path = vec![to];
            let mut cur = to;
            while cur != from {
                cur = parents[&cur];
                path.push(cur);
            }
            path.reverse();
            return Some(path);
        }
        if let Some(targets) = edges.get(&node) {
            for &next in targets.keys() {
                if next != from && !parents.contains_key(&next) {
                    parents.insert(next, node);
                    queue.push_back(next);
                }
            }
        }
    }
    None
}

/// Render a captured backtrace, trimming the capture machinery above the
/// user's frames and the runtime scaffolding below them. Falls back to the
/// full trace if the expected markers aren't found (e.g. symbols stripped).
fn render_backtrace(backtrace: &Backtrace) -> String {
    let full = backtrace.to_string();
    // Frame blocks start with `^\s*N: symbol`; continuation lines (`at ...`)
    // belong to the preceding frame.
    let mut frames: Vec<Vec<&str>> = Vec::new();
    for line in full.lines() {
        let is_frame_start = line
            .trim_start()
            .split_once(':')
            .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        if is_frame_start || frames.is_empty() {
            frames.push(vec![line]);
        } else {
            frames.last_mut().expect("non-empty").push(line);
        }
    }
    // The capture stack always starts with the backtrace machinery followed by
    // keepcalm's own acquire path; trim that contiguous leading run so the
    // first frame shown is the user's call site.
    let is_internal = |f: &Vec<&str>| {
        f.iter().any(|l| {
            l.contains("backtrace") && !l.contains("__rust_begin_short_backtrace")
                || l.contains("keepcalm::")
        })
    };
    let start = frames.iter().position(|f| !is_internal(f)).unwrap_or(0);
    let first_scaffold = frames.iter().position(|f| {
        f.iter()
            .any(|l| l.contains("__rust_begin_short_backtrace") || l.contains("lang_start"))
    });
    let end = first_scaffold.unwrap_or(frames.len());
    if start >= end {
        return full;
    }
    frames[start..end]
        .iter()
        .flat_map(|f| f.iter().copied())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render a lock as `#4 (my_app::HotSet)`, falling back to `#4` if no type name
/// was recorded.
fn label(state: &DetectorState, id: u64) -> String {
    match state.names.get(&id) {
        Some(name) => format!("#{id} ({name})"),
        None => format!("#{id}"),
    }
}

fn relock_message(state: &DetectorState, id: u64, held_mode: Mode, mode: Mode) -> String {
    let consequence = if held_mode == Mode::Shared && mode == Mode::Shared {
        "This may deadlock: parking_lot read locks are fair and will block a \
         recursive read whenever a writer is waiting."
    } else {
        "This will deadlock."
    };
    let lock = label(state, id);
    format!(
        "keepcalm deadlock detection: lock {lock} is already held by this thread!\n\
         \x20 currently held as: {held_mode}\n\
         \x20 attempting to re-acquire as: {mode}\n\
         \x20 {consequence}\n\
         \x20 Note that a projection shares the lock of its root container."
    )
}

fn cycle_message(
    state: &DetectorState,
    held: u64,
    acquiring: u64,
    mode: Mode,
    path: &[u64],
) -> String {
    let acquiring_label = label(state, acquiring);
    let held_label = label(state, held);
    let mut msg = format!(
        "keepcalm deadlock detection: lock-order cycle detected!\n\
         \x20 this thread is about to acquire lock {acquiring_label} ({mode}) while holding lock {held_label},\n\
         \x20 but the opposite ordering has already been observed:\n"
    );
    for pair in path.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let (from_label, to_label) = (label(state, from), label(state, to));
        let _ = write!(
            msg,
            "\n  lock {to_label} was acquired while lock {from_label} was held, first observed at:\n"
        );
        match state.edges.get(&from).and_then(|m| m.get(&to)) {
            Some(backtrace) => {
                let _ = writeln!(msg, "{}", render_backtrace(backtrace));
            }
            None => msg.push_str("  <backtrace unavailable>\n"),
        }
    }
    msg.push_str(
        "\n  Acquiring these locks in a consistent order on every code path will fix this.",
    );
    msg
}

#[cfg(test)]
mod test {
    use crate::{Shared, SharedMut};

    #[test]
    fn consistent_order_is_silent() {
        let a = SharedMut::new(1);
        let b = SharedMut::new(2);
        for _ in 0..3 {
            let _ga = a.write();
            let _gb = b.write();
        }
    }

    #[test]
    #[should_panic(expected = "lock-order cycle detected")]
    fn inverted_order_panics() {
        let a = SharedMut::new(1);
        let b = SharedMut::new(2);
        {
            let _ga = a.write();
            let _gb = b.write();
        }
        // Locks are free, so this cannot actually deadlock -- but the order is
        // inverted and must be reported.
        let _gb = b.write();
        let _ga = a.write();
    }

    #[test]
    #[should_panic(expected = "lock-order cycle detected")]
    fn transitive_cycle_panics() {
        let a = SharedMut::new(1);
        let b = SharedMut::new(2);
        let c = SharedMut::new(3);
        {
            let _ga = a.write();
            let _gb = b.write();
        }
        {
            let _gb = b.write();
            let _gc = c.write();
        }
        let _gc = c.write();
        let _ga = a.write();
    }

    #[test]
    #[should_panic(expected = "already held by this thread")]
    fn rwlock_reentrant_read_panics() {
        // Without detection this does not hang (parking_lot allows a recursive
        // read when no writer is waiting), so this test fails cleanly if the
        // detector regresses.
        let a = SharedMut::new(1);
        let _g1 = a.read();
        let _g2 = a.read();
    }

    #[test]
    #[should_panic(expected = "already held by this thread")]
    fn projection_shares_root_lock() {
        let root = SharedMut::new(("hello".to_string(), 1usize));
        let projected = root.project(crate::project!(x: (String, usize), x.0));
        let _root_guard = root.read();
        // The projection locks the same underlying RwLock as the root.
        let _proj_guard = projected.read();
    }

    #[test]
    #[should_panic(expected = "lock-order cycle detected")]
    fn projection_participates_in_cycles() {
        let a = SharedMut::new((1, 2));
        let a0 = a.project(crate::project!(x: (i32, i32), x.0));
        let b = SharedMut::new(3);
        {
            let _ga = a0.write();
            let _gb = b.write();
        }
        let _gb = b.write();
        // `a` and its projection `a0` are the same lock; this closes the cycle.
        let _ga = a.write();
    }

    #[test]
    fn try_lock_is_exempt() {
        let a = SharedMut::new(1);
        let b = SharedMut::new(2);
        {
            let _ga = a.write();
            // try-acquisitions cannot block, so they are never checked...
            let _gb = b.try_write().expect("uncontended");
        }
        {
            let _gb = b.write();
            // ...and did not record a b-after-a edge, so this is silent too.
            let _ga = a.write();
        }
        // But a try-held lock still creates edges for blocking acquisitions:
        // holding b (try), blocking on a is now a recorded b -> a ordering.
        let _gb = b.try_write().expect("uncontended");
        let _ga = a.write();
    }

    #[test]
    fn arc_and_rcu_are_exempt() {
        let arc = Shared::new(1);
        let rcu = SharedMut::new_rcu(2);
        let a = SharedMut::new(3);
        // Nest in every order; none of these can deadlock.
        let _g1 = arc.read();
        let _g2 = rcu.read();
        let _g3 = rcu.write();
        let _g4 = arc.read();
        let _g5 = a.write();
        let _g6 = rcu.write();
    }

    #[test]
    fn cross_thread_drop_releases() {
        let a = SharedMut::new(1);
        // Ordinary guards are `!Send`
        let guard = a.write_send();
        std::thread::scope(|s| {
            s.spawn(move || drop(guard));
        });
        // If the held-entry leaked, this would report a re-entrant acquisition.
        let _g = a.write();
    }

    #[test]
    #[should_panic(expected = "lock-order cycle detected")]
    fn set_participates_in_order_detection() {
        // `set` acquires the lock (briefly) too, so calling it while holding another lock
        // establishes an ordering edge and can reveal an inversion.
        let a = SharedMut::new(1);
        let b = SharedMut::new(2);
        {
            let _ga = a.write();
            b.set(20); // records a -> b
        }
        let _gb = b.write();
        a.set(10); // b -> a inverts the recorded order
    }

    #[test]
    #[should_panic(expected = "already held by this thread")]
    fn const_global_projection_shares_root_lock() {
        use crate::SharedGlobalMut;
        static G: SharedGlobalMut<(u32, u32)> = SharedGlobalMut::new((1, 2));
        static P0: crate::SharedMut<u32> = crate::project_global!(mut G => 0);
        let _root = G.read();
        // The projection locks the same underlying RwLock as the global.
        let _proj = P0.read();
    }

    #[test]
    fn cycle_panic_does_not_poison_held_lock() {
        // Establish the a -> b ordering.
        let a = SharedMut::new(1);
        let b = SharedMut::new(2);
        {
            let _ga = a.write();
            let _gb = b.write();
        }
        // Now provoke the inversion while holding `b`: acquiring `a` panics. `b` was held but its
        // data is consistent (the detector aborts before taking `a`), so `b` must NOT be poisoned.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _gb = b.write();
            let _ga = a.write(); // lockdep cycle panic
        }));
        assert!(result.is_err(), "expected a lockdep panic");

        // Both locks must remain usable -- no poison cascade.
        assert_eq!(*b.read(), 2);
        assert_eq!(*a.read(), 1);
        *b.write() = 20;
        assert_eq!(*b.read(), 20);
    }

    #[test]
    fn report_names_locks_by_type() {
        struct AlphaState(#[allow(dead_code)] u32);
        struct BetaState(#[allow(dead_code)] u32);
        let a = SharedMut::new(AlphaState(1));
        let b = SharedMut::new(BetaState(2));
        {
            let _ga = a.write();
            let _gb = b.write();
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _gb = b.write();
            let _ga = a.write();
        }));
        let err = result.expect_err("expected a lockdep panic");
        let msg = err
            .downcast_ref::<String>()
            .expect("panic message should be a String");
        assert!(
            msg.contains("AlphaState"),
            "message should name AlphaState: {msg}"
        );
        assert!(
            msg.contains("BetaState"),
            "message should name BetaState: {msg}"
        );
    }

    #[test]
    fn dropped_locks_are_forgotten() {
        let (_, edges_before, names_before) = super::state_sizes();
        // Churn through many short-lived locks, each pair acquired in a consistent order so an edge
        // and two name entries are recorded, then dropped.
        for _ in 0..1000 {
            let a = SharedMut::new(1);
            let b = SharedMut::new(2);
            let _ga = a.write();
            let _gb = b.write();
        }
        let (_, edges_after, names_after) = super::state_sizes();
        // If nothing were purged this would grow by ~1000 edges and ~2000 names. The threshold
        // absorbs the handful of live locks other concurrent tests may hold at the sample instant.
        assert!(
            edges_after <= edges_before + 100,
            "edges leaked across churn: {edges_before} -> {edges_after}"
        );
        assert!(
            names_after <= names_before + 100,
            "names leaked across churn: {names_before} -> {names_after}"
        );
    }

    #[test]
    fn global_locks_are_tracked() {
        use crate::SharedGlobalMut;
        static GLOBAL: SharedGlobalMut<usize> = SharedGlobalMut::new(1);
        let a = SharedMut::new(2);
        // Consistent order: global then local. Runs fine.
        for _ in 0..2 {
            let _g1 = GLOBAL.write();
            let _g2 = a.write();
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g2 = a.write();
            let _g1 = GLOBAL.write();
        }));
        let err = result.expect_err("expected a lock-order cycle panic");
        let msg = err
            .downcast_ref::<String>()
            .expect("panic message should be a String");
        assert!(msg.contains("lock-order cycle detected"), "message: {msg}");
    }
}
