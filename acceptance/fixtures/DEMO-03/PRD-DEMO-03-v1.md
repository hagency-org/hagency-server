# PRD-DEMO-03 v1：Markdown 规格检查服务

状态：草案，待 AtomGit 书面确认（附件 2 §4.3）。确认后冻结，验收以本文件的 SHA-256 为准。

## 目标

检查 Markdown 规格文档的常见结构问题，输出确定性的结构化结果。只检查本地内容，不访问网络。

## 接口

- CLI：`mdcheck [FILE]`，没有 FILE 时读标准输入。输出 JSON 到标准输出。退出码：无 error 级发现为 `0`，有 error 级发现为 `1`，用法或读取错误为 `2`。
- HTTP（可选，同一可执行文件 `mdcheck serve`，默认端口 8080）：`POST /check`，body 为 Markdown 原文，返回同样的 JSON，状态码 `200`；`GET /health` 返回 `200`。

输出格式：

```json
{
  "findings": [
    {"rule_id": "MD004", "severity": "error", "line": 7, "message": "代码围栏未关闭"}
  ],
  "summary": {"error": 1, "warning": 0}
}
```

`findings` 按 `line` 升序，同一行按 `rule_id` 升序。`line` 从 1 开始。`message` 为说明文字，验收不比较其内容。

## 规则

| 规则 | 级别 | 触发条件 | 报告行 |
|---|---|---|---|
| `MD001` | error | 文档中（代码围栏外）没有一级标题（行首 `# ` 的 ATX 标题） | 1 |
| `MD002` | error | 链接目标为空：`[文字]()` 或 `[文字]( )`，代码围栏和行内代码中的不算 | 链接所在行 |
| `MD003` | warning | 同级、文字相同（去首尾空白后逐字比较）的标题再次出现 | 重复出现的那一行 |
| `MD004` | error | 以 ```` ``` ```` 或 `~~~` 开始的代码围栏到文末没有用同样的标记关闭 | 开始行 |

代码围栏内的内容不参与 MD001–MD003。空文档只报告 MD001。

## 要求

相同输入产生逐字节相同的输出；不访问外部 URL；单个输入上限 1 MiB，超过时退出码 `2`。

## 测试

提供通过和失败样例的测试夹具；`cargo test --locked` 通过。

## 验收

`acceptance/fixtures/DEMO-03/samples/*.md` 逐个运行，输出中的 `rule_id`、`severity`、`line` 和 `summary` 与同名 `.expected.json` 一致，退出码与其中 `exit_code` 一致。

镜像中可执行文件 `mdcheck` 在 PATH 上；验收方用 `acceptance/verify-blackbox.sh DEMO-03` 在部署的镜像中运行它，夹具目录只读挂载在 `/fixtures`（用例见 `cases.json`）。
