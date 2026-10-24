//! Commands for reviewing the work done in a period, such as a week or a year.

use std::path::Path;

use super::State;

use super::types::{ReviewEntry, Task, Timestamp};
use super::{CommandError, CommandResult, StorageState, lock};
use crate::domain::review::{self, Period};
use crate::storage::Storage;

/// Tasks of every workspace that reached a milestone or got a commit in `[start, end)`,
/// ordered by outcome, then by time. The caller picks the period, such as a week, a
/// month, or a year in the local time zone.
#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn review_period(
    storage: State<'_, StorageState>,
    start: Timestamp,
    end: Timestamp,
) -> CommandResult<Vec<ReviewEntry>> {
    review_entries(&lock(&storage), start, end)
}

/// Writes a report built by the frontend to `path`, replacing any file there. Only
/// Markdown (`.md`) and CSV (`.csv`) files are written. Desktop only: the web app
/// downloads reports in the browser instead.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn save_report(path: String, contents: String) -> CommandResult<()> {
    write_report(Path::new(&path), &contents)
}

#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
fn write_report(path: &Path, contents: &str) -> CommandResult<()> {
    if !path.is_absolute() {
        return Err(CommandError::invalid_input(format!(
            "{} is not an absolute path",
            path.display()
        )));
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    if !matches!(extension.as_deref(), Some("md" | "csv")) {
        return Err(CommandError::invalid_input(format!(
            "{} is not a Markdown (.md) or CSV (.csv) file",
            path.display()
        )));
    }
    std::fs::write(path, contents).map_err(|error| {
        CommandError::internal(format!(
            "failed to save the report to {}: {error}",
            path.display()
        ))
    })
}

fn review_entries(
    storage: &Storage,
    start: Timestamp,
    end: Timestamp,
) -> CommandResult<Vec<ReviewEntry>> {
    let period = Period::new(start.0, end.0)
        .map_err(|error| CommandError::invalid_input(error.to_string()))?;
    let mut tasks = Vec::new();
    for workspace in storage.list_workspaces()? {
        tasks.extend(storage.list_tasks(workspace.id)?);
    }
    let commits = storage.list_commit_records_in(period)?;
    Ok(review::review(period, &tasks, &commits)
        .into_iter()
        .map(|reviewed| {
            let task = tasks
                .iter()
                .find(|task| task.id == reviewed.task_id)
                .expect("a review entry belongs to a reviewed task");
            ReviewEntry {
                task: Task::from(task),
                outcome: reviewed.outcome.into(),
                at: Timestamp(reviewed.at),
                commits: reviewed.commits.iter().map(Into::into).collect(),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::CommandErrorKind;
    use crate::commands::types::ReviewOutcome;
    use crate::domain::model::{CommitRecord, TaskType, WorkspaceLocation};
    use crate::domain::path::WindowsPath;
    use crate::domain::status::TaskStatus;
    use crate::storage::{NewTask, NewWorkspace};

    #[test]
    fn reviews_tasks_of_every_workspace_and_rejects_an_empty_period() {
        let mut storage = Storage::open_in_memory().unwrap();
        storage.set_clock(|| 500);
        let mut add_task = |name: &str| {
            let workspace = storage
                .create_workspace(&NewWorkspace {
                    name: name.to_owned(),
                    location: WorkspaceLocation::Local {
                        path: WindowsPath::parse(&format!(r"C:\dev\{name}"))
                            .unwrap()
                            .into(),
                    },
                    default_base_branch: "main".to_owned(),
                })
                .unwrap();
            storage
                .create_task(&NewTask {
                    workspace_id: workspace.id,
                    task_type: TaskType::Feat,
                    title: format!("Work on {name}"),
                    description: String::new(),
                    estimate: None,
                    due_date: None,
                    criteria: Vec::new(),
                })
                .unwrap()
        };
        let started = add_task("toodo");
        let committed = add_task("herdr");
        storage
            .set_task_status(started.id, TaskStatus::InProgress)
            .unwrap();
        let record = CommitRecord {
            task_id: committed.id,
            sha: "abc".to_owned(),
            message: "Add herdr".to_owned(),
            author: "Dev <dev@example.com>".to_owned(),
            committed_at: 1500,
        };
        storage
            .replace_commit_records(committed.id, std::slice::from_ref(&record))
            .unwrap();

        let entries = review_entries(&storage, Timestamp(1000), Timestamp(2000)).unwrap();
        let summary: Vec<_> = entries
            .iter()
            .map(|entry| (entry.task.title.as_str(), entry.outcome, entry.at.0))
            .collect();
        assert_eq!(summary, [("Work on herdr", ReviewOutcome::WorkedOn, 1500)]);
        assert_eq!(entries[0].commits, [(&record).into()]);

        let entries = review_entries(&storage, Timestamp(0), Timestamp(1000)).unwrap();
        let summary: Vec<_> = entries
            .iter()
            .map(|entry| (entry.task.title.as_str(), entry.outcome))
            .collect();
        assert_eq!(
            summary,
            [
                ("Work on toodo", ReviewOutcome::Started),
                ("Work on herdr", ReviewOutcome::Planned),
            ]
        );

        let error = review_entries(&storage, Timestamp(10), Timestamp(10)).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::InvalidInput);
    }

    #[test]
    fn saves_reports_only_as_markdown_or_csv_at_an_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let csv = dir.path().join("review.CSV");
        write_report(&csv, "id,title\n1,Add storage\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&csv).unwrap(),
            "id,title\n1,Add storage\n"
        );
        let markdown = dir.path().join("review.md");
        write_report(&markdown, "# Review").unwrap();
        write_report(&markdown, "# Year review").unwrap();
        assert_eq!(std::fs::read_to_string(&markdown).unwrap(), "# Year review");

        let text = dir.path().join("review.txt");
        let error = write_report(&text, "text").unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::InvalidInput);
        assert!(!text.exists());
        let error = write_report(Path::new("review.csv"), "text").unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::InvalidInput);
        let error = write_report(&dir.path().join("review"), "text").unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::InvalidInput);
    }
}
