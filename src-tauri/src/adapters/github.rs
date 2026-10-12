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
}

/// Fields requested with `--json` for a [`PullRequest`].
const PULL_REQUEST_FIELDS: &str = "url,state,baseRefName";

fn parse_pull_request(value: &Value) -> Option<PullRequest> {
    let field = |name: &str| value.get(name).and_then(Value::as_str);
    let state = match field("state")? {
        "OPEN" => PullRequestState::Open,
        "CLOSED" => PullRequestState::Closed,
        "MERGED" => PullRequestState::Merged,
        _ => return None,
    };
    Some(PullRequest {
        url: field("url")?.to_owned(),
        state,
        base: field("baseRefName")?.to_owned(),
    })
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

    fn pull_request(state: PullRequestState, base: &str) -> PullRequest {
        PullRequest {
            url: URL.to_owned(),
            state,
            base: base.to_owned(),
        }
    }

    #[test]
    fn find_pull_request_reads_the_first_pull_request_in_any_state() {
        let (github, runner) = github(vec![
            Ok(format!(
                r#"[{{"baseRefName":"main","state":"MERGED","url":"{URL}"}}]"#
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
                "url,state,baseRefName",
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
        ]);
        for _ in 0..2 {
            assert!(matches!(
                github.find_pull_request(&repository(), "feat/x-1"),
                Err(GitError::UnexpectedOutput { .. })
            ));
        }
    }

    #[test]
    fn pull_request_reads_state_and_base() {
        let view = |state: &str| {
            Ok(format!(
                r#"{{"baseRefName":"develop","state":"{state}","url":"{URL}"}}"#
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
            invocation(&["pr", "view", URL, "--json", "url,state,baseRefName"])
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
