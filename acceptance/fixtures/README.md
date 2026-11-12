# 演示项目夹具

五个演示项目的 PRD 草案和黑盒验收夹具。PRD 待 AtomGit 书面确认后冻结，并把 SHA-256 填进 `acceptance/projects/DEMO-0N.env` 的 `PRD_SHA256`。

| 项目 | PRD | 夹具 |
|---|---|---|
| DEMO-01 健康与版本服务 | `DEMO-01/PRD-DEMO-01-v1.md` | `cases.json`：启动失败用例、HTTP 用例 |
| DEMO-02 任务清单 API | `DEMO-02/PRD-DEMO-02-v1.md` | `cases.json`：按顺序执行的请求序列 |
| DEMO-03 Markdown 检查 | `DEMO-03/PRD-DEMO-03-v1.md` | `samples/*.md` 及对应 `.expected.json` |
| DEMO-04 Release Notes | `DEMO-04/PRD-DEMO-04-v1.md` | `input.json`、`expected.md`、`expected.digest` |
| DEMO-05 账本校验器 | `DEMO-05/PRD-DEMO-05-v1.md` | 已签名的 `valid`/`duplicate`/`conflict`/`tampered`/`float.jsonl`，期望输出，公钥 `keys/` |

DEMO-05 的事件用一把只为夹具生成的密钥签名，私钥已销毁，仓库里只有公钥。

期望结果的来源：DEMO-03 由人工逐条写出，并用独立的参考实现核对一致；DEMO-04 按 PRD 规则生成后逐条人工核对；DEMO-05 与 `gitfab ledger` / `gitfab verify` 的结果一致。
