# PRD-DEMO-02 v1：轻量任务清单 API

状态：草案，待 AtomGit 书面确认（附件 2 §4.2）。确认后冻结，验收以本文件的 SHA-256 为准。

## 目标

一个进程内存储的任务清单 HTTP API，覆盖 Pull Request、Reviewer、Release 和 Container 部署的完整流程。

## 运行

- Rust stable，Cargo 构建；可执行文件名 `demo02`；监听 `0.0.0.0:${PORT}`，默认 `8080`。
- 数据只在进程生命周期内保存，不使用数据库，重启后为空。
- 提供 `GET /health`，返回 `200` 和 `{"status":"ok"}`。

## 数据

任务字段：

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | 整数 | 从 1 开始按创建顺序递增，删除后不复用 |
| `title` | 字符串 | 去掉首尾空白后保存，1 到 200 个字符（按 Unicode 标量计） |
| `completed` | 布尔 | 创建时为 `false` |
| `created_at` | 字符串 | 创建时间，UTC RFC 3339，以 `Z` 结尾 |

## 接口

| 请求 | 成功 | 失败 |
|---|---|---|
| `POST /tasks`，body `{"title":"..."}` | `201`，返回任务 | 见下方错误 |
| `GET /tasks` | `200`，按 `id` 升序的任务数组 | |
| `GET /tasks/{id}` | `200`，返回任务 | `404` `{"error":"not_found"}` |
| `POST /tasks/{id}/complete` | `200`，返回 `completed=true` 的任务；重复调用结果相同 | `404` |
| `DELETE /tasks/{id}` | `204`，无响应体 | `404` |

错误统一为 `{"error":"<代码>"}`：

| 情形 | 状态码 | 代码 |
|---|---|---|
| body 不是 JSON 对象或缺少 `title`、`title` 不是字符串 | `400` | `invalid_json` |
| 去空白后标题为空 | `400` | `title_empty` |
| 标题超过 200 个字符 | `400` | `title_too_long` |
| `{id}` 不是正整数 | `404` | `not_found` |

任何输入都不得使服务崩溃；请求体上限 64 KiB，超过返回 `413`。

## 测试

自动化测试覆盖正常路径、输入校验和未找到三类场景；`cargo test --locked` 通过。

## 不做

不做持久化、分页、认证、并发写冲突控制以外的事务。

## 验收

`acceptance/fixtures/DEMO-02/cases.json` 的请求序列逐条通过（按顺序执行，依赖前一步的 `id`）；PR、Reviewer 结论、Release、镜像、部署回执齐全。
