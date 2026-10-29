# hagency-server 多维质量评审与改进报告

评审日期：2026 年 10 月 29 日，Asia/Shanghai。源码基线：`30264bcff2daebfb5c824b190ba55377ba598513`。覆盖后端集成、Agent 服务、管理前端、锁定依赖、CI 与部署恢复流程；采用主审加三个并行专项审查。

后续实施及新验证见 [2026年10月30日改进记录](server-quality-hardening-2026-10-30.zh.md)。以下保留原基线的发现和证据；源码行号对应评审时版本。

项目在身份绑定、授权使用点重验、持久化接收、执行去重及不确定结果保留方面已有较扎实的设计。本次最需要优先处理的是管理员路由守卫绕过和已公告依赖的处置，其次是长期使用下的容量设计、队列扫描放大、文件写入放大，以及停机和故障检测。现有测试全部通过，并不覆盖这些新发现的边界。两项文件配额限制符合现有合同，属于需要评估的产品容量取舍，不是实现违约。

本次交付评审与改进方案，没有修改生产代码、更新依赖或操作现有业务库。安全复现使用进程内测试处理器；数据库测试和查询实验使用新建的独立 PostgreSQL 集群。

## 综合质量判断

| 方向 | 已有基础 | 主要缺口 |
| --- | --- | --- |
| 安全与权限 | OAuth 与真实 Matrix 身份联合校验，原生与浏览器能力分离，数据库使用点重验 | 管理员守卫与路由解析不一致，登录限流可跨端口绕过 |
| 依赖与供应链 | Cargo.lock 固定提交，资源构建跟随锁定版本，镜像生成 SBOM | 本次 audit 命中 14 条漏洞记录，CI 缺少相应门禁 |
| 性能与扩展 | 有界队列、独立看板连接池、分队列 worker 和游标公平性测试 | 历史队列缺索引，文件 bytea 反复拼接，状态读取与看板 SQL 放大 |
| 数据一致性 | AS 持久化后 ACK，稳定 transaction ID，永久幂等回执，unknown 不自动重放 | 永久历史与活动容量混用，正常删除附件不能恢复字节额度 |
| 可靠性 | 取消事务会丢弃连接，设备撤销与过期有回归覆盖 | 正常停机直接取消投递 worker，可能丢失原始回执 |
| 可观测性 | 结构化 tracing、启动 AS 往返探测 | 队列错误被吞掉，启动成功锁存不能监测持续故障 |
| 测试与交付 | 真实 PostgreSQL 回归、严格 Clippy、WASM 检查、OpenAPI 漂移检查 | 完整嵌入启动与恢复演练未纳入 CI，缺少规模基线 |
| 可维护性与使用体验 | 模块分层、中文运维文档、契约与负向测试较丰富 | 前端并发刷新凭据竞争，部分 SQL 与大模块难以局部审阅 |

优先级：P1 应优先修复或完成风险处置，再扩大公开部署；P2 应安排近期修复；P3 应在性能基线和产品规模目标下推进。以下共 12 项，含 2 项 P1、8 项 P2、2 项 P3。P1 表示处置优先级，不表示每条依赖公告都已在本项目完成攻击复现。

## 优先修复项

### R1 管理员路径守卫可被编码与重复斜杠绕过

**P1，实际守卫复现确认。** [pasion.rs:341](/Volumes/Data/Works/hagency-org/hagency-server/crates/backend/src/pasion.rs:341) 用原始 `req.uri().path()` 判断管理路径，Salvo 路由按解码后的路径片段匹配，并忽略重复分隔符。账号写入限制同样依赖原始字符串，见 [pasion.rs:380](/Volumes/Data/Works/hagency-org/hagency-server/crates/backend/src/pasion.rs:380)。

实际 `DelegatedAdminGuard` 加 Salvo Service/TestClient、下游替换为测试处理器的结果：

| 请求路径 | 结果 |
| --- | --- |
| `/_palpo/admin/v1/appservices` | 守卫拒绝，401 |
| `/_%70alpo/admin/v1/appservices` | 穿过守卫，到达下游，200 |
| `/_palpo//admin/v1/appservices` | 穿过守卫，到达下游，200 |
| `//_palpo/admin/v1/appservices` | 穿过守卫，到达下游，200 |

真实 Palpo 仍要求有效的 Matrix 管理员身份，不能据此声称匿名访问或普通用户提权。问题绕过的是 Hagency 附加的 `urn:palpo:admin:*` scope 检查和“账号创建、密码修改必须经 Pasion”的限制。锁定 Palpo 的更严格 scope 检查受 `unstable-msc4484` feature 控制，当前依赖只启用 default。测试没有执行真实账号或密码操作。

**修复：** 将保护绑定到实际管理路由范围，保证授权与路由使用同一种路径表示；明确拒绝可产生不同解析结果的非规范路径。账号写入规则也应基于实际匹配的路由与参数。单独升级 Palpo 或开启 feature 不会自动补齐 Hagency 的账号写入限制。

**验收：** 覆盖 Palpo/Synapse 两套 alias、编码片段、重复与尾部斜杠；无凭据、低 scope 管理员必须拒绝，受禁止的账号写入始终拒绝。复现源码、命令与限制见 [安全证据](/Volumes/Data/Works/hagency-org/hagency-server/.run/quality-review-20261029/security/README.txt)。

### R2 已公告依赖缺少持续审计与处置门禁

**P1，audit 命中确认，可利用性需逐条判断。** `cargo audit` 对当前锁文件的 1108 个依赖条目报告 **14 条漏洞记录**，另有 **4 条 unmaintained、2 条 unsound 警告**。公告库提交为 `ee1a49a2769a24a596afc1c20cb33748d2769586`，更新时间为 2026 年 10 月 29 日。

主要位置：[Cargo.lock:9152](/Volumes/Data/Works/hagency-org/hagency-server/Cargo.lock:9152)、[Cargo.lock:9217](/Volumes/Data/Works/hagency-org/hagency-server/Cargo.lock:9217)。`rustls 0.23.45`、`rustls-webpki 0.103.15` 等处于正常后端依赖图中。`rustls-webpki` 公告涉及特定信任锚条件下接受已撤信的中间证书，修复版为 `0.103.16`，见 [RustSec 公告](https://rustsec.org/advisories/RUSTSEC-2026-0382.html)。应优先核对部署实际使用的验证器和根证书来源，并更新 TLS 相关锁定版本。

[CI:65](/Volumes/Data/Works/hagency-org/hagency-server/.github/workflows/ci.yml:65) 之后运行格式、Clippy、测试和 WASM 检查，没有 audit/deny 步骤；[dependabot.yml:3](/Volumes/Data/Works/hagency-org/hagency-server/.github/dependabot.yml:3) 只更新 GitHub Actions。公告发布后，现有绿灯不会暴露依赖风险。

**修复：** 对兼容修复版定向更新锁文件，保留 Palpo `branch="main"` 的既定策略；需要上游支持的升级提交上游处理。CI 增加依赖审计，并定期更新公告库。暂时无法修复的项目需记录实际调用路径、平台/feature 前提、负责人和到期时间，避免无期限忽略。

**验收：** TLS、OAuth、联邦及媒体集成仍通过；每条命中都有升级或有证据的风险处置结果。附录列出全部命中与本次已核对的不可直接利用条件。

### R3 文件投递按永久历史计数导致额度无法恢复

**P2，已确认的长期容量设计限制。** [transport_files.rs:220](/Volumes/Data/Works/hagency-org/hagency-server/crates/agent-service/src/transport_files.rs:220) 对同一 owner 的全部 `file_outbox` 历史执行计数，达到 4096 就返回 `file_capacity_exceeded`。已发送、已取消的行也计入；同一段的字节额度则排除了这些终态。[现有合同:25](/Volumes/Data/Works/hagency-org/hagency-server/dev-vibe/agent-file-delivery-contract-2026-10-27.zh.md:25) 明确规定最多 4096 条投递见证，当前实现符合合同。

[file_delivery/schema.sql:17](/Volumes/Data/Works/hagency-org/hagency-server/crates/agent-service/src/file_delivery/schema.sql:17) 禁止删除投递历史；过期与完成仅转换状态或清空 content。因此同一 owner 正常累计 4096 次文件准备后，后续新请求永久被拒，重启不能恢复。

**改进：** 若目标是持续运行且允许超过 4096 次投递，应先修订容量合同，分离永久幂等见证、活动件数与活动字节额度，并为历史存储定义可扩展的归档或计费策略。sent/cancelled 不占活动件数；unknown 继续保留独立上限与显式处置流程。不能删除旧见证或自动重放 unknown 来腾空间。若维持现有上限，应提供剩余额度、预警及明确的耗尽处理说明。

**验收：** 4096 条终态历史不阻止新请求；真正活动记录达到上限仍原子拒绝；旧 requestId 重试保留原结果，不能重新获得发送权。

### R4 浏览器登录限流把源端口纳入身份

**P2，实际 BrowserAuth 复现确认。** [browser_auth.rs:525](/Volumes/Data/Works/hagency-org/hagency-server/crates/backend/src/browser_auth.rs:525) 用 `req.remote_addr()` 构造限流 key，包含 TCP 源端口。同一 IP 新建连接即可获得新的预算。

进程内实验中，同一 IP/端口第 121 次请求返回 429；只更换端口，第 122 次恢复为 415，说明穿过限流、随后才被请求体校验拒绝。实验不需要有效 token。

[browser_auth.rs:159](/Volumes/Data/Works/hagency-org/hagency-server/crates/backend/src/browser_auth.rs:159) 的 map 最多 10000 个 bucket，且登录路由先计数再校验请求体和凭据。轮换端口可填满 map，使新的合法连接在窗口内被拒绝；容量耗尽影响由代码推导，未运行 10000 次请求模拟。

**修复：** 基于可信客户端 IP 共享预算，明确反向代理来源的可信范围；加入全局并发准入和有界存储。不要直接信任任意转发头。评估缩小持有全局 handover 锁进行上游身份查询的范围。

**验收：** 同 IP 跨端口共享预算；可信代理与伪造转发头测试；高连接数时不把其他用户全部阻断。实际输出在 [限流实验](/Volumes/Data/Works/hagency-org/hagency-server/.run/quality-review-20261029/security/rate_check.output.txt)。

## 性能与容量改进项

### R5 AS 待处理队列扫描随全部历史增长

**P2，表结构确认并有合成执行计划验证。** [appservice.rs:102](/Volumes/Data/Works/hagency-org/hagency-server/crates/agent-service/src/appservice.rs:102) 的 `routing_jobs` 只有 transaction 主键，缺少 pending 部分索引。已 routed 历史永久保留。接收时计算 pending 容量，worker 持续过滤 pending，见 [appservice.rs:184](/Volumes/Data/Works/hagency-org/hagency-server/crates/agent-service/src/appservice.rs:184) 和 [transport.rs:1173](/Volumes/Data/Works/hagency-org/hagency-server/crates/agent-service/src/transport.rs:1173)。

在独立 PostgreSQL 18.6 中，按原表结构建立 **100000 条 routed 历史和 100 条 pending**，执行原 `Inbox::pending` 查询及 pending 计数，再加入 `(received_at_ms, transaction_id) WHERE state='pending'` 索引：

| 合成查询 | 原表结构 | 加部分索引后 |
| --- | --- | --- |
| pending 页面 | 顺序扫描，过滤掉 100000 行；1143 shared hit blocks | Index Only Scan；41 shared hit blocks |
| pending 计数 | 顺序扫描，过滤掉 100000 行；737 shared hit blocks | Index Only Scan；3 shared hit blocks |

单次本地执行时间分别为 4.275→0.026 ms、4.046→0.041 ms。这是合成数据的单次计划比较，不代表生产吞吐或固定倍数收益。实际 delivery 使用的环形游标查询包含 `ORDER BY (t.id<=cursor),t.id`，还需单独优化其排序，不能把上述页面结果直接套用到该查询。

**修复：** 为时间排序与 transaction 游标分别设计 pending 部分索引；环形游标拆为两个可索引范围；评估事务内精确维护 pending 件数/字节，减少全量 JSONB 转 text 求和。保留容量和去重的原子性。

**验收：** 在 10 万、100 万历史且 pending 固定的情况下测空轮询、接收和实际游标查询；检查计划、缓冲页、锁等待和并发 p95。完整计划与建表输入在 [执行计划证据](/Volumes/Data/Works/hagency-org/hagency-server/.run/quality-review-20261029/as-index-plans.json)。

### R6 文件分块反复重写完整 bytea

**P2，写入放大由代码推导。** [transport_files.rs:240](/Volumes/Data/Works/hagency-org/hagency-server/crates/agent-service/src/transport_files.rs:240) 接受 1 字节至 64 KiB 的块；[transport_files.rs:250](/Volumes/Data/Works/hagency-org/hagency-server/crates/agent-service/src/transport_files.rs:250) 在单 Transport 数据库连接和共享 advisory lock 下执行 `content=content||chunk`。

一个 8 MiB 文件按 64 KiB 上传需要 128 次追加，拼接表达式累计生成 `64 KiB × (1+…+128) = 516 MiB` 内容；极小合法块可进一步放大。这是逻辑处理量，不是实测 WAL 或磁盘写入量。单连接与共享锁使其他 owner 的租约、投递和相关事务也可能等待。

**修复：** 独立追加 chunk 表，以 `(file_id, offset)` 唯一键核验重复块；完成时一次流式校验与组装。约束非末块最小大小或请求速率，保持原始文件和单次发送 custody。

**验收：** 比较 64 KiB、4 KiB 和极小块下的查询数、WAL、锁等待、其他 owner 心跳 p95；重复块、变更块和撤权仍按原合同拒绝。不要直接扩大连接池来绕过授权锁顺序。

### R7 看板删除附件不能回收字节额度

**P2，已确认的保留容量与删除体验限制。** [collaboration.rs:196](/Volumes/Data/Works/hagency-org/hagency-server/crates/agent-service/src/project_board/collaboration.rs:196) 的件数仅统计未删除附件，字节数累计全部历史 data。删除操作在 [project_board/mod.rs:534](/Volumes/Data/Works/hagency-org/hagency-server/crates/agent-service/src/project_board/mod.rs:534) 仅设置 `removed_at`。[看板合同:198](/Volumes/Data/Works/hagency-org/hagency-server/docs/project-board.md:198) 明确规定 40 MiB retained bytes，并说明删除字节继续计入，因此当前实现符合合同。

同一任务上传 20 个 2 MiB 附件后全部删除，活动列表虽为空，累计字节仍为 40 MiB，任何非空新附件都会返回 `board_attachment_limit`。历史记录未发现正常回收路径。

**改进：** 若产品承诺删除后可继续上传，应修订合同，分开活动配额与历史保留策略；活动字节只统计未删除记录，历史保留设置独立配额或归档流程。物理清理时保留摘要、元数据和原始幂等回执。若维持现有策略，界面应解释软删除与保留额度，并显示历史用量及耗尽后的处理路径。

**验收：** 删除后允许新附件；20 件/40 MiB 活动上限在并发下仍成立；旧上传 intent 重试不复活已删除附件。

## 可靠性与可观测性改进项

### R8 正常停机直接取消文件投递可能丢失回执

**P2，生命周期风险由代码确认，尚未做停机故障注入。** [main.rs:329](/Volumes/Data/Works/hagency-org/hagency-server/crates/backend/src/main.rs:329) 只优雅停止 HTTP，随后在 [main.rs:337](/Volumes/Data/Works/hagency-org/hagency-server/crates/backend/src/main.rs:337) 直接 abort delivery worker。文件已先在数据库提交 uploading/sending，随后才调用 Matrix 并记录结果，见 [workers.rs:539](/Volumes/Data/Works/hagency-org/hagency-server/crates/agent-service/src/workers.rs:539)。

正常部署的 SIGTERM 若落在网络完成、数据库回执尚未提交的窗口，会失去原 worker 的结果。重启后过期 claim 转 unknown，不再自动领取；上传阶段没有消息 AS 回执可补齐，可能长期需要人工核对。这种 unknown 处理本身保留了安全性，但正常停机应尽量避免制造额外不确定结果。

**修复：** 停止新 claim，撤出外部流量，保持内部 Matrix HTTP 可用，在有界期限内排空当前上传/发送并提交回执，最后停止 listener。超过期限仍保留 unknown，禁止自动重发。

**验收：** 分别阻塞上传、发送和回执写入，触发 SIGTERM 后释放，确认结果落库；SIGKILL 仍保留 unknown 和原始见证，不能重复发送。

### R9 队列错误被吞掉且健康检查只记录启动成功

**P2，代码确认。** [workers.rs:591](/Volumes/Data/Works/hagency-org/hagency-server/crates/agent-service/src/workers.rs:591) 的所有队列循环用 `let _ = $pass.await` 丢弃错误；cleanup/身份供应也有同类处理。主进程未监督 worker 的异常完成。[api.rs:147](/Volumes/Data/Works/hagency-org/hagency-server/crates/agent-service/src/api.rs:147) 在启动探测成功后永久置 true 并退出；[Dockerfile:62](/Volumes/Data/Works/hagency-org/hagency-server/Dockerfile:62) 唯一健康检查使用该 `/readyz`。

启动后 SQL、Matrix 持续失败或 worker 停摆时，listener 与启动锁存仍可能健康，队列积压却缺少可定位的错误上下文。文档明确声明 `/readyz` 为启动往返探测，因此应补充持续健康机制，而不是把既有语义误称为实现错误。

**修复：** 节流记录队列名、错误码、连续失败次数、最近成功时间与最老待处理年龄；监督 JoinHandle。增加持续运行健康与指标，按队列进展、依赖故障和异常退出来降级，保留启动门禁语义。

**验收：** 注入持续 SQL 错误、Matrix 超时和 worker panic，应出现对应日志、指标与健康变化；恢复后自动恢复，日志量保持有界。

### R10 前端多个 401 会竞争刷新并清除有效凭据

**P2，代码可达的并发风险，未做浏览器定向复现。** [auth.rs:510](/Volumes/Data/Works/hagency-org/hagency-server/crates/frontend/src/api/auth.rs:510) 每个 401 都独立刷新；[auth.rs:526](/Volumes/Data/Works/hagency-org/hagency-server/crates/frontend/src/api/auth.rs:526) 没有共享刷新任务、互斥或凭据版本校验。dashboard 的多个资源可同时请求 API。

两个请求可以读取同一旧 refresh token。A 刷新并存下新凭据、重试成功；B 继续使用旧 token，刷新失败后直接删除 A 已写入的凭据。锁定 Pasion 的刷新实现会轮换 token，并在新 access token 已使用时拒绝旧 token 重放，故不能依靠服务端重复刷新宽容逻辑消除此竞争。还可能出现较晚响应覆盖更新凭据。

**修复：** 同一登录会话共享一个进行中的刷新任务；401 调用方等待同一结果。保存与清理前检查会话及凭据版本，旧请求不能影响新登录/新凭据；退出时使旧刷新结果失效。

**验收：** 同时返回多个 401，只产生一次刷新；迟到失败不清除新凭据；退出或切换账号期间迟到的刷新结果不能写回。检查 Matrix 与 Pasion 两类 API 客户端都使用同一机制。

## 需要规模基线的优化项

### R11 按事件与 binding 重复读取完整 Matrix 状态

**P3，静态优化机会。** [workers.rs:130](/Volumes/Data/Works/hagency-org/hagency-server/crates/agent-service/src/workers.rs:130) 起逐事件、逐 binding 取 delivery facts；[gateway.rs:136](/Volumes/Data/Works/hagency-org/hagency-server/crates/agent-service/src/gateway.rs:136) 读取 Room 与 Space 状态；[main.rs:208](/Volumes/Data/Works/hagency-org/hagency-server/crates/backend/src/main.rs:208) 获取完整状态并重新序列化、核验完整性。同一 Room 多 Agent、大 Space 会放大投影与扫描成本；上游已有缓存可能减少数据库读取，不能据此直接推定绝对延迟。

按不可变 frame ID 复用完整已验证快照和投影，合并同 frame 的并发读取。每次使用前仍核对当前 frame，不能仅按 Room 与 TTL 复用权限结果。以 100/1000/10000 成员、1/10/50 bindings 测 CPU、分配、序列化次数和路由 p95，并验证成员撤销立即失效。

### R12 看板整页查询存在逐列与逐任务放大

**P3，查询次数确认，饱和点未实测。** [project_board/mod.rs:353](/Volumes/Data/Works/hagency-org/hagency-server/crates/agent-service/src/project_board/mod.rs:353) 每列分别统计活动数、过滤数、取页、统计全部数。在不按单列过滤时，12 列仅这部分就有 48 次串行 SQL，另有权限、快照、成员和标签查询。`TASK_JSON` 为每条任务追加附件、评论、子任务等关联聚合；摘要还先构造 description/checklist 再删除。看板池仅 4 个连接，容易在并发整页刷新下形成排队。

可用 GROUP BY 统一列计数，窗口函数批量取每列页面，并以所选任务 ID 批量聚合评论、附件与子任务；摘要和详情分开投影。优化必须保留 sequence、一致快照、过滤语义与 WIP 正确性。比较 5/12 列、每列 50 项及多人并发的 SQL 数、连接池等待和 p95，再决定实施顺序。

## 交付与维护建议

1. **增加完整部署门禁。** 当前 CI 已运行真实数据库测试，但完整 Palpo/Pasion 启动、PKCE、AS 往返和三库恢复不在该工作流中；镜像构建只在主干或标签 push 后运行。将独立环境启动 smoke 纳入关键依赖/路由变更检查，发布镜像应验证实际就绪与最小业务往返。
2. **补齐生产恢复目标。** 既有 [恢复演练](/Volumes/Data/Works/hagency-org/hagency-server/docs/BACKUP_RESTORE_VALIDATION.zh-CN.md:9) 证明同机、同 PostgreSQL 集群的一致停机三库与文件恢复；异机丢失、在线跨库 PITR、外部存储、主人本地密钥与并发发送快照仍需独立验证。按真实部署定义 RPO/RTO，并定期演练。
3. **按不变量拆分代码。** 优先让授权、投递状态转换、查询投影和迁移各自可局部审阅，避免按文件行数机械拆分。给新增 SQL 与状态转换补迁移、撤权、取消和重开回归；保留既有单次 custody、锁顺序和 unknown 语义。
4. **统一前端会话失效与缓存生命周期。** 将凭据刷新、身份切换、退出、资源缓存和页面授权汇总为明确的会话状态；给缓存增加容量上限与过期清理。浏览器错误恢复和键盘/读屏体验应单独走实际 UI 验证，本次没有对视觉与无障碍作通过结论。
5. **固定开发工具链入口。** 本机默认 Rust 为 1.98.1，项目要求 1.99；本次显式使用 `+1.99.0`。可增加一致的工具链声明，减少本地、脚本和 CI 入口漂移。

## 建议实施顺序

| 批次 | 内容 | 完成条件 |
| --- | --- | --- |
| 第一批 | R1 路由守卫，R2 依赖处置与审计，R4 限流 | 变体路径负向回归；审计逐项闭环；跨端口与代理测试 |
| 第二批 | R3 与 R7 容量，R8 停机，R9 故障检测，R10 刷新 | 终态不耗活动额度；正常停机排空；故障可见；并发刷新稳定 |
| 第三批 | R5 索引与计数，R6 文件分块 | 带历史规模的计划与 p95 改善；锁和幂等回归保持通过 |
| 第四批 | R11 状态复用，R12 看板查询，以及完整部署与灾备演练 | 达到约定数据规模与负载目标，权限撤销和快照语义不退化 |

不建议一次重构全部存储并扩大连接池。先明确并测试授权/幂等不变量，再按热点逐项调整，保留每次变更前后的可比较数据。

## 本次验证结果

| 检查 | 结果与范围 |
| --- | --- |
| workspace rustfmt | 通过；stable 下部分 nightly 格式选项被忽略，命令退出 0 |
| OpenAPI 与检查器自测 | 128 个 owner/native 操作、4 个 AS 操作，34 项自测通过 |
| 后端 前端 xtask 测试 | 49 通过，0 失败 |
| Agent 普通 lib 测试 | 39 通过，95 ignored；不把 ignored 计作通过 |
| Agent 完整隔离 PostgreSQL 测试 | 134 通过，0 失败，0 ignored；include-ignored、test-threads=1 |
| 后端 Agent xtask 严格 all-target Clippy | 通过 |
| 前端 WASM 严格 all-target Clippy | 通过 |
| 实际 Guard 与实际 BrowserAuth 测试 | 分别确认 R1 与 R4；下游使用测试处理器，没有真实账号攻击 |
| 合成 AS 查询计划 | 确认 R5 的历史顺序扫描与部分索引改善 |
| Cargo audit | 未通过；14 漏洞记录和 6 警告，见附录 |

后端/前端/xtask 的 49 项加完整 Agent 的 134 项，共 **183 项测试通过**；普通 Agent 的 39 项与完整套件重叠，不重复累计。测试使用 Rust 1.99.0。数据库为新建的 PostgreSQL 18.6 独立集群，CI 为 PostgreSQL 17，不能把本次结果当作新的 PG17 验证。临时集群已停止，证据保留在 `.run/quality-review-20261029/`。

首次临时数据库启动因 Unix socket 绝对路径超过 103 字节而失败；改为该独立集群仅监听 loopback TCP 后启动成功，原日志保留。安全实验的初始编译/链接设置错误也保留在证据说明中，均不改写为应用测试失败或成功。没有进行真实生产压力测试、完整镜像启动、故障注入或异机恢复。

## 附录 依赖命中与触发条件

原始机器结果：[cargo-audit.json](/Volumes/Data/Works/hagency-org/hagency-server/.run/quality-review-20261029/cargo-audit.json)。下表是锁文件命中清单；修复版范围来自本次 RustSec 数据库。它不等于本项目存在 14 个已证实可利用的入口。

| 锁定依赖 | RustSec 记录 | 公告修复范围 |
| --- | --- | --- |
| crypto-bigint 0.5.5 | [2026 0387](https://rustsec.org/advisories/RUSTSEC-2026-0387.html) | >=0.7.6，需评估跨版本升级 |
| ctutils 0.4.2 | [2026 0385](https://rustsec.org/advisories/RUSTSEC-2026-0385.html) | >=0.4.4 |
| pulldown-cmark 0.13.4 | [2026 0414](https://rustsec.org/advisories/RUSTSEC-2026-0414.html) | >=0.13.5 |
| quick-xml 0.39.4 | [2026 0195](https://rustsec.org/advisories/RUSTSEC-2026-0195.html)、[2026 0194](https://rustsec.org/advisories/RUSTSEC-2026-0194.html) | >=0.41.0 |
| rsa 0.9.10 | [2023 0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html) | 公告没有已修复版本，需按实际算法路径处置 |
| rustls 0.23.45 | [2026 0346](https://rustsec.org/advisories/RUSTSEC-2026-0346.html)、[2026 0347](https://rustsec.org/advisories/RUSTSEC-2026-0347.html)、[2026 0348](https://rustsec.org/advisories/RUSTSEC-2026-0348.html)、[2026 0349](https://rustsec.org/advisories/RUSTSEC-2026-0349.html) | >=0.23.46 |
| rustls-webpki 0.103.15 | [2026 0382](https://rustsec.org/advisories/RUSTSEC-2026-0382.html) | >=0.103.16 且 <0.104.0-alpha.1，或 >=0.104.0-alpha.8 |
| webpage 2.0.1 | [2026 0407](https://rustsec.org/advisories/RUSTSEC-2026-0407.html) | >=2.1.0 |
| webpki-root-certs 1.0.9 | [2026 0383](https://rustsec.org/advisories/RUSTSEC-2026-0383.html) | >=1.0.10 |
| webpki-roots 1.0.9 | [2026 0381](https://rustsec.org/advisories/RUSTSEC-2026-0381.html) | >=1.0.10 |

已核对的限制条件：

- `webpage` 经 Palpo 进入后端，但锁定 Palpo 的预览代码仅调用 `HTML::from_string`，下载由另一条受 URL 检查的 HTTP 路径完成；未发现调用公告涉及的 `HTTP::fetch`/`Webpage::from_url`。因此不能把该命中直接报告为本项目已有 SSRF 或本地文件泄露。[公告的受影响调用](https://rustsec.org/advisories/RUSTSEC-2026-0407.html)
- `pulldown-cmark` 的锁定调用只开启 tables 和 strikethrough，没有开启公告要求的 `ENABLE_WIKILINKS`，该调用不满足触发前提。[公告的选项条件](https://rustsec.org/advisories/RUSTSEC-2026-0414.html)
- 当前默认后端 feature 图没有 `rustls/brotli`，不满足证书 Brotli 解压公告的 feature 前提；仍需处置其他 TLS 公告，并防止后续 feature 变更引入该路径。[公告的配置条件](https://rustsec.org/advisories/RUSTSEC-2026-0348.html)
- crypto-bigint、ctutils、rsa、quick-xml 及 TLS 的其余记录需继续核对具体算法、输入来源、验证器、平台与启用条件。本次没有为这些记录完成逐条攻击复现。unsound 与 unmaintained 警告需独立记录，不能混入 14 条漏洞计数。
