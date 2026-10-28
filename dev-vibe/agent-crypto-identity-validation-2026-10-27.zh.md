# Agent 独立加密身份：当前实现与真实联合证据

本批接通 Server 身份专用 broker、NativeOwner 主机能力和独立 Matrix SDK facade。真实 Synapse、实际 Server/PG/AS 与 NativeOwner 验证通过。它是 A4 的身份基础链；正常运行与界面尚未自动管理这些会话，加密任务、回复、文件与主人验证/恢复流程尚未实现，现有加密 Room 拒绝条件未移除。A3 及既有人工/平台/生产 OAuth 门槛保留，总目标仍进行中。

## 当前实现

- `hagency-agent-service` 提供 `agent-crypto-identity-v1`：七个身份专用入口，要求当前主人、会话、指定执行设备、设备代次与 Agent generation；身份和 Agent 行锁保持到有界 IO 结束，真实行锁等待后重新采样时间。独立数据库连接与四个请求槽避免把外部 HTTP 等待排进普通 domain 连接；整体网络阶段上限12秒，每次 HTTP 都检查实际锁定会话的期限。
- Server 保留 AS 凭据。注册使用 `inhibit_login=true`，随后创建虚拟 Matrix 设备并核验实际 device-bearing whoami。协议版本来自真实 homeserver，未捏造 v1.17 支持、未改用裸 Matrix login。
- Matrix device ID 根据永久 Agent 与稳定执行主机计算；Agent/设备的授权代次不会生成新密钥。设备公钥永久固定；交叉签名公钥按 Agent 全部主机记录固定，因此改派不能静默替换 Agent 身份。公钥摘要在独立事务提交后才联网，未知/HTTP错误/重开不抹除它。
- 仅允许自己的公钥上传/查询/签名与身份读取；拒绝 query 注入、login、sync、peer key claim/sharing、消息、媒体、备份和账户管理。私钥、passphrase、AS token 不进入响应。
- `AgentCryptoSession` 将真实 SDK、bearer 和独立加密 SQLite store 留在主机 facade 内；不导出原始 Client。私有目录、绑定文件、独占锁、原始 key store 缺失与 Unix symlink 拒绝。持久绑定排除授权代次与 advertised versions，凭据轮换/协议升级不切换私钥目录。主机仍须提供本地 secret-store passphrase；产品的自动分配/生命周期管理是后续工作。

协议与边界见 [实施契约](agent-crypto-identity-contract-2026-10-27.zh.md)。

## 最终真实验收 identity6

当前 Server binary、真实 Synapse1.163.2、两个新 Agent、真实 SDK 与 NativeOwner；没有 provider 调用。

- 私有根目录 `/private/tmp/hagency-agent-crypto-synapse-20261027-1/identity6`；原结果 `result.json`，独立只读复核 `independent-proof.json`；日志 `/tmp/hagency-agent-crypto-qualification6.log`，进程最终 exit0。
- Agent `agt_01m5xsm9yzkwc95qnaqhkns6ws` 和 `agt_01m5xsma2162g38efn37xjrne5` 的 user/device、Ed25519 与 Curve25519 均不同；两者也不同于主人 SDK 的 Ed25519。独立实际 homeserver 读取确认双方的公钥和不同的 master key；每个 Agent 仍仅一个 Matrix 设备。
- 原 SDK store 重开不换钥。通过实际 owner session/device API 撤销设备后，原 broker 请求返回401，旧 SDK facade 读取也拒绝；同一 bearer 直接访问 homeserver whoami 返回401，它不是 Matrix access token。
- 完整 checked shutdown 并丢弃原 NativeOwner 后，在同一 installation 重开并重新授权；Hagency device `dev_01m5xsm9xhbje6jgh9182y6qpa` 从 generation1 经撤销/重授权变为3。原两个 crypto identity 记录的 first_device_generation 仍为1，Matrix device ID 与四项公钥摘要都保留；第一个 Agent 的实际 SDK 再 bootstrap/回读仍为原公钥。
- 原生账本独立只读 `calls` 数量为0；当前隔离 Server `execution_leases` 数量为0。本轮不含付费模型推理，不把它记作加密 Codex 任务验收。
- PostgreSQL 使用独立数据库 `agent_files_joint_crypto4`；Server 通过实际 Synapse AS/worker/readiness。OAuth introspection 仍为明确 fixture bridge，未声称生产 Pasion OAuth 通过。
- 受控 Server、原生/SDK 进程及唯一所属 Synapse 容器均已停止，独立 PG 已停止；全部原数据保留，未删除旧失败或重发旧任务。

## 必须保留的失败

1. 第一次驱动启动早于 service-info 生成，文件读取失败，未建立 NativeOwner/Agent crypto 身份；日志 qualification1。
2. identity2 在 provision 时错误给全局 versions 请求附带尚未注册的 Agent user_id；真实 Synapse403。修复为全局版本读取后进行 inhibit-login 注册和实际设备探测。未降低授权或引入 login fallback。
3. identity3 已通过双 Agent SDK、公钥及 store 重开；驱动拼接了双斜杠 whoami URL，得到404而非预期401，资格失败。该 panic 的旧进程101不改写为成功；后续驱动增加 catch-unwind 后 checked shutdown。
4. identity4 已验证独立 SDK/store 与直接 homeserver bearer401；验收脚本误将 Pasion proof 的 `accessToken` 改成 snake_case，实际400。按当前 OpenAPI 和生产 Client 校正，业务 API 不变。
5. identity5 已通过实际设备撤销和旧会话拒绝；驱动在仍持有原 NativeOwner 对象时重开同一目录，被原互斥状态锁拒绝。保留此拒绝，改为 checked shutdown 后 drop 原对象，再重开。没有弱化实例锁。
6. 首轮 Server 测试编译出现 Arc AtomicBool 与 Diesel `load` 方法歧义、Verifier 参数不符；新行锁测试的 `Text` 与 Salvo prelude 同名；Client 首轮使用不存在的 DeviceId::parse 和未声明 hex crate。按实际 API 修正，日志 compile1、client-compile1、server-full1 保留。
7. 本地错误复用已运行过完整套件的数据库时，两个既有 request-notice 分页夹具断言失败：`/tmp/hagency-agent-crypto-server-full4.log`，112 passed/2 failed。保留旧表/通知与失败；未放宽生产过滤/上限。按既有“独立新库、串行”契约重新运行当前源，fresh full5 的114项全过；不把旧失败改写为通过。
8. 新增外部主人 HTTP 断言曾误期望404，实际既有 `hidden` 统一隐藏为401；核对 `domain_object_not_authorized` 后改为401，并保留“未产生 Matrix IO”断言。原 security-final 日志3 passed/1 failed，修正后的 fresh security-final2 四项全过。

## 当前本地验证

- Server 全套 fresh PostgreSQL：114 passed/0 failed/0 ignored；日志 `/tmp/hagency-agent-crypto-server-full5.log`。随后新增外部主人/Host/重复 Authorization 断言的当前四项安全回归全过，`server-security-final2.log`。
- Server all-target strict Clippy、格式与 OpenAPI90 owner/4 AS +22守卫自测通过。新接口确切方法/路径库存有静态校验。
- Engine271 passed/9原忽略；App627 passed/2原忽略，含新增两个私有 store 测试。真实资格 entry 已编译并实际执行；普通 CI 只编译它，不自动连外部 fixture。
- 当前 core/App all-target strict Clippy（App采用 CI 的 --no-deps）、core 格式、46静态自测、一个生产 caller 和1191 Rust bindings 通过；missing/deferred/platformDeferred 均空。本地未安装 typos CLI，拼写由推送后的远端 lane 验证，不伪报本地拼写通过。
- 上一头5ea696dc 的 fast/App/spelling，以及 Full40444655887 实际 Linux/macOS/Windows 全部通过；不能代替本批新源的远端验证，也没有解释更早 A2 Windows store-shutdown OutcomeUnknown 原失败。

## 尚未完成

接入 NativeOwner 正常 Agent 配置/运行及 secret store/关闭等待；主人验证、显式换主机/密钥恢复；加密 Room 的原始事件接收/解密/ACK、当前 Server 成员/绑定/租约授权、Codex 输入和原始加密回复/文件发送；真实加密 Codex 联合链、人工桌面、生产 OAuth 与跨平台。身份或公钥回执不等于信任、消息权限或工具 grant，现有拒绝条件继续生效。
