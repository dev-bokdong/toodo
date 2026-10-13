//! Rules for the task dependency DAG of one workspace: edge validation (no self, cross-
//! workspace, or cyclic edges), readiness, and topological order.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

use super::model::{Task, TaskId};
use super::status::TaskStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyError {
    UnknownTask(TaskId),
    SelfDependency(TaskId),
    CrossWorkspace {
        task: TaskId,
        depends_on: TaskId,
    },
    /// The tasks along a dependency loop, starting and ending with the same task. Each
    /// task depends on the one after it.
    Cycle(Vec<TaskId>),
}

impl fmt::Display for DependencyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTask(id) => write!(f, "unknown task {id}"),
            Self::SelfDependency(id) => write!(f, "task {id} cannot depend on itself"),
            Self::CrossWorkspace { task, depends_on } => write!(
                f,
                "task {task} cannot depend on task {depends_on} from another workspace"
            ),
            Self::Cycle(path) => {
                let path: Vec<String> = path.iter().map(TaskId::to_string).collect();
                write!(f, "dependency cycle: {}", path.join(" -> "))
            }
        }
    }
}

impl std::error::Error for DependencyError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusChange {
    pub task: TaskId,
    pub from: TaskStatus,
    pub to: TaskStatus,
}

fn index(tasks: &[Task]) -> HashMap<TaskId, &Task> {
    tasks.iter().map(|task| (task.id, task)).collect()
}

pub fn validate_dependency(
    tasks: &[Task],
    task: TaskId,
    depends_on: TaskId,
) -> Result<(), DependencyError> {
    if task == depends_on {
        return Err(DependencyError::SelfDependency(task));
    }
    let by_id = index(tasks);
    let dependent = by_id.get(&task).ok_or(DependencyError::UnknownTask(task))?;
    let dependency = by_id
        .get(&depends_on)
        .ok_or(DependencyError::UnknownTask(depends_on))?;
    if dependent.workspace_id != dependency.workspace_id {
        return Err(DependencyError::CrossWorkspace { task, depends_on });
    }
    if let Some(path) = dependency_path(&by_id, depends_on, task) {
        let mut cycle = vec![task];
        cycle.extend(path);
        return Err(DependencyError::Cycle(cycle));
    }
    Ok(())
}

/// Path from `from` to `to` following `depends_on` edges, both ends included.
fn dependency_path(
    by_id: &HashMap<TaskId, &Task>,
    from: TaskId,
    to: TaskId,
) -> Option<Vec<TaskId>> {
    let mut parent: HashMap<TaskId, TaskId> = HashMap::new();
    let mut visited = HashSet::from([from]);
    let mut stack = vec![from];
    while let Some(current) = stack.pop() {
        if current == to {
            let mut path = vec![to];
            let mut node = to;
            while let Some(&previous) = parent.get(&node) {
                path.push(previous);
                node = previous;
            }
            path.reverse();
            return Some(path);
        }
        let Some(task) = by_id.get(&current) else {
            continue;
        };
        for &next in &task.depends_on {
            if visited.insert(next) {
                parent.insert(next, current);
                stack.push(next);
            }
        }
    }
    None
}

fn dependencies_done(by_id: &HashMap<TaskId, &Task>, task: &Task) -> Result<bool, DependencyError> {
    let mut all_done = true;
    for dependency in &task.depends_on {
        let dependency = by_id
            .get(dependency)
            .ok_or(DependencyError::UnknownTask(*dependency))?;
        all_done &= dependency.status == TaskStatus::Done;
    }
    Ok(all_done)
}

pub fn is_ready(tasks: &[Task], task: TaskId) -> Result<bool, DependencyError> {
    let by_id = index(tasks);
    let task = by_id.get(&task).ok_or(DependencyError::UnknownTask(task))?;
    dependencies_done(&by_id, task)
}

/// Status changes that bring `planned` and `ready` tasks in line with their
/// dependencies: `planned -> ready` once all dependencies are done, and
/// `ready -> planned` when a dependency is no longer done or a new one was added.
pub fn readiness_changes(tasks: &[Task]) -> Result<Vec<StatusChange>, DependencyError> {
    let by_id = index(tasks);
    let mut changes = Vec::new();
    for task in tasks {
        let to = match (task.status, dependencies_done(&by_id, task)?) {
            (TaskStatus::Planned, true) => TaskStatus::Ready,
            (TaskStatus::Ready, false) => TaskStatus::Planned,
            _ => continue,
        };
        changes.push(StatusChange {
            task: task.id,
            from: task.status,
            to,
        });
    }
    Ok(changes)
}

/// Orders tasks so that every task comes after its dependencies. Ties are broken by
/// task ID, so the order is deterministic.
pub fn topological_order(tasks: &[Task]) -> Result<Vec<TaskId>, DependencyError> {
    let by_id = index(tasks);
    let mut pending: BTreeMap<TaskId, usize> = BTreeMap::new();
    let mut dependents: HashMap<TaskId, Vec<TaskId>> = HashMap::new();
    for task in tasks {
        for dependency in &task.depends_on {
            if !by_id.contains_key(dependency) {
                return Err(DependencyError::UnknownTask(*dependency));
            }
            dependents.entry(*dependency).or_default().push(task.id);
        }
        pending.insert(task.id, task.depends_on.len());
    }

    let mut available: BTreeSet<TaskId> = pending
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut order = Vec::with_capacity(tasks.len());
    while let Some(id) = available.pop_first() {
        pending.remove(&id);
        order.push(id);
        for dependent in dependents.get(&id).into_iter().flatten() {
            let count = pending
                .get_mut(dependent)
                .expect("dependent tasks are pending until all dependencies are ordered");
            *count -= 1;
            if *count == 0 {
                available.insert(*dependent);
            }
        }
    }

    if pending.is_empty() {
        Ok(order)
    } else {
        Err(DependencyError::Cycle(find_cycle(&by_id, &pending)))
    }
}

/// Walks unordered tasks along unordered dependencies until a task repeats. Every
/// unordered task has at least one unordered dependency, so the walk always loops.
fn find_cycle(by_id: &HashMap<TaskId, &Task>, unordered: &BTreeMap<TaskId, usize>) -> Vec<TaskId> {
    let mut position: HashMap<TaskId, usize> = HashMap::new();
    let mut walk = Vec::new();
    let mut current = *unordered
        .keys()
        .next()
        .expect("find_cycle is only called with unordered tasks");
    loop {
        if let Some(&start) = position.get(&current) {
            let mut cycle = walk[start..].to_vec();
            cycle.push(current);
            return cycle;
        }
        position.insert(current, walk.len());
        walk.push(current);
        current = *by_id[&current]
            .depends_on
            .iter()
            .find(|dependency| unordered.contains_key(dependency))
            .expect("an unordered task has an unordered dependency");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{TaskType, WorkspaceId};

    fn task(id: i64, status: TaskStatus, depends_on: &[i64]) -> Task {
        Task {
            id: TaskId(id),
            workspace_id: WorkspaceId(1),
            task_type: TaskType::Feat,
            title: format!("Task {id}"),
            description: String::new(),
            status,
            depends_on: depends_on.iter().copied().map(TaskId).collect(),
            branch: None,
            worktree_path: None,
            herdr_workspace_id: None,
            pull_request_url: None,
            pull_request_merged_at: None,
            timeline: Vec::new(),
        }
    }

    fn ids(raw: &[i64]) -> Vec<TaskId> {
        raw.iter().copied().map(TaskId).collect()
    }

    use TaskStatus::*;

    #[test]
    fn valid_dependency_is_accepted() {
        let tasks = [
            task(1, Done, &[]),
            task(2, Planned, &[1]),
            task(3, Planned, &[]),
        ];
        assert_eq!(validate_dependency(&tasks, TaskId(3), TaskId(2)), Ok(()));
        assert_eq!(validate_dependency(&tasks, TaskId(3), TaskId(1)), Ok(()));
    }

    #[test]
    fn self_dependency_is_rejected() {
        let tasks = [task(1, Planned, &[])];
        assert_eq!(
            validate_dependency(&tasks, TaskId(1), TaskId(1)),
            Err(DependencyError::SelfDependency(TaskId(1)))
        );
    }

    #[test]
    fn direct_cycle_is_rejected() {
        let tasks = [task(1, Planned, &[2]), task(2, Planned, &[])];
        assert_eq!(
            validate_dependency(&tasks, TaskId(2), TaskId(1)),
            Err(DependencyError::Cycle(ids(&[2, 1, 2])))
        );
    }

    #[test]
    fn indirect_cycle_is_rejected_with_its_path() {
        let tasks = [
            task(1, Planned, &[2]),
            task(2, Planned, &[3]),
            task(3, Planned, &[]),
            task(4, Planned, &[1]),
        ];
        let error = validate_dependency(&tasks, TaskId(3), TaskId(1)).unwrap_err();
        assert_eq!(error, DependencyError::Cycle(ids(&[3, 1, 2, 3])));
        assert_eq!(error.to_string(), "dependency cycle: 3 -> 1 -> 2 -> 3");
    }

    #[test]
    fn cross_workspace_and_unknown_dependencies_are_rejected() {
        let mut other = task(2, Done, &[]);
        other.workspace_id = WorkspaceId(2);
        let tasks = [task(1, Planned, &[]), other];
        assert_eq!(
            validate_dependency(&tasks, TaskId(1), TaskId(2)),
            Err(DependencyError::CrossWorkspace {
                task: TaskId(1),
                depends_on: TaskId(2)
            })
        );
        assert_eq!(
            validate_dependency(&tasks, TaskId(1), TaskId(9)),
            Err(DependencyError::UnknownTask(TaskId(9)))
        );
    }

    #[test]
    fn task_is_ready_only_when_all_dependencies_are_done() {
        let tasks = [
            task(1, Done, &[]),
            task(2, InProgress, &[]),
            task(3, Cancelled, &[]),
            task(4, Planned, &[1]),
            task(5, Planned, &[1, 2]),
            task(6, Planned, &[3]),
            task(7, Planned, &[]),
        ];
        let ready: Vec<bool> = (4..=7)
            .map(|id| is_ready(&tasks, TaskId(id)).unwrap())
            .collect();
        assert_eq!(ready, [true, false, false, true]);
    }

    #[test]
    fn readiness_changes_promote_and_demote_tasks() {
        let tasks = [
            task(1, Done, &[]),
            task(2, InProgress, &[]),
            task(3, Planned, &[1]),
            task(4, Ready, &[2]),
            task(5, Ready, &[1]),
            task(6, InProgress, &[2]),
        ];
        assert_eq!(
            readiness_changes(&tasks).unwrap(),
            [
                StatusChange {
                    task: TaskId(3),
                    from: Planned,
                    to: Ready
                },
                StatusChange {
                    task: TaskId(4),
                    from: Ready,
                    to: Planned
                },
            ]
        );
        for change in readiness_changes(&tasks).unwrap() {
            assert!(change.from.can_transition_to(change.to));
        }
    }

    #[test]
    fn topological_order_puts_dependencies_first_and_breaks_ties_by_id() {
        let tasks = [
            task(5, Planned, &[3, 4]),
            task(4, Planned, &[1]),
            task(3, Planned, &[1]),
            task(2, Planned, &[]),
            task(1, Planned, &[]),
        ];
        assert_eq!(topological_order(&tasks).unwrap(), ids(&[1, 2, 3, 4, 5]));
        assert_eq!(topological_order(&[]).unwrap(), ids(&[]));
    }

    #[test]
    fn topological_order_reports_a_stored_cycle() {
        let tasks = [
            task(1, Planned, &[]),
            task(2, Planned, &[1, 4]),
            task(3, Planned, &[2]),
            task(4, Planned, &[3]),
            task(5, Planned, &[4]),
        ];
        assert_eq!(
            topological_order(&tasks),
            Err(DependencyError::Cycle(ids(&[2, 4, 3, 2])))
        );
        assert_eq!(
            topological_order(&[task(1, Planned, &[7])]),
            Err(DependencyError::UnknownTask(TaskId(7)))
        );
    }
}
