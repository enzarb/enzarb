//! Pure per-channel flow-control state machine (one instance per channel, per
//! endpoint). No I/O, no clocks.
//!
//! Each direction is independent. Only `DATA_BIN` and `DATA_JSON` payload
//! bytes consume credit; `CTRL`, `WINDOW` and `CLOSE` are free.

use crate::frame::{INITIAL_WINDOW, MAX_CREDIT};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FlowError {
    #[error("send of {needed} bytes exceeds credit {available}")]
    InsufficientCredit { needed: u64, available: u64 },
    #[error("send after local close")]
    LocalClosed,
    #[error("local side already closed")]
    AlreadyClosed,
    #[error("peer sent {got} bytes with only {granted} granted")]
    Overrun { got: u64, granted: u64 },
    #[error("data after peer close")]
    DataAfterClose,
    #[error("window grant would push credit past 2^31-1")]
    CreditOverflow,
    #[error("released more bytes than were received")]
    ReleaseUnderflow,
}

#[derive(Debug, Clone)]
pub struct FlowControl {
    /// Bytes we may still send.
    send_credit: u64,
    /// Credit we granted the peer that it has not used yet.
    recv_granted: u64,
    /// Bytes received but not yet released by the application.
    recv_unreleased: u64,
    /// Released bytes not yet re-granted.
    pending_grant: u64,
    /// Re-grant once `pending_grant` reaches this.
    grant_threshold: u64,
    local_closed: bool,
    remote_closed: bool,
}

impl Default for FlowControl {
    fn default() -> Self {
        Self::new(INITIAL_WINDOW, INITIAL_WINDOW)
    }
}

impl FlowControl {
    /// `initial_send`: credit the peer gave us at open. `initial_recv`: credit
    /// we give the peer at open. Both are normally [`INITIAL_WINDOW`].
    pub fn new(initial_send: u32, initial_recv: u32) -> Self {
        Self {
            send_credit: initial_send.min(MAX_CREDIT) as u64,
            recv_granted: initial_recv.min(MAX_CREDIT) as u64,
            recv_unreleased: 0,
            pending_grant: 0,
            grant_threshold: (initial_recv as u64 / 2).max(1),
            local_closed: false,
            remote_closed: false,
        }
    }

    // ---- sending ----

    pub fn send_credit(&self) -> u64 {
        self.send_credit
    }

    /// Largest chunk (<= `want`) that may be sent right now.
    pub fn sendable(&self, want: usize) -> usize {
        if self.local_closed {
            return 0;
        }
        (self.send_credit.min(want as u64)) as usize
    }

    /// Account for sending a data frame of `len` payload bytes.
    pub fn on_send(&mut self, len: usize) -> Result<(), FlowError> {
        if self.local_closed {
            return Err(FlowError::LocalClosed);
        }
        let len = len as u64;
        if len > self.send_credit {
            return Err(FlowError::InsufficientCredit {
                needed: len,
                available: self.send_credit,
            });
        }
        self.send_credit -= len;
        Ok(())
    }

    /// Peer granted more credit (a `WINDOW` frame). Ignored after local close.
    pub fn on_window(&mut self, credit: u32) -> Result<(), FlowError> {
        if self.local_closed {
            return Ok(());
        }
        let total = self.send_credit + credit as u64;
        if total > MAX_CREDIT as u64 {
            return Err(FlowError::CreditOverflow);
        }
        self.send_credit = total;
        Ok(())
    }

    /// We send `CLOSE`: no more data from us.
    pub fn close_local(&mut self) -> Result<(), FlowError> {
        if self.local_closed {
            return Err(FlowError::AlreadyClosed);
        }
        self.local_closed = true;
        self.send_credit = 0;
        Ok(())
    }

    // ---- receiving ----

    /// Credit currently outstanding with the peer.
    pub fn recv_granted(&self) -> u64 {
        self.recv_granted
    }

    /// A data frame of `len` bytes arrived.
    pub fn on_data(&mut self, len: usize) -> Result<(), FlowError> {
        if self.remote_closed {
            return Err(FlowError::DataAfterClose);
        }
        let len = len as u64;
        if len > self.recv_granted {
            return Err(FlowError::Overrun {
                got: len,
                granted: self.recv_granted,
            });
        }
        self.recv_granted -= len;
        self.recv_unreleased += len;
        Ok(())
    }

    /// The application consumed `len` received bytes. Returns a `WINDOW`
    /// credit to send once enough has accumulated (half the initial window);
    /// returns `None` if the peer has closed (no point granting).
    pub fn release(&mut self, len: usize) -> Result<Option<u32>, FlowError> {
        let len = len as u64;
        if len > self.recv_unreleased {
            return Err(FlowError::ReleaseUnderflow);
        }
        self.recv_unreleased -= len;
        self.pending_grant += len;
        if self.remote_closed || self.pending_grant < self.grant_threshold {
            return Ok(None);
        }
        Ok(self.take_grant())
    }

    /// Force out any pending grant regardless of the threshold.
    pub fn flush_grant(&mut self) -> Option<u32> {
        if self.remote_closed {
            return None;
        }
        self.take_grant()
    }

    fn take_grant(&mut self) -> Option<u32> {
        if self.pending_grant == 0 {
            return None;
        }
        let g = self.pending_grant;
        self.pending_grant = 0;
        self.recv_granted += g;
        Some(g as u32)
    }

    /// Peer sent `CLOSE`.
    pub fn on_close_remote(&mut self) {
        self.remote_closed = true;
    }

    // ---- state ----

    pub fn is_local_closed(&self) -> bool {
        self.local_closed
    }

    pub fn is_remote_closed(&self) -> bool {
        self.remote_closed
    }

    /// Both directions closed: the channel id may be forgotten.
    pub fn is_closed(&self) -> bool {
        self.local_closed && self.remote_closed
    }
}
