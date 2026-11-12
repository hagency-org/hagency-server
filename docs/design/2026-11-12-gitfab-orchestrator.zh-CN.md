# GitFab 流程编排器设计（S2）

状态：设计稿，未实现。依据 2026-11-12 对 `crates/agent-service` 派发链路的只读梳理。

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

- `crates/agent-service/src/orchestrator.rs`、`orchestrator_schema.sql`、`orchestrator_tests.rs`。
- worker 在 `workers.rs` 里注册，配置开关在 backend 的 `[gitfab]` 段。
- 数据库访问沿用 Horme + diesel-async，以及启动时的 `batch_execute(include_str!(...))` 建表方式。
- 测试沿用 `scripts/test-agent-service-postgres.py`。

## 限制与风险

- 只支持未加密的项目房间，每个 Agent 都需要在该房间有绑定。
- 每个 Agent 只看到自己那次派发的正文，阶段间的上下文必须写进阶段消息里。
- 服务器发出的消息不能带 `m.mentions`，否则资源池 Agent 会被误触发。资源池的明文路径没有显式的发送者过滤。
- 事件过期、队列满、暂停、绑定或租约撤销都会取消阶段，编排器要把这些视为失败处理。
