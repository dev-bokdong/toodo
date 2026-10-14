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
    /// The first move to `in_progress`; restarting a paused task keeps it.
    pub started_at: Option<i64>,
    /// The first move to `in_review`, when the pull request was opened.
    pub review_at: Option<i64>,
    pub completed_at: Option<i64>,
    /// The latest pause, a move back to `planned` after work started, unless the task
    /// was started again after it.
    pub paused_at: Option<i64>,
}

/// Derives the milestones of a task from its status history, oldest event first.
pub fn milestones(timeline: &[TaskEvent]) -> Milestones {
    let mut milestones = Milestones {
        planned_at: timeline.first().map(|event| event.at),
        ..Milestones::default()
    };
    for event in timeline {
        match event.status {
            TaskStatus::Planned => {
                if milestones.started_at.is_some() {
                    milestones.paused_at = Some(event.at);
                }
            }
            TaskStatus::InProgress => {
                milestones.started_at.get_or_insert(event.at);
                milestones.paused_at = None;
            }
            TaskStatus::InReview => {
                milestones.review_at.get_or_insert(event.at);
            }
            TaskStatus::Done => milestones.completed_at = Some(event.at),
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
    fn a_completed_task_has_every_milestone_but_a_pause() {
        let timeline = events(&[(Planned, 10), (InProgress, 30), (InReview, 40), (Done, 50)]);
        assert_eq!(
            milestones(&timeline),
            Milestones {
                planned_at: Some(10),
                started_at: Some(30),
                review_at: Some(40),
                completed_at: Some(50),
                paused_at: None,
            }
        );
    }

    #[test]
    fn a_paused_task_keeps_the_milestones_it_reached() {
        let timeline = events(&[(Planned, 10), (InProgress, 20), (Planned, 30)]);
        assert_eq!(
            milestones(&timeline),
            Milestones {
                planned_at: Some(10),
                started_at: Some(20),
                review_at: None,
                completed_at: None,
                paused_at: Some(30),
            }
        );
    }

    #[test]
    fn a_restarted_task_is_no_longer_paused_and_keeps_its_first_start() {
        let mut timeline = events(&[
            (Planned, 10),
            (InProgress, 20),
            (Planned, 30),
            (InProgress, 40),
        ]);
        assert_eq!(milestones(&timeline).started_at, Some(20));
        assert_eq!(milestones(&timeline).paused_at, None);

        timeline.extend(events(&[(InReview, 50), (Planned, 60)]));
        assert_eq!(milestones(&timeline).review_at, Some(50));
        assert_eq!(milestones(&timeline).paused_at, Some(60));
    }
}
