//! How a task finished, with what showed it, and the report of it that the server posts
//! to the chat itself, without the lead. A finish is only called a merge when a merge was
//! seen: a task whose criteria were met while its pull request was still open is reported
//! as a pull request that is ready, not as merged.

use super::model::{MergedWith, TaskId};

/// The kind of a finish, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishKind {
    PullRequestReady,
    Merged,
    NotAdopted,
    Completed,
}

impl FinishKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PullRequestReady => "pr_ready",
            Self::Merged => "merged",
            Self::NotAdopted => "not_adopted",
            Self::Completed => "completed",
        }
    }
}

/// How a task became `done`, or changed how it is done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finish {
    /// Its criteria were met while its pull request at `url` was open with passing checks
    /// on `head_sha`; no merge was seen.
    PullRequestReady {
        url: String,
        head_sha: Option<String>,
    },
    /// Its own pull request was seen merged.
    Merged {
        url: String,
        number: Option<i64>,
        merge_commit_sha: Option<String>,
    },
    /// Its branch was seen merged into the base branch by git, without a pull request.
    MergedLocally,
    /// Its work shipped with the merged pull request of another task, or one no task has.
    MergedWith(MergedWith),
    /// It was concluded without a merge; `closed_pull_request` was closed with it.
    NotAdopted {
        note: String,
        closed_pull_request: Option<String>,
    },
    /// Its criteria were met with neither a merge nor a ready pull request seen, e.g. by
    /// manual criteria only.
    Completed,
}

/// The longest part of a conclusion quoted in its report, in characters.
const NOTE_CHARS: usize = 200;

impl Finish {
    pub fn kind(&self) -> FinishKind {
        match self {
            Self::PullRequestReady { .. } => FinishKind::PullRequestReady,
            Self::Merged { .. } | Self::MergedLocally | Self::MergedWith(_) => FinishKind::Merged,
            Self::NotAdopted { .. } => FinishKind::NotAdopted,
            Self::Completed => FinishKind::Completed,
        }
    }

    /// The pull request that showed the finish.
    pub fn pull_request_url(&self) -> Option<&str> {
        match self {
            Self::PullRequestReady { url, .. } | Self::Merged { url, .. } => Some(url),
            Self::MergedWith(merged) => Some(&merged.pull_request_url),
            Self::NotAdopted {
                closed_pull_request,
                ..
            } => closed_pull_request.as_deref(),
            Self::MergedLocally | Self::Completed => None,
        }
    }

    pub fn pull_request_number(&self) -> Option<i64> {
        match self {
            Self::Merged {
                number: Some(number),
                ..
            } => Some(*number),
            _ => self.pull_request_url().and_then(pull_request_number),
        }
    }

    /// The commit that showed the finish: the merge commit of a merged pull request, or
    /// the head of a ready one.
    pub fn commit_sha(&self) -> Option<&str> {
        match self {
            Self::PullRequestReady { head_sha, .. } => head_sha.as_deref(),
            Self::Merged {
                merge_commit_sha, ..
            } => merge_commit_sha.as_deref(),
            _ => None,
        }
    }

    /// The Markdown report of task `task`, titled `title`, finishing like this.
    pub fn report(&self, task: TaskId, title: &str) -> String {
        let named = format!("#{} {}", task.0, title.trim());
        let link = |url: &str| match pull_request_number(url) {
            Some(number) => format!("[pull request #{number}]({url})"),
            None => format!("[pull request]({url})"),
        };
        let short = |sha: &str| format!("`{}`", sha.chars().take(7).collect::<String>());
        match self {
            Self::PullRequestReady { url, head_sha } => {
                let at = head_sha
                    .as_deref()
                    .map_or_else(String::new, |sha| format!(" at {}", short(sha)));
                format!(
                    "**Pull request ready:** {named}. Its {} is open with passing checks{at} \
                     and is not merged yet.",
                    link(url)
                )
            }
            Self::Merged {
                url,
                merge_commit_sha,
                ..
            } => {
                let as_commit = merge_commit_sha
                    .as_deref()
                    .map_or_else(String::new, |sha| format!(" as {}", short(sha)));
                format!(
                    "**Merged:** {named}. Its {} was merged{as_commit}.",
                    link(url)
                )
            }
            Self::MergedLocally => {
                format!("**Merged:** {named}. Its branch was merged into the base branch.")
            }
            Self::MergedWith(merged) => {
                let of = merged
                    .task_id
                    .map_or_else(String::new, |other| format!(" of #{}", other.0));
                format!(
                    "**Merged:** {named} shipped with the merged {}{of}.",
                    link(&merged.pull_request_url)
                )
            }
            Self::NotAdopted {
                note,
                closed_pull_request,
            } => {
                let reason = note.lines().map(str::trim).find(|line| !line.is_empty());
                let reason = reason.map_or_else(String::new, |line| {
                    let clipped: String = line.chars().take(NOTE_CHARS).collect();
                    let more = if clipped.chars().count() < line.chars().count() {
                        "..."
                    } else {
                        ""
                    };
                    format!("\n\n> {clipped}{more}")
                });
                let closed = closed_pull_request
                    .as_deref()
                    .map_or_else(String::new, |url| format!(" Its {} was closed.", link(url)));
                format!("**Not adopted:** {named} was concluded without a merge.{closed}{reason}")
            }
            Self::Completed => format!(
                "**Completed:** {named}. Its completion criteria are met; no merge was seen."
            ),
        }
    }
}

/// The number of a GitHub pull request from its URL, `.../pull/<number>`.
pub fn pull_request_number(url: &str) -> Option<i64> {
    let (_, number) = url.trim_end_matches('/').rsplit_once("/pull/")?;
    number.parse().ok().filter(|number| *number > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PR: &str = "https://github.com/acme/toodo/pull/34";

    #[test]
    fn only_a_merge_that_was_seen_is_reported_as_merged() {
        let ready = Finish::PullRequestReady {
            url: PR.to_owned(),
            head_sha: Some("0123456789abcdef".to_owned()),
        };
        assert_eq!(ready.kind(), FinishKind::PullRequestReady);
        assert_eq!(ready.pull_request_number(), Some(34));
        assert_eq!(ready.commit_sha(), Some("0123456789abcdef"));
        assert_eq!(
            ready.report(TaskId(12), "Add login"),
            format!(
                "**Pull request ready:** #12 Add login. Its [pull request #34]({PR}) is open \
                 with passing checks at `0123456` and is not merged yet."
            )
        );

        let merged = Finish::Merged {
            url: PR.to_owned(),
            number: Some(34),
            merge_commit_sha: Some("fedcba9876".to_owned()),
        };
        assert_eq!(merged.kind(), FinishKind::Merged);
        assert_eq!(
            merged.report(TaskId(12), "Add login"),
            format!(
                "**Merged:** #12 Add login. Its [pull request #34]({PR}) was merged as `fedcba9`."
            )
        );

        let shipped = Finish::MergedWith(MergedWith {
            task_id: Some(TaskId(5)),
            pull_request_url: PR.to_owned(),
        });
        assert_eq!(
            (shipped.kind(), shipped.commit_sha()),
            (FinishKind::Merged, None)
        );
        assert_eq!(
            shipped.report(TaskId(12), "Add login"),
            format!(
                "**Merged:** #12 Add login shipped with the merged [pull request #34]({PR}) of #5."
            )
        );
        assert_eq!(Finish::MergedLocally.kind(), FinishKind::Merged);
        assert_eq!(Finish::Completed.kind(), FinishKind::Completed);
        assert_eq!(Finish::Completed.pull_request_number(), None);
    }

    #[test]
    fn a_conclusion_quotes_the_first_line_of_its_note() {
        let concluded = Finish::NotAdopted {
            note: format!("\n  {}\nmore", "x".repeat(NOTE_CHARS + 1)),
            closed_pull_request: Some(PR.to_owned()),
        };
        assert_eq!(concluded.kind(), FinishKind::NotAdopted);
        assert_eq!(concluded.pull_request_number(), Some(34));
        assert_eq!(
            concluded.report(TaskId(3), "Spike"),
            format!(
                "**Not adopted:** #3 Spike was concluded without a merge. Its [pull request \
                 #34]({PR}) was closed.\n\n> {}...",
                "x".repeat(NOTE_CHARS)
            )
        );
    }

    #[test]
    fn pull_request_numbers_come_from_github_urls() {
        assert_eq!(pull_request_number(PR), Some(34));
        assert_eq!(pull_request_number(&format!("{PR}/")), Some(34));
        for url in [
            "https://github.com/acme/toodo",
            "https://x/pull/abc",
            "https://x/pull/0",
        ] {
            assert_eq!(pull_request_number(url), None, "{url}");
        }
    }
}
