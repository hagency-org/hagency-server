# 在已有 Hagency 部署上启用 GitFab：升级说明

面向运维：把 `feat/gitfab-events` 部署到一个已经在用的 Hagency 服务端（例如 river）之前，需要知道的全部变化。原则是只增补：不配置 `[gitfab]` 时，服务的行为与原来相同。

## 1. 不配置 `[gitfab]` 时有什么变化

| 方面 | 变化 |
|---|---|
| 数据库 | 启动时由 transport 迁移 `2026-11-23-000414_gitfab_orchestrator` 执行一次（全部 `CREATE … IF NOT EXISTS`，此前已建过表的安装也能直接升级）：在 `hagency_agent_v1` 下新增 3 张表、2 个索引，表为空。不修改任何已有表、列或数据 |
| 房间消息路由 | 不变。`/gitfab` 开头的消息只有在启用编排器时才被拦截；未启用时与其他消息一样交给 Agent |
| Agent 准入规则 | 不变。新增的“编排器阶段消息免提及”只对 `orchestrator_stage_events` 中登记过的事件生效，未启用时该表为空 |
| HTTP | `/hagency/api/v1/projects/{project}/gitfab/runs…` 返回 404 `gitfab_unsupported`；不注册公钥与大屏接口 |
| 后台任务 | 不启动编排器 worker |
| 依赖 | 不新增外部 crate（只链接本仓库的 `gitfab-events`、`gitfab-forge`） |

改动范围：`crates/agent-service` 与 `crates/backend` 共 19 个文件，新增约 5100 行，删改原有代码 5 行，都在 `transport.rs`：一处 import，以及 `ingest_routed_inner` 的布尔参数改为三值的准入方式（原有两种路径的判定不变，新增编排器一种）。agent-service 全量测试（含数据库，593 个）通过。

## 2. 新增的数据库对象

| 对象 | 用途 |
|---|---|
| `orchestrator_runs` | 每个任务 Thread 一个流程：状态、修订号、待写事件；`UNIQUE (room_id, thread_root)` |
| 索引 `orchestrator_runs_open_task` | 同一房间同一任务只允许一个未结束的流程 |
| `orchestrator_stage_events` | 编排器派发的阶段消息，用于读回结果 |
| `orchestrator_commands` | 房间里的 `/gitfab` 命令，编排器处理后回复 |
| 索引 `orchestrator_commands_pending` | 待处理命令 |

回退到不含 GitFab 的版本时这些对象可以保留（旧版本不读它们）；要删除时先停服务，再 `DROP TABLE hagency_agent_v1.orchestrator_commands, hagency_agent_v1.orchestrator_stage_events, hagency_agent_v1.orchestrator_runs;`。

## 3. 启用步骤

1. 部署新版本，先不加 `[gitfab]`，确认服务与原来一致（房间、Agent、资源池照常）。
2. 生成事件签名密钥：`gitfab keygen --key-id gitfab-1 --dir /etc/hagency/keys`（私钥 0600）。
3. 在配置中加入 `[gitfab]`（字段见 `user-guide.zh-CN.md` 第 1 节）。`journal_file` 一旦设置即启用编排器；平台 Token、大屏 Token 均可选，文件必须 0600，否则服务拒绝启动并说明原因。
4. 重启，日志出现 `GitFab orchestrator enabled`；`GET /hagency/gitfab/v1/public-keys` 返回公钥。
5. 建一个不加密的项目房间，加入要参与的 Agent，按手册第 3 节发 `/gitfab start`。
6. （可选）用 cron 把事件日志写入 Git：`scripts/gitfab-journal-git.sh`（手册第 1 节）。

## 4. 启用后有什么变化

- 房间里 Human 发的 `/gitfab …` 消息由编排器处理，不再交给 Agent（加密房间不受影响，编排器只处理不加密房间）。
- 编排器以 `@_hagency_service` 身份在任务 Thread 里发 `m.notice` 阶段消息，不提及任何人；被派发的 Agent 通过原有 bindings 或资源池路由收到。资源池的成员、额度、队列检查照常生效。
- 配置了平台 Token 时，编排器在 GitHub / GitCode 上建 PR、读测试与审查状态、发 Release；Agent 拿不到这些 Token。

## 5. 回退

- 只想停用：去掉 `[gitfab]` 段并重启。进行中的流程停在当前状态，数据保留，重新启用后可在 Thread 里 `/gitfab continue`。
- 回到旧版本：直接部署旧版本即可，新增表不影响旧版本；需要时按第 2 节删除。
- 事件日志与 Runner 回执是只追加的 JSONL 文件，回退不会改动它们。
