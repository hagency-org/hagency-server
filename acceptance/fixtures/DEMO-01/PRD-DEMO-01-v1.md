# PRD-DEMO-01 v1：健康状态与版本服务

状态：草案，待 AtomGit 书面确认（附件 2 §4.1）。确认后冻结，验收以本文件的 SHA-256 为准。

## 目标

一个最小 HTTP 服务，报告自身健康状态和所运行的版本，作为 GitFab 全流程（Spec → 实现 → 测试 → 评审 → Release → Container 部署 → 回退）的最小样例。

## 运行与配置

- Rust stable，Cargo 构建；可执行文件名 `demo01`。
- 监听 `0.0.0.0:${PORT}`，`PORT` 默认 `8080`。
- 必需配置（环境变量）：
  - `DEMO_RELEASE_VERSION`：Release 版本，形如 `v1.2.3`；
  - `DEMO_GIT_SHA`：40 位小写十六进制提交 SHA。
- 可选配置：`DEMO_ADMIN_TOKEN`，若设置则长度至少 16 个字符（本服务不使用其值，仅用于验证“错误信息不泄露密钥”）。
- 任一必需配置缺失或格式不对、或 `DEMO_ADMIN_TOKEN` 过短时：不监听端口，退出码 `2`，在标准错误输出一行 `config error: <变量名> <原因>`。错误信息中不得出现任何配置值。

## 接口

| 请求 | 响应 |
|---|---|
| `GET /health` | `200`，`Content-Type: application/json`，`{"status":"ok","checks":{"config":"ok"}}` |
| `GET /version` | `200`，`{"version":"<DEMO_RELEASE_VERSION>","git_sha":"<DEMO_GIT_SHA>"}` |
| 其他路径 | `404`，`{"error":"not_found"}` |
| 已知路径的其他方法 | `405`，`{"error":"method_not_allowed"}` |

JSON 响应可以包含额外字段，但上表字段的名称、类型和取值必须一致。

## Container

- 提供 `Dockerfile`；镜像以非 root 用户运行。
- `HEALTHCHECK` 访问 `GET /health`，间隔不超过 10 秒。
- 构建时把版本和提交 SHA 写入镜像环境，使 `GET /version` 返回实际 Release。Runner 构建时传入构建参数 `GITFAB_RELEASE`（Release 标签）和 `GITFAB_COMMIT`（提交 SHA），Dockerfile 以 `ARG` 接收并写入 `ENV DEMO_RELEASE_VERSION`、`ENV DEMO_GIT_SHA`。

## 测试

- 单元测试覆盖：配置校验（缺失、格式错误、过短 token、错误信息不含值）、两个接口的响应、404/405。
- `cargo test --locked` 通过。

## 不做

不做认证、指标、日志收集、数据库。

## 验收

`acceptance/fixtures/DEMO-01/cases.json` 中的启动和 HTTP 用例全部通过；Release、镜像 digest、SBOM、签名、部署回执和回退记录齐全（附件 3）。
