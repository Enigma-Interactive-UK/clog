//! 60 Hz coalescing layer for streaming IPC channels.
//!
//! Design.md s10 caps streaming IPC at one message per 16 ms regardless of
//! upstream volume. Tail (P4) ticks at 250 ms so coalescing is rarely
//! load-bearing; the same emitter is the seam search (P6) hooks into when
//! hits can fan in much faster than the UI can usefully redraw.
//!
//! `TailEmitter` stays a pure pass-through (250 ms cadence is far below
//! the 60 Hz budget). `SearchEmitter` adds genuine batching: hits are
//! buffered and flushed in chunks of `SEARCH_BATCH_SIZE` so the UI sees
//! one message per chunk rather than thousands of single-hit messages.

use std::marker::PhantomData;

use serde::Serialize;
use tauri::ipc::Channel;

use crate::SearchDelta;

pub struct TailEmitter<T>
where
    T: Clone + Serialize + Send + 'static,
{
    channel: Channel<T>,
    _phantom: PhantomData<T>,
}

impl<T> TailEmitter<T>
where
    T: Clone + Serialize + Send + 'static,
{
    pub fn new(channel: Channel<T>) -> Self {
        Self {
            channel,
            _phantom: PhantomData,
        }
    }

    /// Deliver `payload` to the UI. P4 ships every event as-is because
    /// tail's 250 ms cadence is far below the 60 Hz budget.
    pub fn emit(&mut self, payload: T) -> Result<(), tauri::Error> {
        self.channel.send(payload)
    }

    /// Force-flush any buffered state. No-op in the pass-through impl;
    /// kept as a stable seam for P6.
    #[allow(clippy::unused_self)]
    pub fn flush(&mut self) {}
}

/// One emitted batch can carry up to this many hits. Picked so a fast
/// regex search across a 75k-record file produces tens of messages, not
/// thousands -- the UI re-renders are cheaper to amortise.
pub const SEARCH_BATCH_SIZE: usize = 512;

/// Buffering wrapper around the search channel. `push` accumulates hits,
/// `flush` ships a non-empty batch with `done=false`, and `finish`
/// flushes any remainder and ships a terminal `done=true` marker.
pub struct SearchEmitter {
    channel: Channel<SearchDelta>,
    search_id: u64,
    buffer: Vec<clog_core::HitRef>,
    /// Running tally of hits delivered so far. The UI shows this as a
    /// growing count while the search is in flight.
    total_emitted: u64,
}

impl SearchEmitter {
    pub fn new(channel: Channel<SearchDelta>, search_id: u64) -> Self {
        Self {
            channel,
            search_id,
            buffer: Vec::with_capacity(SEARCH_BATCH_SIZE),
            total_emitted: 0,
        }
    }

    pub fn push(&mut self, hit: clog_core::HitRef) -> Result<(), tauri::Error> {
        self.buffer.push(hit);
        if self.buffer.len() >= SEARCH_BATCH_SIZE {
            self.flush_batch(false)?;
        }
        Ok(())
    }

    fn flush_batch(&mut self, done: bool) -> Result<(), tauri::Error> {
        if self.buffer.is_empty() && !done {
            return Ok(());
        }
        let drained: Vec<_> = self.buffer.drain(..).collect();
        self.total_emitted += drained.len() as u64;
        let delta = SearchDelta {
            search_id: self.search_id,
            hits: drained,
            total: self.total_emitted,
            done,
        };
        self.channel.send(delta)
    }

    /// Drain remaining hits and ship a terminal `done=true` message.
    pub fn finish(mut self) -> Result<(), tauri::Error> {
        self.flush_batch(true)
    }

    /// Abort: ship a final `done=true` with no remaining hits. Used when
    /// the search was cancelled mid-flight so the UI can free its state.
    pub fn abort(mut self) -> Result<(), tauri::Error> {
        self.buffer.clear();
        self.flush_batch(true)
    }
}
