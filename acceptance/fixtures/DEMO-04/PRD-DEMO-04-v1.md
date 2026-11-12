# PRD-DEMO-04 v1：Release Notes 生成服务

状态：草案，待 AtomGit 书面确认（附件 2 §4.4）。确认后冻结，验收以本文件的 SHA-256 为准。

## 目标

从冻结的提交/PR JSON 生成 Markdown Release Notes。分类规则固定，不调用生成式 AI。

## 接口

CLI：`relnotes --version <VERSION> --date <YYYY-MM-DD> [--digest] <INPUT.json>`

- 输出 Markdown 到标准输出；`--digest` 时只输出正文的 `sha256:<64 位小写十六进制>`。
- 退出码：成功 `0`；输入不是合法 JSON 或缺少必需字段为 `2`，并在标准错误说明哪一条记录。

输入为 JSON 数组，每条记录：

| 字段 | 类型 | 必需 | 说明 |
|---|---|---|---|
| `id` | 字符串 | 是 | 唯一 ID，去重依据 |
| `kind` | `"commit"` 或 `"pr"` | 是 | |
| `ref` | 字符串 | 是 | 提交短 SHA 或 `#编号` |
| `title` | 字符串 | 是 | |
| `labels` | 字符串数组 | 否 | |
| `author` | 字符串 | 是 | 贡献者 |

## 规则

1. **去重**：`id` 相同的记录只保留第一次出现的那条。
2. **分组**：依次判断，命中即停：
   - `labels` 含 `feature` 或 `enhancement` → feature；含 `bug` 或 `fix` → fix；含 `docs` 或 `documentation` → docs；
   - 否则看 `title` 前缀（不区分大小写）：`feat:` 或 `feat(...)`: → feature；`fix:` → fix；`docs:` → docs；
   - 都不命中 → other。
3. **排序**：组内按 `id` 的字节序升序。
4. **贡献者**：所有保留记录的 `author` 去重后按字节序升序。

## 输出格式

```markdown
# Release <VERSION> (<DATE>)

## Features
- <title> (<ref>) @<author>

## Fixes
- ...

## Documentation
- ...

## Other
- ...

## Contributors
@a, @b
```

空组整节省略（包括标题）。每行以 `\n` 结尾，文件末尾恰好一个换行。相同输入产生逐字节相同的正文和摘要。

## 测试

去重、分组、排序、空组、摘要的测试；`cargo test --locked` 通过。

## 验收

`acceptance/fixtures/DEMO-04/input.json` 以 `--version v1.0.0 --date 2026-11-12` 运行，输出与 `expected.md` 逐字节一致，`--digest` 与 `expected.digest` 一致。
