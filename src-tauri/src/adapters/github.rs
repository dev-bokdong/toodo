//! Opens GitHub pull requests and reads their state with the `gh` CLI, reusing its authentication. `gh` always
//! runs on this machine and names the repository with `--repo`, so it does not need a
//! local checkout and works for workspaces on SSH machines too.
//!
//! Processes are started through the [`Runner`] of the git adapter, so tests can fake
//! them the same way.

use serde_json::Value;

use super::git::{GitError, Invocation, Runner, describe};
use crate::domain::remote::GitHubRepository;

/// A pull request to open from `head` into `base`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewPullRequest<'a> {
    pub repository: &'a GitHubRepository,
    pub head: &'a str,
    pub base: &'a str,
    pub title: &'a str,
    pub body: &'a str,
}

/// Lifecycle state of a pull request as reported by `gh`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestState {
    Open,
    Closed,
    Merged,
}

/// A pull request as reported by `gh`, whoever opened it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub url: String,
    pub state: PullRequestState,
    /// Branch the pull request merges into.
    pub base: String,
    /// Merge time in seconds since the Unix epoch; `None` until it is merged.
    pub merged_at: Option<i64>,
}

/// Fields requested with `--json` for a [`PullRequest`].
const PULL_REQUEST_FIELDS: &str = "url,state,baseRefName,mergedAt";

fn parse_pull_request(value: &Value) -> Option<PullRequest> {
    let field = |name: &str| value.get(name).and_then(Value::as_str);
    let state = match field("state")? {
        "OPEN" => PullRequestState::Open,
        "CLOSED" => PullRequestState::Closed,
        "MERGED" => PullRequestState::Merged,
        _ => return None,
    };
    // `gh` reports `"mergedAt": null` for a pull request that is not merged.
    let merged_at = match value.get("mergedAt")? {
        Value::Null => None,
        Value::String(raw) => Some(parse_utc_timestamp(raw)?),
        _ => return None,
    };
    Some(PullRequest {
        url: field("url")?.to_owned(),
        state,
        base: field("baseRefName")?.to_owned(),
        merged_at,
    })
}

/// Seconds since the Unix epoch of a UTC timestamp as printed by `gh`, e.g.
/// `2026-10-09T14:47:45Z`.
fn parse_utc_timestamp(raw: &str) -> Option<i64> {
    let bytes = raw.as_bytes();
    let separators_match = bytes.len() == 20
        && [
            (4, b'-'),
            (7, b'-'),
            (10, b'T'),
            (13, b':'),
            (16, b':'),
            (19, b'Z'),
        ]
        .iter()
        .all(|&(index, separator)| bytes[index] == separator);
    if !separators_match {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let digits = &raw[range];
        digits
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| digits.parse().ok())
            .flatten()
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => return None,
    };
    if !(1..=days_in_month).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Days from 1970-01-01 to the given proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil` algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub struct GitHub {
    runner: Box<dyn Runner>,
}

impl GitHub {
    pub fn new(runner: Box<dyn Runner>) -> Self {
        Self { runner }
    }

    /// The most recent pull request whose head is `head`, in any state, if there is one.
    /// It may have been opened outside Toodo, e.g. by an agent in herdr.
    pub fn find_pull_request(
        &self,
        repository: &GitHubRepository,
        head: &str,
    ) -> Result<Option<PullRequest>, GitError> {
        let repository = repository.to_string();
        let invocation = invocation(&[
            "pr",
            "list",
            "--repo",
            &repository,
            "--head",
            head,
            "--state",
            "all",
            "--json",
            PULL_REQUEST_FIELDS,
            "--limit",
            "1",
        ]);
        let output = self.runner.run(&invocation)?;
        let unexpected = || unexpected(&invocation, &output);
        let pull_requests: Value = serde_json::from_str(&output).map_err(|_| unexpected())?;
        let pull_requests = pull_requests.as_array().ok_or_else(unexpected)?;
        pull_requests
            .first()
            .map(|pull_request| parse_pull_request(pull_request).ok_or_else(unexpected))
            .transpose()
    }

    /// Opens a pull request and returns its URL.
    pub fn create_pull_request(
        &self,
        pull_request: &NewPullRequest<'_>,
    ) -> Result<String, GitError> {
        let repository = pull_request.repository.to_string();
        let invocation = invocation(&[
            "pr",
            "create",
            "--repo",
            &repository,
            "--head",
            pull_request.head,
            "--base",
            pull_request.base,
            "--title",
            pull_request.title,
            "--body",
            pull_request.body,
        ]);
        let output = self.runner.run(&invocation)?;
        output
            .lines()
            .map(str::trim)
            .rfind(|line| line.starts_with("https://") || line.starts_with("http://"))
            .map(str::to_owned)
            .ok_or_else(|| unexpected(&invocation, &output))
    }

    /// The pull request at `url`.
    pub fn pull_request(&self, url: &str) -> Result<PullRequest, GitError> {
        let invocation = invocation(&["pr", "view", url, "--json", PULL_REQUEST_FIELDS]);
        let output = self.runner.run(&invocation)?;
        serde_json::from_str::<Value>(&output)
            .ok()
            .as_ref()
            .and_then(parse_pull_request)
            .ok_or_else(|| unexpected(&invocation, &output))
    }
}

fn invocation(args: &[&str]) -> Invocation {
    Invocation {
        program: "gh",
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
    }
}

fn unexpected(invocation: &Invocation, output: &str) -> GitError {
    GitError::UnexpectedOutput {
        command: describe(invocation),
        output: output.trim().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::adapters::git::fake::FakeRunner;

    fn github(results: Vec<Result<String, GitError>>) -> (GitHub, Arc<FakeRunner>) {
        let runner = Arc::new(FakeRunner::new(results));
        (GitHub::new(Box::new(runner.clone())), runner)
    }

    fn repository() -> GitHubRepository {
        GitHubRepository::from_remote_url("git@github.com:acme/toodo.git").unwrap()
    }

    const URL: &str = "https://github.com/acme/toodo/pull/7";

    const MERGED_AT: &str = "2026-10-09T14:47:45Z";
    const MERGED_AT_SECONDS: i64 = 1_791_557_265;

    fn pull_request(state: PullRequestState, base: &str) -> PullRequest {
        PullRequest {
            url: URL.to_owned(),
            state,
            base: base.to_owned(),
            merged_at: (state == PullRequestState::Merged).then_some(MERGED_AT_SECONDS),
        }
    }

    #[test]
    fn utc_timestamps_are_converted_to_unix_seconds() {
        assert_eq!(parse_utc_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc_timestamp(MERGED_AT), Some(MERGED_AT_SECONDS));
        assert_eq!(
            parse_utc_timestamp("2024-02-29T23:59:59Z"),
            Some(1_709_251_199)
        );
        for malformed in [
            "",
            "2026-10-09",
            "2026-10-09 14:47:45Z",
            "2026-10-09T14:47:45+09:00",
            "2026-13-09T14:47:45Z",
            "2023-02-29T00:00:00Z",
            "2026-10-09T24:00:00Z",
            "2026-1a-09T14:47:45Z",
            "+026-10-09T14:47:45Z",
        ] {
            assert_eq!(parse_utc_timestamp(malformed), None, "{malformed}");
        }
    }

    #[test]
    fn find_pull_request_reads_the_first_pull_request_in_any_state() {
        let (github, runner) = github(vec![
            Ok(format!(
                r#"[{{"baseRefName":"main","mergedAt":"{MERGED_AT}","state":"MERGED","url":"{URL}"}}]"#
            )),
            Ok("[]\n".to_owned()),
        ]);
        assert_eq!(
            github.find_pull_request(&repository(), "feat/x-1").unwrap(),
            Some(pull_request(PullRequestState::Merged, "main"))
        );
        assert_eq!(
            github.find_pull_request(&repository(), "feat/x-1").unwrap(),
            None
        );
        assert_eq!(
            runner.invocations.lock().unwrap()[0],
            invocation(&[
                "pr",
                "list",
                "--repo",
                "github.com/acme/toodo",
                "--head",
                "feat/x-1",
                "--state",
                "all",
                "--json",
                "url,state,baseRefName,mergedAt",
                "--limit",
                "1",
            ])
        );
    }

    #[test]
    fn find_pull_request_rejects_unexpected_output() {
        let (github, _) = github(vec![
            Ok("no pull requests\n".to_owned()),
            Ok(format!(r#"[{{"url":"{URL}","state":"OPEN"}}]"#)),
            Ok(format!(
                r#"[{{"baseRefName":"main","mergedAt":"yesterday","state":"MERGED","url":"{URL}"}}]"#
            )),
        ]);
        for _ in 0..3 {
            assert!(matches!(
                github.find_pull_request(&repository(), "feat/x-1"),
                Err(GitError::UnexpectedOutput { .. })
            ));
        }
    }

    #[test]
    fn pull_request_reads_state_base_and_merge_time() {
        let view = |state: &str| {
            let merged_at = if state == "MERGED" {
                format!("\"{MERGED_AT}\"")
            } else {
                "null".to_owned()
            };
            Ok(format!(
                r#"{{"baseRefName":"develop","mergedAt":{merged_at},"state":"{state}","url":"{URL}"}}"#
            ))
        };
        let (github, runner) = github(vec![
            view("MERGED"),
            view("OPEN"),
            view("CLOSED"),
            view("DRAFT"),
        ]);
        for state in [
            PullRequestState::Merged,
            PullRequestState::Open,
            PullRequestState::Closed,
        ] {
            assert_eq!(
                github.pull_request(URL).unwrap(),
                pull_request(state, "develop")
            );
        }
        assert!(matches!(
            github.pull_request(URL),
            Err(GitError::UnexpectedOutput { .. })
        ));
        assert_eq!(
            runner.invocations.lock().unwrap()[0],
            invocation(&[
                "pr",
                "view",
                URL,
                "--json",
                "url,state,baseRefName,mergedAt"
            ])
        );
    }

    #[test]
    fn create_pull_request_returns_the_printed_url() {
        let (github, runner) = github(vec![
            Ok("\nhttps://github.com/acme/toodo/pull/8\n".to_owned()),
            Ok("Creating pull request\n".to_owned()),
        ]);
        let repository = repository();
        let new = NewPullRequest {
            repository: &repository,
            head: "feat/x-1",
            base: "main",
            title: "Add x",
            body: "",
        };
        assert_eq!(
            github.create_pull_request(&new).unwrap(),
            "https://github.com/acme/toodo/pull/8"
        );
        assert_eq!(
            runner.invocations.lock().unwrap()[0].args,
            [
                "pr",
                "create",
                "--repo",
                "github.com/acme/toodo",
                "--head",
                "feat/x-1",
                "--base",
                "main",
                "--title",
                "Add x",
                "--body",
                "",
            ]
            .map(str::to_owned)
        );
        assert!(matches!(
            github.create_pull_request(&new),
            Err(GitError::UnexpectedOutput { .. })
        ));
    }
}
