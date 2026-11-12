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
- 测试：`orchestrator_tests.rs` 用模拟房间和模拟 Agent 走真实 transport，覆盖完整流程加一次返工、超时转人工、被拒后重试一次、崩溃后日志补写。运行：`python3 scripts/test-agent-service-postgres.py --lib orchestrator`。

## 限制与风险

- 只支持未加密的项目房间，每个 Agent 都需要在该房间有绑定。
- 每个 Agent 只看到自己那次派发的正文，阶段间的上下文必须写进阶段消息里。
- 服务器发出的消息不能带 `m.mentions`，否则资源池 Agent 会被误触发。资源池的明文路径没有显式的发送者过滤。
- 事件过期、队列满、暂停、绑定或租约撤销都会取消阶段，编排器要把这些视为失败处理。

## 待做：资源池 Agent

本机验证时发现：启用托管服务（资源接管）后，桌面端按设计永久拒绝旧版写入者，个人（本地）Agent 在该机器上不再可用，项目房间里的 Agent 只能来自资源池。资源池 Agent 走 `management_shared_work` 一路（`route_shared_event`、运行时、分配、房间成员核验、持久化接收），不经过 `bindings`。

计划：新增与 `ingest_orchestrated` 对应的资源池准入，只接受登记过的阶段消息，以发起流程的 Human 为请求者写入一条 `management_shared_work`，复用原有的运行时、成员和队列检查；结果从资源池工作项的终态和回复读取。`bindings` 中的角色值改为可以指向资源池分配。原有资源池派发逻辑不改。
