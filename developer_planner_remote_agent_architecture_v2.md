# Developer Planner Architecture v2
## Windows Client + Remote Agent + GitHub Integration

> 목표:  
> 개발자가 Planner에서 Task를 선택하면, 로컬 Windows/WSL/SSH Linux 중 원하는 실행 환경에 연결되고,  
> 해당 환경의 Git Repository/Branch/Terminal/Commit Context가 자동으로 준비되며,  
> GitHub App을 통해 Push/PR/Merge 상태까지 연결되는 개발자용 작업 컨텍스트 플래너를 만든다.

---

# 1. 제품 정의

이 제품은 단순한 Todo 앱이 아니다.

핵심은:

```text
계획
  ↓
Task 생성
  ↓
작업 환경 선택
  ↓
Repository Workspace 활성화
  ↓
Branch 생성 / 전환
  ↓
개발
  ↓
Commit 추적
  ↓
Push
  ↓
PR
  ↓
Merge
  ↓
Task 완료
```

를 하나의 작업 흐름으로 만드는 것이다.

제품을 한 문장으로 정의하면:

> **Task를 선택하면 개발 환경의 Git Context와 실행 환경이 함께 전환되는 개발자용 Work Context Planner**

---

# 2. 핵심 설계 원칙

## 2.1 Project는 업무의 경계

```text
Project
= 제품 / 서비스 / 업무 단위
```

예:

```text
Project: 쇼핑몰 리뉴얼
```

---

## 2.2 Repository는 코드베이스의 경계

하나의 Project는 여러 Git Repository를 가질 수 있다.

```text
Project: 쇼핑몰 리뉴얼

Repositories
├─ shop-frontend
├─ shop-backend
├─ shop-admin
├─ shop-mobile
└─ shop-infra
```

따라서:

```text
Project != Repository
```

로 설계해야 한다.

---

## 2.3 Task는 실제 실행 단위

예:

```text
TASK-142
결제 API 오류 처리
```

Task는 하나 이상의 Repository를 사용할 수 있다.

```text
TASK-142
├─ shop-frontend
└─ shop-backend
```

---

## 2.4 Git 기록은 실행 증거

```text
Branch
Commit
Pull Request
Merge
```

는 Task 자체가 아니라 Task 수행 기록이다.

따라서:

```text
Task != Branch
Task != Commit
```

이다.

---

# 3. 가장 중요한 구조 변화

초기에는 다음 구조를 생각할 수 있다.

```text
Desktop App
   ↓
GitService
   ↓
Local Git / SSH Git
```

하지만 Windows 로컬, WSL, SSH Linux를 모두 자연스럽게 지원하려면 더 좋은 구조가 필요하다.

추천 구조:

```text
Windows Planner
      │
      ▼
  AgentClient
      │
      ├─ LocalTransport
      ├─ WslTransport
      └─ SshTransport
             │
             ▼
       Planner Agent
             │
        ┌────┼────┐
        ▼    ▼    ▼
       Git   FS   PTY
```

핵심:

> **Git을 어디에서 실행하는지를 Planner Agent가 추상화한다.**

---

# 4. VS Code Remote 스타일 구조

Windows 앱은 UI와 오케스트레이션을 담당한다.

실제 Git, File System, Terminal 처리는 작업 대상 머신의 Agent가 담당한다.

```text
┌──────────────────────────────────────┐
│         Planner Windows App          │
│                                      │
│  Project / Task / Timeline UI        │
│  Environment Selector                │
│  Agent Client                        │
└──────────────────┬───────────────────┘
                   │
             Agent Protocol
                   │
       ┌───────────┼────────────┐
       │           │            │
       ▼           ▼            ▼
  Local Agent   WSL Agent   SSH Remote Agent
       │           │            │
       ▼           ▼            ▼
     Git/FS       Git/FS       Git/FS
     PTY          PTY          PTY
```

이 구조는 다음을 가능하게 한다.

```text
Windows Local
WSL
SSH Linux
Cloud VM
Dev Server
Mac
Docker
Dev Container
```

---

# 5. 핵심 객체

전체 시스템에서 중요한 개념은 다음과 같다.

```text
Project
Repository
Task
ExecutionTarget
RepositoryCheckout
TaskWorkspace
TaskSession
Commit
PullRequest
GitHubInstallation
```

---

# 6. ExecutionTarget

ExecutionTarget은 실제 명령이 실행되는 환경이다.

예:

```text
My Windows
Local
Windows 11
```

```text
WSL Ubuntu
WSL
Ubuntu
```

```text
API Dev
SSH
api-dev
Ubuntu
```

추천 모델:

```ts
type ExecutionTarget =
  | {
      id: string;
      name: string;
      type: "local";
      os: "windows";
    }
  | {
      id: string;
      name: string;
      type: "wsl";
      distro: string;
      os: "linux";
    }
  | {
      id: string;
      name: string;
      type: "ssh";
      sshHost: string;
      os: "linux" | "macos";
    };
```

---

# 7. Repository

Repository는 논리적인 Git Repository다.

예:

```text
company/shop-backend
```

추천 모델:

```ts
type Repository = {
  id: string;

  name: string;

  provider?: "github" | "gitlab" | "bitbucket" | "local";

  remoteUrl?: string;

  githubRepositoryId?: number;

  defaultBranch?: string;
};
```

중요:

```text
Repository에 localPath를 직접 넣지 않는다.
```

왜냐하면 동일 Repository가 여러 머신에 존재할 수 있기 때문이다.

---

# 8. RepositoryCheckout

RepositoryCheckout은:

> 특정 ExecutionTarget에 존재하는 특정 Repository의 실제 Checkout

이다.

예:

```text
Repository: shop-backend
│
├─ Checkout A
│   ├─ Target: My Windows
│   └─ Path: C:\dev\shop-backend
│
└─ Checkout B
    ├─ Target: API Dev
    └─ Path: /home/dev/shop-backend
```

모델:

```ts
type RepositoryCheckout = {
  id: string;

  repositoryId: string;

  executionTargetId: string;

  path: string;

  lastSeenAt?: Date;
};
```

---

# 9. Task

추천 Task Key:

```text
TASK-142
```

멀티 프로젝트에서는:

```text
SHOP-142
AUTH-32
PLANNER-204
```

형태가 더 좋다.

DB PK와 사용자용 Key는 분리한다.

```text
id = UUID
task_key = "SHOP-142"
```

---

# 10. TaskWorkspace

TaskWorkspace는:

> Task가 특정 Repository Checkout에서 어떤 Git Context를 사용하는지 나타내는 객체

다.

모델:

```ts
type TaskWorkspace = {
  id: string;

  taskId: string;

  repositoryCheckoutId: string;

  branchName: string;

  baseCommitSha: string;

  status:
    | "ready"
    | "active"
    | "paused"
    | "done";

  startedAt?: Date;

  completedAt?: Date;
};
```

예:

```text
TASK-142

Workspace #1

Target
API Dev

Repository
shop-backend

Path
/home/dev/shop-backend

Branch
task/TASK-142-payment-timeout

Base Commit
82da123
```

---

# 11. TaskSession

TaskWorkspace는 Git Context를 나타낸다.

TaskSession은:

> 사용자가 실제로 특정 환경에서 Task를 작업하고 있는 현재 실행 세션

을 나타낸다.

예:

```text
TASK-142

Session
────────────────────

Target
api-dev

Repository
payment-server

Path
/home/dev/projects/payment-server

Branch
task/TASK-142-payment-timeout

Base Commit
827ada1

Started
14:32

Agent
Connected

Terminal
1 session

Changes
3 files

Commits
2
```

추천 모델:

```ts
type TaskSession = {
  id: string;

  taskId: string;

  executionTargetId: string;

  status:
    | "active"
    | "disconnected"
    | "paused"
    | "closed";

  startedAt: Date;

  lastConnectedAt?: Date;
};
```

---

# 12. Agent 구조

Planner Agent는 실행 환경 안에서 동작한다.

```text
planner-agent

├─ SystemService
│  ├─ OS
│  ├─ Architecture
│  ├─ Environment
│  └─ Process
│
├─ GitService
│  ├─ status
│  ├─ branch
│  ├─ switch
│  ├─ log
│  ├─ diff
│  ├─ commit
│  ├─ stash
│  ├─ remote
│  └─ fetch
│
├─ WorkspaceService
│  ├─ repo discovery
│  ├─ path validation
│  └─ repo metadata
│
├─ FileSystemService
│  ├─ list
│  ├─ stat
│  └─ watch
│
└─ TerminalService
   ├─ create PTY
   ├─ write
   ├─ resize
   └─ close
```

---

# 13. Local Agent

Windows 로컬 작업도 Agent를 사용한다.

```text
Planner.exe
   │
   │ stdio
   ▼
planner-agent.exe
   │
   ▼
C:\dev\shop-backend
```

장점:

```text
Local과 Remote의 API가 동일해짐
```

---

# 14. WSL Agent

WSL도 동일한 구조다.

```text
Planner.exe
   │
   │ wsl.exe
   ▼
planner-agent
   │
   ▼
Ubuntu
   │
   ▼
/home/dev/project
```

Transport만 다르다.

---

# 15. SSH Remote Agent

원격 Linux에서는:

```text
Planner.exe
   │
   │ ssh
   ▼
planner-agent --stdio
   │
   ▼
Linux Server
```

구조가 된다.

---

# 16. Transport 추상화

추천 인터페이스:

```ts
interface AgentTransport {
  connect(): Promise<void>;

  send(message: AgentRequest): Promise<void>;

  close(): Promise<void>;
}
```

구현:

```text
LocalTransport
WslTransport
SshTransport
```

---

# 17. AgentClient

Windows Planner는 AgentClient를 통해서만 Agent와 통신한다.

```ts
interface AgentClient {
  request<T>(
    method: string,
    params?: unknown
  ): Promise<T>;
}
```

예:

```ts
await agent.request("git.currentBranch", {
  workspaceId: "ws_123"
});
```

---

# 18. Agent Protocol

초기 구현은 JSON-RPC 스타일이면 충분하다.

예:

```json
{
  "id": 1,
  "method": "git.currentBranch",
  "params": {
    "path": "/home/dev/project"
  }
}
```

Response:

```json
{
  "id": 1,
  "result": {
    "branch": "task/TASK-142-payment-timeout"
  }
}
```

---

# 19. SSH 통신 방식

MVP에서는 별도 서버 포트를 열지 않는다.

다음 방식이 가장 단순하다.

```text
ssh api-dev "~/.planner/bin/planner-agent --stdio"
```

그리고:

```text
SSH stdin
  ↓
Agent requests

SSH stdout
  ↓
Agent responses
```

로 통신한다.

장점:

- 별도 TCP 포트 불필요
- SSH 인증 재사용
- VPN 환경에서 동작
- 서버 daemon 불필요
- 별도 TLS 구성 불필요

---

# 20. SSH 설정

가능하면 사용자의 기존 OpenSSH 환경을 그대로 사용한다.

예:

```sshconfig
Host api-dev
    HostName 10.0.0.20
    User dev
    IdentityFile ~/.ssh/id_ed25519
```

Planner에서는:

```text
SSH Target

Host Alias
api-dev
```

정도만 등록한다.

중요:

> Planner 앱이 private key를 직접 저장하는 구조는 초기에는 피한다.

가능하면 다음을 재사용한다.

```text
OpenSSH
~/.ssh/config
ssh-agent
Windows SSH Agent
```

---

# 21. Remote Agent 자동 설치

SSH 연결 후 다음을 확인한다.

```text
~/.planner/bin/planner-agent
```

없으면 Windows 앱이 자동 업로드한다.

예:

```text
~/.planner/
├─ bin/
│  └─ planner-agent
└─ state/
```

실행 권한이 없으면:

```bash
chmod +x ~/.planner/bin/planner-agent
```

처리한다.

---

# 22. Agent Version 관리

Windows 앱과 Agent는 프로토콜 버전을 관리해야 한다.

Handshake 예:

```json
{
  "protocolVersion": 3,
  "agentVersion": "0.8.0",
  "platform": "linux",
  "arch": "x64",
  "capabilities": [
    "git",
    "filesystem",
    "terminal"
  ]
}
```

Windows Client가 더 높은 호환 Agent를 요구하면 자동 업데이트한다.

---

# 23. Agent 배포

추천 바이너리:

```text
planner-agent-windows-x64.exe
planner-agent-linux-x64
planner-agent-linux-arm64
planner-agent-darwin-arm64
```

Rust 또는 Go가 잘 맞는다.

장점:

```text
단일 바이너리
Node/Python 런타임 불필요
설치 간단
Cross compile 가능
```

---

# 24. Agent 세션 전략

## MVP

SSH 연결마다 Agent 실행:

```text
ssh api-dev planner-agent --stdio
```

SSH가 종료되면 Agent도 종료된다.

장점:

```text
설치 단순
상태 관리 단순
보안 단순
```

---

## 이후

Persistent Agent Daemon:

```text
planner-agent daemon
```

지원 가능.

```text
Git Watcher
Persistent Terminal
Task Session
Reconnect
Background Sync
```

등이 가능해진다.

하지만 초기에는 Session Agent를 추천한다.

---

# 25. Repository 등록 흐름

사용자:

```text
Environment
> SSH: api-dev
```

선택.

연결:

```text
Connecting...

✓ SSH connected
✓ Agent installed
✓ Agent started
✓ Git available
```

그다음:

```text
Select Repository

/home/dev/projects
├─ payment-server
├─ auth-server
└─ shop-backend
```

선택:

```text
/home/dev/projects/payment-server
```

Agent가 확인:

```bash
git rev-parse --show-toplevel
git remote get-url origin
git branch --show-current
git rev-parse HEAD
```

---

# 26. 작업 시작 UX

Task:

```text
TASK-142
결제 API 오류 처리

Repository
payment-server

Environment
SSH · api-dev

[ 작업 시작 ]
```

버튼을 누르면:

```text
1. Agent 연결
2. Repository 존재 확인
3. Git 상태 확인
4. Dirty Working Tree 검사
5. Base Branch 확인
6. Base Commit 저장
7. Task Branch 확인
8. Branch 생성 또는 Switch
9. TaskWorkspace 생성
10. TaskSession 시작
11. Task → In Progress
```

---

# 27. Branch Naming

기본:

```text
task/{TASK_KEY}-{slug}
```

예:

```text
task/TASK-142-payment-timeout
```

또는:

```text
feature/TASK-142-payment-timeout
fix/TASK-142-payment-timeout
```

MVP에서는:

```text
Branch name 안에 Task Key 포함
```

정도만 강제해도 충분하다.

---

# 28. Base Commit

작업 시작 시 반드시 기준 Commit을 저장한다.

```text
base_commit_sha = 8da21f3
```

그러면:

```bash
git log 8da21f3..HEAD
```

로 Task 이후 Commit을 정확히 추적할 수 있다.

---

# 29. Commit 연결 우선순위

추천:

```text
1. TaskWorkspace에 연결된 Branch
2. Branch name의 Task Key
3. Commit message의 Task Key
4. Pull Request의 Task Key
5. Manual Mapping
```

---

# 30. Local Commit 추적

Agent가:

```bash
git log <baseCommit>..HEAD
```

를 실행한다.

Commit 모델:

```ts
type Commit = {
  id: string;

  repositoryId: string;

  sha: string;

  message: string;

  authorName?: string;

  authorEmail?: string;

  committedAt: Date;
};
```

---

# 31. Commit Unique Key

동일 Commit 중복 방지:

```text
repository_id + commit_sha
```

를 unique key로 둔다.

---

# 32. Task와 Commit 관계

N:M 가능하게 설계한다.

```text
task_commits
- task_id
- commit_id
- mapping_source
```

예:

```text
mapping_source:
workspace
branch
commit_message
pull_request
manual
```

---

# 33. Context Switching

현재:

```text
TASK-142
API Dev
task/TASK-142-payment
```

작업 중인데:

```text
TASK-153
로그인 토큰 오류
```

선택.

Agent가:

```bash
git status --porcelain
```

확인.

Dirty 상태이면:

```text
현재 변경사항이 있습니다.

3 files modified

[Stash 후 전환]
[Commit 후 전환]
[현재 상태 유지]
[취소]
```

---

# 34. Stash

동일 머신에서 Task 전환 시:

```bash
git stash push -m "planner:TASK-142"
git switch task/TASK-153-login
```

복귀:

```bash
git switch task/TASK-142-payment
```

후 사용자에게:

```text
저장된 작업 내용이 있습니다.

[복원]
[나중에]
```

제공.

자동 stash pop은 conflict 가능성이 있으므로 조심해서 처리한다.

---

# 35. 머신 간 이동

중요한 차이:

```text
Task Context Switching
!=
Machine Switching
```

예:

```text
Windows
TASK-142
3 files modified
```

이 상태를 그대로:

```text
SSH Linux
```

로 가져갈 수는 없다.

Git Stash는 머신 간 공유되지 않기 때문이다.

---

# 36. 머신 이동의 기본 규칙

초기 정책:

> 다른 머신으로 작업을 이동하려면 Commit + Push를 기준으로 한다.

예:

```text
Windows
   ↓
Commit
   ↓
Push
   ↓
GitHub
   ↓
SSH Linux
   ↓
Fetch
   ↓
Switch
```

Planner UX:

```text
현재 환경에 Push되지 않은 변경사항이 있습니다.

다른 환경에서는 이 변경사항을 사용할 수 없습니다.

[Commit & Push]
[현재 환경에 유지]
[취소]
```

---

# 37. Checkpoint

추후 기능으로 Planner 전용 Checkpoint를 고려할 수 있다.

예:

```text
Task Checkpoint
  ↓
Temporary Commit
  ↓
Temporary Remote Branch
  ↓
Other Machine Restore
```

하지만 Git history/Remote branch 관리가 복잡해지므로 MVP에서는 제외하는 것을 추천한다.

---

# 38. Active Environment

같은 Task가 여러 환경에 Checkout되어 있을 수 있다.

예:

```text
TASK-142

● API Dev
  /home/dev/project
  task/TASK-142

○ My Windows
  C:\dev\project
  task/TASK-142
```

TaskSession의 Active Target을 하나 두는 것이 좋다.

---

# 39. Remote Terminal

Agent 구조를 만들면 Terminal 지원도 자연스럽다.

Task 화면:

```text
[Open Terminal]
```

요청:

```text
terminal.create
```

Agent:

```text
PTY
 ↓
bash
```

초기 cwd:

```text
/home/dev/projects/payment-server
```

Planner UI:

```text
dev@api-dev:~/projects/payment-server$
```

---

# 40. File System

초기에는 Git 중심으로 시작하되 Agent의 FileSystemService를 두면 확장 가능하다.

예:

```text
fs.list
fs.stat
fs.watch
```

단, Planner 자체가 VS Code처럼 전체 Editor가 되는 방향은 신중해야 한다.

핵심은 Git/Task Context 관리다.

---

# 41. IDE 연동 방향

Planner가 직접 Editor를 만들 필요는 없다.

대신:

```text
[Open in VS Code]
```

기능을 제공할 수 있다.

Local:

```text
code C:\dev\project
```

Remote:

```text
code --remote ssh-remote+api-dev /home/dev/project
```

같은 개념으로 VS Code Remote와 연동할 수 있다.

장기적으로는 VS Code Extension을 별도로 둘 수도 있다.

---

# 42. GitHub App 역할

Planner Agent:

```text
Local 상태
Working Tree
Local Branch
Local Commit
Stash
Terminal
```

GitHub App:

```text
Remote 상태
Push
Remote Commit
Pull Request
Review
Merge
```

Planner Backend:

```text
업무 상태
Project
Task
Workspace
Session
Git Mapping
Timeline
```

---

# 43. 전체 아키텍처

```text
                         GitHub
                           │
                           │ GitHub App
                           │ Webhook / API
                           ▼
                 ┌──────────────────────┐
                 │   Planner Backend    │
                 │                      │
                 │ Project              │
                 │ Repository           │
                 │ Task                 │
                 │ TaskWorkspace        │
                 │ TaskSession          │
                 │ Commit               │
                 │ PullRequest          │
                 │ Timeline             │
                 └──────────┬───────────┘
                            │
                            │ API / Sync
                            ▼
                 ┌──────────────────────┐
                 │  Windows Planner App │
                 │                      │
                 │ Task UI              │
                 │ AgentClient          │
                 │ Environment Manager  │
                 └──────────┬───────────┘
                            │
                ┌───────────┼────────────┐
                │           │            │
                ▼           ▼            ▼
        LocalTransport  WslTransport  SshTransport
                │           │            │
                ▼           ▼            ▼
         Windows Agent   WSL Agent   Linux Agent
                │           │            │
                └──────┬────┴─────┬──────┘
                       ▼          ▼
                      Git        PTY
                       │          │
                       ▼          ▼
                  Repository    Shell
```

---

# 44. GitHub Push 연동

GitHub App 설치 후:

```text
push webhook
```

을 받는다.

Backend:

```text
Webhook
  ↓
Signature 검증
  ↓
Repository 확인
  ↓
Branch 확인
  ↓
TaskWorkspace 찾기
  ↓
Commit Upsert
  ↓
TaskCommit 연결
```

---

# 45. 초기 GitHub Sync

GitHub App 설치 후 과거 Commit이 모두 Webhook으로 오는 것은 아니다.

따라서:

```text
초기 상태
→ GitHub API

실시간 변화
→ Webhook
```

구조로 가져간다.

---

# 46. Pull Request

PR 연결:

```text
Task
  ↓
Branch
  ↓
Commit
  ↓
Pull Request
  ↓
Merge
```

매핑 우선순위:

```text
1. PR head branch
2. TaskWorkspace branch
3. PR title/body Task Key
4. PR commits의 Task mapping
```

---

# 47. Merge와 Task 완료

자동 완료는 설정 가능하게 한다.

```text
Auto Complete on Merge
```

예:

```text
false
```

이면:

```text
PR merged
→ Ready to Complete
```

```text
true
```

이면:

```text
PR merged
→ Done
```

---

# 48. Multi Repository Task

예:

```text
TASK-201 회원가입 기능

frontend
→ Workspace A

backend
→ Workspace B
```

Workspace마다 Branch, Base Commit, PR이 다를 수 있다.

Task Done 조건:

```text
모든 필수 Workspace 완료
```

로 판단해야 한다.

---

# 49. 추천 DB 스키마

## projects

```text
id
key
name
description
created_at
updated_at
```

---

## repositories

```text
id
provider
github_repository_id
owner
name
remote_url
default_branch
created_at
updated_at
```

---

## project_repositories

```text
project_id
repository_id
created_at
```

---

## execution_targets

```text
id
name
type
os
ssh_host
wsl_distro
created_at
updated_at
```

---

## repository_checkouts

```text
id
repository_id
execution_target_id
path
last_seen_at
created_at
updated_at
```

---

## tasks

```text
id
project_id
task_key
title
description
status
priority
started_at
completed_at
created_at
updated_at
```

---

## task_workspaces

```text
id
task_id
repository_checkout_id
branch_name
base_branch
base_commit_sha
status
started_at
completed_at
created_at
updated_at
```

---

## task_sessions

```text
id
task_id
execution_target_id
status
started_at
last_connected_at
ended_at
```

---

## commits

```text
id
repository_id
sha
message
author_name
author_email
committed_at
created_at
```

Unique:

```text
(repository_id, sha)
```

---

## task_commits

```text
task_id
commit_id
mapping_source
created_at
```

---

## pull_requests

```text
id
repository_id
github_pr_id
number
title
state
head_branch
base_branch
merged_at
url
created_at
updated_at
```

---

## task_pull_requests

```text
task_id
pull_request_id
mapping_source
created_at
```

---

## github_installations

```text
id
github_installation_id
account_login
account_type
created_at
updated_at
```

---

## github_webhook_deliveries

```text
id
delivery_id
event_type
processed_at
status
```

---

# 50. 데이터 관계

```text
Project
│
├─ Repository
│   │
│   └─ RepositoryCheckout
│       │
│       └─ ExecutionTarget
│
└─ Task
    │
    ├─ TaskWorkspace
    │   │
    │   └─ RepositoryCheckout
    │
    ├─ TaskSession
    │   └─ ExecutionTarget
    │
    ├─ TaskCommit
    │   └─ Commit
    │
    └─ TaskPullRequest
        └─ PullRequest
```

---

# 51. Backend API 예시

## Projects

```text
POST  /projects
GET   /projects/:id
PATCH /projects/:id
```

---

## Repositories

```text
POST /projects/:id/repositories
GET  /projects/:id/repositories
```

---

## Execution Targets

```text
POST /execution-targets
GET  /execution-targets
GET  /execution-targets/:id
```

---

## Repository Checkouts

```text
POST /repositories/:id/checkouts
GET  /repositories/:id/checkouts
```

---

## Tasks

```text
POST   /projects/:id/tasks
GET    /projects/:id/tasks
GET    /tasks/:id
PATCH  /tasks/:id
```

---

## Workspaces

```text
POST /tasks/:id/workspaces
GET  /tasks/:id/workspaces
```

---

## Sessions

```text
POST /tasks/:id/sessions
PATCH /task-sessions/:id
```

---

# 52. Agent API 예시

```text
system.info
agent.handshake

workspace.validate
workspace.discover

git.status
git.currentBranch
git.listBranches
git.createBranch
git.switchBranch
git.head
git.log
git.diff
git.fetch
git.stash

terminal.create
terminal.write
terminal.resize
terminal.close
```

---

# 53. Agent 보안

중요한 원칙:

> Agent는 임의 Shell 실행 API를 기본 제공하지 않는다.

예를 들어:

```text
exec(command)
```

를 아무 명령이나 받을 수 있게 하지 않는 것이 좋다.

대신:

```text
git.status
git.switchBranch
workspace.validate
```

같은 고수준 API를 제공한다.

Terminal 기능은 별도의 명시적 사용자 세션으로 분리한다.

---

# 54. Command Injection 방지

나쁜 예:

```ts
exec(`git switch ${branch}`);
```

좋은 예:

```ts
spawn("git", ["switch", branch], {
  cwd
});
```

추가:

```text
branch validation
path validation
allowed command
timeout
process kill
stderr handling
```

---

# 55. 작업 시작 예외 처리

## Repository가 없음

```text
Repository를 선택하세요.
```

## Checkout이 없음

```text
이 환경에는 Repository가 없습니다.

[기존 폴더 선택]
[Clone]
```

## Dirty Working Tree

```text
현재 변경사항이 있습니다.
```

## Existing Branch

```text
Task Branch가 이미 존재합니다.

[기존 Branch 사용]
```

## Detached HEAD

작업 시작 중단.

## Merge/Rebase 중

작업 전환 중단.

## Agent Disconnect

```text
Remote Agent connection lost.
```

TaskSession을 disconnected 상태로 유지.

---

# 56. Agent Reconnect

Windows 앱 재시작 후:

```text
TASK-142
[Resume]
```

누르면:

```text
SSH reconnect
  ↓
Agent start
  ↓
Repository 상태 조회
  ↓
Branch 확인
  ↓
TaskWorkspace 확인
  ↓
TaskSession 복원
```

---

# 57. Timeline

예:

```text
14:32
Task started

14:33
Connected to api-dev

14:33
Branch created
task/TASK-142-payment-timeout

14:57
Commit
handle payment timeout
abc123

15:18
Commit
add retry logic
def456

15:35
Push detected

15:42
PR #231 opened

16:10
PR merged

16:11
Task completed
```

---

# 58. 핵심 사용자 UX

최종 사용자는 내부 구조를 몰라도 된다.

```text
1. Project 생성

2. Repository 연결

3. Environment 등록
   - My Windows
   - WSL Ubuntu
   - SSH api-dev

4. Task 생성

5. Task에서 작업 환경 선택

6. 작업 시작

7. Planner가 자동 처리:
   - Agent 연결
   - Repo 상태 확인
   - Branch 생성/전환
   - Base Commit 저장
   - TaskSession 시작

8. 개발자는 평소대로 작업

9. Commit 자동 추적

10. Push/PR/Merge 자동 연결
```

---

# 59. 화면 예시

```text
TASK-142
결제 API 오류 처리

Status
● In Progress

Environment
SSH · api-dev

Repository
payment-server

Path
/home/dev/projects/payment-server

Branch
task/TASK-142-payment-timeout

Base
827ada1

Changes
3 files

Commits
2

Terminal
1 active

[Open Terminal]
[Open in VS Code]
[Pause Task]
[Complete Task]
```

---

# 60. MVP 구현 순서

## Phase 1 — Windows Local

목표:

> Task를 선택하면 로컬 Git Context가 전환된다.

기능:

```text
Windows App
Project
Multi Repository
Task
RepositoryCheckout
TaskWorkspace
Local Agent
Branch 생성
Branch 전환
Base Commit
Commit 추적
```

---

## Phase 2 — SSH Remote

목표:

> Windows UI에서 Linux 작업 환경을 제어한다.

기능:

```text
SSH Target
SSH Config 연동
Remote Agent Upload
Agent Handshake
Remote Repository 등록
Remote Git
Task Start
Remote Commit Tracking
```

---

## Phase 3 — WSL

기능:

```text
WslTransport
WSL Agent
Linux Repository Checkout
```

Agent API는 그대로 재사용.

---

## Phase 4 — Terminal

기능:

```text
Remote PTY
Terminal UI
Task cwd
Reconnect 정책
```

---

## Phase 5 — Context Switching

기능:

```text
Dirty detection
Stash
Branch switching
Workspace restore
```

---

## Phase 6 — GitHub App

기능:

```text
GitHub App
Repository Sync
Push Webhook
Remote Commit Sync
```

---

## Phase 7 — Pull Request

기능:

```text
PR Webhook
PR Mapping
Merge
Task Completion
```

---

# 61. 기술 선택 추천

## Windows App

추천 후보:

```text
Tauri + React
```

또는:

```text
Electron + React
```

장기적으로 Agent도 Rust를 사용할 계획이면 Tauri가 구조적으로 잘 맞는다.

---

## Remote Agent

추천:

```text
Rust
```

또는:

```text
Go
```

중요한 조건:

```text
Single Binary
Cross Platform
Small Runtime Dependency
Stable Process Control
PTY Support
```

---

## Backend

예:

```text
TypeScript
Node.js
PostgreSQL
```

또는 팀이 익숙한 서버 스택.

---

# 62. 초기 구현에서 피해야 할 것

## Project = Repository

Multi Repo에서 깨짐.

## Task = Branch

도메인 결합이 너무 강함.

## Commit Message만으로 Task 연결

사용자 UX가 나빠짐.

## SSH에서 매 명령마다 새 ssh process 실행

성능/상태/터미널 확장성이 나빠짐.

## Planner Client가 OS별 Git Output을 직접 파싱

Agent가 표준화해야 함.

## Agent에 무제한 exec API 제공

보안상 위험.

## 처음부터 Agent Daemon

MVP 복잡도가 너무 높음.

## 처음부터 Editor 구현

제품의 핵심이 흐려짐.

---

# 63. 가장 중요한 기술적 결정

이 제품에서 가장 중요한 아키텍처 결정은:

> **Windows Planner가 Git을 직접 제어하는 것이 아니라, 실행 환경별 Planner Agent와 통신한다.**

이다.

이 결정을 하면:

```text
Windows
WSL
SSH Linux
Mac
Cloud VM
Container
```

를 같은 모델로 확장할 수 있다.

---

# 64. 가장 중요한 제품적 결정

사용자의 핵심 경험은 다음이어야 한다.

```text
Task 선택
  ↓
Environment 선택
  ↓
작업 시작
  ↓
Git Context 자동 준비
  ↓
Terminal/IDE 연결
  ↓
작업
  ↓
Commit 자동 기록
  ↓
PR 자동 연결
```

즉:

> **Planner에서 오늘 할 일을 고르면 개발 환경이 그 작업에 맞춰진다.**

---

# 65. 최종 개념 요약

```text
Project
= 업무의 경계

Repository
= 코드의 경계

ExecutionTarget
= 명령이 실행되는 머신/환경

RepositoryCheckout
= 특정 환경에 존재하는 Repository 복사본

Task
= 실제 작업 단위

TaskWorkspace
= Task + RepositoryCheckout + Git Context

TaskSession
= 현재 실행 중인 Task 작업 세션

Planner Agent
= 실행 환경의 Git/FS/PTY 추상화

AgentTransport
= Local / WSL / SSH 연결 방식

Commit
= 작업 기록

Pull Request
= Remote Review/Merge 기록

GitHub App
= Remote GitHub 상태 동기화

Planner Backend
= 업무 상태와 개발 상태를 연결하는 중심 시스템
```

---

# 66. 최종 추천 아키텍처

```text
                       GitHub
                         │
                    GitHub App
                         │
                         ▼
                 Planner Backend
                         │
                         ▼
                 Windows Planner
                         │
                     AgentClient
                         │
              ┌──────────┼──────────┐
              ▼          ▼          ▼
           Local        WSL        SSH
              │          │          │
              ▼          ▼          ▼
            Agent      Agent      Agent
              │          │          │
              └──────┬───┴───┬──────┘
                     ▼       ▼
                    Git     Terminal
                     │
                     ▼
              Repository Checkout
                     │
                     ▼
                 TaskWorkspace
                     │
                     ▼
                  TaskSession
```

이 구조를 기준으로 구현하면, 처음에는 Windows Local + SSH Linux 정도로 시작하면서도 이후 WSL, Mac, Cloud VM, Dev Container까지 자연스럽게 확장할 수 있다.
