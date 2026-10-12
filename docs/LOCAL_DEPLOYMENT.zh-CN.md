# 本地部署与连接排障

[English](LOCAL_DEPLOYMENT.md) · [文档索引](README.zh-CN.md)

首次安装和管理员 bootstrap 按[根 README](../README.zh-CN.md)，完整配置参考
[配置指南](guide.zh-CN.md)。这里说明后续启动与浏览器、Desktop、owner client 的连接。
命令均在 server 仓库根目录运行。

## 选择一种服务运行方式

| 方式 | 配置 | 服务 / 数据库 | 启动 |
| --- | --- | --- | --- |
| 源码开发 | `config/dev/*.toml` | `127.0.0.1:8088` / PostgreSQL `127.0.0.1:55438` | `just db-up` 后 `just dev` |
| Compose 部署 | `config/docker/*.toml`、`.env` | 宿主 `127.0.0.1:8088` / 容器 `postgres:5432` | `just docker-up` |
| 协议测试 fixture | 临时私人配置 | 随机 loopback 端口 / 随机数据库 | [测试指南](TESTING.zh-CN.md) |

源码服务和 Compose server 默认争用同一宿主端口。只运行所选服务，同一组数据库
只允许一个写入进程。Docker 初始化生成新 `.env` 密码后，应核对 dev 的私人数据库
URL 与实际凭据：已有数据卷保留原密码，修改 `.env` 不会修改数据库密码。
初始化 SQL 仅新卷首次运行；不要用 `down -v` 解决缺库或密码不匹配。

```sh
just check-config config/dev/hagency.toml
curl --fail --silent --show-error http://127.0.0.1:8088/healthz
curl --fail --silent --show-error http://127.0.0.1:8088/readyz
curl --fail --silent --show-error http://127.0.0.1:8088/api/hagency/v1/discovery
curl --fail --silent --show-error http://127.0.0.1:8088/_matrix/client/versions
curl --fail --silent --show-error http://127.0.0.1:8088/_matrix/client/v1/auth_metadata
curl --fail --silent --show-error http://127.0.0.1:8088/auth/.well-known/openid-configuration
```

`--check-config` 读取引用配置并检查 Pasion 委托认证要求，不连接数据库或验证 TLS。
`/healthz` 只证明监听器健康；`/readyz` 要求集成 Appservice 的实际启动收发探针成功，
这是启动 latch，不是持续传输健康探针；
仅 health 200 不足以证明 Agent 创建就绪。就绪也不代表用户的异步 Matrix 身份配置完成。

Web 控制台访问 `/login`。Desktop/client 登录填写服务器 **origin**，如
`http://127.0.0.1:8088`，不追加 `/login` 或 `/auth/`。开发 OAuth 的 HTTP 仅限
loopback IP，本地域名需要可信 HTTPS。无需 Fleet 注册、`[fleet_access]`、
用户 Appservice 密钥或凭据导入。

## HTTPS 与容器访问

新 HTTPS 部署按 README 使用最终公开 origin 与稳定 Matrix server name 生成配置。
反向代理将所有路径转发至 8088，保留公开 Host，不改写子路径。
issuer 必须是 `<public_origin>/auth/`，发现、token、callback、Matrix metadata
需一致。复用数据库时保留原 `server_name` 与签名密钥。

`hagency.local` 等私人域名必须在浏览器、原生客户端和 **server 容器内**均可解析且
通过证书验证。宿主 `127.0.0.1` 不是容器 loopback；容器访问宿主代理可能需要
`host.docker.internal` 或 Compose `extra_hosts`。本地 CA 需加入相应信任库，
不要禁用 TLS 验证。代理需监听路由实际指向的接口；宿主 IPv6 成功不证明容器 IPv4 可用。

```sh
curl -4 --fail --silent --show-error https://hagency.local/auth/.well-known/openid-configuration
curl -6 --fail --silent --show-error https://hagency.local/auth/.well-known/openid-configuration
docker compose exec server curl --fail --silent --show-error \
  https://hagency.local/auth/.well-known/openid-configuration
docker compose exec server curl --fail --silent --show-error \
  https://hagency.local/_matrix/client/v1/auth_metadata
```

仅配置 IPv6 时才测试该路径。代理若在其他容器/网络，应使用实际网络路由。
若采用宿主 LAN IP，在私人本地配置记录中注明，并在地址变化后同步代理 bind 与
容器映射。其他项目可能占用 IPv4 443 并返回另一证书，同时 IPv6 正常；
先确认接口/端口归属，再处理，避免修改无关项目的服务。

## 部署维护

```sh
docker compose ps
docker compose logs --tail 100 server
# 修改宿主配置后，重新生成容器的运行时副本。
docker compose up -d --force-recreate server
# 源码变化后构建并替换 server。
just docker-build
docker compose up -d server
# 停止服务，保留两个命名数据卷。
just docker-down
```

entrypoint 将 `config/docker/` 复制为受保护的 `/run/hagency/config/`，以 UID 10001
运行。修改挂载配置需重启/重建容器才能更新副本；配置树中的符号链接会被拒绝。
引用的文件须在容器内可用且可读，不能只存在于宿主。

升级前记录源码/镜像版本，按[恢复指南](RECOVERY.zh-CN.md)停机一致备份三个数据库、
配置、密钥和媒体；`database` 与 `server-data` 两个卷都重要。后续启动不带 bootstrap，
初始化和 bootstrap 只用于首次安装。旧 Fleet 业务数据没有自动导入；旧合并库拆分
说明针对组件存储，不恢复已经移除的 Hagency 业务架构。

## 常见故障

| 现象 | 检查 |
| --- | --- |
| 地址占用 | 源码 watcher、Compose server、同接口/端口代理 |
| 缺库或密码错误 | 数据卷初始化历史、三个私人连接 URL |
| 空页面或资源 404 | 两个 prepare 命令、`public_dir`、Pasion resources |
| health 200、ready 503 | 内部 Appservice 注册/收发与服务日志，不绕过就绪门禁 |
| Matrix auth_metadata 400 / issuer 获取失败 | 容器 DNS、CA 信任、IPv4/IPv6、公开 issuer 路由 |
| 登录成功但 Agent 操作拒绝 | 当前账号、独立 Space/Room 成员权限、创建策略、服务暂停 |
| 成员离开后绑定 suspended | 恢复实际权限，再按允许范围显式重新采纳/恢复；旧 dispatch 继续隔离 |
| 模型未执行 | 本机模型账号、模型、额度、活动绑定、显式启动范围；server 启动不代表推理 |
