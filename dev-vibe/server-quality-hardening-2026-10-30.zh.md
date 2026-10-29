# hagency-server 质量改进实施与验收

本批落实 [2026年10月29日质量评审](server-quality-review-2026-10-29.zh.md) 的 12 项发现。
原评审基线为 `30264bc`，实施分支为 `codex/server-quality-hardening`，基于已复核且
CI 通过后合并的前置 PR 11。用户授权提交 PR，复核与 CI 通过后合并。

## 实施结果

| 项目 | 结果 |
| --- | --- |
| R1 管理员路径守卫 | 直接使用 Salvo 路径解析，编码与重复斜杠不再跳过 scope 和账号写入限制；原实现失败、修复后通过。真实 Palpo 仍要求管理员身份。 |
| R2 依赖 | 更新 TLS、证书、ctutils、Markdown、webpage、Tokio 和 Pasion/OpenDAL，Palpo 保持当前 upstream main；14 条漏洞记录减少为 2 条。剩余两条是明确未解决的上游密码库风险，见下节。 |
| R3 文件活动容量 | 永久历史不计入 4096 件活动额度；128 MiB 字节额度和 unknown 保留不变，终态见证不能删除或重新授权。 |
| R4 登录限流 | 按直接 socket IP 计数，忽略不可信转发头，16 请求并发准入，保留登录交接和注销原子性。代理后的用户共享该代理的预算。 |
| R5 队列历史扫描 | 新增 pending 部分索引；环形游标改为两个有界索引范围后合并，保留公平性和同一 SQL 快照。 |
| R6 文件分块 | 追加不可改写的 chunk，queue/原 claim 组合并校验；遗留 bytea prefix 可升级，终态清块、unknown 保留。每文件 2048 块预算且提前保护剩余完成能力。 |
| R7 看板附件 | v7 迁移保留原大小、SHA256、身份及回执，清除已删除 payload；删除释放活动额度，旧 intent 不复活。 |
| R8 正常停机 | 先停止领取、保留内部 HTTP、最多排空 35 秒，再停止 HTTP/Pasion；容器和开发工具允许 75 秒，超时保留不确定状态。 |
| R9 运行健康 | 独立 workers-healthz，连续失败/停滞/异常退出可见，节流错误与恢复日志；readyz 保持启动锁存语义。 |
| R10 前端会话 | 共享刷新、凭据 generation 和 actor pin；迟到401、退出及换号不能污染本 tab 新会话，缓存有容量、寿命和在途写入边界。 |
| R11 Matrix 状态 | 按 Room 和不可变 frame 共享完整已验证投影；加载与等待后复查 current frame，错误不缓存，128 项/估算32 MiB 预算。 |
| R12 看板查询 | 列计数统一 GROUP BY，整页统计/取页由4N改为N+1；摘要不再构造后丢弃 description/checklist 正文。 |

额外修复跨标签页的 BFF 账号边界：GET 同样要求原 CSRF，原 tab 保留自己的 actor
pin，换号后不自动以新 Cookie 重试旧请求。Logout 撤销原服务器 session 和 token，
不发可能迟到覆盖新 Cookie 的 Set-Cookie 清除响应。Hagency 退出不再顺带调用
未绑定账号的 Pasion Cookie SSO 退出；Pasion 自身账户页保留其独立退出能力。
被动失效只清本 tab 状态，不清另一个 tab 的共享身份与偏好。

## 剩余依赖风险

`crypto-bigint 0.5.5` 受 Pasion JOSE 的 elliptic-curve 0.13 依赖约束，修复版
0.7.6 需要协调上游 API 迁移。`rsa 0.9.10` 的公告没有已修复版本；当前 JOSE
路径是 JWT 签名/验证，未找到 RSA padding 解密调用，但不据此声称整个侧信道
问题不可利用。没有静默轮换永久签名密钥或添加无效的另一个直接依赖来消除告警。

新增审计策略保留完整 JSON 和全部命中，阻断新发现、包版本/上游来源变更、过期
及陈旧处置。现有风险记录到 2026-11-29 到期，并在每次 CI 明确输出。策略通过
不等于 cargo-audit 清零。[完整依赖安全记录](../docs/DEPENDENCY_SECURITY.md) 列出
未解决项和后续边界；四条未维护、一个旧 rand unsound 警告另行保留。

## 提交前验证

- Rust 全量：Agent 新建独立 PostgreSQL 18.6 完整套件147通过、0失败、0忽略；
  backend23、配置8、frontend33、xtask12通过，共223项。原普通 Agent 的 ignored
  测试不重复计数。CI 另用 PostgreSQL17 验证，不把本地18.6结果混用。
- Python 工具25项；OpenAPI129个 owner/native/health操作加4个AS操作、36项守卫自测；
  全库格式、同CI版本的typos1.48拼写、WASM与native严格Clippy。
- R1/R4/R7及跨标签页两项回归保留原实现失败证据；存储升级、活动上限、分块不可改写、
  unknown不重领、终态清理、真实SQL故障恢复和HTTP回执锁等待均有真实数据库回归。
- CI 新增不发布的完整镜像门禁：实际PKCE、Matrix/AS、文件回执SIGTERM排空和重启。
  PR、每周或手动追加同镜像二进制/资源的停机三库及私有文件恢复。

这里记录提交前验证范围。最终镜像、GitHub CI 与合并结果以 PR 的最终提交和实际
检查记录为准。没有进行生产压测、大 Space 的 p95 实测、真实浏览器多 tab OAuth
人工验收或异机灾备；原生可控并发回归不能替代这些不同层次的验证。

## 升级与运行约束

升级块存储和看板 v7 前停止旧 writer，不支持混跑或直接降级旧二进制。
已有 staging/queued/unknown 文件的原始字节与授权见证保留。看板 v7 在事务内
回填摘要再清除旧 removed blob，受原 statement timeout 约束；大历史库需维护窗口，
超时回滚。数据库实际磁盘回收由 VACUUM 完成。

workers-healthz 表示队列进展和已观察的依赖故障，不是所有外部依赖的主动探针。
正常停机尽量记录原回执，SIGKILL或排空超时仍保留unknown，不能自动重新执行。
永久身份/摘要/回执会随业务增长，数据库容量规划仍需保留这些历史见证。

证据位于 ignored `.run/quality-hardening-20261029/`、
`.run/quality-review-20261029/security/`、`.run/quality-deployment/`。
本批没有操作现有业务服务/数据库或调用模型。源 checkout 未配置 task-writer，
未创建替代的 canonical task 状态。
