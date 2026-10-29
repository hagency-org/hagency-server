# 本地测试

[English](TESTING.md) · [文档索引](README.zh-CN.md)

所有命令在 server 仓库根目录运行，使用仓库固定的 Rust 工具链、Docker Compose。
`scripts/*.py` 需要 Python 3；只有 `tests/*.mjs` 需要 Node.js。
先按[开发指南](guide.zh-CN.md#开发环境)准备配置和网页资源。构建成功、HTTP 健康、
Appservice 就绪和实际模型回答分别验证不同层次，报告时必须区分。

## 静态检查和 Rust 测试

```sh
just check-tools
just check-agents
python3 scripts/validate-agent-openapi.py
cargo fmt --all -- --check
cargo test --locked -p hagency-server
cargo clippy --locked -p hagency-server --all-targets -- -D warnings
cargo check --locked -p hagency-frontend --target wasm32-unknown-unknown
```

`check-agents` 执行 Agent 单元测试与严格 Clippy；ignored 数据库测试需要下面的
专用 runner 才算执行。OpenAPI 检查验证代码与接口契约漂移，不代替 HTTP 测试。

## 隔离 PostgreSQL 回归

```sh
just db-up
just check-agents-postgres
```

runner 从 Compose PostgreSQL 容器读取凭据，创建随机 `hagency_agent_test_*`
数据库，执行包含 ignored 的 Agent 测试，在 `finally` 中删除该数据库。Cargo 使用
`--offline`，新 checkout 需先获取/构建锁定依赖。它不迁移应用 Agent 数据库；
选择的 PostgreSQL 服务应为测试基础设施。

不同 Compose 项目或端口需显式指定实际值：

```sh
HAGENCY_TEST_POSTGRES_CONTAINER=my-test-postgres-1 \
HAGENCY_TEST_POSTGRES_PORT=55439 just check-agents-postgres
```

进程被外部中断时，先核对遗留库，只删除这次运行的 fixture 库。不要用删除 Compose
数据卷的方式清理测试。

## 实际内嵌 Pasion/Palpo 集成

首次按根 README 生成 dev 配置和网页资源，然后运行：

```sh
cargo build --locked -p hagency-server --bin hagency-server
HAGENCY_TEST_SERVER_BINARY="$PWD/target/debug/hagency-server" \
HAGENCY_TEST_NATIVE_PKCE=1 python3 scripts/test-agent-integration.py
```

必须显式指定 `HAGENCY_TEST_SERVER_BINARY`：脚本当前默认值指向开发者另一个
checkout。脚本使用环境变量，没有 CLI 参数解析器。它读取 `config/dev/palpo.toml`
和 `pasion.toml` 作为模板，替换数据库、身份与存储路径，在随机 loopback 端口启动
独立 backend 并创建临时主人账号；资源来自 `resources/frontend/public` 和
`resources/pasion`。

| 环境变量 | 用途 |
| --- | --- |
| `HAGENCY_TEST_SERVER_BINARY` | 本次源码构建出的二进制绝对路径 |
| `HAGENCY_TEST_POSTGRES_CONTAINER` | 默认 `hagency-server-postgres-1` |
| `HAGENCY_TEST_POSTGRES_PORT` | 默认 `55438`，宿主 loopback 端口 |
| `HAGENCY_TEST_NATIVE_PKCE=1` | 实际 Pasion DCR/PKCE、Agent 与 Matrix 往返 |
| `HAGENCY_TEST_BACKUP_RESTORE=1` | 同时启用 PKCE 与停机后三库/文件恢复演练 |
| `HAGENCY_TEST_HOLD_CLIENT=1` / `HAGENCY_TEST_HOLD_PKCE=1` | 交互 fixture，会等待操作者，不适合无人值守 CI |

恢复演练也必须指定本次二进制：

```sh
HAGENCY_TEST_SERVER_BINARY="$PWD/target/debug/hagency-server" \
HAGENCY_TEST_BACKUP_RESTORE=1 python3 scripts/test-agent-integration.py
```

脚本创建随机 `hagency_smoke_*` 数据库，尝试清理全部原始/恢复库，停止自己启动的
服务；成功后删除私人 fixture，失败则保留日志与配置用于排障，这些文件可能含凭据。
测试覆盖实际认证、账号配置、消息传输与恢复，不调用 Codex 或工具执行器。
参考[恢复演练边界](BACKUP_RESTORE_VALIDATION.zh-CN.md)。

## Desktop/client 验收

先按[本地部署排障](LOCAL_DEPLOYMENT.zh-CN.md)检查，再使用
[Desktop 开发指南](../../hagency-desktop/docs/local-development.zh-CN.md)或
[owner-client 指南](../../hagency-client/docs/local-development.zh-CN.md)。先验证浏览器
登录、Project/Room 与主人权限，再显式启动 Agent。推理需要自己的模型账号并消耗
配置的资源。协议 fixture 的 Matrix 回复不证明实际模型执行。

[VALIDATION.zh-CN.md](VALIDATION.zh-CN.md) 是历史执行记录，不是当前测试清单。


账号路径和页面权限变更可通过真实浏览器验证：
`HAGENCY_TEST_HOLD_BROWSER=1` 会在隔离服务启动后保留私有 `browser-fixture.json`，
供成员/管理员 UI 检查。检查完成后写入同目录的 `browser-done.json`，
内容为 `{"passed":true}`，文件权限为 0600，再继续原集成流程。
`HAGENCY_TEST_PASION_RESOURCES` 与 `HAGENCY_TEST_FRONTEND_RESOURCES` 可指定
本次构建的绝对资源目录，避免替换正在运行的其他部署的资源。

## 依赖与部署检查

`python3 -m unittest discover -s scripts -p 'test_*.py'` 检查审计规则和构建工具。
`python3 scripts/audit-dependencies.py` 获取公告并执行有时限的风险策略；策略通过
不等于 cargo-audit 没有命中，完整剩余记录见 `docs/DEPENDENCY_SECURITY.md`。

`Embedded Deployment` 工作流在 PR 中构建不发布的 amd64 镜像，真实执行 Pasion
PKCE、Matrix AS 往返、投递、SIGTERM 排空和重启。手动选择 backup_restore 或
每周运行时，还从同一镜像提取二进制与资源执行三库/私有文件恢复；只使用新建
fixture 数据库、目录和容器，不接触现有部署，也不调用模型。
