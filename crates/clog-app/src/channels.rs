//! 60 Hz coalescing layer for streaming IPC channels.
//!
//! Design.md s10 caps streaming IPC at one message per 16 ms regardless of
//! upstream volume. Tail (P4) ticks at 250 ms so coalescing is rarely
//! load-bearing; the same emitter is the seam search (P6) will hook into
//! when hits can fan in much faster than the UI can usefully redraw.
//!
//! The current impl is a pure pass-through that always flushes the latest
//! value through the channel. The tail loop calls `emit()` on every event
//! and `flush()` after each tick; both behave the same today. When P6
//! lands, swap the body of `emit` for a merge + 16 ms gating step.

use std::marker::PhantomData;

use serde::Serialize;
use tauri::ipc::Channel;

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
