# Hagency Agent 归属与消息处理

## 资源池 Agent 流程（schema 24）

新版 Desktop 通过 `resource-pool-agents-v1` 发现此流程。新 Agent 直接绑定资源池，不创建项目、项目容量分配或 Matrix Space。旧项目及其原始回执保留，供历史查询和清理使用；管理 API 不再接受新项目申请。

1. 资源 owner 在 Desktop 连接执行设备、建立资源配置并提交资源准入。资源获准接入后，owner 核对并发布容量授权，将可申请 Agent 的 Matrix 账号列入“Agent 申请者”。发布额度需要 owner 在原生确认窗口确认。
2. 申请者打开 **Hagency → 资源池**，可以分页查看不同 owner 提供的资源池。选择一个已向自己开放的资源池，填写 Agent 名称、说明、能力和额度，提交申请。
3. Desktop 使用申请者当前 Matrix 登录创建独立私聊并邀请 Hagency appservice，随后提交申请。私聊创建和申请分别保存原始命令；网络结果不明确时使用“继续已保存的申请”恢复，不重复创建。
4. 所选资源 owner 在 **我的 Agent 与审批** 或 **待办** 中批准或拒绝。服务器管理员不能代替 owner 批准。批准后，原执行设备以原生账本确认余额并预留容量；批准本身不等于容量预留成功。
5. 服务器核验私聊成员和访问规则，创建并加入 Agent 身份；设备核验运行时。申请者可在 Agent 详情查看进度、调整自动回复开关，并在同名私聊中向 Agent 发消息。
6. 申请者可移除自己的 Agent；资源 owner 可停止供给。停止会阻止新任务并启动清理，已消耗及尚未明确的用量继续保留，不能从界面推定已退款。

每个 Agent 使用一个选定的资源池。资源池列表展示已发布额度；实时可用余额由执行设备确认，不能把发布额度当作当前剩余量。新的容量授权不再有“批准自己管理的项目”选项。

验证至少覆盖两位资源 owner、一个普通申请者和一个管理员：跨 owner 可见性、只有选定 owner 能审批、请求重试不重复、余额不足拒绝、私聊执行及下一次任务、申请者移除与 owner 停供、断线恢复和旧数据升级。不要为测试发布真实资源额度；使用单独测试设备、账号和账本。


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

管理 schema 22 起，项目经理与项目管理员永久合并为一个**项目经理**角色。
项目获批时自动将项目权限赋予该项目经理。由同一人设置并验证 Matrix 工作区、
申请 Agent、确认 Agent 准入与预算追加，以及移除 Agent。确认操作仍然保留，
无需另一个项目管理员账号或指定步骤。资源所有者同时负责容量协调；服务器全局管理员继续负责准入审批。

升级时，原有独立项目权限合并到项目经理，同时保留旧权限记录及不可变的命令、
审批历史。权限发生变化的项目需由经理更新设置条件，并在原始 Matrix 房间中
重新验证成员权限；依赖旧权限的执行会被停止授权，未解决的容量占用仍需等待
清理完成。旧 API 中名为 `administrator` 的字段表示项目经理，指定其他人会被
拒绝。会话能力声明包含 `project-manager-authority-v1`。

管理 schema 23 起，资源所有者自动成为容量协调者，资源登记和原生容量核对
不再提供独立协调员选择。资源所有者批准项目容量预留，项目经理批准预留内的
Agent。若所有者同时是项目经理，仍需遵守容量授权中明确的自行审批设置。

升级保留原始容量凭据、已接受的执行命令与用量历史，把当前资源权限及分配
审批人设为资源所有者，并保存不可变的迁移审计记录。待办通知转交所有者。
所有者可继续使用历史委托凭据，不修改原始摘要或重置容量。会话能力包含
`resource-owner-coordinator-v1`；无需单独的协调员账号。旧通信字段名称继续
保留，以支持已保存命令的恢复。

启用项目容量预留时，会话在第一笔授权产生之前就声明
`admission-bound-grants-v2`。Desktop 据此核对并发布 version 2 原生容量凭据，
绑定资源当前已批准的 `admissionRequestId`；发布者必须是已登记的所有者执行设备。
审批或执行设备不匹配时拒绝发布。同一准入审批的授权撤销后不能另发一笔授权，
原命令重试则恢复已有结果。version 1 历史凭据保持原始序列化和摘要。
第一笔有效授权发布后，服务器会为符合条件的已获批项目生成容量审批，进入资源所有者待办。

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
