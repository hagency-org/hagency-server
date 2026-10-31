# Diesel 类型化查询、Horme 与旧数据兼容

日期：2026-10-31。范围：本仓库的 agent-service；Palpo/Pasion 是外部依赖，不声称改写其内部数据库层。不部署、不连接线上数据库。

## 实现

- 从独立 PostgreSQL 重放真实初始化/升级入口，通过 Diesel CLI 2.3.5 生成两个命名空间的 Rust 定义：`crates/agent-service/src/schema/agent.rs`、`project.rs`。生成文件没有手工编辑。
- 193 个查询调用点改为 Diesel DSL，模型保留原生 SQL 的按名解码，并为类型化查询增加 Queryable/Selectable。复杂 CTE、视图、目录检查及 JSON 投影保留参数化 SQL。类型检查明确了密文载荷可空性：待处理状态的读取显式约束载荷非空，原数据库 CHECK 同时保证该条件。
- 新增小型 db-runtime，使用 Horme 0.4.0 / diesel-async / deadpool。普通 Store/Domain/Transport/Inbox 不再长驻业务连接，而在有界回调内借用。事务和 helper 复用该连接，结果返回前归还。
- 默认每个业务池 8 个连接，可通过正整数 `HAGENCY_AGENT_DB_POOL_SIZE` 配置；看板独立池 4 个。Clone 共享池，独立 Store 实例不会共享，需要按实例数/副本数核算数据库总连接预算。
- 同任务回调内再次借用同一个池会在 checkout 前报错。外部 spawn 不继承此检查，不能将其描述为任意调用链的编译期证明。准入上限为 2N，checkout 5 秒，操作和会话清理共 30 秒，另有数据库 statement/lock/idle-transaction 超时。
- 错误不回收不确定会话；取消/超时/panic 会拆离连接、发送 PostgreSQL Cancel、尝试回滚并关闭，清理期间保留准入额度。

## 专用连接与安全语义

成员同步 controller 的 session advisory lock 必须随专用连接存活，不能交给请求池清理。Crypto 身份请求也保留单独串行连接，继续在有界 HTTP 期间锁住真实授权行，防止撤权竞争。两者不会占用普通业务池。

看板初始化/历史升级使用专用连接，不受业务回调 30 秒期限约束；保留原迁移锁和版本检查，迁移 SQL statement timeout 120 秒、lock timeout 30 秒。其他历史初始化/升级 SQL 没有改写。

## 已有数据

本次没有新增表、字段或数据格式，也没有修改任何历史 `.sql` 迁移。发布到已经运行当前 main 的数据库不需要额外数据搬迁。不要用新库替换旧库，不要删除 schema 或重置迁移记录。

新增独立数据库回归重建历史 v6 看板，保存真实用户会话、任务、版本、幂等回执及有效/已删除附件，再执行真实 DomainStore/BoardStore 启动升级和重新打开：身份、会话、任务/回执完整保留；有效附件字节和摘要一致；现有 v7 迁移继续清除已删除附件的内容字节，保留其大小与摘要墓碑。

## 验证与维护

- `scripts/check-db-schema.py` 在显式指定的临时 schema 数据库运行真实初始化，再检查 Diesel CLI 输出；`--write` 才重生成文件。两份单 schema CLI 配置避免将多个 schema 的 stdout 拼到同一个生成文件。
- CI 包含 schema 漂移检查、runtime 单连接池真实 PostgreSQL 回归，以及全部 Agent PostgreSQL 测试。
- 行锁等待测试检查 PostgreSQL 实际阻塞 PID，避免依赖 SQL 字符串的格式/引号。
- 全部本地 target、数据库数据、日志和临时目录使用 `D:/BuildCache/hagency-db-20261031`，Cargo 缓存使用 `D:/BuildCache/cargo`；没有运行 Docker 或生产迁移。
- 最终测试计数在对应 memory 记录中。部分全量测试会留下待发通知；重复整套测试必须另建空的测试数据库，历史数据兼容由独立升级用例验证，不能以清理线上数据代替修复。
