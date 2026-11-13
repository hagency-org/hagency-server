# GitFab 系统架构说明

GitFab 在 hagency 现有能力上增补“自然语言进、可部署工件出”的软件工坊流水线：Human 在 Matrix 房间提出任务，编排器按 Spec → 确认 → Plan → Execute → Test → Review → Release 推进，Agent 在任务分支上实现，编排器在 GitHub 或 GitCode 上建 PR、读测试状态、发 Release，Runner 部署并回执，所有关键事件签名留痕，大屏只读展示。原有的房间、Agent、资源池和执行设备逻辑不改，只增补。

本文描述的是 `feat/gitfab-events` 分支上的实现。设计取舍见 `docs/design/2026-11-12-gitfab-orchestrator.zh-CN.md`。

## 组件

```
 Human（Hagency Desktop / Matrix 客户端）
   │  项目房间：/gitfab start…、/gitfab approve|revise|reject|continue|reassign|abort
   ▼
 Palpo（Matrix）◄──────────────── 阶段消息（m.notice，Thread 内，不提及任何人）
   │                                     ▲
   ▼                                     │
 hagency-server ─ agent-service ─────────┤
   ├─ 路由 worker：识别 /gitfab 命令 → orchestrator_commands
   ├─ 编排器 worker（orchestrator.rs，每 5 秒一轮）
   │     流程规则：orchestrator_flow.rs（纯函数）
   │     派发：个人 Agent 走 bindings；资源池 Agent 走原有 route_shared_event
   │     平台：orchestrator_git.rs → gitfab-forge（GitHub / GitCode REST）
   │     签名日志：Recorder → gitfab-events journal（JSONL，追加）
   ├─ HTTP：/hagency/api/v1/projects/{project}/gitfab/runs…（发起、查看、确认、人工决定）
   ├─ HTTP：/hagency/gitfab/v1/public-keys（公钥）、/hagency/gitfab/v1/agents（大屏 Agent 状态，只读 Token）
   └─ PostgreSQL：orchestrator_runs、orchestrator_stage_events、orchestrator_commands
        │ 派发 / 回复（原有 transport）
        ▼
 Agent（执行设备上的 Codex、Claude 等；个人 Agent 或资源池 Agent）
   └─ 只做 git：在 agent/<project_id>/<task_id> 分支提交并推送

 GitHub / GitCode ◄── 编排器：建 PR、读测试状态、发 Release（服务器保管的 Token）

 gitfab-runner（Linux Runner，独立密钥）
   └─ 构建镜像 → SBOM（syft）→ 替换容器 → 健康与版本检查 → 签名部署回执；失败自动恢复上一版

 gitfab-board（只读大屏）◄── 事件日志 + 部署回执（逐条验签）+ 实时探测 + /gitfab/v1/agents

 acceptance/（验收包）── preflight、run-project、verify-events、verify-ledger、verify-release、rollback
```

## 代码位置

| 组件 | 位置 | 说明 |
|---|---|---|
| 流程规则 | `crates/agent-service/src/orchestrator_flow.rs` | 状态机，纯函数无 I/O：阶段、角色、返工上限（2 次）、超时与人工介入 |
| 编排器 worker | `crates/agent-service/src/orchestrator.rs` | 推进运行、派发阶段消息、记录签名事件、房间命令 |
| 平台动作 | `crates/agent-service/src/orchestrator_git.rs` | Execute 成功建/更新 PR，Test 成功读测试状态，Release 成功发 Release |
| Agent 状态 | `crates/agent-service/src/orchestrator_status.rs` | 大屏用的 Running / Pending / Active |
| HTTP | `crates/agent-service/src/api_gitfab.rs` | 发起、查看、Spec 确认、人工决定 |
| 存储 | `crates/agent-service/src/orchestrator_schema.sql`、`transport.rs` | 运行、阶段派发、房间命令三张表 |
| 事件与签名 | `crates/gitfab-events/` | schema 校验、规范化、Ed25519 签名与验签、身份事件、账本、证据包；命令 `gitfab` |
| Git 平台 | `crates/gitfab-forge/` | GitHub 与 GitCode 一套接口；命令 `gitfab-forge` |
| 部署 | `crates/gitfab-runner/` | 部署、自动恢复、回退、SBOM 证明；命令 `gitfab-runner` |
| 大屏 | `crates/gitfab-board/` | 只读页面与 `/api/snapshot`；命令 `gitfab-board` |
| 服务端接线 | `crates/backend/src/{config.rs,main.rs,gitfab.rs}` | `[gitfab]` 配置、签名密钥加载、启动编排器 |
| 验收包 | `acceptance/` | 六个入口、项目配置、夹具、自测 |

## 一次运行的数据流

1. **发起**：Human 在不加密的项目房间发 `/gitfab start <任务 ID>`（附仓库、基线、Spec、角色、目标），或调用 API。这条消息成为任务 Thread 的根。运行写入 `orchestrator_runs`，记录 intake 事件。
2. **派发**：编排器把阶段消息发到 Thread，并按角色交给对应 Agent：个人 Agent 通过原有 bindings 派发；资源池 Agent（`mas_…`）构造“发起人提及该 Agent”的事件交给原有 `route_shared_event`，原有的成员、额度、队列检查照常生效。
3. **Spec 闸门**：Spec 回复后运行停在 `awaiting_spec_approval`，只有发起人能确认、要求修改或拒绝。
4. **执行与平台**：Plan → Execute → Test → Review。Execute 成功后编排器在平台上建或更新 PR；Test 成功后读取平台测试状态；Review 驳回自动返工最多 2 次，第 3 次转人工；超时转人工，绝不自动重派。
5. **发布**：Release 阶段成功后编排器以 PR 头提交为目标发 Release。PR 与 Release 链接写入该阶段签名事件的 `artifact_refs`，并在 Thread 里说明。
6. **部署**：Runner 取该 Release 部署，写签名回执；起不来就恢复上一版并如实记录。
7. **验收与展示**：`verify-release.sh` 核对事件、平台、回执、SBOM、容器与线上版本；大屏随时读取同一批记录。

## 信任与密钥

| 密钥 / 凭据 | 持有方 | 用途 | 存放 |
|---|---|---|---|
| GitFab 事件签名密钥 | 服务端 | 任务创建、执行结果、评审结论、Release 等关键事件 | `[gitfab] signing_key_file`（0600），公钥经 `/hagency/gitfab/v1/public-keys` 公布 |
| Runner 回执密钥 | Runner | 部署回执、SBOM 证明 | Runner 本机文件（0600），与服务端密钥分开 |
| Git 平台 Token | 服务端 | 建 PR、读状态、发 Release | `[gitfab] github_token_file` / `gitcode_token_file`（0600），只进 Authorization 头 |
| 大屏 Token | 服务端与大屏 | 读 Agent 状态 | `[gitfab] board_token_file`（0600），定长比较 |
| Agent 推送凭据 | 执行设备 | 推送任务分支 | 执行端自己的仓库凭据，不经过 GitFab |

原则：
- Agent 拿不到服务器的平台 Token，也不能提供 PR 或 Release 链接，链接由服务器从平台读回并签名记录。
- 每个关键事件附一条身份事件（`gitfab.identity.v1`），`gitfab verify` 交叉核对。
- 私钥、Token 不进日志、报告、证据包或大屏；证据包含凭据或本机路径时拒绝生成 manifest。
- 大屏只响应 GET，没有写路径。

## 存储

| 数据 | 位置 | 性质 |
|---|---|---|
| 运行状态 | PostgreSQL `hagency_agent_v1.orchestrator_runs` | 带修订号，一次状态转换与它的待写事件一起保存 |
| 阶段派发 | `orchestrator_stage_events` | 派发 ID 与阶段对应，用于读回结果 |
| 房间命令 | `orchestrator_commands` | `/gitfab` 命令入队，编排器处理后回复 |
| 签名事件日志 | `[gitfab] journal_file`（JSONL） | 只追加；同键同内容不重复写、内容不同拒绝；文件被改则拒绝打开 |
| 部署回执 | Runner 的回执日志（JSONL） | 同上，签名 + 身份事件 |
| SBOM | Runner 的 SBOM 目录 | `.spdx.json` + `.sha256` + 签名 `.sig` |
| 验收证据 | `evidence/<acceptance_run_id>/` | 各命令报告与 manifest |

## 配置

服务端 `[gitfab]`：`signing_key_file`、`signing_key_id`、`journal_file`（设置即启用编排器，需 Agent 服务）、`stage_timeout_minutes`（默认 120）、`github_token_file`、`gitcode_token_file`、`github_api`、`gitcode_api`、`board_token_file`。未配置平台 Token 时不调用平台，流程照常；未配置大屏 Token 时不开放 Agent 状态接口。

验收包：`acceptance/gitfab.env`（示例见 `gitfab.env.example`）与 `acceptance/projects/DEMO-0N.env`，详见 `acceptance/README.zh-CN.md`。

## 已知限制

- 编排器只支持不加密的项目房间。
- 平台审查状态可用 `gitfab-forge review` 读取（GitHub 取每位审查人最后一次批准或要求修改，GitCode 取指派审查人的 accept），但编排器尚未把它写进阶段说明；流程里的评审结论来自 Reviewer Agent。测试状态 GitHub 取提交状态与 check-runs（细粒度 Token 时改读 Actions），GitCode 取 PR 的 pipeline_status。
- GitCode 经典 Token 不能限定单个仓库。
- 事件日志尚未写入 Git。
