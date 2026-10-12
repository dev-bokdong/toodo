//! Task states and the transitions allowed between them (see `docs/workflow.md`).

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskStatus {
    Planned,
    Ready,
    InProgress,
    Done,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Ready => "ready",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        use TaskStatus::*;
        matches!(
            (self, next),
            (Planned, Ready | Cancelled)
                | (Ready, Planned | InProgress | Cancelled)
                | (InProgress, Done | Cancelled)
        )
    }

    pub fn transition_to(self, next: Self) -> Result<Self, TransitionError> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(TransitionError {
                from: self,
                to: next,
            })
        }
    }
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionError {
    pub from: TaskStatus,
    pub to: TaskStatus,
}

impl fmt::Display for TransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a {} task cannot become {}", self.from, self.to)
    }
}

impl std::error::Error for TransitionError {}

#[cfg(test)]
mod tests {
    use super::TaskStatus::*;
    use super::*;

    const ALL: [TaskStatus; 5] = [Planned, Ready, InProgress, Done, Cancelled];

    #[test]
    fn only_documented_transitions_are_allowed() {
        let allowed = [
            (Planned, Ready),
            (Planned, Cancelled),
            (Ready, Planned),
            (Ready, InProgress),
            (Ready, Cancelled),
            (InProgress, Done),
            (InProgress, Cancelled),
        ];
        for from in ALL {
            for to in ALL {
                let expected = allowed.contains(&(from, to));
                assert_eq!(from.can_transition_to(to), expected, "{from} -> {to}");
            }
        }
    }

    #[test]
    fn illegal_transition_reports_both_states() {
        assert_eq!(
            Planned.transition_to(InProgress),
            Err(TransitionError {
                from: Planned,
                to: InProgress
            })
        );
        assert_eq!(
            Done.transition_to(Ready).unwrap_err().to_string(),
            "a done task cannot become ready"
        );
        assert_eq!(Ready.transition_to(InProgress), Ok(InProgress));
    }
}
