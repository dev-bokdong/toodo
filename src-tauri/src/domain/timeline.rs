//! Status history of a task and the milestones derived from it (see
//! `docs/architecture.md`, Task Timeline).

use super::status::TaskStatus;

/// One status change of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskEvent {
    pub status: TaskStatus,
    /// Time of the change in seconds since the Unix epoch (UTC).
    pub at: i64,
}

/// Milestones of a task, in seconds since the Unix epoch. `None` when the task has not
/// reached the milestone or its history does not record it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Milestones {
    /// Task creation: the first event.
    pub planned_at: Option<i64>,
    /// The latest move to `ready` before work started, i.e. when the last dependency was
    /// done. Cleared when a new dependency moves the task back to `planned`.
    pub ready_at: Option<i64>,
    pub started_at: Option<i64>,
    /// The first move to `in_review`, when the pull request was opened.
    pub review_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub cancelled_at: Option<i64>,
}

/// Derives the milestones of a task from its status history, oldest event first.
pub fn milestones(timeline: &[TaskEvent]) -> Milestones {
    let mut milestones = Milestones {
        planned_at: timeline.first().map(|event| event.at),
        ..Milestones::default()
    };
    for event in timeline {
        let started = milestones.started_at.is_some();
        match event.status {
            TaskStatus::Planned if !started => milestones.ready_at = None,
            TaskStatus::Ready if !started => milestones.ready_at = Some(event.at),
            TaskStatus::Planned | TaskStatus::Ready => {}
            TaskStatus::InProgress => {
                milestones.started_at.get_or_insert(event.at);
            }
            TaskStatus::InReview => {
                milestones.review_at.get_or_insert(event.at);
            }
            TaskStatus::Done => milestones.completed_at = Some(event.at),
            TaskStatus::Cancelled => milestones.cancelled_at = Some(event.at),
        }
    }
    milestones
}

#[cfg(test)]
mod tests {
    use super::TaskStatus::*;
    use super::*;

    fn events(raw: &[(TaskStatus, i64)]) -> Vec<TaskEvent> {
        raw.iter()
            .map(|&(status, at)| TaskEvent { status, at })
            .collect()
    }

    #[test]
    fn a_task_without_history_has_no_milestones() {
        assert_eq!(milestones(&[]), Milestones::default());
    }

    #[test]
    fn a_completed_task_has_every_milestone_but_cancellation() {
        let timeline = events(&[
            (Planned, 10),
            (Ready, 20),
            (InProgress, 30),
            (InReview, 40),
            (Done, 50),
        ]);
        assert_eq!(
            milestones(&timeline),
            Milestones {
                planned_at: Some(10),
                ready_at: Some(20),
                started_at: Some(30),
                review_at: Some(40),
                completed_at: Some(50),
                cancelled_at: None,
            }
        );
    }

    #[test]
    fn ready_at_is_the_last_release_before_work_started() {
        let waiting_again = events(&[(Ready, 10), (Planned, 20)]);
        assert_eq!(milestones(&waiting_again).ready_at, None);
        assert_eq!(milestones(&waiting_again).planned_at, Some(10));

        let released_again = events(&[(Ready, 10), (Planned, 20), (Ready, 30), (InProgress, 40)]);
        assert_eq!(milestones(&released_again).ready_at, Some(30));
    }

    #[test]
    fn a_cancelled_task_keeps_the_milestones_it_reached() {
        let timeline = events(&[(Ready, 10), (InProgress, 20), (Cancelled, 30)]);
        assert_eq!(
            milestones(&timeline),
            Milestones {
                planned_at: Some(10),
                ready_at: Some(10),
                started_at: Some(20),
                review_at: None,
                completed_at: None,
                cancelled_at: Some(30),
            }
        );
    }
}
