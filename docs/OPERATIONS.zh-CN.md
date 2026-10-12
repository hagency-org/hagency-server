# Hagency Agent 归属与消息处理

[English](OPERATIONS.md) · [文档目录](README.zh-CN.md)

当前实现采用必装的集成 Appservice、永久个人 owner 与本地执行。
Fleet/Hafleet、Engagement、资源 allocation 审批、旧 native enrollment、
authority import 和旧 Miniapp aliases 已从生产代码删除。不兼容、不导入旧业务结构。
具体切除审计见 [实施边界](../crates/agent-service/legacy-cutover.md)。

## 组件边界

| 组件 | 职责 |
| --- | --- |
| Palpo | 原有 Matrix 用户、Room、Space、事件、客户端/联邦协议、Appservice 与通用管理 API |
| Pasion | 个人账号、OAuth/OIDC、授权 scope、会话、注册与管理员角色 |
| `crates/agent-service` | 新 PostgreSQL 身份/设备、永久 owner、Project/Room 创建权、租约、owner 事件队列与回复 outbox |
| `crates/backend` | 同一进程/listener，必装 Appservice 注册、可信 Matrix gateway/worker、受限网页认证/BFF |
| `crates/frontend` | Palpo/Pasion 通用管理与 Project/Agent 身份、绑定、创建策略界面 |
| `chrislearn/hagency-client` | Codex 执行、每 Room/user 配额、请求用户过滤、风险工具决策和实际用量记录 |

Palpo/Pasion 的原有功能与数据库 schema/migrations 不由 Hagency 新域修改。
三个独立数据库继续使用：Hagency 新 schema 为 `hagency_agent_v1`，Palpo/Pasion
各自管理自己的 schema。旧 `hagency_admin_state` document 不再加载或迁移。

## 身份与创建权限

用户使用自己的 Matrix 账号通过 Pasion Authorization Code + PKCE 登录。
服务器经 Pasion introspection 和真实 Matrix whoami 核对 issuer/subject/client/MXID
后才建立短期用户授权；每个本地安装的设备凭据受用户 session、generation、撤销与
真实时钟约束。客户端不能提交“我是某个 owner”的 JSON 来取得权限。

Agent 创建后 owner 与 puppet MXID 永久固定，无转让接口或修改/删除归属路径。
Agent 不属于单一 Project，同一 Agent 可以有多个 Project/Room bindings。
每个 binding 独立检查权限、生命周期、generation 和 Room 范围；跨 Project
不能借用另一处创建许可或投递授权。本地上下文/预算也应按 binding 隔离。

Project 一对一登记 Matrix Space。Room 保留独立成员关系，加入 Space 不等于
自动加入所有子 Room；创建要求 owner 当前同时加入 Space 与 Room，Room 实际
关联该 Space，且集成服务具备邀请能力。现有 Space/Room 由管理者登记为 Project/Room，
普通讨论组的创建和成员管理继续使用原有 Matrix 功能。

Project 创建默认允许成员，deny 优先于 default/allow。关闭默认许可可以设置
explicit allow；Matrix Space 管理者通过真实 power/state 权限更新政策，revision
防止并发覆盖。禁止创建不等于暂停已有 Agent；管理员暂停 Project/Room 与 owner
暂停 Agent/binding 是独立状态，owner 不能绕过管理员暂停。

## Appservice 与执行

集成 Appservice 是部署必装组件，不为每个用户或本地安装另建 Fleet/Appservice。
命名空间中的 puppet 必须有真实永久 owner 映射；未知保留名称不会凭查询自动创建
ownerless 身份。普通人员的 Matrix/Pasion 管理仍走原有权限接口。

AS transaction 先持久化再 ACK，客户端离线不阻止 ACK。可信 worker 根据 canonical
事件与新鲜 Matrix facts 路由，只有明文 mention 或已经登记的 thread 跟进触发。
Room/Space/owner/requester 的成员事实与 active Agent/binding/generation 在投递和
回复时重验；重放内容不同会冲突，队列有界。当前加密 Room 的执行明确拒绝并等待
客户端 crypto 支持，不能将保存 encrypted event 说成已经解密执行。

每 Agent 同时只有一个有效设备 execution lease。epoch/device generation/session
撤销令旧授权失效。ACK 只证明客户端持久化接收，start 与完成是另外的 durable 状态。
同 execution ID 的 start 重放不应重新运行模型。过期/接管后已开始任务转为 unknown，
不会自动交给新设备重复执行；已发生的本地 tool 副作用无法由服务器强制撤回。

回复先写 durable outbox，稳定 Matrix transaction ID 和固定 payload 支持精确重试。
未知网络结果保留原 txn，不构造新回复 attempt；authority 被拒绝后禁止自动重发，
已发送事实只能用实际 canonical event 与原 txn/payload 证明。后续跨 epoch 已知结果
对账必须是显式操作，不能放松原 submit_reply fencing，也不能释放未知模型费用/工具
副作用 holds。实施情况以新 transport API/测试为准，尚未实现的操作不能宣称可用。

显式已知结果恢复已由 `/hagency/api/v1/execution/replies/reconcile-known` 实现。
它保留原 `dispatchEpoch`，通过独立 `deliveryEpoch` 记录当前授权。已有 sent receipt
返回原 event ID；pending/unknown 仍使用原 txn/payload，blocked 不可解除。
见 [transport 合同](../crates/agent-service/src/transport-README.md)。

## 网页与原生 API

网页通过 Pasion 登录。通用 BrowserAuth 只提供 `/hagency/api/login/token`、`/hagency/api/session`、
`/hagency/api/logout`；session 每次重验身份/管理员资格，保留 cookie、CSRF、同源与 OAuth
handover 的安全处理，不初始化旧业务 store。

网页 `/hagency/api/browser/v1` 是封闭 BFF，只允许 Project、Agent、binding 的管理
操作。服务器从已保存的个人 OAuth grant 获取新服务授权，操作后注销，凭据不返回
网页；不开放设备执行或通用 HTTP 转发。原生 `/hagency/api/v1` 拒绝 browser
Cookie/Origin，负责 Pasion proof、设备和执行协议。AS 使用原有 Matrix AS 路由。

网页入口为 `/hagency/projects` 和 `/hagency/agents`。本地 token 配额、模型与工具
策略不在服务端界面审批。旧 Fleet/资源审批/账号审批页面及 aliases 均已删除。

## 验证

```sh
just check-agents
just check-agents-postgres
cargo test --locked -p hagency-server --lib browser_auth::tests
cargo clippy --locked -p hagency-server --all-targets -- -D warnings
cargo check --locked -p hagency-frontend --target wasm32-unknown-unknown
```

PostgreSQL runner 创建并删除专用随机测试库，不对现有业务库运行测试迁移。
真实部署与 PKCE/Matrix 投递测试见 `scripts/test-agent-integration.py`，运行参数以该
脚本说明为准。历史 Fleet 的验证记录仅是历史证据，不代表当前兼容或支持。


离线未开始请求的期限由 `queue.event_ttl_ms` 配置，默认 24 小时，范围 1 秒至 30 天，按可信 AS 入站时间计时，路由重试不延长。过期请求不能领取、ACK 或开始新的模型/工具动作；已开始任务的已知原结果仍须核验当前权限后结算及发送，unknown 与成本保留不自动释放。设备 poll 必须指定 `bindingId`，只领取明确启动的 Room。发现所有者成员资格或关联失效、傀儡被移除后，撤销受影响 binding 并递增 generation，进入 leaving 并在确认实际退群后 left；重新加入不自动恢复，需显式重新绑定/采纳，旧 dispatch 不复活。发送还核验傀儡当前 `m.room.message` power level；已观测撤权的旧回复不会在恢复权限后自动发送。


## 成员资格丢失与重新绑定

所有者离开或 Space/Room 关系失效、Agent 被移除时，绑定递增 generation 并进入
leaving，实际 Matrix 退群确认后成为 left。后台也检查 suspended；查询失败保留待定，
不误判离开，被踢的 Agent 不自动重新 JOIN。恢复真实权限后需在确认 left 后显式
重新绑定/采纳，主人私聊可重新采纳原 Room。普通服务暂停仍可 resume，与成员资格
撤销不同；旧 dispatch 不复活，排队工作取消、已运行未知结果保留。详见
[实现记录](design/2026-10-10-agent-owner-room-membership.zh-CN.md)。
