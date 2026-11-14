//! State machine of one recorded source.
//!
//! Pure logic: the session feeds it what the capture thread reports and what the user asks
//! for, and it decides what the source looks like from the outside.
//!
//! ```text
//! Starting --running--> Recording --stopped--> Ended(Removed | SessionStopped | SourceClosed)
//!    |                      |
//!    +------failed----------+--------------> Failed
//! ```

/// What the capture thread of a source reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureStatus {
    Starting,
    Running,
    Stopped,
    Failed,
}

/// Why a source stopped recording without an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// The user removed the source during the recording.
    Removed,
    /// The whole recording was stopped.
    SessionStopped,
    /// The device or application went away on its own.
    SourceClosed,
}

/// Phase of a source as seen by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourcePhase {
    Starting,
    Recording,
    Ended(EndReason),
    Failed,
}

/// Why a transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TransitionError {
    #[error("the source is no longer recording")]
    NotActive,
}

#[derive(Debug, Clone, Copy)]
pub struct SourceMachine {
    phase: SourcePhase,
    removal_requested: bool,
    session_stopping: bool,
}

impl Default for SourceMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl SourceMachine {
    pub fn new() -> Self {
        Self {
            phase: SourcePhase::Starting,
            removal_requested: false,
            session_stopping: false,
        }
    }

    pub fn phase(&self) -> SourcePhase {
        self.phase
    }

    /// True while the source still produces audio or is about to.
    pub fn is_active(&self) -> bool {
        matches!(self.phase, SourcePhase::Starting | SourcePhase::Recording)
    }

    /// The user asked to remove the source. Refused once it has ended.
    pub fn request_remove(&mut self) -> Result<(), TransitionError> {
        if !self.is_active() {
            return Err(TransitionError::NotActive);
        }
        self.removal_requested = true;
        Ok(())
    }

    /// The whole session is being stopped.
    pub fn request_session_stop(&mut self) {
        self.session_stopping = true;
    }

    /// Apply the latest status of the capture thread. Terminal phases never change.
    pub fn observe(&mut self, status: CaptureStatus) {
        if !self.is_active() {
            return;
        }
        self.phase = match status {
            CaptureStatus::Starting => SourcePhase::Starting,
            CaptureStatus::Running => SourcePhase::Recording,
            CaptureStatus::Failed => SourcePhase::Failed,
            CaptureStatus::Stopped => SourcePhase::Ended(if self.removal_requested {
                EndReason::Removed
            } else if self.session_stopping {
                EndReason::SessionStopped
            } else {
                EndReason::SourceClosed
            }),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_in_starting() {
        assert_eq!(SourceMachine::new().phase(), SourcePhase::Starting);
    }

    #[test]
    fn running_then_stopped_by_itself_is_source_closed() {
        let mut m = SourceMachine::new();
        m.observe(CaptureStatus::Running);
        assert_eq!(m.phase(), SourcePhase::Recording);
        m.observe(CaptureStatus::Stopped);
        assert_eq!(m.phase(), SourcePhase::Ended(EndReason::SourceClosed));
    }

    #[test]
    fn removal_gives_removed() {
        let mut m = SourceMachine::new();
        m.observe(CaptureStatus::Running);
        m.request_remove().unwrap();
        m.observe(CaptureStatus::Stopped);
        assert_eq!(m.phase(), SourcePhase::Ended(EndReason::Removed));
    }

    #[test]
    fn session_stop_gives_session_stopped() {
        let mut m = SourceMachine::new();
        m.observe(CaptureStatus::Running);
        m.request_session_stop();
        m.observe(CaptureStatus::Stopped);
        assert_eq!(m.phase(), SourcePhase::Ended(EndReason::SessionStopped));
    }

    #[test]
    fn removal_wins_over_session_stop() {
        let mut m = SourceMachine::new();
        m.observe(CaptureStatus::Running);
        m.request_remove().unwrap();
        m.request_session_stop();
        m.observe(CaptureStatus::Stopped);
        assert_eq!(m.phase(), SourcePhase::Ended(EndReason::Removed));
    }

    #[test]
    fn failure_while_starting() {
        let mut m = SourceMachine::new();
        m.observe(CaptureStatus::Failed);
        assert_eq!(m.phase(), SourcePhase::Failed);
        assert!(!m.is_active());
    }

    #[test]
    fn terminal_phases_do_not_change() {
        let mut m = SourceMachine::new();
        m.observe(CaptureStatus::Running);
        m.observe(CaptureStatus::Stopped);
        m.observe(CaptureStatus::Running);
        m.observe(CaptureStatus::Failed);
        assert_eq!(m.phase(), SourcePhase::Ended(EndReason::SourceClosed));
    }

    #[test]
    fn cannot_remove_an_ended_source() {
        let mut m = SourceMachine::new();
        m.observe(CaptureStatus::Failed);
        assert_eq!(m.request_remove(), Err(TransitionError::NotActive));
    }

    #[test]
    fn can_remove_while_starting() {
        let mut m = SourceMachine::new();
        assert!(m.request_remove().is_ok());
        m.observe(CaptureStatus::Stopped);
        assert_eq!(m.phase(), SourcePhase::Ended(EndReason::Removed));
    }
}
