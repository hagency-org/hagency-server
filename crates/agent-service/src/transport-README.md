# 新 owner transport 接口

本模块仅使用 `hagency_agent_v1` 新领域身份，既不导入 Fleet 数据，也不授予设备 Appservice 管理能力。`TransportStore::open(database_url, Limits)` 必须在认证和 DomainStore 初始化后运行；重复初始化不重装表或 trigger。

## 身份与授权边界

设备入口只接收服务端已认证的 `Principal`。每个敏感事务锁定并重验用户、会话、设备 ID、generation、撤销和真实数据库时间；只持有用户会话而没有设备身份不能执行 transport 操作。`LeaseRef` 只包含 Agent ID 和 epoch，用户不能提交 owner、傀儡发送者或 Room 范围。

`RoutedEvent` 和 `DeliveryFacts` 没有 Deserialize。它们只能来自可信 Appservice / Matrix gateway。HTTP handler 必须自己读取当前 Matrix state，确认创建者仍在 Space 和 Room、Room 仍链接该 Space、请求者仍在 Room、精确傀儡仍加入，然后构造 facts。不得从客户端 JSON 构造 facts。数据库状态仍在同一事务中再次核验，Matrix 观察最多允许 30 秒且不能用调用者的陈旧时间延长。

## 收件到回复

1. Appservice ingress 先持久化完整 transaction 并 ACK homeserver。独立路由 worker 按当前 binding 解析 canonical event，调用 `ingest_routed`。客户端离线不阻止 transaction ACK，也不阻止合法事件入 owner queue。过载使路由 worker 保留可重试意图；不能把 transaction 当作处理完后丢弃。
2. `acquire_lease` 给一个 Agent 分配一个运行设备。租约 TTL 最多 120 秒（默认上限 60 秒）且不超过认证期限。另一个仍在线的设备须显式 `takeover=true`；Agent 永远属于同一 owner。
3. 网关通过 `event_candidates` 获取内部候选元数据，再取得实时 facts，调用 `claim_event` 返回正文。候选列表不是把所有正文直接暴露给设备的依据。
4. 客户端先在本地 inbox 持久化 event，再 `acknowledge`。ACK 只表示持久收件，服务器不将它当作模型完成。
5. 客户端在任何模型或工具调用前持久化本地 execution identity，然后 `start_execution`。首次返回 `newlyStarted=true`；同一 execution ID 重放返回 false，不能据此自动再启动模型。崩溃窗口必须与本地运行器状态核对。
6. 本地策略拒绝、执行失败或用量/工具结果不明时，用 `finish_without_reply` 记录 `rejected`、`failed` 或 `unknown`。它不会产生回复。客户端预算由本地账本按真实已知/未知状态处理，服务器不控制额度。
7. `submit_reply` 仅允许相同 owner、device、epoch、dispatch、execution、binding generation 和当前 Room 授权；正文并不包含可选 Room、发送者或 owner。事务中同时完成 dispatch 并持久化 reply outbox。重复相同内容返回相同 intent，改变内容返回 conflict。
8. 可信 sender worker 从 `reply_candidates` 取内部候选，重新读取 facts，然后 `claim_reply` 持久化发送意图，再通过既有 Matrix API 按 intent 的精确 puppet、Room、thread 和 `matrix_txn_id` 发送。HTTP 成功后 `confirm_reply(event_id)`；超时或不明确则 `confirm_reply(None)`。
9. 不明确的网络发送只重试原 intent 和原 Matrix transaction ID，绝不重新调用模型。worker nonce 防止旧 worker 把新尝试的结果写错。worker token 不在用户 JSON 响应中序列化。

## 接管、撤销与未知结果

租约更换或释放后，未开始执行的 offered/acknowledged 事件可以重新 pending；已经 running 的事件变为 unknown，数据库 trigger 禁止 unknown 重新转为 pending/running。未知工具副作用需要后续明确人工核对，不能自动重执行。已提交而未发送的 pending 回复被取消，不能用旧 epoch 继续发送。

客户端断网时，服务器不能物理终止该机器已开始的模型请求或高风险工具。设备撤销、账号退出、租约 epoch 改变以及 binding generation 改变阻止后续领取、ACK、开始或提交回复。已发出的 Matrix 网络请求可能仍完成，因此 sender 的真实成功结果可以在撤销后记录；不能把撤销等同于远程删除既发生的副作用。

创建禁令不参与运行检查。实际暂停、离房、退役、账号停用、Project/Room 停用和 binding generation 变更参与运行检查；跨 Project 的绑定使用独立 generation，单一 Project 的暂停不会改变其他合法 Room 的执行范围。

## 首期边界

只接收明文文本和 canonical Matrix mention / 已登记线程跟进；编辑事件不重新触发执行，普通消息、未知线程和傀儡发送者不触发。加密事件和加密 Room 明确返回 `encrypted_room_requires_client_crypto`。现有 crypto 复用尚需单独完成，不能宣称此 transport 支持加密 Room。

这些方法是服务端库接口。`ingest_routed`、`reply_candidates`、`claim_reply`、`confirm_reply` 为可信 worker 专用，绝不能作为无认证或普通用户可调用的 REST endpoints。

## 拒绝与恢复的收敛

只在新鲜且精确匹配 canonical owner/requester/puppet/Room/Space 的事实证明权限已失效时，网关调用 `reject_event_delivery` 取消未执行事件、封存运行中事件为 unknown。这样永久拒绝的首批事件不会饿死其他 Project 的合法请求。

sender 对确定不可发送的 intent 调用 `block_reply_delivery`：未发送的 pending 取消；unknown/sending 保留原状态、固定 transaction ID 和正文，同时永久标记 `delivery_blocked`，不会在成员重新加入后自动发送旧回复。`reconcile_reply_sent` 仅接受可信 Matrix 查询的精确 sender、Room、thread、正文、event ID 和原 transaction ID 证据，记录既发生的历史发送，不发新消息，也不解除 blocked。只有相同正文而没有 transaction 对应证据时必须继续 unknown。

每轮可信 maintenance 调用 `invalidate_stale_dispatches`，清理数据库已知的 scope、generation、会话或设备失效。`reply_candidates` 过滤当前无效 scope 与 lease；确定的 Matrix 成员权限拒绝使用上述事实接口单独收敛。暂时无法读取 Matrix state 时保留可重试意图，不把超时当作永久拒绝。

可信生命周期使用 DomainStore 的 `cleanup_scopes` 定位 leaving/revoked 或可最终退役的记录，按精确 generation 和真实 Matrix departure 依次调用 `confirm_left_trusted`、`confirm_retired_trusted`。退出或停用用户不阻止清理收敛，审计仍记录原永久 owner，不能临时制造普通用户 Principal。Matrix provisioning 的在途 join 无法被数据库 generation 物理取消；旧操作被 fence 后必须补偿 leave 并核对实际成员状态，不确定结果需保留恢复意图。

## 显式恢复已知旧结果

`POST /hagency/api/v1/execution/replies/reconcile-known` 使用当前有效设备授权，body
为 `{lease:{agentId,epoch},reply:{dispatchId,executionId,body}}`；未知字段拒绝。
HTTP gateway 从 owner-checked dispatch 元数据取得真实 requester/Room，重新观察 facts，
调用 `reconcile_known_reply`。不能从 DTO 提交 owner、Room、sender 或 Matrix facts。

它只恢复客户端已经持久化的已知结果，不重启旧 execution。原 dispatch/execution ID、
binding generation、sender/Room/thread、正文摘要和 Matrix transaction ID 固定。
`ReplyIntent.dispatchEpoch` 始终是原执行 epoch；新增 `deliveryEpoch` 记录当前显式
发送授权。普通 `submit_reply` 的 original device/epoch fencing 完全保留。

- 原 execution 已 unknown 且没有 outbox 时，可建立该原执行的固定回复 intent；
  execution 仍保持 unknown，不据此解除模型用量或 tool 副作用 holds。
- 因旧 lease 失效取消的**确定尚未发送** intent 只在更高当前 epoch、没有 worker token、
  当前 Room scope 有效且未 delivery_blocked 时恢复 pending；ID/txn/payload 不变。
- unknown/sending 的历史网络意图保留同 txn/payload；活跃 worker 仍有 lease 时返回
  conflict，过期发送恢复 unknown，绝不创建第二 attempt。
- `delivery_blocked` 永久保持，成员重新加入也不能绕过。binding generation 改变、
  user/device/session/lease 无效、requester 退出或 Room facts 过期均拒绝。
- 已 sent 返回原 event ID 和历史 receipt。它的 deliveryEpoch 可能是历史授权，不能
  宣称这次重新发送；返回前仍核对当前 owner/device/lease 与 Matrix facts。

客户端必须明确持有同 dispatch/execution 的 durable reply_ready，检查 receipt 的
原 identity/payload/txn，并对未发送 intent 核对 deliveryEpoch 等于当前租约。没有
已知结果的 unknown execution 不能靠该接口自动重新运行或结算。操作审计仍归于
永久 owner。旧开发版缺少 delivery_epoch 的 schema 会在 open 时明确拒绝，不做迁移。

## Owner-direct context and reply shape

For newly routed owner-direct plaintext messages without an explicit Matrix thread,
`threadRoot` is the exact Room ID (`!…`): it is a durable logical conversation key,
not a Matrix event ID. This shares one local conversation within that binding and
requester scope. The worker emits a flat `m.text` reply without `m.relates_to` only
when the trusted binding is owner-direct and the stored root equals its Room ID.
Explicit threads and Project requests keep event roots (`$…`) and threaded replies.
Old event-root queue/outbox entries remain unchanged, including their stable send
transaction and content. Unknown-send recovery retains this original reply shape;
a trusted flat-send observation uses the persisted Room context as its root.

New server-allocated entity IDs use lowercase monotonic ULIDs with type prefixes. Dispatch and reply entity IDs follow `evt_<ulid>` and `rep_<ulid>`; Matrix IDs, payload digests and immutable Matrix transaction IDs retain their existing semantics. Session/device bearer tokens and worker claims continue to use independent 256-bit random values. Within-process ID order is not a distributed execution or commit order.

## Agent execution device (domain schema 3 / discovery protocol 3)

An Agent has one nullable `executionDeviceId`; each device may execute several
Agents. New identity creation requires an authenticated device bearer and assigns
that current device automatically. No caller-specified device, instance ID or
instance name is accepted. `PUT /agents/{id}/execution-device` accepts only
`{expectedGeneration}` and uses the same current device proof. Agent generation
is its CAS version. Same-device saves leave generation and lease unchanged;
different-device saves increment generation, expire the lease, preserve running
executions as unknown and cancel definitely unsent pending replies. Unknown or
already issued sends retain their original transaction and historical evidence.
All starts, tools, completions, reconciliations and reply-worker claims check the
assigned device/current lease. A request already issued to Matrix cannot be undone.

Version 2 deployments are rejected at startup; there is no automatic compatibility
handler or migration. A separately approved, offline local maintenance conversion
is documented in `../execution-device-cutover.sql`; production restore procedures
must back up all databases and private keys before changing a deployed schema.

## Processing-start reactions

`POST /hagency/api/v1/execution/events/processing` uses a device bearer and strict
`{lease,dispatchId,executionId}` (the Start DTO), returning
`{processing:{id,dispatchId,executionId,state,matrixEventId}}`. Call only after the
provider accepted the actual model turn. Enqueue requires an exact running
execution, the currently assigned device, its original live lease, unchanged
binding generation, and fresh Matrix owner/requester/puppet authority.

The independent `processing_outbox` permanently fixes a single `m.reaction`
`m.annotation` with key 👀 to the dispatch's original Matrix event, never its
logical thread context. No caller room/event/key/content is accepted. It uses
one deterministic transaction ID per dispatch. Same-attempt retries are durable;
an already completed attempt may return an existing receipt but cannot enqueue
one. A short completed turn may deliver its already queued start notification
while the original lease and permissions remain valid. Unknown/cancelled
executions cannot deliver. Lost device/lease/scope cannot authorize a new send.

Unknown network effects preserve original content/transaction for idempotent
retry under the same current authority; permission denial permanently blocks
retry. A previously issued network effect cannot be undone; its exact worker may
record the historical outcome after authority loss. Model/tool work is never
repeated to produce this notification, and reply outbox semantics are unchanged.

Schema 4 strictly rejects earlier domain versions. The standalone offline
`../processing-reaction-cutover.sql` creates only the new processing table,
indexes, guards/view and advances the exact version-3 marker in one transaction.
It never resets identities, execution devices, generations, leases or messages.
Do not run it against a live deployment; first stop its sole server and back up.
