# hagency-server

[English](README.md) · [详细文档](docs/README.zh-CN.md)

在一个 Rust 服务器中运行 Palpo Matrix、Pasion 认证和 Padmin/Hagency 管理界面。

集成 Agent Appservice 是必装组件。用户使用自己的 Matrix 账号通过 Pasion 登录，
永久拥有自己创建的 Agent。Project 对应一个 Space，子 Room 保留独立成员关系。
服务器管理创建权与消息投递，本地 hagency-client 执行 Codex 并控制资源/工具策略。
网页入口为 `/hagency/projects` 与 `/hagency/agents`。

Fleet/Hafleet、Engagement/allocation 审批与 authority import 已删除，无旧数据导入
或兼容模式。Palpo/Pasion 通用管理仍使用原有组件 API。

## 本地开发

macOS 与相邻 `hagency-client` 联调可直接执行
`python3 scripts/local-dev.py start --pull`（或在 Client 执行 `./tools/local-dev.sh start --pull`）。
它自动启动独立 PostgreSQL、真实 Pasion/Matrix/看板后端和原生客户端；
地址为 `http://127.0.0.1:8088`。首次管理员密码保存在
`.run/local-dev/admin-password`。该隔离测试环境默认开放注册，联系方式可留空；
手动部署模板仍默认关闭注册。停止用 `python3 scripts/local-dev.py stop`，
保留配置和数据。依赖、登录与排障见
[完整一键启动指南](../hagency-client/docs/local-development.zh-CN.md)。
构建使用独立产物目录和来源/文件指纹，不导入旧镜像资源；
`python3 scripts/local-dev.py doctor` 核对源码、产物及实际返回的 HTTP 资源。

以下是手动管理环境的方式；不要与一键环境同时占用相同端口。

安装 Rust ≥ 1.99、[just](https://github.com/casey/just)、Docker、Git、curl
和 `libpq`。下面的密码生成示例还需要 OpenSSL。
首次配置时，在仓库根目录执行：

```sh
just init-dev
just db-up
just prepare-pasion
just prepare-frontend
```

配置生成在 `config/dev/`：`hagency.toml`、`palpo.toml`、`pasion.toml`。
一个 PostgreSQL 服务中分别使用 `hagency`、`palpo`、`pasion` 三个数据库。

创建受保护的密码文件，并初始化第一个 Pasion 管理员：

```sh
umask 077
mkdir -p secrets
openssl rand -base64 32 > secrets/admin-password
just run --config config/dev/hagency.toml --bootstrap-admin admin \
  --bootstrap-password-file secrets/admin-password
```

服务器启动后，用 `Ctrl+C` 停止，再启动开发监听：

```sh
just dev
```

打开[管理界面](http://127.0.0.1:8088/login)，选择 **Sign in with Pasion**，
使用 `admin` 和 `secrets/admin-password` 中的密码登录。
Rust 修改会自动编译；前端修改后刷新页面即可。
后续启动只需 `just db-up` 和 `just dev`，不再执行初始化或管理员创建命令。

## Docker 部署

全新部署使用相同的工具依赖，并先按上面的方式创建密码文件。
将 `palpo.instance` 替换为你的域名：

```sh
just init-docker --origin https://palpo.instance --server-name palpo.instance
just db-up
just docker-build
docker compose run --rm --service-ports \
  -v "$PWD/secrets/admin-password:/app/bootstrap-password:ro" server \
  --config /app/config/hagency.toml --bootstrap-admin admin \
  --bootstrap-password-file /run/hagency/bootstrap-password
```

服务器启动后，用 `Ctrl+C` 停止，再执行：

```sh
docker compose up -d server
```

配置位于 `config/docker/`，`.env` 保存生成的数据库密码。
把 `https://palpo.instance` 反向代理到 `127.0.0.1:8088`，保留路径和公开域名的 Host 头。
打开 `https://palpo.instance/login`，通过 Pasion 使用 `admin` 登录。
后续启动只需 `just docker-up`。

schema 34 的原生 CLI 切换要求配套的当前 Desktop 和 owner daemon。
工作进程请求声明 `native-context-v3;claude-v4`，声明在五分钟后失效。
旧版本或离线工作进程对应的消息会保留待处理，并显示更新或重新上线的原因。
恢复连接后继续处理原消息；声明过期不会阻断已启动任务的结算。
这次部署只支持向前升级：`*_down.sql` 是测试夹具，不是生产回滚方案。
只降低版本号不会恢复旧的上下文校验触发器，旧 schema 也不能表示 Claude 运行。
若要重新运行旧二进制，必须恢复切换前的 Hagency 数据库，或使用全新的 Hagency
数据库；仅替换可执行文件不构成回滚。Matrix 和 Pasion 数据库不属于该 Hagency 重置范围。

schema 35 允许所有者设备更换本地账本数据库。设备通过
`POST /hagency/api/v1/management/executor-ledger/declare` 声明新数据库的标识，
此后服务器只在该数据库编号的回执之间检查编号是否重复。
这次迁移只新增表和列：已有回执保持在 epoch 0，从未声明的设备行为不变，
因此可以先部署服务器，再更新客户端。
与 schema 34 一样只支持向前升级：旧二进制遇到 schema 35 数据库会以
`management_schema_incompatible` 拒绝；`management_executor_ledgers_down.sql`
是测试夹具，一旦有设备声明过账本就拒绝执行。

替换已声明数据库的声明，或带有 `"fresh": true` 的首次声明，会在同一事务中结束
旧数据库为该设备留下的内容：资源池和授权被撤销，池化 Agent 被停用，
项目分配和上级结算被关闭，未应答的命令不再下发，也不再接受回执
（`executor_ledger_replaced`）。已记录的回执和历史保持原样，设备从未上报的用量不计入。
其他设备、其他所有者以及该所有者的本地 Agent 不受影响。
不带 `fresh` 的首次声明只是为设备原有的数据库命名，不结束任何内容。
对这类设备保留下来的对象，顺序规则仍然直接比较编号：在声明之前创建的授权、
Agent 或运行，后续回执的编号必须大于已上报的编号。

默认关闭注册。初始化工具不会覆盖已有配置。
已有数据请先阅读[迁移说明](docs/guide.zh-CN.md#拆分旧的合并数据库)。

详细配置、注册、源码开发和迁移说明：
[中文](docs/guide.zh-CN.md) · [English](docs/guide.md)。

Hagency 业务架构与迁移：[中文](docs/OPERATIONS.zh-CN.md) · [English](docs/OPERATIONS.md)。

本地操作：[连接与 TLS 排障](docs/LOCAL_DEPLOYMENT.zh-CN.md) ·
[隔离测试](docs/TESTING.zh-CN.md)。

[GitHub CI 与发布](docs/github-ci.md)
