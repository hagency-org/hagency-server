# Server Diesel 查询方式核查

## 2026-10-31 实施（覆盖下方早期分析）

- PR #25 CI 跟进：quality 在 `validate-agent-openapi.py --self-test` 因旧源码指纹失败；PG17 schema 校验、5 个池测试和 compile-fail 已通过。复核 9 个存储重构文件后仅更新两个 OpenAPI 文档的对应 SHA，未改路由/DTO/安全定义；本地 129+4 个 operation、36 个反向校验测试通过。将此检查提前到 schema 编译前。检查时 Server #25 与 Cloud #17 的 GitHub review、行内评论均为空。
- 分支 `chris/diesel-horme-pool-safety`，基于 main `05e9f62`；用户要求分别提交 Server/Cloud PR，未授权部署或合并。
- 新增 Diesel CLI 生成的 `src/schema/agent.rs`、`project.rs`，两个配置为根目录 diesel.toml / diesel-project.toml。`scripts/check-db-schema.py` 重放真实启动路径并验证生成文件。没有手写生成文件，没有修改历史 SQL。
- 193 个查询调用改为 DSL；Queryable/Selectable 明确字段映射，复杂 CTE/JSON/视图/目录查询保留原生 SQL。待处理密文显式检查 nullable payload 非空。
- 普通业务通过本地 db-runtime 的 Horme 0.4.0 回调借池并在返回结果前归还；默认每业务池 8，可设 HAGENCY_AGENT_DB_POOL_SIZE，看板池 4。准入 2N、checkout 5s、callback+reset 30s、Cancel/rollback/discard；同任务同池嵌套会立即报错，spawn 不继承 task-local 检查。
- controller 选举及 crypto 授权行锁继续用专用非业务池连接；看板升级也使用专用连接，statement 120s / lock 30s，不受请求 30s 期限限制。
- 真实 PG 18：全量 Agent 148 passed、0 ignored；包含 v6 历史数据升级/重复打开、身份/会话/任务/回执/附件保留。runtime 5 passed + 1 compile-fail doctest；Agent/runtime 全目标全特性严格 Clippy、schema 漂移检查、typos 通过。完整后端检查结果见任务验收日志。
- 构建/临时/数据库/日志：D:/BuildCache/hagency-db-20261031，Cargo 缓存 D:/BuildCache/cargo。没有本机 Docker、生产数据库操作或部署。整套 Agent 用例会留待发通知，重跑全量要创建新的测试库，不能把历史数据兼容验证等同于清库。
- 说明：`../diesel-horme-pool-safety-2026-10-31.zh.md`。线上沿用原迁移记录和数据，无本次新增迁移。

## 早期分析（实施前）

2026-10-31，只读代码分析；未实施查询或数据库迁移。

- workspace 使用 Diesel 2.3 / diesel-async 0.9，PostgreSQL；本地 agent-service 未找到 schema.rs 或 diesel.toml，主要使用 sql_query + bind + QueryableByName。
- Store::open、DomainStore::open、TransportStore::open 执行内嵌 SQL 初始化/升级；BoardStore::open 自行维护 hagency_project_v1.migrations，当前支持版本 1..=7。不能表述为“没有迁移”。
- 业务命名空间为 hagency_agent_v1 / hagency_project_v1。Palpo、Pasion 是外部 Git 依赖，本地 agent-service 分析不代表其内部查询方式。
- 代码有普通 CRUD，也有 CTE UPDATE、数据库时钟、定向行锁、advisory lock、触发器/视图和数据库 JSON 投影。未发现解释全面采用原生 SQL 的明确设计记录；“SQL 优先实现沿用”是推断，不能当作已证实的历史动机。
- 不含 test 文件名的 Rust 文件粗略扫描有 413 个 sql_query 调用点；可能包含文件内测试，不等于 413 条独立业务查询。
- 建议（尚未实施）：保留现有迁移链，在独立 PostgreSQL 重放完整初始化/升级后，使用 Diesel CLI 为两个命名空间生成 schema；按规则不得手写 schema.rs。普通 CRUD 先改 DSL，复杂语句逐条评估扩展或保留集中封装；模型改 Queryable/Selectable/Insertable/AsChangeset，JSON 返回契约不变。
- 查询 DSL 改造不要求同时换用 Diesel migrations。要测试旧库升级、事务/锁顺序、数据库时间、并发领取/幂等、NULL 和 JSON 返回语义；CI 重生成 schema 并检查漂移。所有 PostgreSQL 数据、构建和缓存使用 D:/BuildCache。
