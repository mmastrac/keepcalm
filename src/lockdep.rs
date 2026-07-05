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
}

static STATE: Mutex<Option<DetectorState>> = Mutex::new(None);

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
) {
    let Some(mode) = tracked_mode(sync_type, write) else {
        return;
    };
    let id = metadata.lock_id.get();
    let thread = std::thread::current().id();

    let mut guard = STATE.lock();
    let state = guard.get_or_insert_with(Default::default);

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
        let msg = relock_message(id, held_mode, mode);
        drop(guard);
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
fn find_path(edges: &HashMap<u64, HashMap<u64, Backtrace>>, from: u64, to: u64) -> Option<Vec<u64>> {
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
    let start = frames
        .iter()
        .position(|f| !is_internal(f))
        .unwrap_or(0);
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

fn relock_message(id: u64, held_mode: Mode, mode: Mode) -> String {
    let consequence = if held_mode == Mode::Shared && mode == Mode::Shared {
        "This may deadlock: parking_lot read locks are fair and will block a \
         recursive read whenever a writer is waiting."
    } else {
        "This will deadlock."
    };
    format!(
        "keepcalm deadlock detection: lock #{id} is already held by this thread!\n\
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
    let mut msg = format!(
        "keepcalm deadlock detection: lock-order cycle detected!\n\
         \x20 this thread is about to acquire lock #{acquiring} ({mode}) while holding lock #{held},\n\
         \x20 but the opposite ordering has already been observed:\n"
    );
    for pair in path.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let _ = write!(
            msg,
            "\n  lock #{to} was acquired while lock #{from} was held, first observed at:\n"
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
        let guard = a.write();
        // Guards are Send: drop it on another thread.
        std::thread::scope(|s| {
            s.spawn(move || drop(guard));
        });
        // If the held-entry leaked, this would report a re-entrant acquisition.
        let _g = a.write();
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
