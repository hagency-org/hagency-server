# 旧 Fleet/Operations 切除实施边界

审计日期：2026-10-07。实际仓库：`/Volumes/Data/Works/chrislearn/hagency-server`。

本文保留实施前的切除审计与步骤。切除现已完成：`backend/main.rs` 使用新 Agent Appservice 与独立 BrowserAuth，不再创建旧 `Admin/Store` 或挂载其 router；旧 Operations/Contract 产品 crate 已删除。下文的“仍调用”“先提取”等描述是实施前的依赖分析，不是当前运行状态。实施总报告保存在 `chrislearn/hagency-client/docs/design/2026-10-07-server-appservice-client-refactor.zh-CN.md`，最终复审见同目录 `2026-10-07-final-code-review.zh-CN.md`。

## 1. 必须遵守的边界

- 删除 Hagency Fleet、Hafleet 别名、Engagement、资源授予/额度审批、旧 client enrollment、authority import 的生产代码和入口；不保留旧模式开关、数据迁移或兼容 adapter。
- Agent 永久 owner、Project/Space 与独立 Room、设备认证及租约、消息队列与 outbox 只由新 `agent-service` 实现。
- Palpo/Pasion 上游依赖、数据库 schema/migrations、标准协议接口与通用管理功能保持原有能力；切除只修改 Hagency host 自己的扩展。
- 服务端不代用户审批其本地资源、每 Room/user 配额或高风险工具策略。Project/Room 创建权仍由新 domain 授权。
- 不删除已有业务数据库或 Palpo appservices/user/Room 数据以“清空旧模式”。旧 Hagency document 状态不再加载、解释或导入；删除旧代码不意味着自动删除旧 Matrix 资产。

## 2. 审计结论：不能直接删除全部 Admin 然后启动

`backend/src/admin/api.rs` 没有通用用户/Room/Space CRUD。通用前端 `api/users.rs`、`api/rooms.rs`、`api/palpo_admin.rs` 直接携带个人 Bearer 调用 `/_palpo/admin/...`；`api/pasion.rs` 调用 `/_pasion/...`。这些 API 来自现有 Matrix/Pasion router，应保留。

但是，整个前端 OAuth 回调、管理员检查及退出仍调用 `frontend/src/api/hagency.rs` 的通用 cookie 会话桥：`POST /hagency/api/login/token`、`GET /hagency/api/session`、`POST /hagency/api/logout`。`api/auth.rs::verify_admin` 和 authenticated layout 依赖它。直接删除旧 Admin 会同时破坏 Palpo/Pasion 的正常管理登录。

**实施选择：先提取不依赖旧 Store/Operations 的独立 BrowserAuth，再切除旧 Admin。** 三个通用登录接口可继续作为当前网页的正常会话接口；它们不处理 Fleet，不接受旧业务数据，不构成旧模式。后续可以统一重命名前端 auth 模块，但不得以重命名为理由保留旧业务 router。

BrowserAuth 只保留以下行为：

1. 固定本部署 Matrix/Pasion endpoint，个人 Bearer 经真实 `whoami` 验证，拒绝 guest/错误本域身份；当前管理员资格来自真实 Palpo 管理接口检查，403 是已验证普通用户，网络故障不能视为已授权。
2. HttpOnly、SameSite=Strict cookie；HTTPS Secure；有界 session TTL、过期清理、登录速率及请求大小限制。Cookie 不能用作新原生 device API 身份。
3. session GET 每次重验当前 token、身份与管理员资格，撤销/切换账号及时失效；不能仅返回缓存的 isAdmin。
4. token 切换前先验证新 token，随后通过 Pasion revocation 撤销旧 OAuth grant；保留失败处理。不能调用 Matrix logout 来替换同设备的 grant，否则可能退出新 grant 共用的设备。
5. cookie mutation 的 CSRF 与同源检查、退出清理。新 Agent 的网页/API 权限由新服务验证，不能用管理员 cookie 绕过 owner 或设备绑定。
6. Session response 只含真实通用登录字段。删除 `outboundAvailable` 和 Fleet callback/resource capabilities；若保留 serverName，应来自真实 deployment。

删除旧 `POST /api/login` 密码代理。Pasion 登录/注册及 Matrix 标准 login 路由继续由原有组件处理，不需要 Hagency 再提供密码授权流程。

## 3. 文件归属与具体切除动作

| 文件/模块 | 动作 | 保留事项/依赖 |
| --- | --- | --- |
| `backend/src/main.rs` | 删除 authority import 参数、反序列化/写旧 Store 的 early branch；删除旧 Admin/Store 初始化、通知与账号审批 worker 的启动/abort、旧 router 挂载；换成 BrowserAuth | 保留 Palpo/Pasion 初始化、配置检查、正常 bootstrap、Matrix router 与 DelegatedAdminGuard、Pasion router、健康检查、Frontend、新 agent service/AS/workers |
| `backend/src/lib.rs` | 删除旧 admin export；导出独立 browser auth | `MatrixServer`、`PasionServer` re-export 及新 agent_appservice 保留 |
| `backend/src/admin/api.rs` | 通用登录代码提取后删除整个旧 handler 与 catch-all | 不以 `/api/{**path}` 保留业务兜底；BrowserAuth 精确挂载三个方法/路径 |
| `admin/fleet.rs`、`native_client.rs`、`outbound.rs`、`workflow.rs` | 删除 | 均为旧 Hagency domain，非 Palpo 上游管理实现 |
| `admin/accounts.rs` | 删除自建加密密码/私有 Room 人工注册审批 worker 与 endpoints | Pasion 自身注册、账号管理、OAuth/个人会话管理与 bootstrap 另有实现，继续保留 |
| `admin/store.rs` | 删除旧 JSON document store wrapper | 新 `hagency_agent_v1` schema 保留；启动不得读取旧 document 或构建 Operations Store |
| `admin/upstream.rs`、`admin/mod.rs` | 必要 HTTP/auth 工具以最小实现复制到 BrowserAuth 后删除旧模块 | 不复制 `native_client::embedded_admin`、server_authority 提权 adapter、operations Error conversion、Fleet audit 字段 |
| `crates/operations` | 删除生产 crate、workspace/backend 依赖与旧测试 | 全部为 Hagency 扩展；`/_palpo/miniapp` 是此 crate 的历史别名，并非 Palpo core，不需保留 |
| `crates/hagency-contract` | 删除旧 Engagement/allocation 合约及依赖 | 当前新 agent-service/frontend 不依赖此 crate；config 中唯一 remaining MatrixUserId 校验随旧通知配置删除。若实际检查发现其他通用引用，改用原有 identifier 校验，不能留下旧 domain |
| `backend/src/config.rs` | 移除 fleet_access/hafleet_access、account_config、retirement_admin_token_file、action_notifications 及解析/验证/文件跟踪 | 队列 bounds 已用于新 AS inbox，不能全部删除；session TTL/read timeout 可供 BrowserAuth 使用；callback_origins 若只旧 Fleet 使用则删除，不影响 Pasion 独立 OAuth client redirects |
| `config/{dev,docker,examples}/hagency.toml`、文档示例 | 删除旧配置，更新新 queue/auth 说明 | 保留 listen/public_origin/data_dir、三个独立 DB、Palpo/Pasion config/signing key；deny_unknown_fields 下旧配置应明确拒绝 |
| `backend/src/frontend.rs` | 删除 legacy_account_approval_enabled runtime 标记 | Pasion availability/OAuth issuer/runtime 部署信息继续保留 |
| `frontend/src/api/hagency.rs` | 拆出纯 browser_session/auth 模块；删除 Fleet/resource request calls | auth.rs 改用独立模块；clear_session 不再触发旧 common::reset_renewal |
| `frontend/src/pages/hagency` | 删除 Fleet、旧 inbox/requests/resource approvals、旧 accounts 审批 UI；Project 页必须按新接口重写 | 如保留 Heading/RoomLink 等纯展示组件，应先抽到通用 components；不能带自动资源续约/Fleet schema |
| `frontend/src/router.rs`、sidebar、dashboard | 删除 Fleet/Hafleet/connection/id route、request-agent approval 等旧页面和重定向；加入真实新 Agent/Project 页 | 保留通用用户/Room/媒体/举报/目标/registration token/appservice/server 通知/设置及 Pasion 全套管理路由；普通用户权限 whitelist 与登录落地页必须同时更新 |
| `justfile` | 删除 import-authority；旧 check-operations 换成新 agent-service 检查 | 不留下旧代码“可选验证”入口 |
| `xtask/src/dev.rs` | 去掉旧 contract/operations rebuild inputs，增加新 agent-service | 保留正常 server/frontend 配置文件 watch |
| Cargo manifests/lock | 删除旧依赖与目录后重新生成 lock/check workspace | `members = ["crates/*", "xtask"]` 会自动包含旧目录，只删除 backend dependency 不够 |

`admin::Upstream::server_authority` 调用的 embedded appservice adapter 是旧 Fleet enrollment 的宿主扩展。删除它不应删除 Palpo 自身 appservice admin API。新 `agent_appservice` 必装注册/identity worker 必须保留，它是另一条实现路径。

## 4. 删除的 HTTP 能力清单

移除所有方法与别名，不再返回新凭据、旧审批结果或转发：

- `/_hagency/client/v1/discovery`、`identity`、`fleets`、`fleets/{id}/connect` 及 Hafleet aliases。新版本的登录/设备 API 单独提供，不能复用旧 Fleet 请求语义。
- `/api/fleet/v2/{id}/{poll,ack,updates,retire-agent}`、Hafleet alias。
- `/api/relay/v2/{id}/...` 下 transactions/users/rooms/identity 转发。
- `/api/pair/{id}`、`/api/my/fleets` 及 pair/connect；`/api/fleets` 及 install/pause/resume/revoke/agents/outbound/retire。
- `/api/operations/call`、旧 `/api/{projects,requests,catalog}`；这些 projects 是旧资源审批模型，不是新的 Project=Space。
- `/api/account-access`、`/api/account-requests` 与 status；旧 `/api/audit` document 审计读取。
- `/_hagency/miniapp/v1/{operation}` 与 `/_palpo/miniapp/v1/{operation}` 的 Operations 服务，以及其 machine/router mounts。
- 旧网页 `/hagency/fleets`、`hafleets`、`my-hagencys`、connections/fleet agent 页面及审批页面。不得把旧 Fleet 书签转成成功的隐藏入口。

重要：Frontend 静态 fallback 可能给任意路径返回 `200 text/html`。旧机器/API路径必须返回明确 JSON 404 或 410，而非 SPA 页面；没有载荷处理/身份验证路由不等于实际 HTTP 404。可以用 Host 的固定“未知 API”边界解决，但不得遮蔽 Matrix/Pasion 与新 service 已挂载路由。

## 5. 推荐原子实施顺序与并行分工

1. 宿主负责者实现 BrowserAuth 精确 router，并在测试中证明完全无旧 Store/operations 依赖；保留通用登录安全行为。
2. 前端负责者先改 auth callback/verify/logout imports，移除 reset_renewal，保持正常管理登录；接新 Agent/Project API 后移除全部旧业务 UI/route/menu。不要在路由中将普通用户强制送入只允许管理员的默认 dashboard。
3. 宿主 main/config 负责者删除旧 Admin 初始化/import/worker/router 及配置，挂载 BrowserAuth 与新 agent service，明确未知 API HTTP 边界。
4. 旧代码负责者删除 admin 旧业务文件及旧 crates；保留从旧模块提取并审核的通用 auth 小工具，不将整个 Upstream/Admin 原样搬家。
5. manifests/config/xtask/justfile 与 frontend runtime 联动清理；不保留启动开关。重建 lock，检查 backend 与 wasm/frontend，再执行负向旧入口与正向核心烟测。
6. 更新 guide、OPERATIONS、WEB_ADMIN_PARITY。既有 VALIDATION 中的历史 Fleet 证据可注明仅适用于旧实现；不能继续宣称当前支持这些 API。新增本次精确测试结果与未验证项。

并行编辑需明确文件所有权：main/lib/config/agent_appservice 属宿主；frontend auth/router/pages 属前端；admin 旧文件、旧 crates 属切除执行者。提取 BrowserAuth 完成之前，不删除 parent 正在依赖的 admin/mod/upstream/api，避免共享工作区中间态破坏正在运行的 smoke。

## 6. 验证细则与完成判据

### 构建与静态检查

- backend、frontend wasm 与 workspace check/clippy。删除旧 crate 后正常 `cargo test --workspace` 不再加载旧 Engagement/Fleet 测试。
- `rg` 检查 main/lib/config 的 `Admin::new`、旧 Store、authority import、fleet_access/hafleet_access、旧 worker 全部清除。检查新的 auth 模块不依赖 `hagency_operations`、`hagency_contract` 或旧 document schema。
- 配置测试保留：独立 DB、Palpo signing key 重启持久、Pasion issuer/client/密钥检查、拼写/未知字段拒绝。旧 action_notifications 路径测试改为旧字段拒绝；不要误删通用配置安全测试。
- CLI `--import-authority` 必须被拒绝；旧 TOML fleet/account审批/通知字段必须被拒绝，不默默忽略。

### 旧入口不可达（服务真实挂载后的 HTTP 测试）

- 逐个请求上述旧 route family 与 alias 的 GET/POST/PUT，覆盖无凭据、有效普通用户、有效管理员、旧形状 Bearer/cookie。旧入口必须 404/410；有效管理员也不能启用隐藏旧功能。
- `/api/fleets`、`/_hagency/client/v1/fleets`、`/_palpo/miniapp/v1/call` 等关键路径验证 JSON Content-Type，不能以 SPA 200、401 或 403 作为“已切除”证据。
- 删除前后不应创建旧 state document、Fleet AS registration、账户审批 Room，也不能发送旧审批通知。

### 通用功能保留（真实正向冒烟）

- Pasion 用户登录、OAuth callback/admin 额外授权、refresh、账号切换、退出及普通用户落地；管理员资格撤销后立刻拒绝管理权限。
- 管理员打开并调用 Palpo 用户列表/详情、Room 列表/成员/state/Space hierarchy、appservice 列表；普通用户仍被真实 Palpo 权限拒绝。
- Pasion accounts/session/upstream providers 管理页面及 API 可用；正常注册入口使用 Pasion，非旧 account approval。
- Matrix versions/whoami、普通消息/成员管理、原有通用 server/media/reports/notifications API 可达。新服务注册不能覆盖原有无关 appservice。
- 新 Agent 永久 owner、跨 Project binding、权限 default/deny、device rotate/revoke、租约 fencing、AS durable ACK/outbox 回归通过；不得因旧创建审批删除而绕过新创建权限。

完成条件：上述编译、旧路径负向和通用/新服务正向证据齐全，生产启动只构建新 Agent 域与必要的通用 browser auth。单纯“删除了源文件”或新 PG 测试通过不能证明全站切除成功。

## 7. 明确暂未执行事项

本轮只审核并写本文。main/lib/config/frontend/manifests 和旧 admin/crates 尚未切除；启动路径仍同时挂载新服务与旧 Admin，因此不能把当前状态描述为已移除 Fleet。后续执行者需按上述顺序完成并回填真实验证结果。

## 8. 本轮实施进度（覆盖前文审计时的运行状态）

已提取 `backend/src/browser_auth.rs`，宿主已改挂 BrowserAuth；它不构造旧 Store，不读取
旧 document。原 admin 目录、operations/contract crates 与旧 export/dependencies 已删除。
Config/main 的旧 Fleet/账号审批/authority import 已由宿主负责者切除；xtask watch 和
justfile 新检查命令已同步。前端改为独立 browser_auth 模块、新 Project/Agent 页，旧
业务页面、Fleet 别名、审批导航、密码代理与 legacy runtime 标记已经移除。

网页 BFF 为 `/hagency/api/browser/v1`，只白名单 owner Agent/Binding 操作与经过真实
Space/Room 管理权检查的 Project adoption/creation-policy 操作。Cookie+CSRF、固定
Host/同源约束先通过，随后服务器保存的个人 OAuth token 调用新 native sessions/pasion
获取独立短期授权，再调用现有 domain API，最后注销该短期 session。凭据不返回网页，
不允许 device/execution、通用反向代理或 body 指定 owner。短期授权清理任务在浏览器
请求取消后继续收敛；授权获取阶段的未知网络结果最多存续真实 Pasion 授权窗口。

backend/frontend 静态编译已通过；HTTP BrowserAuth 回归和宿主真实挂载 smoke 的最终
结果由执行者补入验证记录。独立 BrowserAuth router 的旧路径 404 测试不能替代完整宿主
路由的 404 检查，Palpo/Pasion 正向 smoke 仍必需。
