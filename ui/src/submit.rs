//! Composer submission policy shared by plain Enter and Cmd/Ctrl+Enter.
//!
//! The official client resolves one submission gesture into a delivery mode
//! against the busy-Enter preference (`ui-conversation/src/client/input/submission-policy.ts`):
//! plain Enter and the Send button follow the preference, the amplified chord
//! uses the opposite mode, and a session that cannot steer always queues.
//! Keeping this decision here (instead of in the view) lets the Host-facing
//! call sites stay a two-line match and makes the rule host-testable.

/// Which composer gesture produced one submission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmitGesture {
    /// Plain Enter or the Send button.
    Enter,
    /// Cmd+Enter / Ctrl+Enter.
    Accelerated,
}

/// How the Host should deliver a message composed while the agent is busy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SubmitMode {
    /// Append a whole new turn after the running one.
    #[default]
    Queue,
    /// Join the running turn at its next step boundary.
    Steer,
}

impl SubmitMode {
    /// Wire spelling accepted by `_dsh/gpui/session/queue-prompt`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queue => "queue",
            Self::Steer => "steer",
        }
    }

    /// Parse the Host's echoed mode; an unknown value falls back to queue.
    pub fn from_wire(value: Option<&str>) -> Self {
        match value {
            Some("steer") => Self::Steer,
            _ => Self::Queue,
        }
    }

    /// Dock badge text for one pending row.
    pub fn label(self) -> &'static str {
        match self {
            Self::Queue => "排队",
            Self::Steer => "插话",
        }
    }
}

/// Default busy-Enter behavior, matching the official `DEFAULT_BUSY_ENTER_BEHAVIOR`.
pub const DEFAULT_BUSY_ENTER: SubmitMode = SubmitMode::Queue;

/// Resolve one gesture into its delivery mode.
///
/// Mirrors official `resolveSubmitMode` exactly: a busy session that cannot
/// steer (or an idle one) always queues, plain Enter follows the preference,
/// and the accelerated chord takes the complementary mode.
pub fn resolve_submit_mode(
    preferred: SubmitMode,
    running: bool,
    gesture: SubmitGesture,
    steering_available: bool,
) -> SubmitMode {
    if !running || !steering_available {
        return SubmitMode::Queue;
    }
    match (gesture, preferred) {
        (SubmitGesture::Enter, preferred) => preferred,
        (SubmitGesture::Accelerated, SubmitMode::Queue) => SubmitMode::Steer,
        (SubmitGesture::Accelerated, SubmitMode::Steer) => SubmitMode::Queue,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_composer_always_queues() {
        // Not running: the message starts a turn, so the mode is irrelevant and
        // the policy reports the default rather than steering into nothing.
        for gesture in [SubmitGesture::Enter, SubmitGesture::Accelerated] {
            for preferred in [SubmitMode::Queue, SubmitMode::Steer] {
                assert_eq!(
                    resolve_submit_mode(preferred, false, gesture, true),
                    SubmitMode::Queue
                );
            }
        }
    }

    #[test]
    fn a_transport_without_steering_queues() {
        assert_eq!(
            resolve_submit_mode(SubmitMode::Steer, true, SubmitGesture::Enter, false),
            SubmitMode::Queue
        );
        assert_eq!(
            resolve_submit_mode(SubmitMode::Queue, true, SubmitGesture::Accelerated, false),
            SubmitMode::Queue
        );
    }

    #[test]
    fn enter_follows_the_preference_and_the_chord_is_complementary() {
        assert_eq!(
            resolve_submit_mode(SubmitMode::Queue, true, SubmitGesture::Enter, true),
            SubmitMode::Queue
        );
        assert_eq!(
            resolve_submit_mode(SubmitMode::Queue, true, SubmitGesture::Accelerated, true),
            SubmitMode::Steer
        );
        assert_eq!(
            resolve_submit_mode(SubmitMode::Steer, true, SubmitGesture::Enter, true),
            SubmitMode::Steer
        );
        assert_eq!(
            resolve_submit_mode(SubmitMode::Steer, true, SubmitGesture::Accelerated, true),
            SubmitMode::Queue
        );
    }

    #[test]
    fn wire_spelling_round_trips_and_unknown_values_queue() {
        assert_eq!(SubmitMode::Queue.as_str(), "queue");
        assert_eq!(SubmitMode::Steer.as_str(), "steer");
        assert_eq!(SubmitMode::from_wire(Some("steer")), SubmitMode::Steer);
        assert_eq!(SubmitMode::from_wire(Some("queue")), SubmitMode::Queue);
        assert_eq!(SubmitMode::from_wire(Some("later")), SubmitMode::Queue);
        assert_eq!(SubmitMode::from_wire(None), SubmitMode::Queue);
    }
}
