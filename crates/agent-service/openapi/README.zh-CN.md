# 当前 HTTP 契约：protocol v1

本目录描述实际实现，不是需求草案。仓库中的 Palpo/Matrix、Pasion/OIDC、浏览器代理及 hagency-client 本机 Console API 不在这两份 OpenAPI 的范围内。

- `hagency-v1.openapi.json`：OpenAPI 3.1，契约版本 1.0.0；103 个操作，包含用户会话、设备、Project/Room 登记与权限、永久归属 Agent、Binding、命令查询、租约与执行、工具前远端事实核验、已知回复恢复及 `/readyz`。
- `appservice-v1.openapi.json`：独立的 homeserver → 必装 Appservice 信任边界；4 个操作，包含事务、ping、用户和 Room alias 查询。
- `scripts/validate-agent-openapi.py`：Python 标准库验证，不增加生产依赖。从仓库根目录执行 `python3 scripts/validate-agent-openapi.py --self-test`。

OpenAPI 中的 `x-source` 指向每个操作的真实路由条件。`x-rust-dto` 标注直接对应的 Rust 类型；生成结果保留当前序列化行为。例如 `Project.creationPolicy` 和 `Room.creationPolicy` 当前是 JSON 编码的字符串；请求的 `policy` 则是对象，不能混淆。`Option` 返回字段仍然存在且允许 `null`；私有 `ReplyIntent.worker_token` 不在 HTTP 响应中。

## 身份、权限和状态边界

`/hagency/api/v1` 面向 native HTTP 调用：必须恰好一个与配置公开 authority 相同的 Host；禁止 Origin、Cookie 和重复 Authorization。反向代理添加的 X-Forwarded-Host 被忽略，不能代替实际 Host 验证。需要 Bearer 的操作区分有限期 **用户会话 token** 与 **设备 token**，不能互换。前者用于管理自己的 Agent/Binding 及获 Matrix 管理权的作用域，后者用于租约与执行。上游 Pasion token 只作为 `sessions/pasion` 或 renew 的证明提交，不是此 API 的设备 token。首次证明的授权窗口上限为 30 秒；设备依附其用户会话，续期不能复活已撤销会话或旧设备 generation。模型不能得到上述管理凭据。

Agent 的 `ownerUserId` 只来自经过验证的会话，终身不变。没有转让、owner 修改或旧 Fleet/Engagement 导入 API。Project 是登记过的现有 Matrix Space；Room 是登记过且仍有真实父子关联的 Matrix Room，成员集合独立。创建权限同时受 Project 与 Room 策略约束，deny 优先，客户端不能上传可信成员/管理员事实。管理员的创建权限和服务暂停接口不批准用户的模型凭据、资源、token 配额或工具放行。

`GET /projects` 与 `GET /projects/{projectId}/rooms` 列表项的 `name`/`topic` 来自当前 Matrix Space/Room state；缺失时为 `null`，不把 ID 冒充名称，也不另存一份会漂移的 Project 名称。读取失败不会默认通过成员授权。

当前支持 `GET /projects/{projectId}/rooms`，只返回调用者当前同时属于 Space 和 Room 且关联仍存在的已登记 Room。`GET /projects/{projectId}/rooms/{roomId}/agents` 要求调用者当前属于该 Room 且关联仍存在，返回实际已加入 Room 的 active/suspended Agent；调用者无需额外成为 Space 成员。Space 成员身份本身不能读取私密 Room 的 Agent 名单。Room ID 需按一个 URL segment 编码。

Agent 创建与绑定提交的是**持久化接入意图**。首次通常返回 HTTP **202**、`commandState: pending`，后台 worker 执行 Matrix puppet 注册/加入及真实状态核验。HTTP 200/active 才证明绑定已激活。使用同一 owner/operation/idempotencyKey 重试，参数改变返回冲突；命令查询的 operation 是 `agent.create` 或 `agent.bind`。取消 HTTP 请求不删除已提交意图。离开和退休也依赖后台清理。管理员解除服务暂停不自动恢复 owner 暂停的绑定。

`/readyz` 在启动 AS 真实 send→receive 探针成功前返回 503，之后返回 200；这是启动 latch，不是持续健康探针。`/hagency/api/v1/readiness` 的返回码始终为 200，通过字段表达是否就绪，仍受 native-origin 校验。

## 执行、审批及回复恢复

执行租约被限定到当前 owner/device/generation/epoch，expiry 不超过用户授权 deadline。默认租约 TTL 最大 60000ms、poll batch 最大 100；底层可配置，因此文档不会把默认值当作永远固定的协议上限。JSON 请求上限 131072 字节；reply 默认文本上限 65536 UTF-8 字节。poll 的 bindingId 必填，服务端验证其 owner/Agent 归属并在 limit 之前过滤，不能让其他 Room 事件占满所选 Room 的 batch。请求 TTL 从原始 Appservice 持久接收时间开始，默认 24h、部署可配置 1s..30d，迟到 routing 不能重新开始 TTL。过期 queued/offered/acknowledged 请求被取消，不能领取、ACK 或新开始；running 请求过期后不能取得新的模型/工具执行权，但已经开始的已知原文允许在当前权限下发送或恢复，不产生新推理。

客户端先持久化收到的 dispatch，再 ACK；在 start 前持久化 executionId。`newlyStarted: false` 必须复用已有记录，不得重新调用模型或重放工具。`authorize-tool` 只核验这个正在执行的 dispatch/execution 的当前 Matrix 与设备权限，不开始新执行，不授予 token 额度，也不替代本机策略或 owner 对精确 proposal 的一次性确认。

`replies` 返回 durable outbox receipt，`pending`、`sending` 或 `unknown` 均不等于 Matrix 已送达。客户端应保留原回复和执行身份，直到 `sent` 且有 `matrixEventId`。租约换代后，只能通过 `replies/reconcile-known` 显式重新授权已知原文；请求格式仍只有 lease 和原始 SubmitReply，不能上传“可信事实”。服务端核对原 dispatch/execution、scope、binding generation、payload 和 Matrix transaction 身份及当前权限，保留原 `dispatchEpoch`，更新当前 `deliveryEpoch`。已发送历史 receipt 可保留旧 deliveryEpoch。发送前还核验 puppet 的 m.room.message 实际 power level（events_default，非 state_default）。明确观测到发言权撤销后，旧 outbox 永久阻断，unknown 网络证据保持原文/事务身份；权限后来恢复也不盲目重发。即使发送前检查通过，实际 Matrix 发送返回 403 也会由精确 sender worker 持久阻断原回复，不能当作普通 unknown 自动重试；可能存在的先前网络副作用证据仍保留。明确观测到 owner/puppet 成员或 Space→Room 关联丢失时，先独立提交 binding 暂停/generation 隔离，避免后续 API 拒绝事务回滚导致旧执行复活。deliveryBlocked、作用域撤销及仍占用的旧 sender 不会被恢复请求绕过。未知模型/工具结果不能因此重新执行，本地预算和副作用不确定性仍需保守处理。

Appservice 的 **hs_token** 与 owner/device token 是不同凭据。推荐 Authorization Bearer；兼容 Matrix 的 `access_token` query，仅在二者相同且无重复时接受。事务 ACK 只在原始入站事务及 routing intent 一起持久化后返回；重复事务内容相同幂等，内容不同 409，队列满 503 且不 ACK。这里的 ACK 不表示模型完成。AS 用户查询只确认已知 service/puppet，不创建账号；Room alias 查询目前总是 404。

## 尚未提供的能力

当前不是以下草案 API 的实现：

- `POST /hagency/api/v1/projects`：直接创建 Matrix Space。
- `POST /hagency/api/v1/projects/{projectId}/rooms`：直接创建 Matrix Room。
- `GET /hagency/api/v1/rooms`：全局 Room catalog。
- Agent E2EE 解密执行路径：加密 Room 不能按明文路径处理。

创建 Space/Room 应由使用者自己的 Matrix 授权，通过 Palpo 正常 Matrix API 完成，再调用 adopt；这里没有修改 Palpo 默认功能。Agent 转让、服务端模型凭据保管/资源审批属于明确禁止的设计，不是等待新增的 API。

## 校验的范围

验证器检查版本、operationId 唯一、路径参数、真实 literal 路由覆盖、每个动态操作的源码条件与 method、执行/管理 bearer 区分、router 源码漂移、Rust DTO 字段/类型/可空性/严格请求属性、Room 策略变体、引用可解析及文档示例。8 个负例覆盖漏路由、method/bearer 错误、私有响应字段、DTO 类型、非严格请求、无效引用和示例。

动态 path 使用源码条件锚点和已审核的 operation inventory；这是轻量静态契约检查，不是完整 Rust AST/OpenAPI/JSON Schema 实现，也不替代真实 HTTP/PG 集成测试。路由源码修改会使 hash 检查失败：应重新审核变化、更新文档和 hash，不能仅忽略漂移。两个版本化 JSON 是交付物；无需运行生成器或安装新的生产依赖。

`POST execution/history` 分页返回永久 owner/device 范围的已开始执行元数据（每页 128），包含原作用域与本地 inbox 的 immutableDigest，不返回消息原文或费用。Acquire 必须提交 historySnapshot，并与 start 共用事务锁原子检查；过期快照返回 409 且不改变旧租约。客户端仍须核对本地费用/保守预留证据，前后检查与同 owner 完整恢复；仅 tuple 覆盖不代表费用已结算。退休、暂停或 TTL 不删除已开始历史。当前服务端 owner_events 原文不可变；未来正文压缩必须先持久化原 immutableDigest。分页每页重算全历史摘要为 O(N)，当前同授权事务锁串行化，后续可在保持永久证据语义下优化。

## 实体 ID

新分配的 Hagency 实体 ID 为类型前缀加26字符小写 ULID：`usr_`、`ses_`、`dev_`、`prj_`、`agt_`、`bnd_`、`ins_`、`evt_`、`rep_`。生成器在同一服务器进程内单调递增；跨进程/重启不承诺全局严格生成顺序。Agent 傀儡账号使用 `@_hagency_agt_<ulid>:<server>`（配置 namespace 可带额外后缀）。客户端仍将 ID 作为不透明字符串，不能解析时间戳来推断权限、租约、业务时间或完成状态。

`entity_id()` 与 `secret_token()` 分开：会话/设备 bearer 和 reply worker claim 保留独立32字节随机值，不用 ULID。数据库字段和 API DTO 不变；已持久化身份及幂等请求仍引用原 ID，Matrix Room/Event ID、消息事务 ID、摘要、客户端安装标识与已开始的 execution ID 保留原生成规则。


## Agent 所有权与服务范围

Agent 的创建者保持永久归属；服务范围通过 `visibility: public | private` 独立控制。新建默认 private，仅允许创建者与 Agent 私聊；所有共享 Room 绑定（包括邀请制群聊）都要求 public。执行、投递和绑定恢复同样检查该限制。

`GET /hagency/api/v1/agents` 仍只返回本人创建的 Agent。`GET /hagency/api/v1/agents/discover` 返回最多 256 个公开服务及 `isMine`，不暴露执行设备、内部 owner ID 或创建者私聊房间。Room roster 同样返回 `isMine` 和 `visibility`，并过滤他人的 private Agent。客户端与网页端据此显示“我的”和服务范围标签。

创建者通过 `PUT /hagency/api/v1/agents/{agentId}/visibility` 修改范围。切换 private 前必须完成所有共享 Room 的退出；失败返回 `leave_shared_rooms_before_private`。private 绑定请求返回 `private_agent_owner_direct_only`。此操作不启动或停止设备模型。

升级只迁移尚无范围的旧记录：存在未完成退出的 project 绑定则 public，否则 private。已有范围不被重复迁移覆盖。旧创建命令保留原幂等摘要；更改服务范围使用独立接口。先升级 Server，再使用新 Client 的范围管理功能。

## 共享项目看板

新增 12 个 `project-board-v1` 操作使用用户会话与当前 Space 成员权限，覆盖任务、排序、归档、评论、活动与变更游标。它与 Agent 执行和本地 OperatorTask 分离。契约、可选 Server 关闭开关及跨仓库验证见 [project-board.md](../../../docs/project-board.md)。无需修改 Cloud 产品代码。


## 独立 Agent 密钥接收

`agent-crypto-owner-receive-v1` 使用单独的
`/hagency/agent-crypto/{agentId}/{generation}/receive` 命名空间，共8个操作。
原身份命名空间仍只有7个操作。两者共享同一永久 Agent/主机设备公钥与
当前会话、设备 generation、Agent generation 和执行设备分配校验。
接收入口 POST 返回以 `/receive` 结尾的 SDK homeserver；不分发 Matrix 登录 token。

新增 sync 只转发当前 Agent 设备的 to-device 加密更新。服务端固定
`timeout=0`、`set_presence=offline`，过滤所有 Room、presence 和 account-data；
调用方不能提供 filter、user_id、device_id 或 access_token。成功响应最多1MiB，
最多256个 to-device 事件，出现房间内容直接拒绝。query 仅允许本 Agent 和
永久主人；主人 master key 的签名必须匹配已经固定的 Agent user-signing key，
转发前移除其他用户的签名。它不开放发送、房间历史、key claim、备份或任意用户查询。

此接口仅提供客户端密码学接收能力，不能作为入站事件授权、ACK、租约或模型执行许可。
加密执行仍需独立完成原始密文接纳、当前成员/绑定/租约核验及加密回复链路。


## 原始加密入站队列

`agent-encrypted-inbox-v1` 提供独立的 `POST execution/ciphertexts/poll` 与
`read`，只接受当前设备 lease、binding/原 inbox ID，最多4项。逐条使用真实
Gateway成员事实并再次核对 lease、设备/Agent/绑定代次、暂停、过期和加密状态。
原 AS event 在同一事务锁下核对持久原事务后保存；路由接纳成功后才允许压缩
AS正文。摘要为排序 UTF-8 JSON `["agent-encrypted-inbox-v1",event]` 的 SHA256。

普通 Poll/附件 Poll 不返回密文。相同事件幂等，内容改变冲突；过期保留不可重启
墓碑，不能改写或删除。读取不修改执行状态，不能作为任务 ACK/准入/start/lease。
Server 不接收解密明文；任务路由、加密回复和文件还需 Client完整执行链。
标准外部 `m.relates_to` 只作为原始不可信元数据保留，不据此执行 thread/mention。

`agent-encrypted-admission-v1` 增加独立 ciphertexts/prepare、ack、admission。
仅提交原始 inboxId/sourceDigest 与解密后认证的 thread/mentions 投影；Server
保存原始密文作为历史正文，以 encrypted_* 状态隔离普通执行接口。主机须先原子
持久化本地明文、原始密文与两种历史摘要，再 ACK。重复 ACK、重启和换租约保留
原 dispatch；同一加密载荷改外层 event_id 不得创建新任务。ACK 不允许模型/工具
执行；完整加密 start/reply/file 流程仍未实现。
