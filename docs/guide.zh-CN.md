# 配置与开发指南

[English](guide.md) · [快速配置](../README.zh-CN.md) · [文档目录](README.zh-CN.md)

Palpo Matrix homeserver、Pasion 认证和 Palpo web-admin 的功能运行在**同一个 Rust 进程、同一个 HTTP 监听端口**中。
后端使用 Rust、Tokio、Salvo、Diesel/diesel-async 和 PostgreSQL，与 Palpo 技术栈一致。生产环境不运行 Node 服务器。

Cargo workspace 采用类似 Pasion 的后端、前端组织方式：

```text
crates/
├─ backend/   # hagency-server：内嵌 Palpo、Pasion 和 Hagency API
├─ agent-service/ # 永久 Agent 域和投递协议
└─ frontend/  # hagency-frontend：基于复制的 Padmin 源码，使用 Dioxus/WASM
xtask/       # 开发、配置工具（workspace 成员）
resources/   # 生成的前端与 Pasion 资源，不提交到 Git
```

前端将 Padmin 的通用 Matrix/Pasion 管理页面与新的永久 Agent 归属、Project/Room 绑定和创建政策界面结合。
Rust 后端在 `/` 提供无需登录的站点首页；成员 Home 位于 `/hagency/home`，项目和我的 Agent 位于 `/hagency/projects`、`/hagency/agents`，管理员页面位于 `/admin` 下。Matrix 路径为 `/_matrix`，Palpo 管理 API 为 `/_palpo`，Matrix 发现路径为 `/.well-known/matrix`，原生 Hagency API 为 `/hagency/api/v1`，网页管理 BFF 为 `/hagency/api/browser/v1`，Pasion 挂载在 `/auth/`。

网页登录、会话查询和退出分别使用 `/hagency/api/login/token`、`/hagency/api/session` 和 `/hagency/api/logout`。Hagency 自定义 API 统一位于 `/hagency/` 下；`/_matrix/` 仅承载 Matrix 协议定义的接口，包括标准 Appservice 回调。旧 `/api/hagency/v1`、`/api/browser/hagency/v1` 及网页会话地址返回 404。升级时需同步更新 Server、Client/Desktop 和网页资源；此次路径变更不改变 OAuth issuer 或存储中的用户身份。
`/healthz` 表示进程 HTTP 服务可访问，不代表异步账号同步已经完成。

## 目录

- [架构](#架构)
- [开发环境](#开发环境)
- [统一前端认证](#统一前端认证)
- [自定义公共首页](#自定义公共首页)
- [组件配置与首次管理员](#组件配置与首次管理员)
- [Compose 部署](#compose-部署)
- [保留的业务行为与限制](#保留的业务行为与限制)
- [验证方法](#验证方法)
- [内嵌 Pasion](#内嵌-pasion)
- [数据库迁移](#拆分旧的合并数据库)
- [配置迁移](#将旧的单一配置拆成组件配置)

以下命令均在仓库根目录执行。

## 架构

```text
hagency-server（一个进程、一个端口）
├─ MatrixServer：Palpo 初始化、Matrix/管理/发现路由及后台任务
├─ BrowserAuth/BFF：个人会话与 Project/Agent 管理
├─ 必装 Agent Appservice：持久化 inbox、设备租约 fencing 与回复 outbox
├─ PasionServer：/auth/ 下的 OAuth/OIDC、账号界面/API 和后台任务
└─ 集成 Padmin + Hagency 的 Dioxus/WASM 静态资源
           │
           └─ PostgreSQL 服务
              ├─ hagency：新 Agent 域和持久化 inbox/outbox
              ├─ palpo：Matrix homeserver 数据
              └─ pasion：认证数据
```

Palpo 以 Rust 库形式链接，不作为独立进程启动，也不依赖外部反向代理连接。
Matrix 授权和 App Service 操作仍由 Palpo 的 API 验证。web-admin 的 Rust 适配层通过固定的 loopback 地址调用**同一个进程**挂载的 Matrix/管理 API，以保留原有授权语义。
内部地址从 `listen` 推导，公开传输地址从 `public_origin` 推导。

Palpo 当前配置与连接池是全局单例，因此每个进程只允许一个 MatrixServer。
宿主负责 Tokio、tracing、HTTP 关闭流程和 TLS 终止方式；二进制将 Tokio 工作线程栈设置为 8 MiB。
Matrix 后台任务随运行时停止。默认 Compose 的 HTTP 服务由前置 HTTPS 代理提供 TLS。

## 开发环境

依赖 Rust ≥ 1.99、[just](https://github.com/casey/just)、PostgreSQL 客户端库 `libpq`，以及用于启动 PostgreSQL 的 Docker。
Pasion 资源构建还需要 `wasm32-unknown-unknown` 目标和 Dioxus 0.7.5 资源；准备工具会在需要时下载匹配的 Dioxus CLI。
Git 和 curl 用于获取工具或源码。两个前端都通过 Rust 和 Dioxus CLI 构建，不需要 Node.js 或 npm。
`tests/` 下 JavaScript 测试需要可选的 Node.js；当前 `scripts/` 下 Agent PostgreSQL、集成与 OpenAPI 检查需要 Python 3。
`rust-toolchain.toml` 固定 Rust 1.99.0，并配置 rustfmt、Clippy 和 WASM 目标；Docker 构建阶段使用对应的 Rust 1.99 镜像。

`just --list` 显示全部命令。Just 负责组织命令；workspace 中的 Rust `xtask` 实现私有配置生成、资源准备和开发监听，运行这些工具不需要先编译服务器。

```sh
just init-dev
just db-up
just prepare-pasion
just prepare-frontend
# 新数据库需要先按后文说明创建管理员。
just dev
```

打开 `http://127.0.0.1:8088`。开发配置的固定 Matrix 服务器名是 `localhost:8088`。
后端 Rust 修改编译成功后会优雅重启；编译失败时保留上一次正常运行的服务器。
前端 Rust/CSS 修改会重新生成 WASM 资源，运行中的宿主直接读取这些资源，刷新浏览器即可生效。仅前端修改不会重启后端，不需要重新构建 Docker 镜像。

浏览器 access/refresh token 只保存在内存中。整页刷新后 Project/Agent 页面通过
`/hagency/api/session` 验证 HttpOnly 会话并使用封闭 BFF；通用 Matrix 管理需要 bearer 时重新
进入 Pasion PKCE。会话到期或撤销需重新登录，token 不写入浏览器持久存储。

开发 Palpo 源码时，使用具有 MatrixServer API 的本地仓库：

```sh
just dev --palpo-source /absolute/path/to/palpo-checkout
```

开发工具在忽略的 `.run/` 下生成本地 Cargo patch，监听 Palpo 源码变化并重新编译，不修改上游仓库，也不需要重建 Docker 镜像。
在固定 Git 依赖和本地 patch 之间切换时，Cargo 可能更新锁文件；移除覆盖后可用 `cargo update -p palpo` 恢复普通依赖。

复制的 Padmin 还包含可选 `palpo_admin` sidecar 的维护、定时命令和通知页面。
这个独立 sidecar 未内嵌，因此对应菜单保持禁用。Matrix 用户、房间、媒体、注册和 App Service 管理直接使用挂载的 Palpo API。

## 统一前端认证

Pasion 是默认身份提供方。宿主通过 `/config.json` 发布非敏感运行配置，前端只有一个 **Sign in with Pasion** 入口，使用 Authorization Code + PKCE。
前端读取已登录 Pasion 账号的角色，仅为管理员申请管理 scope。
全新浏览器中的管理员登录会先确认身份，再为同一个账号继续申请管理权限；普通成员不会获得管理 scope。

Pasion 管理密码、注册、账号状态和管理员角色，后台 provisioning 任务将对应 Matrix 身份和管理员标记同步到 Palpo。
在 `/admin/pasion/accounts` 管理人员账号，`/admin/users` 查看 Matrix 记录和服务身份。修改密码和创建人员账号走 Pasion。
Hagency 保存永久 Agent owner、Project/Room 创建权和持久化投递，不建立另一套密码账号库。

宿主创建预留的公开 OAuth 客户端，并自动推导 `/oauth/callback`，无需客户端密钥或单独部署前端。
`POST /hagency/api/login/token` 验证 token 后发放 HttpOnly 会话 cookie 和 CSRF token。
Palpo 管理接口还在宿主边界检查 Pasion 实时 introspection 与精确的 `urn:palpo:admin:*` scope；即使账号是管理员，仅有成员 scope 的 token 也无法管理服务器。
关闭 introspection 缓存后，撤销角色或 token 会在下一次请求生效。Pasion 自身的管理 API 也检查 scope 和当前角色。

集成部署要求 Pasion 委托认证，没有 Hagency 原生密码或账号审批兼容模式。

前端来源和上游许可证信息见根目录 `NOTICE`。
复制的 Padmin 基于提交 `83d4567470ada808b89914aa0c786cdc3a7ac89a`。

## 客户端 Pasion 登录与自助接入

`chrislearn/hagency-client` 使用用户自己的 Matrix 账号通过 Pasion PKCE 登录。
原生 `/hagency/api/v1` 核验 Pasion proof 与 Matrix whoami 后建立短期用户授权、
登记本地设备；拒绝 browser Cookie/Origin。设备轮换/撤销、session 与真实时钟在
敏感事务内重验。

集成 Appservice 必装并由服务器管理，用户无需登记 Fleet 或自行安装 Appservice。
Agent 的 owner 永久固定，Project/Room bindings 独立。Space 管理者设置成员默认
创建权和 allow/deny 清单，deny 优先；禁止新建不会暂停已有 Agent。本地 Codex、
资源额度和工具风险决策由客户端管理。

网页使用封闭 `/hagency/api/browser/v1` BFF，页面为 `/hagency/projects` 与
`/hagency/agents`，不向网页返回原生授权/设备凭据。详见 [Agent 架构](OPERATIONS.zh-CN.md)。

## 公开首页

公开路径 `/` 与 `/login` 使用同一登录页面，沿用控制台的 Soft Orbit 样式、
中英文切换和明暗主题。使用个人账号登录后进入 `/hagency/home`；已有经过验证的
网页会话可以直接进入工作台。启用 Pasion 时显示创建账号与账号中心入口。
管理页面继续执行原有权限检查。

首页是随前端一起构建的页面，不再提供 `home_page` 配置、本地 HTML 覆盖或
远程页面获取。升级时请删除 hagency.toml 中已有的 `home_page` 字段。
Palpo 组件的首页配置不控制 Hagency 首页。

## 组件配置与首次管理员

配置由各组件分别持有：

```text
config/
├─ examples/       # 提交到 Git 的原生配置模板
│  ├─ hagency.toml
│  ├─ palpo.toml
│  └─ pasion.toml
├─ dev/            # 生成的开发配置，Git 忽略，文件权限 0600
│  ├─ hagency.toml
│  ├─ palpo.toml
│  └─ pasion.toml
└─ docker/         # 生成的部署配置，Git 忽略，文件权限 0600
   ├─ hagency.toml
   ├─ palpo.toml
   └─ pasion.toml
```

启动参数为 `--config config/dev/hagency.toml`，也是 CLI 默认值。
Hagency 管理监听、管理连接、数据目录，并引用其余两个文件：

```toml
palpo_config = "palpo.toml"
pasion_config = "pasion.toml"
```

Palpo 使用原生顶层 ServerConfig，包括 `[db]`、`[storage]`、`[well_known]`，不套 `[matrix]`。
Pasion 使用原生 `[database]`、`[account]`、`[email]`、`[[clients]]`、`[[upstream_oauth2.providers]]` 等段，不套 `[pasion.settings]`。
额外的 `[hagency]` 段保存内嵌选项 `resources_dir` 和 `delegate_matrix_auth`。

| 配置项 | 数据库 | 内容 |
| --- | --- | --- |
| `hagency.toml` 的 `database_url` | `hagency` | 新 Agent 身份/域、AS inbox 和回复 outbox |
| `palpo.toml` 的 `db.url` | `palpo` | Matrix 用户、房间、事件和 homeserver 状态 |
| `pasion.toml` 的 `database.uri` | `pasion` | 账号、OAuth/OIDC token 和会话 |

数据库名必须不同。当前集成 Agent server 必须引用 `pasion_config`，并在 Pasion
配置的 `[hagency]` 下设置 `delegate_matrix_auth = true`。缺失或关闭委托会被启动与
`--check-config` 拒绝。
组件引用、媒体路径及支持的原生密钥文件引用，均相对于声明它们的配置文件解析。
开发监听会监控所有三个文件，也包括主配置目录之外的引用文件。

`--check-config` 加载并检查全部引用文件，不启动服务器；`--list-config-files` 输出路径 JSON，不连接数据库。
内嵌模式下使用 Hagency 的监听配置，覆盖 Palpo/Pasion 的监听配置。
如果 `palpo.toml` 没有显式 `[keypair]`，首次生成 Matrix 签名密钥，保存为 `data_dir/matrix-signing-key.json`，权限 0600。
数据目录与 PostgreSQL 应一同备份。`server_name` 是 Matrix 身份的一部分，复用数据库时不能改变。

默认关闭注册。为新数据库创建第一个管理员时，先把强密码放进权限受保护的 `secrets/admin-password` 文件，再启动应用：

```sh
# 将强密码保存到受保护的 secrets/admin-password 文件。
just run --config config/dev/hagency.toml --bootstrap-admin admin \
  --bootstrap-password-file secrets/admin-password
```

默认 Pasion 委托模式下，bootstrap 按 Pasion 密码策略创建首个管理员，排队执行正常的 Matrix 身份及角色同步。
通过 Pasion 使用 `admin` 登录，Matrix 身份是 `@admin:localhost:8088`。
bootstrap 不会覆盖现有 Pasion 账号；已有 Pasion 管理员时也会拒绝执行。后续管理员角色在账号管理中授予。

对于以前使用原生 Matrix 认证初始化的部署，可显式关联现有活跃人员管理员，使用相同用户名和选定的 Pasion 密码文件：

```sh
just run --config config/dev/hagency.toml --bootstrap-admin admin \
  --bootstrap-password-file secrets/admin-password --link-existing-matrix-admin
```

关联会保留 Matrix ID、房间和 Hagency 所有权。访客、服务账号、暂停、停用或非管理员身份不能关联。
其他原生账号及密码不会自动导入；开启委托前应先准备或迁移对应 Pasion 账号。
原密码文件满足 Pasion 策略时可以复用。执行 bootstrap 前停止开发监听，确保一个数据库只对应一个服务器进程；后续启动省略全部 bootstrap 参数。

旧 Fleet/账号审批/退役 token/业务提醒配置会被拒绝。人员注册与账号管理由 Pasion 处理。

## Compose 部署

```sh
just init-docker --origin https://palpo.instance \
  --server-name palpo.instance
just db-up
just docker-build
```

这会创建 `config/docker/{hagency,palpo,pasion}.toml`，以及保存随机数据库密码的受保护 `.env`。
开发生成器对应创建 `config/dev/`；`--output-dir` 可指定其他目录。生成器拒绝覆盖已有目录。
首个命令只准备配置，后两个准备数据库和镜像，供下文创建管理员使用。
Compose 部署 PostgreSQL 与单个 hagency-server 进程。Docker 构建使用一个编译任务，并降低大型 Palpo crate 的优化等级以减少峰值内存；其加密、HTTP 和数据库依赖仍保持完整优化。

把仅暴露在 loopback 上的 HTTP 端口放在 HTTPS 代理后，保留公开 Host 头。
将 `/`、`/api`、`/_matrix`、`/_palpo`、`/auth/`、`/.well-known/matrix` 和 `/healthz` 转发到 8088。
浏览器、管理和 outbound 客户端使用公开 origin；Palpo App Service relay 使用自动推导的内部 origin。
无需为 web-admin 单独配置 origin、端口或 relay URL。

Compose 把 `config/docker/` 挂载到 `/app/config/`。
入口脚本先复制配置树到 `/run/hagency/config/`，目录权限 0700、文件权限 0600，再切换到 UID 10001。
组件文件之间及目录内密钥文件的相对引用保持有效，Rust 进程以非 root 用户运行。
生成的生产路径是绝对路径；可选挂载的账号配置和 token 文件也应使用绝对路径，并允许 UID 10001 读取。

在 PostgreSQL 健康后创建首个管理员：

```sh
docker compose run --rm --service-ports \
  -v "$PWD/secrets/admin-password:/app/bootstrap-password:ro" server \
  --config /app/config/hagency.toml --bootstrap-admin admin \
  --bootstrap-password-file /run/hagency/bootstrap-password
```

账号创建后停止这个初始进程，使用 `docker compose up -d server` 正常启动。
已初始化的部署可使用 `docker compose up -d --build`。
bootstrap 拒绝覆盖已有账号，不应在镜像或配置里放置共享默认管理员密码。

## 保留的业务行为与限制

Palpo/Pasion 原有用户、Room/Space、媒体、举报、Appservice、联邦、服务器操作/
通知及账号管理保留，仍使用真实组件 API 和授权检查。新功能见 [Agent 架构](OPERATIONS.zh-CN.md)
与 [能力清单](WEB_ADMIN_PARITY.zh-CN.md)。

Fleet/Hafleet、Engagement、资源 allocation 审批、账号审批 Room、旧 enrollment、
Miniapp aliases 和 authority import 已删除，无旧模式或数据转换。新域/队列使用
`hagency_agent_v1`。加密 Room 执行等待客户端 crypto，存储密文不代表已执行；
租约撤销也不能撤回本地已经发生的 tool 副作用。

## 验证方法

```sh
just check-tools
just check-agents
just check-agents-postgres
cargo check --all-targets --locked
cargo test --locked -p hagency-server
cargo clippy --locked -p hagency-server --all-targets -- -D warnings
cargo check --locked -p hagency-frontend --target wasm32-unknown-unknown
```

PG runner 创建/删除随机独立测试库，不应对现有业务库运行测试迁移。真实 Pasion PKCE、
Matrix 与新 Agent 投递检查使用 `scripts/test-agent-integration.py`，需用
`HAGENCY_TEST_SERVER_BINARY` 指定当前 checkout 构建出的二进制；前置条件、环境变量和
清理步骤见[可执行测试指南](TESTING.zh-CN.md)。
通用 Pasion/Compose 检查保留 `tests/pasion-integration.mjs` 与 `tests/docker-smoke.mjs`，
需要其专用测试资源/数据库。旧 Fleet contract scripts 与 fixture server 已移除。

BrowserAuth 回归覆盖真实身份/管理员撤销、cookie/CSRF/同源、退出、封闭 BFF 与
旧路径不可达。完整宿主还需验证旧 API 为 404/410、Palpo/Pasion 通用 API 正常可用。
历史证据与边界见 [验证记录](VALIDATION.zh-CN.md)。

### 内嵌 Pasion

Pasion 与 Palpo、web-admin 共用 Rust 进程和端口。
内嵌 API 及子路径支持见[Pasion PR #102](https://github.com/meldry-com/pasion/pull/102)。

| 组件 | URL |
| --- | --- |
| 整合的管理前端 | `/` |
| Matrix 客户端/联邦/管理 API | 原 Matrix/Palpo 路径 |
| Pasion 账号界面 | `/auth/`、`/auth/login` |
| OIDC 发现 | `/auth/.well-known/openid-configuration` |
| OIDC issuer | `<public_origin>/auth/` |
| OAuth token / JWKS | `/auth/oauth2/token`、`/auth/oauth2/keys.json` |

无需单独的 Pasion daemon、公开 IP、Node 服务器或代理路径重写。
Pasion 后台任务通过同一监听端口的 loopback 地址调用受保护的 Palpo MAS API。
宿主生成并持久化 OAuth 签名密钥、cookie 加密密钥和 Matrix 共享密钥，保存在权限 0600 的 `data/pasion-secrets.json`，应随数据库一起备份。

三个组件在同一个 PostgreSQL 服务中使用**独立数据库**。
Compose 通过 `POSTGRES_DB` 创建 `hagency`，首次初始化新卷时 `deploy/databases.sql` 创建 `palpo` 和 `pasion`。
已有卷不会重新执行初始化脚本。升级旧的 Hagency/Palpo 合并数据库前，请先完成[数据库拆分](#拆分旧的合并数据库)。

在 `hagency.toml` 用 `pasion_config` 引用 `pasion.toml` 即可启用内嵌，生成的配置默认包含该引用。
Pasion 数据库配置放在自身文件的 `[database].uri`，资源目录放在 `[hagency].resources_dir`。
原生邮件、SMS、账号注册、客户端、上游 OAuth 提供方、连接池上限、限流和品牌设置直接写在 `pasion.toml`。
网络/issuer、Matrix 连接、模板、存储及密钥归属由 Hagency 推导，这些段不能覆盖宿主连接设置。
原生数据库连接池和 TLS 设置会保留，三个数据库依然独立。

本地开发时先构建一次 Pasion Dioxus WASM 前端及资源：

```sh
just prepare-pasion
just dev --pasion-source /path/to/pasion --palpo-source /path/to/palpo
```

`just prepare-pasion` 默认使用固定版本源码，`--source` 可指定本地仓库。
命令会在需要时安装 Rust `wasm32-unknown-unknown` 目标。
工具复用 `dx` 0.7.5，或下载并校验匹配版本到 `.run/tools/`，不替换全局已安装版本。
Hagency 的注册可用性修复保存在 `xtask/patches/`，构建时应用于独立的
`.run/pasion-frontend-source/` 副本；不改写固定版本源码、Cargo 缓存或 `--source`
指定的仓库。上游源码不再匹配补丁时构建会明确失败。
开发监听会重新构建修改后的 Pasion 后端/前端，复制模板、翻译和策略，再优雅替换服务器，不需要重建镜像。
Docker 镜像构建过程会构建并打包这些资源。

本地注册测试先启用 `[account].password_registration_enabled`，再在 `config/dev/pasion.toml` 设置：

```toml
[experimental]
fixed_verification_code = "123456"
[email.provider]
type = "blackhole"
[sms.provider]
type = "blackhole"
```

邮件、SMS 联系方式验证会保存 `123456`，不发送验证通知。
在验证页面填写该值即可；若注册开始于设置启用之前，先重新发送一次验证码。
开发监听会重载配置。本地测试之外不要设置固定验证码。
可用 `[account].password_registration_contact_required = false` 取消注册时必填联系方式。

生成的配置默认启用 Pasion 委托。宿主自动连接 Palpo 发现、token introspection 和兼容的 Matrix 密码登录。
`/auth/` 是账号中心，`/auth/register` 是注册页，`/login` 是整合管理界面的登录入口。
注册使用 Pasion 自有注册/访问政策。旧 `/account-request`、账号审批 worker 与菜单已删除，无原生认证兼容模式。

Pasion 不实现旧 Matrix SSO 重定向。OAuth 客户端使用挂载的 issuer，旧 Matrix 密码客户端走委托密码登录。
保留的业务流程和认证改动见[web-admin 功能对照](WEB_ADMIN_PARITY.zh-CN.md)。

项目清单声明 Apache-2.0，项目仅保留一个许可证文件 `LICENSE`。
`NOTICE` 保留 Palpo、Padmin、Pasion 的来源与上游许可证链接。
Padmin 源码和 Pasion 依赖仍保留其上游 AGPL 许可；修改项目清单不会重新许可这些代码。

### 拆分旧的合并数据库

本版本不迁移旧 Hagency document/Fleet/Engagement 数据。保留 Palpo/Pasion
数据库 URL、Matrix server identity 与签名密钥/媒体文件；为新的 `hagency_agent_v1`
配置独立 Hagency 数据库。启动不会重命名、删除或复制现有业务表。

此前合并数据库的部署须先独立审查备份与组件分离方案；复制 `hagency_admin_state`
到新服务不是支持的迁移方法。

### 将旧的单一配置拆成组件配置

已有三个独立数据库的部署，在拆分配置时保留数据库 URL、服务器身份、密钥/媒体路径及认证设置：

1. 把宿主设置放进 `hagency.toml`，用 `palpo_config` 和必需的 `pasion_config` 文件引用替代内联组件段。
2. 将 `[matrix]` 内容移到 `palpo.toml` 顶层，去除子段的 `matrix.` 前缀，例如 `[matrix.db]` 改为 `[db]`。
3. 将 `[pasion].database_url` 改为 `pasion.toml` 中的 `[database].uri`；`resources_dir`、`delegate_matrix_auth` 移入 `[hagency]`；展开 `[pasion.settings.*]` 到对应原生段。
4. 按新文件位置调整相对路径，或继续使用绝对路径；通过 `hagency-server --config <path>/hagency.toml --check-config` 检查。

旧的私有 `config.dev.toml`、`config.docker.toml` 不会自动改写或导入，迁移期间应保留。
如果数据库仍合并，先独立审查组件分离方案，再启用新配置。


## 成员页面与管理员页面

登录后的首页 `/hagency/home`、项目 `/hagency/projects` 和我的 Agent `/hagency/agents`
普通成员和管理员均可访问。公共站点首页 `/` 无需登录。全站用户/房间管理、审核、身份服务管理、
服务器操作和管理仪表盘放在 `/admin` 下。页面加载时验证当前登录账号，不定时轮询。
角色变化后刷新页面即可重新确认；新提升的管理员会为同一个账号补充 OAuth 管理授权。
路径和前端保存的标志不赋予管理员 API 权限。
账号中心及 OAuth/OIDC 接口统一挂载于 `/auth/`。

### 从旧账号路径升级

已有数据库应先停止旧服务并备份数据库与状态文件，再用新二进制及原部署配置执行：

```sh
hagency-server --config config/dev/hagency.toml --migrate-auth-path
```

该单向事务只接受同一 server name 和 public origin，把 Agent 部署/用户的 issuer
从 `/_pasion/` 改成 `/auth/`，保留用户 ID 与 Agent 所有权，恢复身份不可修改约束，
并撤销旧 Agent 会话/设备凭证。升级启动前执行一次，重复执行不会再次修改。
全新安装无需迁移。Client/Desktop 应配套升级并重新登录。Native 账号 ID 和本地
Agent 账本路径保持原存储命名空间；旧身份标记只有在同身份的原账本校验成功后才更新。
新客户端不接受旧 OAuth issuer。
