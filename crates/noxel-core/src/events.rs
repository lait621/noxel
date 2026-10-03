//! Typed, double-buffered event queues.
//!
//! Noxel keeps systems decoupled with events rather than direct calls: physics
//! emits "something was hit", the NPC system emits "a path failed", the
//! visibility system emits "a roof became occluding". Systems that care read
//! them during the same frame.
//!
//! # Why double buffering
//!
//! A handler frequently wants to emit in response to an event it just read.
//! Appending to the same queue you are iterating is how you get an infinite
//! loop. Noxel therefore separates the two:
//!
//! * [`EventBus::emit`] queues into **this** frame.
//! * [`EventBus::defer`] queues into the **next** frame — the right call from
//!   inside a handler.
//! * [`EventBus::update`] rotates at the frame boundary: unread events are
//!   dropped and deferred events become current.
//!
//! # Reading without borrow pain
//!
//! Iterating with [`EventBus::reader`] borrows the bus immutably, which forbids
//! emitting from inside the loop. When a handler needs to do that, take the
//! events instead:
//!
//! ```
//! use noxel_core::events::EventBus;
//!
//! #[derive(Debug, PartialEq)]
//! enum Ev { Hit(u32), Broke(u32) }
//!
//! let mut bus = EventBus::new();
//! bus.emit(Ev::Hit(3));
//!
//! let mut scratch = Vec::new();
//! bus.take_into(&mut scratch);          // no allocation: the buffers are swapped
//! for e in &scratch {
//!     if let Ev::Hit(n) = e {
//!         bus.defer(Ev::Broke(*n));     // safe: lands next frame
//!     }
//! }
//! assert!(bus.reader().is_empty(), "still nothing queued for this frame");
//! bus.update();
//! assert_eq!(bus.reader().count(), 1);
//! ```
//!
//! `take_into` is the hot path used by the app runtime: it swaps the bus's
//! buffer with a per-system scratch `Vec`, so a steady-state frame performs no
//! allocation at all.

/// A queue of events of one type.
#[derive(Clone, Debug)]
pub struct EventBus<E> {
    current: Vec<E>,
    next: Vec<E>,
    /// Total events ever accepted, for diagnostics.
    total_emitted: u64,
    /// Events rejected because the queue hit its cap.
    dropped: u64,
    cap: usize,
}

impl<E> EventBus<E> {
    /// The default maximum number of queued events.
    ///
    /// A runaway emitter (a script bug, a physics test fixture inside a loop)
    /// would otherwise exhaust memory. Exceeding it is a bug in the emitter, and
    /// the engine surfaces it through [`EventBus::dropped`] rather than silently
    /// running out of memory.
    pub const DEFAULT_CAP: usize = 65_536;

    /// An empty bus with the default cap.
    #[must_use]
    pub fn new() -> Self {
        Self::with_cap(Self::DEFAULT_CAP)
    }

    /// An empty bus with an explicit cap.
    #[must_use]
    pub fn with_cap(cap: usize) -> Self {
        Self {
            current: Vec::new(),
            next: Vec::new(),
            total_emitted: 0,
            dropped: 0,
            cap: cap.max(1),
        }
    }

    /// Queues an event for readers in the current frame.
    pub fn emit(&mut self, event: E) {
        if self.current.len() + self.next.len() >= self.cap {
            self.dropped += 1;
            return;
        }
        self.total_emitted += 1;
        self.current.push(event);
    }

    /// Queues an event for the **next** frame.
    ///
    /// Use this from inside a handler so a reaction cannot re-trigger itself
    /// within the same frame.
    pub fn defer(&mut self, event: E) {
        if self.current.len() + self.next.len() >= self.cap {
            self.dropped += 1;
            return;
        }
        self.total_emitted += 1;
        self.next.push(event);
    }

    /// A read-only iterator over this frame's events.
    ///
    /// Prefer [`EventBus::take_into`] when the handler also emits.
    #[must_use]
    pub fn reader(&self) -> EventReader<'_, E> {
        EventReader {
            slice: &self.current,
            index: 0,
        }
    }

    /// This frame's events, by reference.
    #[must_use]
    pub fn events(&self) -> &[E] {
        &self.current
    }

    /// Number of events in the current frame.
    #[must_use]
    pub fn len(&self) -> usize {
        self.current.len()
    }

    /// True when there are no events this frame.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.current.is_empty()
    }

    /// Events queued for the next frame (reactions to this frame's events).
    #[must_use]
    pub fn pending_next_frame(&self) -> usize {
        self.next.len()
    }

    /// Moves this frame's events out of the bus into `out`, leaving the bus
    /// empty but keeping an allocation ready for the next frame.
    ///
    /// `out` is cleared first. After the swap the bus owns `out`'s previous
    /// (now empty but allocated) buffer, so a steady-state frame allocates
    /// nothing.
    pub fn take_into(&mut self, out: &mut Vec<E>) {
        out.clear();
        std::mem::swap(&mut self.current, out);
    }

    /// Moves this frame's events out into a fresh `Vec`.
    ///
    /// Convenient but allocating; prefer [`EventBus::take_into`] in a hot loop.
    pub fn take(&mut self) -> Vec<E> {
        std::mem::take(&mut self.current)
    }

    /// Rotates the buffers: deferred events become current, unread current
    /// events are dropped.
    ///
    /// Call once per frame, after every system has run.
    pub fn update(&mut self) {
        std::mem::swap(&mut self.current, &mut self.next);
        self.next.clear();
    }

    /// Drops every event, including those deferred to the next frame.
    pub fn clear(&mut self) {
        self.current.clear();
        self.next.clear();
    }

    /// Total events accepted since construction.
    #[must_use]
    pub fn total_emitted(&self) -> u64 {
        self.total_emitted
    }

    /// Events rejected because the cap was reached. Must be zero in a healthy
    /// build; the demo's statistics panel shows it.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The configured cap.
    #[must_use]
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Sets the cap.
    pub fn set_cap(&mut self, cap: usize) {
        self.cap = cap.max(1);
    }
}

impl<E> Default for EventBus<E> {
    fn default() -> Self {
        Self::new()
    }
}

/// An iterator over the events of one frame.
pub struct EventReader<'a, E> {
    slice: &'a [E],
    index: usize,
}

impl<'a, E> EventReader<'a, E> {
    /// How many events are left to read.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.slice.len().saturating_sub(self.index)
    }

    /// True when the reader is exhausted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Calls `f` for every remaining event.
    pub fn for_each_remaining(&mut self, mut f: impl FnMut(&E)) {
        for e in self.by_ref() {
            f(e);
        }
    }

    /// The last remaining event, consuming the reader.
    #[must_use]
    pub fn last_event(self) -> Option<&'a E> {
        self.last()
    }
}

impl<'a, E> Iterator for EventReader<'a, E> {
    type Item = &'a E;
    fn next(&mut self) -> Option<&'a E> {
        let e = self.slice.get(self.index)?;
        self.index += 1;
        Some(e)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.remaining();
        (n, Some(n))
    }
}

impl<E> ExactSizeIterator for EventReader<'_, E> {}

/// A ring of the last `N` events of a type, for debugging and replay.
///
/// The demo records one of these so a scripted repro can show the last few
/// things that happened without attaching a debugger.
#[derive(Clone, Debug)]
pub struct EventLog<E> {
    events: Vec<(u64, E)>,
    capacity: usize,
    next_index: u64,
}

impl<E> EventLog<E> {
    /// Creates a log holding the last `capacity` events.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            events: Vec::new(),
            capacity: capacity.max(1),
            next_index: 0,
        }
    }

    /// Records an event, evicting the oldest when full.
    pub fn push(&mut self, event: E) {
        if self.events.len() == self.capacity {
            self.events.remove(0);
        }
        self.events.push((self.next_index, event));
        self.next_index += 1;
    }

    /// Newest first.
    pub fn iter_recent(&self) -> impl Iterator<Item = (u64, &E)> {
        self.events.iter().rev().map(|(i, e)| (*i, e))
    }

    /// Oldest first.
    pub fn iter_chronological(&self) -> impl Iterator<Item = (u64, &E)> {
        self.events.iter().map(|(i, e)| (*i, e))
    }

    /// The most recent event.
    #[must_use]
    pub fn last(&self) -> Option<&E> {
        self.events.last().map(|(_, e)| e)
    }

    /// How many are stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// True when nothing is stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Total events ever pushed, including evicted ones.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.next_index
    }

    /// Empties the log but keeps the sequence counter.
    pub fn clear(&mut self) {
        self.events.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    enum Ev {
        A(u32),
        B,
    }

    #[test]
    fn emit_then_read() {
        let mut bus = EventBus::new();
        bus.emit(Ev::A(1));
        bus.emit(Ev::B);
        assert_eq!(bus.len(), 2);
        let got: Vec<Ev> = bus.reader().cloned().collect();
        assert_eq!(got, vec![Ev::A(1), Ev::B]);
    }

    #[test]
    fn update_drops_unread_and_promotes_deferred() {
        let mut bus = EventBus::new();
        bus.emit(Ev::A(1));
        bus.defer(Ev::A(2));
        assert_eq!(bus.len(), 1);
        assert_eq!(bus.pending_next_frame(), 1);
        bus.update();
        assert_eq!(bus.len(), 1);
        assert_eq!(bus.reader().next(), Some(&Ev::A(2)));
    }

    #[test]
    fn reacting_inside_a_handler_lands_next_frame() {
        let mut bus = EventBus::new();
        bus.emit(Ev::A(1));
        let mut reactions = 0;
        let mut scratch = Vec::new();
        bus.take_into(&mut scratch);
        for e in &scratch {
            if let Ev::A(_) = e {
                bus.defer(Ev::A(2));
                reactions += 1;
            }
        }
        assert_eq!(reactions, 1);
        assert!(bus.is_empty(), "the reaction must not re-enter this frame");
        bus.update();
        assert_eq!(bus.len(), 1);
    }

    #[test]
    fn take_into_reuses_buffers() {
        let mut bus = EventBus::new();
        let mut scratch = Vec::new();
        bus.emit(Ev::A(1));
        bus.take_into(&mut scratch);
        assert_eq!(scratch.len(), 1);
        assert!(bus.is_empty());
        // Second round: still works, still no growth of the bus's buffer.
        bus.emit(Ev::A(2));
        bus.take_into(&mut scratch);
        assert_eq!(scratch.len(), 1);
        assert_eq!(scratch[0], Ev::A(2));
    }

    #[test]
    fn take_returns_owned_events() {
        let mut bus = EventBus::new();
        bus.emit(Ev::A(9));
        let taken = bus.take();
        assert_eq!(taken, vec![Ev::A(9)]);
        assert!(bus.is_empty());
    }

    #[test]
    fn cap_drops_excess_and_reports() {
        let mut bus = EventBus::with_cap(4);
        for i in 0..10 {
            bus.emit(Ev::A(i));
        }
        assert_eq!(bus.len(), 4);
        assert_eq!(bus.dropped(), 6);
        assert_eq!(bus.total_emitted(), 4);
    }

    #[test]
    fn cap_counts_deferred_too() {
        let mut bus = EventBus::with_cap(2);
        bus.emit(Ev::A(1));
        bus.defer(Ev::A(2));
        bus.defer(Ev::A(3));
        assert_eq!(bus.dropped(), 1);
    }

    #[test]
    fn reader_helpers() {
        let mut bus = EventBus::new();
        bus.emit(Ev::A(1));
        bus.emit(Ev::A(2));
        bus.emit(Ev::A(3));
        let mut r = bus.reader();
        assert_eq!(r.remaining(), 3);
        assert_eq!(r.next(), Some(&Ev::A(1)));
        assert_eq!(r.remaining(), 2);
        assert_eq!(r.last_event(), Some(&Ev::A(3)));
    }

    #[test]
    fn exact_size_iterator() {
        let mut bus = EventBus::new();
        for i in 0..5 {
            bus.emit(Ev::A(i));
        }
        assert_eq!(bus.reader().len(), 5);
    }

    #[test]
    fn clear_drops_everything() {
        let mut bus = EventBus::new();
        bus.emit(Ev::A(1));
        bus.defer(Ev::A(2));
        bus.clear();
        assert!(bus.is_empty());
        assert_eq!(bus.pending_next_frame(), 0);
    }

    #[test]
    fn event_log_keeps_newest() {
        let mut log = EventLog::with_capacity(3);
        for i in 0..10 {
            log.push(Ev::A(i));
        }
        assert_eq!(log.len(), 3);
        assert_eq!(log.total(), 10);
        assert_eq!(log.last(), Some(&Ev::A(9)));
        let recent: Vec<u32> = log
            .iter_recent()
            .map(|(_, e)| match e {
                Ev::A(v) => *v,
                Ev::B => 999,
            })
            .collect();
        assert_eq!(recent, vec![9, 8, 7]);
    }

    #[test]
    fn event_log_chronological_order() {
        let mut log = EventLog::with_capacity(10);
        for i in 0..3 {
            log.push(Ev::A(i));
        }
        let idx: Vec<u64> = log.iter_chronological().map(|(i, _)| i).collect();
        assert_eq!(idx, vec![0, 1, 2]);
    }
}
