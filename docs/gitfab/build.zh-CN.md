# GitFab 构建、测试与安装

服务端基础环境（Rust、PostgreSQL、Palpo、Pasion）的准备见仓库根目录 `README.zh-CN.md`。本文只补充 GitFab 增加的部分。

## 环境

- Rust 1.99（`rust-toolchain.toml` 固定，rustup 会自动安装）
- Git、curl
- Docker：Runner 部署与 SBOM 需要；服务端编排器的数据库测试也用 Docker 里的 PostgreSQL（`just db-up` 启动 `hagency-server-postgres-1`）
- Python 3：验收自测与数据库测试脚本

## 构建

所有命令在仓库根目录执行，带 `--locked`，依赖严格按 `Cargo.lock`：

```sh
cargo build --release --locked \
  -p hagency-server \
  -p hagency-gitfab-events -p hagency-gitfab-forge -p hagency-gitfab-runner -p hagency-gitfab-board
```

产物在 `target/release/`：

| 程序 | 用途 |
|---|---|
| `hagency-server` | 服务端（含编排器，`[gitfab]` 配置启用） |
| `gitfab` | 事件签名、验签、账本、证据包、报告、追溯 |
| `gitfab-forge` | GitHub / GitCode 平台操作（验收脚本使用） |
| `gitfab-runner` | 部署、回退、签名回执与 SBOM |
| `gitfab-board` | 只读监控大屏 |

自动化环境（CI、多个 agent 共用机器）建议加 `CARGO_INCREMENTAL=0`，避免增量编译缓存占用大量磁盘。

内存少于约 8 GB 的机器（包括默认配置的 Docker 虚拟机）要限制并行编译数，例如 `CARGO_BUILD_JOBS=2`；否则编译 ICU 相关依赖时可能因内存不足被系统终止（`signal: 9, SIGKILL`）。在 4 GB 内存、8 核的 Debian 12 容器中，不加限制会失败，加 `CARGO_BUILD_JOBS=2` 后构建成功。

## 测试

```sh
# GitFab 各 crate（不需要数据库）
cargo test --locked -p hagency-gitfab-events -p hagency-gitfab-forge -p hagency-gitfab-runner -p hagency-gitfab-board

# 编排器（需要 Docker 中的 PostgreSQL：just db-up）
python3 scripts/test-agent-service-postgres.py --lib orchestrator

# 验收包自测（需先构建 gitfab）
GITFAB_BIN=$PWD/target/release/gitfab bash acceptance/tests/selftest.sh

# 日志不含凭据（对运行中的服务端；只输出计数，不输出日志内容）
scripts/gitfab-log-canary.sh http://127.0.0.1:8099 <服务端日志文件>
```

`gitfab-log-canary.sh` 发送带随机假凭据的请求（查询串 `access_token`、`Authorization: Bearer`、`Cookie`、登录密码），再在日志里查找这个假值，并统计已有日志中的凭据模式（Bearer 值、URL 中的 access_token、带密码的数据库 URL、GitHub Token、私钥）。都没有返回 0，有则返回 1。服务端的 HTTP 请求日志（Palpo 使用的 salvo `Logger`）只记录路径，不记录查询串与请求头；Agent 服务的错误只输出固定错误码。

## 在 Linux 上安装

以下假设服务端已按 `README.zh-CN.md` 部署，只增加 GitFab 部分。

1. 复制程序：
   ```sh
   sudo install -m 0755 target/release/{gitfab,gitfab-forge,gitfab-runner,gitfab-board} /usr/local/bin/
   ```
2. 生成服务端事件签名密钥，并配置 `[gitfab]`（见 `user-guide.zh-CN.md` 第 1 节）：
   ```sh
   sudo install -d -m 0700 /etc/hagency/keys
   sudo gitfab keygen --key-id gitfab-1 --dir /etc/hagency/keys
   ```
3. Runner 机器（需要 Docker）：
   ```sh
   sudo install -d -m 0700 /etc/gitfab-runner /var/lib/gitfab
   sudo gitfab keygen --key-id runner-1 --dir /etc/gitfab-runner
   ```
   把 `/etc/hagency/keys/gitfab-1.pub` 和 `/etc/gitfab-runner/runner-1.pub` 交给验收方，放进 `acceptance/keys/`。
4. Token 文件一律 `chmod 600`，服务端启动时会检查。
5. 大屏可作为普通进程运行，只监听 127.0.0.1；对外展示时放在反向代理之后，只放行 GET。

## 第三方组件

依赖清单与许可证见 `third-party.zh-CN.md`，依赖变化后运行 `python3 scripts/gitfab-third-party.py` 重新生成。
