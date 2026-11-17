# GitFab 流程编排器设计（S2）

状态：已实现（worker、存储、HTTP 接口）；尚未在真实 Agent 上验收。

## 目标

按固定流程 spec → plan → execute → test → review → release，在同一个项目房间、同一个 Thread 里依次触发不同的 Agent，并把每一步写入 GitFab 事件日志（`hagency-gitfab-events` 的 `Journal`）。不改动现有派发状态机，尤其不改“已开始的执行永不重跑”。

## 现状约束

- **Agent 不能触发 Agent。** `transport.rs` 的 `ingest_routed_inner` 会忽略 Agent 傀儡和所有 `@_hagency_*` 账号发出的消息。
- **项目房间里必须真正 @ 到 Agent**（`m.mentions`）才会派发。只有 owner 与 Agent 的一对一房间可以不 @。
- **没有内部 API 能在没有人类消息的情况下创建派发。** 资源池（shared）路径的派发也只能来自 Matrix 消息。
- **结果只能轮询读取：** `owner_events` 的终态，以及 `reply_outbox` 的发送状态。没有事件钩子。加密房间的回复服务器读不到明文。

## 方案

1. **触发：** 编排器先以 `@_hagency_service` 在 Thread 里发一条阶段消息，带上上一阶段的产出摘要和本阶段的输入。这样既得到真实的 `$event_id`，房间里也留下可见的审计痕迹。然后调用新增的内部方法 `TransportStore::ingest_orchestrated`，把这条事件准入给下一阶段 Agent 的绑定。
   - 该方法复用 `ingest_routed_inner` 的全部检查（绑定、成员关系、暂停、队列上限），只插入一条 `owner_events`，不新增状态。
   - 请求者记为发起流程的人类，因此 `requester_in_room` 和投递事实仍然成立。
   - 只放行登记在新表 `orchestrator_stages` 里的事件 ID。普通路由对 `@_hagency_*` 的过滤保持不变，不开后门。
2. **观察：** 新增一个轮询 worker，仿照 `request_notice_needed` 视图，读 `owner_events ⨝ reply_outbox`。
   - `outcome='replied'` 且回复已发送：本阶段完成。
   - `failed`、`rejected`、`expired`：按规则重试或转入人工介入。
   - `unknown`：立即停在人工介入，绝不自动再派发。
3. **流程状态：** 存在自己的追加式表里（`orchestrator_runs`、`orchestrator_stages`），不写进看板，看板只接收回写的结果。
4. **返工：** Reviewer 驳回后，以新的一轮（`round + 1`）对 Implementer 发起一次新的派发，不重放旧派发。超过 2 轮转入人工介入，保留原 findings。
5. **事件：** 每个阶段的开始和结束都写入 `gitfab.task.v1`；四类关键事件由 `Journal` 签名，并附带身份事件。

## 代码位置

- 流程规则：`crates/agent-service/src/orchestrator_flow.rs`（纯函数，无 I/O）。
- worker：`orchestrator.rs`。阶段消息以 `@_hagency_service` 发成 Thread 里的 `m.notice`，带 `org.hagency.gitfab_stage`，不带 mention；普通路由和资源池路由都不会接收它。
- 存储：`transport.rs` 的 `orchestrator_runs`、`orchestrator_stage_events`（`orchestrator_schema.sql`）。
- 防重：一次状态转换和它产生的日志事件一起保存（`pending_events`），之后再写日志；日志按事件键幂等，所以重启后每个事件只写一次、内容不变。阶段消息的事务 ID 由 run、阶段、轮次、次数、Spec 版本决定，重发得到同一个事件。
- HTTP：`api_gitfab.rs`。
  - `POST /hagency/api/v1/projects/{project}/gitfab/runs`：发起（Space 成员即可）。body：`taskId`、`roomId`、`threadRoot`、`bindings{coordinator,implementer,reviewer}`、`context{goal,gitBackend,repoRef,baselineSha,specRef[,projectId]}`。
  - `GET .../gitfab/runs/{run}`：查看。
  - `POST .../gitfab/runs/{run}/spec`，`{"decision":"approve|reject|revise"}`：Spec 确认，只有发起人可以。
  - `POST .../gitfab/runs/{run}/decision`，`{"decision":"continue|reassign|abort"}`：人工介入后的决定，只有发起人可以。
- 房间命令（附件 3：Human 请求和确认以 Matrix 事件为锚点）：在项目房间发 `/gitfab start <任务ID>`，后面每行 `repo`、`baseline`、`spec`、可选 `backend`、`project`、`coordinator`/`implementer`/`reviewer`，最后 `goal:`。这条消息成为任务 Thread 的根。在该 Thread 里发 `/gitfab approve|revise|reject` 处理 Spec，`/gitfab continue|reassign|abort` 处理人工介入，只认发起人。路由 worker 把这些消息记入 `orchestrator_commands`，不派给 Agent；编排器处理后在 Thread 里回复结果或原因。只在启用编排器时拦截。
- 配置：backend `[gitfab]` 段设置 `journal_file` 即启用；`stage_timeout_minutes` 默认 120。需要 `pasion.delegate_matrix_auth`（Agent 服务）。
  另可设 `github_token_file`、`gitcode_token_file`（0600，单个 Token）启用 PR 与 Release；自建实例用 `github_api`、`gitcode_api` 指定 API 根。
- 测试：`orchestrator_tests.rs` 用模拟房间和模拟 Agent 走真实 transport，覆盖完整流程加一次返工、超时转人工、被拒后重试一次、崩溃后日志补写。运行：`python3 scripts/test-agent-service-postgres.py --lib orchestrator`。

## 限制与风险

- 只支持未加密的项目房间，每个 Agent 都需要在该房间有绑定。
- 每个 Agent 只看到自己那次派发的正文，阶段间的上下文必须写进阶段消息里。
- 服务器发出的消息不能带 `m.mentions`，否则资源池 Agent 会被误触发。资源池的明文路径没有显式的发送者过滤。
- 事件过期、队列满、暂停、绑定或租约撤销都会取消阶段，编排器要把这些视为失败处理。

## 资源池 Agent

本机验证时发现：启用托管服务（资源接管）后，桌面端按设计永久拒绝旧版写入者，个人（本地）Agent 在该机器上不再可用，项目房间里的 Agent 只能来自资源池。资源池 Agent 走 `management_shared_work` 一路，不经过 `bindings`。

做法（纯增补，原有资源池逻辑不改）：
- 角色值可以是资源池分配 ID（`mas_…`）。`/gitfab start` 解析角色时同时列出房间里的个人 Agent 和资源池 Agent（`shared_room_agents`）。
- 派发：照常在 Thread 里发阶段消息，然后把它作为“发起流程的 Human 发出、提及该 Agent 的消息”交给原有的 `route_shared_event`。直接派发、持久化接收、需求和预算几种模式下，原有的运行时、成员、额度和队列检查全部照常生效。阶段消息本身是 `m.notice`，普通路由会忽略它，不会重复处理。
- 结果：按（分配 ID，阶段消息事件 ID）读取工作项状态和已发送的回复；还没有工作项时，读取持久化接收记录的状态（`shared_dispatch` → `shared_row`），映射成与个人 Agent 相同的结果判断。
- 限制：与个人 Agent 相同，只支持未加密的项目房间。资源池路由要求事件的加密状态与房间一致。

## Git 后端（PR 与 Release）

附件 1 要求在 GitHub 和 GitCode 上创建任务分支、创建或更新 PR、读取测试状态、生成 Release，并把链接回写任务记录。平台调用由新 crate `gitfab-forge` 完成（两个后端一套接口，模拟传输测试），另有 `gitfab-forge` 命令供验收脚本使用。

分工（已接入，提交见下）：
- Agent 只做 git：在 `agent/<project_id>/<task_id>` 分支上提交并推送，回复提交 SHA。推送用的是执行端自己的仓库凭据。
- 编排器做平台动作，用服务器保管的 Token（`[gitfab]` 新增各后端的 Token 文件路径，文件须为 0600）：
  - Execute 完成后：确认任务分支存在且头提交等于 Agent 回复的 SHA，创建或更新 PR，把 PR 链接写入阶段事件。
  - Test 阶段：除 Agent 的测试回复外，读取 PR 的平台测试状态（GitHub 为提交状态与 check-runs，GitCode 为 PR 的 pipeline_status），记入事件。
  - Review 通过后：以 PR 头提交为目标创建 Release（已存在同名 Release 则复用），把 Release 链接写入 Release 事件。
- 理由：链接由服务器从平台读回并签名记录，Agent 无法伪造；Token 不交给 Agent，也不进入阶段消息。
- 未配置 Token 时，以上平台动作跳过，并在 Thread 里说明，流程照常推进（与现状一致）。

实现：`orchestrator_git.rs`。`StageRoom::git()` 默认返回 `None`（不调用平台），`MatrixStageRoom` 持有启动时按配置加载的主机。阶段成功的判断与事件记录在同一处（`apply`），平台链接写入该阶段事件的 `artifact_refs`，说明以 `m.notice` 发在任务 Thread 里（事务 ID 固定，重试不重复）。平台调用失败只写说明，不阻断流程。阶段消息新增一行“任务分支：agent/<project_id>/<task_id>”。Release 标签默认 `gitfab-<task_id>-r<轮次>`，可用 context 的 `releaseTag` 指定。

## 部署与回退（Runner）

附件 3 §9：每个 Release 生成 OCI 镜像（按不可变 digest 验收）、SBOM（SPDX JSON）和签名部署回执；回退目标为同项目上一个可运行的 Release，回退成功条件为容器重新启动、健康检查通过、版本接口返回目标 Release、回执签名有效。部署回执由 Runner 自己的密钥签名（与 Agent 事件密钥分开）。

方案（新增 `gitfab-runner`，编排器不改）：
- 输入：日志里该项目最近一次已签名的 Release 事件（Release 链接、PR 头提交）和仓库检出目录。
- `gitfab-runner deploy`：在检出目录 `docker build`，标签为不可变的镜像 ID（`sha256:…`）；推送到 Registry 时改用 Registry 的 manifest digest。用 `anchore/syft` 生成 `sbom.spdx.json` 并计算摘要。停掉该项目的上一个容器，启动新容器，轮询健康接口，再读版本接口，确认返回本次 Release 标签。成功后写 `gitfab.release.v1` 回执（含 healthcheck、rollback 指向上一个回执），用 Runner 密钥签名，追加到回执日志，并附身份事件。
- 健康检查或版本不符：立即恢复上一个镜像并检查；两条回执都写，失败的那条标明失败原因。
- `gitfab-runner rollback`：取回执日志里上一个可运行的 Release，按同样条件部署并写回执。
- 验收：`verify-release.sh` 增加部署回执核对（回执验签、镜像 digest 与本地镜像一致、容器在运行、健康和版本接口当前可用）；`rollback.sh` 调用 rollback 并核对。
