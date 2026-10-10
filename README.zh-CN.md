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

默认关闭注册。初始化工具不会覆盖已有配置。
已有数据请先阅读[迁移说明](docs/guide.zh-CN.md#拆分旧的合并数据库)。

详细配置、注册、源码开发和迁移说明：
[中文](docs/guide.zh-CN.md) · [English](docs/guide.md)。

Hagency 业务架构与迁移：[中文](docs/OPERATIONS.zh-CN.md) · [English](docs/OPERATIONS.md)。

本地操作：[连接与 TLS 排障](docs/LOCAL_DEPLOYMENT.zh-CN.md) ·
[隔离测试](docs/TESTING.zh-CN.md)。

[GitHub CI 与发布](docs/github-ci.md)
