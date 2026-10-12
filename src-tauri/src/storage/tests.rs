use super::*;
use crate::domain::dag::StatusChange;
use crate::domain::model::{
    CommitRecord, HostPath, Machine, MachineLabel, MachineSettings, Task, TaskType, Workspace,
    WorkspaceLocation,
};
use crate::domain::path::{PosixPath, WindowsPath};
use TaskStatus::*;

fn local_location(path: &str) -> WorkspaceLocation {
    WorkspaceLocation::Local {
        path: WindowsPath::parse(path).unwrap(),
    }
}

fn ssh_location(machine: &str, path: &str) -> WorkspaceLocation {
    WorkspaceLocation::Ssh {
        machine: MachineLabel::parse(machine).unwrap(),
        path: PosixPath::parse(path).unwrap(),
    }
}

fn add_workspace(storage: &mut Storage, name: &str, location: WorkspaceLocation) -> Workspace {
    storage
        .create_workspace(&NewWorkspace {
            name: name.to_owned(),
            location,
            default_base_branch: "main".to_owned(),
        })
        .unwrap()
}

fn add_task(storage: &mut Storage, workspace: &Workspace, title: &str) -> Task {
    storage
        .create_task(&NewTask {
            workspace_id: workspace.id,
            task_type: TaskType::Feat,
            title: title.to_owned(),
            description: format!("Implement {title}"),
        })
        .unwrap()
}

fn status(storage: &Storage, task: &Task) -> TaskStatus {
    storage.get_task(task.id).unwrap().unwrap().status
}

fn commit(task: TaskId, sha: &str, committed_at: i64) -> CommitRecord {
    CommitRecord {
        task_id: task,
        sha: sha.to_owned(),
        message: format!("Commit {sha}"),
        author: "Dev <dev@example.com>".to_owned(),
        committed_at,
    }
}

fn row_count(storage: &Storage, table: &str) -> i64 {
    storage
        .conn
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[test]
fn entities_survive_reopening_the_database_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("toodo.sqlite3");

    let mut storage = Storage::open(&path).unwrap();
    let local = add_workspace(&mut storage, "Toodo", local_location(r"C:\src\toodo"));
    let remote = add_workspace(
        &mut storage,
        "Server",
        ssh_location("devbox", "/home/me/server"),
    );
    let local_settings = MachineSettings::Local {
        worktree_root: Some(WindowsPath::parse(r"D:\worktrees").unwrap()),
    };
    let ssh_settings = MachineSettings::Ssh {
        machine: MachineLabel::parse("devbox").unwrap(),
        worktree_root: Some(PosixPath::parse("/srv/wt").unwrap()),
    };
    storage.save_machine_settings(&local_settings).unwrap();
    storage.save_machine_settings(&ssh_settings).unwrap();

    let base = add_task(&mut storage, &local, "Base");
    let mut started = add_task(&mut storage, &local, "Login page");
    let remote_task = add_task(&mut storage, &remote, "Remote fix");
    storage.add_dependency(started.id, base.id).unwrap();
    storage.set_task_status(base.id, InProgress).unwrap();
    storage.set_task_status(base.id, Done).unwrap();
    storage.set_task_status(started.id, InProgress).unwrap();
    started.task_type = TaskType::Fix;
    started.branch = Some(format!("fix/login-page-{}", started.id));
    started.worktree_path = Some(HostPath::Windows(
        WindowsPath::parse(r"D:\worktrees\toodo\fix-login-page-2").unwrap(),
    ));
    started.herdr_workspace_id = Some("w-123".to_owned());
    started.pull_request_url = Some("https://github.com/me/toodo/pull/1".to_owned());
    storage.update_task(&started).unwrap();
    let commits = vec![commit(started.id, "bbb", 20), commit(started.id, "aaa", 10)];
    storage
        .replace_commit_records(started.id, &commits)
        .unwrap();

    let workspaces = storage.list_workspaces().unwrap();
    let local_tasks = storage.list_tasks(local.id).unwrap();
    let remote_tasks = storage.list_tasks(remote.id).unwrap();
    drop(storage);

    let storage = Storage::open(&path).unwrap();
    assert_eq!(
        migrations::schema_version(&storage.conn).unwrap(),
        migrations::LATEST_VERSION
    );
    assert_eq!(storage.list_workspaces().unwrap(), workspaces);
    assert_eq!(workspaces, [local.clone(), remote.clone()]);
    assert_eq!(
        storage.get_workspace(remote.id).unwrap(),
        Some(remote.clone())
    );
    assert_eq!(
        storage.get_machine_settings(&Machine::Local).unwrap(),
        local_settings
    );
    assert_eq!(
        storage
            .get_machine_settings(&ssh_settings.machine())
            .unwrap(),
        ssh_settings
    );

    assert_eq!(storage.list_tasks(local.id).unwrap(), local_tasks);
    assert_eq!(storage.list_tasks(remote.id).unwrap(), remote_tasks);
    let mut expected_started = started.clone();
    expected_started.status = InProgress;
    expected_started.depends_on = [base.id].into();
    assert_eq!(
        storage.get_task(started.id).unwrap(),
        Some(expected_started)
    );
    assert_eq!(status(&storage, &base), Done);
    assert_eq!(remote_tasks, [remote_task]);
    assert_eq!(storage.list_commit_records(started.id).unwrap(), commits);
}

#[test]
fn machines_without_saved_settings_have_no_worktree_root() {
    let mut storage = Storage::open_in_memory().unwrap();
    assert_eq!(
        storage.get_machine_settings(&Machine::Local).unwrap(),
        MachineSettings::Local {
            worktree_root: None
        }
    );
    let label = MachineLabel::parse("devbox").unwrap();
    let cleared = MachineSettings::Ssh {
        machine: label.clone(),
        worktree_root: None,
    };
    storage
        .save_machine_settings(&MachineSettings::Ssh {
            machine: label.clone(),
            worktree_root: Some(PosixPath::parse("/wt").unwrap()),
        })
        .unwrap();
    storage.save_machine_settings(&cleared).unwrap();
    assert_eq!(
        storage.get_machine_settings(&Machine::Ssh(label)).unwrap(),
        cleared
    );
    assert_eq!(row_count(&storage, "machine_settings"), 1);
}

#[test]
fn workspace_updates_and_cascading_deletes() {
    let mut storage = Storage::open_in_memory().unwrap();
    let mut workspace = add_workspace(&mut storage, "Toodo", local_location(r"C:\src\toodo"));
    let kept = add_workspace(&mut storage, "Kept", local_location(r"C:\src\kept"));
    let a = add_task(&mut storage, &workspace, "A");
    let b = add_task(&mut storage, &workspace, "B");
    let kept_task = add_task(&mut storage, &kept, "Kept");
    storage.add_dependency(b.id, a.id).unwrap();
    storage
        .replace_commit_records(a.id, &[commit(a.id, "aaa", 1)])
        .unwrap();

    workspace.name = "Renamed".to_owned();
    workspace.location = ssh_location("devbox", "/home/me/toodo");
    workspace.default_base_branch = "develop".to_owned();
    storage.update_workspace(&workspace).unwrap();
    assert_eq!(
        storage.get_workspace(workspace.id).unwrap(),
        Some(workspace.clone())
    );

    storage.delete_workspace(workspace.id).unwrap();
    assert_eq!(storage.get_workspace(workspace.id).unwrap(), None);
    assert_eq!(storage.get_task(a.id).unwrap(), None);
    assert_eq!(row_count(&storage, "task_dependencies"), 0);
    assert_eq!(row_count(&storage, "commit_records"), 0);
    assert_eq!(storage.list_tasks(kept.id).unwrap(), [kept_task]);

    assert!(matches!(
        storage.delete_workspace(workspace.id),
        Err(StorageError::WorkspaceNotFound(id)) if id == workspace.id
    ));
    assert!(matches!(
        storage.update_workspace(&workspace),
        Err(StorageError::WorkspaceNotFound(_))
    ));
    assert!(matches!(
        storage.create_task(&NewTask {
            workspace_id: workspace.id,
            task_type: TaskType::Docs,
            title: "Orphan".to_owned(),
            description: String::new(),
        }),
        Err(StorageError::WorkspaceNotFound(_))
    ));
}

#[test]
fn rejected_dependencies_write_nothing() {
    let mut storage = Storage::open_in_memory().unwrap();
    let workspace = add_workspace(&mut storage, "Toodo", local_location(r"C:\src\toodo"));
    let other = add_workspace(&mut storage, "Other", local_location(r"C:\src\other"));
    let a = add_task(&mut storage, &workspace, "A");
    let b = add_task(&mut storage, &workspace, "B");
    let c = add_task(&mut storage, &workspace, "C");
    let foreign = add_task(&mut storage, &other, "Foreign");
    storage.add_dependency(b.id, a.id).unwrap();
    storage.add_dependency(c.id, b.id).unwrap();
    let before = storage.list_tasks(workspace.id).unwrap();

    let rejected = [
        (
            a.id,
            c.id,
            DependencyError::Cycle(vec![a.id, c.id, b.id, a.id]),
        ),
        (a.id, a.id, DependencyError::SelfDependency(a.id)),
        (
            a.id,
            foreign.id,
            DependencyError::CrossWorkspace {
                task: a.id,
                depends_on: foreign.id,
            },
        ),
        (a.id, TaskId(999), DependencyError::UnknownTask(TaskId(999))),
        (TaskId(999), a.id, DependencyError::UnknownTask(TaskId(999))),
    ];
    for (task, depends_on, expected) in rejected {
        match storage.add_dependency(task, depends_on) {
            Err(StorageError::Dependency(error)) => assert_eq!(error, expected),
            other => panic!("{task} -> {depends_on}: expected {expected:?}, got {other:?}"),
        }
    }
    assert_eq!(storage.list_tasks(workspace.id).unwrap(), before);
    assert_eq!(row_count(&storage, "task_dependencies"), 2);
}

#[test]
fn dependency_changes_update_readiness() {
    let mut storage = Storage::open_in_memory().unwrap();
    let workspace = add_workspace(&mut storage, "Toodo", local_location(r"C:\src\toodo"));
    let a = add_task(&mut storage, &workspace, "A");
    let b = add_task(&mut storage, &workspace, "B");
    assert_eq!(status(&storage, &b), Ready);

    assert_eq!(
        storage.add_dependency(b.id, a.id).unwrap(),
        [StatusChange {
            task: b.id,
            from: Ready,
            to: Planned
        }]
    );
    assert_eq!(status(&storage, &b), Planned);
    assert_eq!(storage.add_dependency(b.id, a.id).unwrap(), []);

    assert_eq!(
        storage.remove_dependency(b.id, a.id).unwrap(),
        [StatusChange {
            task: b.id,
            from: Planned,
            to: Ready
        }]
    );
    assert_eq!(status(&storage, &b), Ready);
    assert!(
        storage
            .get_task(b.id)
            .unwrap()
            .unwrap()
            .depends_on
            .is_empty()
    );
}

#[test]
fn status_changes_follow_transitions_and_release_dependents() {
    let mut storage = Storage::open_in_memory().unwrap();
    let workspace = add_workspace(&mut storage, "Toodo", local_location(r"C:\src\toodo"));
    let a = add_task(&mut storage, &workspace, "A");
    let b = add_task(&mut storage, &workspace, "B");
    let c = add_task(&mut storage, &workspace, "C");
    storage.add_dependency(b.id, a.id).unwrap();
    storage.add_dependency(c.id, a.id).unwrap();
    storage.add_dependency(c.id, b.id).unwrap();

    assert!(matches!(
        storage.set_task_status(b.id, InProgress),
        Err(StorageError::Transition(error)) if error.from == Planned && error.to == InProgress
    ));
    assert!(matches!(
        storage.set_task_status(a.id, Ready),
        Err(StorageError::DerivedStatus(Ready))
    ));
    assert!(matches!(
        storage.set_task_status(TaskId(999), Done),
        Err(StorageError::TaskNotFound(TaskId(999)))
    ));
    assert_eq!(status(&storage, &b), Planned);

    assert_eq!(storage.set_task_status(a.id, InProgress).unwrap(), []);
    assert_eq!(
        storage.set_task_status(a.id, Done).unwrap(),
        [StatusChange {
            task: b.id,
            from: Planned,
            to: Ready
        }]
    );
    assert_eq!(status(&storage, &c), Planned);
    storage.set_task_status(b.id, InProgress).unwrap();
    assert_eq!(storage.set_task_status(b.id, Cancelled).unwrap(), []);
    assert_eq!(status(&storage, &c), Planned);
    assert!(matches!(
        storage.set_task_status(b.id, Done),
        Err(StorageError::Transition(_))
    ));
}

#[test]
fn deleting_a_task_releases_its_dependents() {
    let mut storage = Storage::open_in_memory().unwrap();
    let workspace = add_workspace(&mut storage, "Toodo", local_location(r"C:\src\toodo"));
    let a = add_task(&mut storage, &workspace, "A");
    let b = add_task(&mut storage, &workspace, "B");
    storage.add_dependency(b.id, a.id).unwrap();
    storage
        .replace_commit_records(a.id, &[commit(a.id, "aaa", 1)])
        .unwrap();

    assert_eq!(
        storage.delete_task(a.id).unwrap(),
        [StatusChange {
            task: b.id,
            from: Planned,
            to: Ready
        }]
    );
    assert_eq!(storage.get_task(a.id).unwrap(), None);
    assert!(
        storage
            .get_task(b.id)
            .unwrap()
            .unwrap()
            .depends_on
            .is_empty()
    );
    assert_eq!(row_count(&storage, "commit_records"), 0);
    assert!(matches!(
        storage.delete_task(a.id),
        Err(StorageError::TaskNotFound(_))
    ));
}

#[test]
fn task_ids_are_not_reused_after_deletion() {
    let mut storage = Storage::open_in_memory().unwrap();
    let workspace = add_workspace(&mut storage, "Toodo", local_location(r"C:\src\toodo"));
    let first = add_task(&mut storage, &workspace, "First");
    storage.delete_task(first.id).unwrap();
    let second = add_task(&mut storage, &workspace, "Second");
    assert!(second.id > first.id);
}

#[test]
fn worktree_path_must_match_the_workspace_machine() {
    let mut storage = Storage::open_in_memory().unwrap();
    let remote = add_workspace(&mut storage, "Server", ssh_location("devbox", "/srv/app"));
    let mut task = add_task(&mut storage, &remote, "Remote");
    task.worktree_path = Some(HostPath::Windows(WindowsPath::parse(r"C:\wt\x").unwrap()));
    assert!(matches!(
        storage.update_task(&task),
        Err(StorageError::WorktreePathMismatch(id)) if id == task.id
    ));
    task.worktree_path = Some(HostPath::Posix(PosixPath::parse("/wt/server/x").unwrap()));
    storage.update_task(&task).unwrap();
    assert_eq!(storage.get_task(task.id).unwrap(), Some(task.clone()));

    task.id = TaskId(999);
    assert!(matches!(
        storage.update_task(&task),
        Err(StorageError::TaskNotFound(_))
    ));
}

#[test]
fn commit_records_are_replaced_in_order() {
    let mut storage = Storage::open_in_memory().unwrap();
    let workspace = add_workspace(&mut storage, "Toodo", local_location(r"C:\src\toodo"));
    let a = add_task(&mut storage, &workspace, "A");
    let b = add_task(&mut storage, &workspace, "B");
    storage
        .replace_commit_records(a.id, &[commit(a.id, "old", 1)])
        .unwrap();
    let refreshed = vec![
        commit(a.id, "ccc", 30),
        commit(a.id, "aaa", 10),
        commit(a.id, "bbb", 20),
    ];
    storage.replace_commit_records(a.id, &refreshed).unwrap();
    assert_eq!(storage.list_commit_records(a.id).unwrap(), refreshed);
    assert_eq!(storage.list_commit_records(b.id).unwrap(), []);

    assert!(matches!(
        storage.replace_commit_records(b.id, &[commit(a.id, "ddd", 40)]),
        Err(StorageError::CommitTaskMismatch { expected, found }) if expected == b.id && found == a.id
    ));
    assert!(matches!(
        storage.replace_commit_records(TaskId(999), &[]),
        Err(StorageError::TaskNotFound(_))
    ));
    let duplicate = [commit(a.id, "eee", 1), commit(a.id, "eee", 2)];
    assert!(matches!(
        storage.replace_commit_records(a.id, &duplicate),
        Err(StorageError::Sqlite(_))
    ));
    assert_eq!(storage.list_commit_records(a.id).unwrap(), refreshed);
}

#[test]
fn corrupt_stored_values_are_reported_as_errors() {
    let mut storage = Storage::open_in_memory().unwrap();
    let workspace = add_workspace(&mut storage, "Toodo", local_location(r"C:\src\toodo"));
    let task = add_task(&mut storage, &workspace, "A");

    storage
        .conn
        .execute(
            "UPDATE tasks SET worktree_path = 'relative' WHERE id = ?1",
            [task.id.0],
        )
        .unwrap();
    assert!(matches!(
        storage.get_task(task.id),
        Err(StorageError::InvalidData(message)) if message.contains("relative")
    ));

    storage
        .conn
        .execute(
            "UPDATE workspaces SET path = '/not/windows' WHERE id = ?1",
            [workspace.id.0],
        )
        .unwrap();
    assert!(matches!(
        storage.list_workspaces(),
        Err(StorageError::InvalidData(_))
    ));

    storage
        .conn
        .execute(
            "INSERT INTO machine_settings (kind, label, worktree_root) VALUES ('local', '', 'x')",
            [],
        )
        .unwrap();
    assert!(matches!(
        storage.get_machine_settings(&Machine::Local),
        Err(StorageError::InvalidData(_))
    ));
}

#[test]
fn databases_from_a_newer_version_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("toodo.sqlite3");
    drop(Storage::open(&path).unwrap());
    rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", migrations::LATEST_VERSION + 1)
        .unwrap();
    assert!(matches!(
        Storage::open(&path),
        Err(StorageError::UnsupportedSchemaVersion { found, supported })
            if found == migrations::LATEST_VERSION + 1 && supported == migrations::LATEST_VERSION
    ));
}
