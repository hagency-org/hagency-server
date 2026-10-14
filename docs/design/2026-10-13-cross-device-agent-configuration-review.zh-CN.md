# Desktop issue #2 实施方案独立复审

日期：2026-10-13。复审范围：issue 原文、已保存方案，以及 server DomainStore/API、client NativeOwner/owned_agents/provider/agent-local ledger、Desktop backend/UI/CI dependency。

结论：总体架构可行；在下列纠正项写入最终方案后，可进入分仓库并行实现。复审是设计审查，不代表实现或验收已经通过。

## 必须修正的具体问题

1. **不能把已有 save API 当作充分的目标能力验证。** `owned_agents.rs` 只 canonicalize 工作目录；`hagency-agent-local/src/lib.rs::write_model_profile` 只检查 reference 前缀、通用 effort 枚举及绝对路径。A 必须重新读取对应 runtime 的当前 catalog/readiness，确认 model、credential_ref 与 reasoning_effort 的组合属于 A 的 catalog，然后保存。不能凭 B 提交的合法前缀 reference 证明它可用。
2. **provider_connect 是主人级关联，不能隐式停止其他 Agent。** Codex `OwnerProvider::UseLocal` 现会 `stop_profile`，外部 runtime 使用主人级 consent 文件。远端 connect 必须先检查该 owner profile 下活动服务/执行，存在冲突则返回 `configuration_runtime_busy`；在同一控制锁内检查和关联，不能通过远端 connect 触发停机。UI 明示关联作用于执行设备的该主人 provider。登录、断开、注销不远传。
3. **worker 不可递归持锁，也不可令心跳依赖慢 provider。** `NativeOwner::execute` 持 `calls` Mutex；worker 在同一锁内调 public execute 会死锁。应提取共享的已授权内部 dispatch，所有本地与远端配置使用相同序列化锁。独立心跳 poll `{acceptWork:false}`；仅空闲 worker 用 true 领取。启动必须覆盖已授权但未启用 Agent 服务的 NativeOwner，退出停止并 drain。
4. **凭据投影不能直接转发 ProviderStatus。** 现响应含 authUrl、credentialStore 等管理字段。远端只允许下节定义字段，model/policy 也逐字段投影。`credentialRef`/`credential_ref` 是非秘密的本地 opaque reference，为 Octos 同模型不同 profile 的准确选择保留；不得包含原始配置、token、账户或登录 URL。服务端同样校验 whitelist，不可信 A 不得借 complete 注入任意 JSON。
5. **缓存、过期、幂等必须固定语义。** 缓存绑定 Agent generation 与目标 device generation；心跳时间和观察时间分开。过期不能重发已领取请求。幂等检查要先于离线/配额检查，使完成响应丢失后的同内容重试能取回原请求。每 Agent 最多一个未完成 mutation，第二个不同键写请求返回 configuration_request_busy；device 最多一个 claimed，读操作按 FIFO；Agent 全部操作保持串行，policy 保持现有 expectedRevision CAS，model 保持显式 last applied wins。
6. **无法承诺跨 DB 与本地文件的原子撤销。** A 执行前与发布结果前重新鉴权及核对版本；若在本地写入期间设备被撤销/Agent 重分配，server 必须拒绝陈旧确认并呈现 unknown，不能声称实际本地写入被撤销。后续运行仍通过正常的实时 assignment gates。
7. **Desktop CI 必须使用新 client。** `.github/actions/setup-hagency-client/action.yml` 固定旧 SHA，Cargo path dependency 指 sibling client；client commit 完成后更新 pin 与必要 Cargo.lock，不能只在本地 sibling 上编译通过。
8. **provider 支持信息需诚实。** 安装与版本采用 A 的安全 executable resolver 与有界 `--version` 探测；不回传原始 stdout/stderr/完整 binary 路径。不支持的版本/readiness/catalog 显式 null/unsupported。Octos 存在 profile 只证明可配置，不证明已验证 provider 登录，保留 accountVerified=false。

## 冻结的跨仓库 JSON 协议

API 前缀 `/hagency/api/v1`；capability `execution-configuration-v1`。字段默认 camelCase，唯一例外是已有 ModelProfile 内部 snake_case 和现有 Policy 字段，保持兼容。所有 input/envelope deny unknown fields，ID/reference/string/array 都有上限。所有接口强制 device-authorized principal，不能仅调用允许 session principal 的 `DomainStore::authorize`。

`GET agents/{agent}/configuration` 返回：

```json
{"configuration":{"agentId":"agt_A","agentGeneration":2,"executionDeviceId":"dev_A","deviceGeneration":1,"online":true,"lastSeenAtMs":1,"observedAtMs":null,"cached":false,"environment":null}}
```

无有效目标的 deviceGeneration/lastSeenAtMs 为 null、online=false。environment 仅保留 environment operation 的最后一次成功投影；observedAtMs 是 server 接收成功观察的时间，不是 A 传来的时间。旧 generation 缓存不返回；offline=true 对应 online=false 时缓存必须 cached=true。GET 不执行 provider。

`POST agents/{agent}/configuration/requests` 输入：

```json
{"idempotencyKey":"unique-request","expectedGeneration":2,"operation":"agent_model","input":{}}
```

返回 `{ "request": Request }`；GET 同一路径 `.../requests/{id}` 返回相同 envelope。Request 的精确字段：

```json
{"id":"cfg_X","agentId":"agt_A","agentGeneration":2,"executionDeviceId":"dev_A","deviceGeneration":1,"operation":"agent_model","input":{},"state":"pending","createdAtMs":1,"expiresAtMs":60001,"claimedAtMs":null,"completedAtMs":null,"result":null,"errorCode":null}
```

state 仅 pending/claimed/succeeded/failed/unknown/expired。errorCode 仅静态稳定错误码。幂等键作用域为主人+发起 device+agent；digest 覆盖 expectedGeneration、operation、规范化 input。相同键不同 digest 返回 409 `configuration_idempotency_conflict`。创建固定 owner、发起 device generation、目标 device generation；任何一方撤销/版本变化或 Agent 退役使旧请求不能领取或完成。

`POST device-configuration/poll` 输入 `{ "acceptWork": true }`，返回 `{ "request": Request|null }`。每次更新当前授权设备心跳；false 绝不领取，true 原子领取一条最早 pending 并转 claimed。已领取工作永不再发，包括 poll 响应丢失。请求状态/时限在读、poll、complete 中统一计算，不能依赖定时清理才失效。

`POST device-configuration/requests/{id}/complete` 输入严格二选一：

```json
{"state":"succeeded","result":{},"errorCode":null}
```

```json
{"state":"failed","result":null,"errorCode":"invalid_local_policy"}
```

返回 `{ "request": Request }`。只允许原 claim device/generation、当前 owner/agent pins；终态同内容重复确认幂等，异内容 409。超过 expiresAtMs 的未完成 claimed 保持 unknown，晚到 complete 返回 409 `configuration_request_expired`；worker 不能重新应用。领取前 pending 超时为 expired。所有查询仍重新验证 owner 和 device；陈旧 assignment 的结果不能更新缓存或 UI。

operation/input/result 对照：

| operation | input | result |
|---|---|---|
| environment | `{}` | `Environment` |
| agent_model / agent_policy | `{}` | `AgentConfiguration` |
| save_agent_model | `{"profile":ModelProfile}` | `AgentConfiguration` |
| save_agent_policy | `{"expectedRevision":n,"policy":Policy}` | `AgentConfiguration` |
| prepare_workspace | `{}` | `{"agentId":id,"workspace":path}` |
| provider_status / provider_models / provider_connect | `{"runtime":"codex"}` | `Provider` |

ModelProfile 精确字段 `{model,credential_ref,workspace_root,reasoning_effort}`；read 可 null，write 不可 null。A 从 credential reference 解析 runtime 并验证当前 catalog；空 workspace_root 使用现有私有默认目录。AgentConfiguration 精确字段 `{agentId,policy,usage,modelProfile}`，PolicyVersion/usage 复用已有 typed contract。providerModels 与保存后的 profile 使用同一 reference，不能跨 runtime 替换。

Environment 精确字段 `{providers:[Provider],configuration:AgentConfiguration}`。Provider 只允许 `{runtime,installed,version,versionSupported,state,authenticated,accountVerified,credentialRef,credentialSource,shared,strictTokenCap,nativeTools,models,defaultModel}`；version/defaultModel/credentialRef 可 null，models 可 null 或长度至多 100 的投影数组。state 是 ready/disconnected/unavailable/unsupported；installed 与 readiness 分开。Model 项只允许现有 `{id,model,displayName,description,isDefault,defaultReasoningEffort,supportedReasoningEfforts,credentialRef?}`；Effort 项只有 `{reasoningEffort,description}`。任意 provider 原错误文本均转为稳定 code；限长字段仍须通过 provider parser 的安全验证。server 拒绝非法 result envelope，而不是盲存 worker 的 Value。

建议统一硬上限：JSON body/result 64 KiB，idempotencyKey 128 bytes，owner/device outstanding queue 16，所有 terminal 记录有有界保留/清理策略；不允许用户通过重复 GET 导致无限创建。在线窗口 15 秒、心跳 3 秒、请求 60 秒。worker 每次最多一条，各 provider 探测有单独 timeout；长操作不能阻塞在线心跳。

## 验收补充

已有方案的 A/B 独立状态目录、授权隔离、无秘密泄漏、实际 ledger/reopen/后续 resolver 验证均保留。另加：伪造 credential reference 或 A catalog 无此 model/effort 拒绝；两模型同名但 Octos reference 不同能精确选择；worker 未开服务仍发现；活动服务时 connect 不会停止任何 Agent；心跳期间慢 provider 仍在线；初次 GET 无环境时 UI 会自动请求 environment 并轮询；旧缓存 generation 不显示；撤销发起 B 的 pending/claimed 工作失效；成功 complete 响应丢失重试不二次保存；model 显式 last applied wins（不宣称拥有 revision CAS），并发未完成 mutation 必须拒绝，policy CAS 冲突清晰显示；非本机 tab 仅开放 agent scope，room/requester scope 和 reset 不得误发本地；source client SHA 更新后 CI locked build。

设计未解决的现实限制须报告：本地写入与服务器撤销不原子；claimed 响应丢失会 unknown；provider 身份验证取决于 upstream 能力。真实跨物理设备与真实 provider 推理没有证据时不得标为通过。

## NativeOwner / Desktop 内部接口

推荐 token-free Command 入口：`AgentConfiguration {agent_id,expected_generation,idempotency_key,command:Box<Command>}`、`ExecutionEnvironment {agent_id}`、`AgentConfigurationStatus {agent_id}`。外层只用于本地编程接口；严格匹配上表支持的 Local/Provider/RuntimeProvider/PrepareAgentWorkspace，映射为固定 operation/input，不序列化任意 Command 到 server。未知 nested Command 必须拒绝。新入口在普通 request() 之前路由；普通 request() 对这些 Command 显式拒绝。

`AgentConfiguration` 对外异步负责 create + poll，到成功返回经验证 result，失败返回静态 error；60 秒后 unknown/expired 绝不能返回 succeeded。Desktop 保存 idempotency_key 覆盖丢响应重试；ExecutionEnvironment 同样只拉 A 的 environment，AgentConfigurationStatus 返回 configuration envelope，用于设备在线/缓存呈现。worker 使用相同调用锁及已授权内部本地 dispatch，避免递归执行自身 wrapper。heartbeat 网络调用不需要 calls 锁，但仍使用原身份 pin 并 sync 当前 authority。
