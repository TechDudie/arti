//! Management for flow control windows.
//!
//! Tor maintains a separate windows on circuits and on streams.
//! These are controlled by SENDME cells, which (confusingly) are
//! applied either at the circuit or the stream level depending on
//! whether they have a stream ID set.
//!
//! Circuit sendmes are _authenticated_: they include a cryptographic
//! tag generated by the cryptography layer.  This tag proves that the
//! other side of the circuit really has read all of the data that it's
//! acknowledging.

use std::collections::VecDeque;

use tor_cell::relaycell::RelayCmd;
use tor_cell::relaycell::UnparsedRelayMsg;
use tor_error::internal;

use crate::{Error, Result};

/// Tag type used in regular v1 sendme cells.
///
// TODO(nickm):
// Three problems with this tag:
//  - First, we need to support unauthenticated flow control, but we
//    still record the tags that we _would_ expect.
//  - Second, this tag type could be different for each layer, if we
//    eventually have an authenticator that isn't 20 bytes long.
#[derive(Clone, Debug, derive_more::Into)]
pub(crate) struct CircTag([u8; 20]);

impl From<[u8; 20]> for CircTag {
    fn from(v: [u8; 20]) -> CircTag {
        Self(v)
    }
}
impl PartialEq for CircTag {
    fn eq(&self, other: &Self) -> bool {
        crate::util::ct::bytes_eq(&self.0, &other.0)
    }
}
impl Eq for CircTag {}
impl PartialEq<[u8; 20]> for CircTag {
    fn eq(&self, other: &[u8; 20]) -> bool {
        crate::util::ct::bytes_eq(&self.0, &other[..])
    }
}

/// A circuit's send window.
pub(crate) type CircSendWindow = SendWindow<CircParams>;
/// A stream's send window.
pub(crate) type StreamSendWindow = SendWindow<StreamParams>;

/// A circuit's receive window.
pub(crate) type CircRecvWindow = RecvWindow<CircParams>;
/// A stream's receive window.
pub(crate) type StreamRecvWindow = RecvWindow<StreamParams>;

/// Tracks how many cells we can safely send on a circuit or stream.
///
/// Additionally, remembers a list of tags that could be used to
/// acknowledge the cells we have already sent, so we know it's safe
/// to send more.
#[derive(Clone, Debug)]
pub(crate) struct SendWindow<P>
where
    P: WindowParams,
{
    /// Current value for this window
    window: u16,
    /// Marker type to tell the compiler that the P type is used.
    _dummy: std::marker::PhantomData<P>,
}

/// Helper: parametrizes a window to determine its maximum and its increment.
pub(crate) trait WindowParams {
    /// Largest allowable value for this window.
    #[allow(dead_code)] // TODO #1383 failure to ever use this is probably a bug
    fn maximum() -> u16;
    /// Increment for this window.
    fn increment() -> u16;
    /// The default starting value.
    fn start() -> u16;
}

/// Parameters used for SENDME windows on circuits: limit at 1000 cells,
/// and each SENDME adjusts by 100.
#[derive(Clone, Debug)]
pub(crate) struct CircParams;
impl WindowParams for CircParams {
    fn maximum() -> u16 {
        1000
    }
    fn increment() -> u16 {
        100
    }
    fn start() -> u16 {
        1000
    }
}

/// Parameters used for SENDME windows on streams: limit at 500 cells,
/// and each SENDME adjusts by 50.
#[derive(Clone, Debug)]
pub(crate) struct StreamParams;
impl WindowParams for StreamParams {
    fn maximum() -> u16 {
        500
    }
    fn increment() -> u16 {
        50
    }
    fn start() -> u16 {
        500
    }
}

/// Object used to validate SENDMEs as in managing the authenticated tag and verifying it.
#[derive(Clone, Debug)]
pub(crate) struct SendmeValidator<T>
where
    T: PartialEq + Eq + Clone,
{
    /// Tag values that incoming "SENDME" messages need to match in order
    /// for us to send more data.
    tags: VecDeque<T>,
}

impl<T> SendmeValidator<T>
where
    T: PartialEq + Eq + Clone,
{
    /// Constructor
    pub(crate) fn new() -> Self {
        Self {
            tags: VecDeque::new(),
        }
    }

    /// Record a SENDME tag for future validation once we receive it.
    pub(crate) fn record<U>(&mut self, tag: &U)
    where
        U: Clone + Into<T>,
    {
        self.tags.push_back(tag.clone().into());
    }

    /// Validate a received tag (if any). A mismatch leads to a protocol violation and the circuit
    /// MUST be closed.
    pub(crate) fn validate<U>(&mut self, tag: Option<U>) -> Result<()>
    where
        T: PartialEq<U>,
    {
        match (self.tags.front(), tag) {
            (Some(t), Some(tag)) if t == &tag => {} // this is the right tag.
            (Some(_), None) => {}                   // didn't need a tag.
            (Some(_), Some(_)) => {
                return Err(Error::CircProto("Mismatched tag on circuit SENDME".into()));
            }
            (None, _) => {
                return Err(Error::CircProto(
                    "Received a SENDME when none was expected".into(),
                ));
            }
        }
        self.tags.pop_front();
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn expected_tags(&self) -> Vec<T> {
        self.tags.iter().map(Clone::clone).collect()
    }
}

impl<P> SendWindow<P>
where
    P: WindowParams,
{
    /// Construct a new SendWindow.
    pub(crate) fn new(window: u16) -> SendWindow<P> {
        SendWindow {
            window,
            _dummy: std::marker::PhantomData,
        }
    }

    /// Return true iff the SENDME tag should be recorded.
    pub(crate) fn should_record_tag(&self) -> bool {
        self.window % P::increment() == 0
    }

    /// Remove one item from this window (since we've sent a cell).
    /// If the window was empty, returns an error.
    pub(crate) fn take(&mut self) -> Result<()> {
        self.window = self.window.checked_sub(1).ok_or(Error::CircProto(
            "Called SendWindow::take() on empty SendWindow".into(),
        ))?;
        Ok(())
    }

    /// Handle an incoming sendme.
    ///
    /// On failure, return an error: the caller must close the circuit due to a protocol violation.
    #[must_use = "didn't check whether SENDME was expected."]
    pub(crate) fn put(&mut self) -> Result<()> {
        // Overflow check.
        let new_window = self
            .window
            .checked_add(P::increment())
            .ok_or(Error::from(internal!("Overflow on SENDME window")))?;
        // Make sure we never go above our maximum else this wasn't expected.
        if new_window > P::maximum() {
            return Err(Error::CircProto("Unexpected stream SENDME".into()));
        }
        self.window = new_window;
        Ok(())
    }

    /// Return the current send window value.
    pub(crate) fn window(&self) -> u16 {
        self.window
    }
}

/// Structure to track when we need to send SENDME cells for incoming data.
#[derive(Clone, Debug)]
pub(crate) struct RecvWindow<P: WindowParams> {
    /// Number of cells that we'd be willing to receive on this window
    /// before sending a SENDME.
    window: u16,
    /// Marker type to tell the compiler that the P type is used.
    _dummy: std::marker::PhantomData<P>,
}

impl<P: WindowParams> RecvWindow<P> {
    /// Create a new RecvWindow.
    pub(crate) fn new(window: u16) -> RecvWindow<P> {
        RecvWindow {
            window,
            _dummy: std::marker::PhantomData,
        }
    }

    /// Called when we've just received a cell; return true if we need to send
    /// a sendme, and false otherwise.
    ///
    /// Returns None if we should not have sent the cell, and we just
    /// violated the window.
    pub(crate) fn take(&mut self) -> Result<bool> {
        let v = self.window.checked_sub(1);
        if let Some(x) = v {
            self.window = x;
            // TODO: same note as in SendWindow.take(). I don't know if
            // this truly matches the spec, but tor accepts it.
            Ok(x % P::increment() == 0)
        } else {
            Err(Error::CircProto(
                "Received a data cell in violation of a window".into(),
            ))
        }
    }

    /// Reduce this window by `n`; give an error if this is not possible.
    pub(crate) fn decrement_n(&mut self, n: u16) -> crate::Result<()> {
        self.window = self.window.checked_sub(n).ok_or(Error::CircProto(
            "Received too many cells on a stream".into(),
        ))?;
        Ok(())
    }

    /// Called when we've just sent a SENDME.
    pub(crate) fn put(&mut self) {
        self.window = self
            .window
            .checked_add(P::increment())
            .expect("Overflow detected while attempting to increment window");
    }
}

/// Return true if this message type is counted by flow-control windows.
pub(crate) fn cmd_counts_towards_windows(cmd: RelayCmd) -> bool {
    cmd == RelayCmd::DATA
}

/// Return true if this message is counted by flow-control windows.
#[cfg(test)]
pub(crate) fn msg_counts_towards_windows(msg: &tor_cell::relaycell::msg::AnyRelayMsg) -> bool {
    use tor_cell::relaycell::RelayMsg;
    cmd_counts_towards_windows(msg.cmd())
}

/// Return true if this message is counted by flow-control windows.
pub(crate) fn cell_counts_towards_windows(cell: &UnparsedRelayMsg) -> bool {
    cmd_counts_towards_windows(cell.cmd())
}

#[cfg(test)]
mod test {
    // @@ begin test lint list maintained by maint/add_warning @@
    #![allow(clippy::bool_assert_comparison)]
    #![allow(clippy::clone_on_copy)]
    #![allow(clippy::dbg_macro)]
    #![allow(clippy::mixed_attributes_style)]
    #![allow(clippy::print_stderr)]
    #![allow(clippy::print_stdout)]
    #![allow(clippy::single_char_pattern)]
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::unchecked_duration_subtraction)]
    #![allow(clippy::useless_vec)]
    #![allow(clippy::needless_pass_by_value)]
    //! <!-- @@ end test lint list maintained by maint/add_warning @@ -->
    use super::*;
    use tor_basic_utils::test_rng::testing_rng;
    use tor_cell::relaycell::{msg, AnyRelayMsgOuter, RelayCellFormat, StreamId};

    #[test]
    fn what_counts() {
        let mut rng = testing_rng();
        let m = msg::Begin::new("www.torproject.org", 443, 0)
            .unwrap()
            .into();
        assert!(!msg_counts_towards_windows(&m));
        assert!(!cell_counts_towards_windows(
            &UnparsedRelayMsg::from_singleton_body(
                RelayCellFormat::V0,
                AnyRelayMsgOuter::new(StreamId::new(77), m)
                    .encode(&mut rng)
                    .unwrap()
            )
            .unwrap()
        ));

        let m = msg::Data::new(&b"Education is not a prerequisite to political control-political control is the cause of popular education."[..]).unwrap().into(); // Du Bois
        assert!(msg_counts_towards_windows(&m));
        assert!(cell_counts_towards_windows(
            &UnparsedRelayMsg::from_singleton_body(
                RelayCellFormat::V0,
                AnyRelayMsgOuter::new(StreamId::new(128), m)
                    .encode(&mut rng)
                    .unwrap()
            )
            .unwrap()
        ));
    }

    #[test]
    fn recvwindow() {
        let mut w: RecvWindow<StreamParams> = RecvWindow::new(500);

        for _ in 0..49 {
            assert!(!w.take().unwrap());
        }
        assert!(w.take().unwrap());
        assert_eq!(w.window, 450);

        assert!(w.decrement_n(123).is_ok());
        assert_eq!(w.window, 327);

        w.put();
        assert_eq!(w.window, 377);

        // failing decrement.
        assert!(w.decrement_n(400).is_err());
        // failing take.
        assert!(w.decrement_n(377).is_ok());
        assert!(w.take().is_err());
    }

    fn new_sendwindow() -> SendWindow<CircParams> {
        SendWindow::new(1000)
    }

    #[test]
    fn sendwindow_basic() -> Result<()> {
        let mut w = new_sendwindow();

        w.take()?;
        assert_eq!(w.window(), 999);
        for _ in 0_usize..98 {
            w.take()?;
        }
        assert_eq!(w.window(), 901);

        w.take()?;
        assert_eq!(w.window(), 900);

        w.take()?;
        assert_eq!(w.window(), 899);

        // Try putting a good tag.
        w.put()?;
        assert_eq!(w.window(), 999);

        for _ in 0_usize..300 {
            w.take()?;
        }

        // Put without a tag.
        w.put()?;
        assert_eq!(w.window(), 799);

        Ok(())
    }

    #[test]
    fn sendwindow_erroring() -> Result<()> {
        let mut w = new_sendwindow();
        for _ in 0_usize..1000 {
            w.take()?;
        }
        assert_eq!(w.window(), 0);

        let ready = w.take();
        assert!(ready.is_err());
        Ok(())
    }
}
