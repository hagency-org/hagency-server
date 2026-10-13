# Desktop issue #2：执行设备环境发现与跨设备配置

状态：最终实施方案，已独立复审并纳入纠正项，可开发。来源：https://github.com/hagency-org/hagency-desktop/issues/2

## 目标与边界

主人在 B 查看、设置分配给 A 的 Agent；provider、模型、工作目录和本地资源政策以 A 为准。保持永久主人及 executionDeviceId，不因设置而接管设备，不自动启动模型执行。凭据、登录 URL、OAuth token、原始 provider 配置和任意命令均不得进入远端响应。

server、desktop 均在 codex/issue-2-cross-device-config 新分支；共享 client native owner 引擎在同名配套分支。直接修改三个 sibling 源仓库并从各自路径验证。此源码 checkout 没有 provisioned task-writer，不以文档代替控制平面状态。

## 通道与鉴权

新增 server 的 execution-configuration-v1 capability。复用当前设备 bearer 与 DomainStore 主人/设备授权，增加独立配置队列，不复用模型执行租约/Room 派发。所有读写都必须是当前未撤销的主人设备；Agent 必须属于当前主人。任务固定 agentId、executionDeviceId、Agent generation、目标 device generation、发起设备与幂等键。设备调整/撤销/身份退役后旧请求失效，不能将结果当作当前配置。

路由（/hagency/api/v1 前缀）：

- GET agents/{agent}/configuration：目标设备、在线状态、最近观察时间；过期观察显式标为离线/缓存，不返回本机替代数据。
- POST agents/{agent}/configuration/requests：{idempotencyKey,expectedGeneration,operation,input}。仅允许下述操作；服务器选择目标设备，不允许请求伪造目标。相同幂等键/内容返回同一请求，不同内容冲突。
- GET agents/{agent}/configuration/requests/{id}：主人设备读取同一请求的 pending/claimed/succeeded/failed/unknown/expired 状态与非敏感结果。
- POST device-configuration/poll：输入 {acceptWork:bool}，false 仅心跳不领取；当前设备心跳并原子领取最多一条属于其主人、当前分配与版本的请求；领取之后不自动重发。
- POST device-configuration/requests/{id}/complete：仅原领取目标设备和原版本可确认结果；相同最终结果重试幂等，结果不同拒绝。

心跳约 3 秒，在线窗口 15 秒；请求 60 秒有效。领取后过期按 unknown 呈现，不能声称未应用，也不能自动重新执行。pending 过期为 expired；离线不接受新的写入。请求/结果 JSON 上限 64 KiB，队列最多 owner/device 16 个 outstanding，每 Agent 最多一个未完成 mutation，每 device 最多一个 claimed；幂等原请求查找优先于离线/配额检查。配置结果、工作目录属于主人私有信息，日志仅记录稳定错误码。

## 支持的操作

operation 固定为 environment、agent_model、save_agent_model、agent_policy、save_agent_policy、prepare_workspace、provider_status、provider_models、provider_connect。不转送登录、注销、任意文件访问、命令行、运行启停、审批或设备接管。provider 操作 input 中只有 runtime（codex/claude/octos）；模型/政策操作只使用现有 native owner 的严格本地输入契约。未知字段/操作拒绝。

environment 返回 A 的可用 runtime 安装/版本信息、provider 支持的非秘密 readiness 和配置选项、该 Agent 已保存的模型/工作目录/政策摘要。Codex/Claude/Octos 各自不支持的能力显式说明；不得凭工具安装声称已登录。对 provider 原始输出做字段白名单投影；server 同样校验 result schema。只保留必要非秘密 opaque credential reference，禁止 authUrl/凭据管理细节。安装与版本通过 A 的可信 resolver 和有界探测取得，Octos profile 不等于已经验证登录。

## A 的应用与持久化

client NativeOwner 增加随已授权 owner 会话运行的配置 worker，即使 Agent 未启用服务也能工作。任务执行前重新核对当前身份、设备、Agent 归属/分配/generation；复用已有 Provider/Local/PrepareAgentWorkspace 入口校验和保存，确保后续执行读取同一 ledger/profile。同一 NativeOwner 串行处理；提取持 calls 锁的内部 dispatch，worker 不递归调用 public execute；独立 heartbeat acceptWork=false 不被慢 provider 阻塞。使用明确的运行冲突约束：remote provider_connect 在任何 owner profile 服务/执行活动时拒绝，不隐式 stop_profile。保存模型前按 A 的当前 catalog 重新验证 model、credential_ref 与 reasoning_effort 组合。保存成功仅指 A 的原持久化 API 成功，server 入队不算成功。

领取不重发；完成响应丢失只能重试原结果确认，不能重新调用保存。worker 在本地保留待确认结果（原请求和身份 pin），重启不重放已领取工作；无法确认的历史操作保留 unknown。退出登录/切换账号/关闭引擎停止 worker，所有结果按原身份隔离。provider credentials 只由目标设备已有 provider 管理。

## B 的呈现

复用现有模型/资源和政策表单，新增远端执行设备说明、在线/离线、检查中、目标发现时间和应用结果。允许他机 Agent 打开模型和政策 tab；本地 runtime 启停和审批仍仅本机。远端表单只使用 A 返回的 profile/model/policy；隐藏目标需要本地交互的登录/注销入口。工作目录明确为执行设备路径。请求完成只在 identity、agent、generation、UI request fence 全部匹配时更新页面。离线/不支持新 capability 时关闭远端提交并给出原因，刷新不能覆盖未完成 mutation 的错误。

## 多 agent 分工与验收

保存方案后先独立 agent 复审并记录，修正阻塞项，然后并行分配 server 协议/数据库/API、client worker/本地应用、desktop UI/翻译/验证，文件所有权按仓库隔离。主 agent 负责契约对齐、跨仓库回归和最终独立复审。

跨仓库精确 JSON envelope、operation/input/result 白名单及 NativeOwner token-free wrapper 见同目录 [独立复审记录](2026-10-13-cross-device-agent-configuration-review.zh-CN.md)，作为本方案协议附件。client 先 commit，Desktop CI pin 和必要 Cargo.lock 必须更新到此 client SHA。

执行前及确认前均重新鉴权；server 与本地持久化不是同一事务，若本地写入期间发生撤销/重分配只能保持 unknown，不声称已撤销实际写入。model 保持现有 last applied wins；policy 保留 expectedRevision CAS。

必须验证：A/B 独立 owner 状态目录、B 无工具情况下发现 A；A 校验拒绝 B 本机路径；A 保存后重开读到实际 provider/model/workspace/policy；后续执行的 profile resolver 使用已保存值；主人隔离、未授权/撤销设备、重分配/generation fence、并发请求、幂等冲突、完成响应丢失、领取后断线 unknown；无凭据/原始 provider 输出泄漏；旧 server capability 明确拒绝。

执行 server 隔离 PostgreSQL+HTTP 测试、OpenAPI validator、fmt/strict Clippy；client focused owner/provider/runtime tests 与严格检查；desktop focused UI/路由/翻译测试、全库 tests、locked build。真实跨物理设备、原生可视检查、真实 provider 推理分别记录证据，未执行不得当作通过。现有 ignored 场景保持原 verdict。
