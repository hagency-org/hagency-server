# PRD-DEMO-05 v1：Open Token 演示账本校验器

状态：草案，待 AtomGit 书面确认（附件 2 §4.5）。确认后冻结，验收以本文件的 SHA-256 为准。Git 后端：GitHub（供应商默认，待确认）。

Token 只是演示计量单位，不具备货币、证券、兑换或支付属性。

## 目标

读取附件 3 规定的 `gitfab.memory.v1` / `gitfab.token.v1` JSONL 事件，校验签名，幂等去重，拒绝冲突，按维度确定性求和。

## 接口

CLI：`ledger --keys <DIR> [--report md] <FILE.jsonl>...`

- `DIR` 下每个 `<key-id>.pub` 文件是一把 Base64 Ed25519 公钥。
- 默认输出 JSON 汇总；`--report md` 输出人类可读报告（内容不做逐字验收）。
- 退出码：全部合法为 `0`；有冲突、签名失败、字段不合法或浮点数为 `1`；用法或读取错误为 `2`。

## 规则

1. 每行一个事件，字段按附件 3 §7.3。`token_delta` 必须是有符号整数，出现浮点数（含 `5.0`）即不合法。
2. **签名**：按附件 3 §8 的规范化方式计算 payload 摘要并用 `signature.key_id` 对应的公钥验签。
3. **幂等**：`event_id` 相同且规范化内容完全相同的记录只计一次，计入 `duplicates`。
4. **冲突**：`event_id` 相同但内容不同，整份输入不汇总，退出码 `1`，在 `conflicts` 中列出 `event_id` 和两处行号。
5. **求和**：分别按 `project`、`module`、`spec`、`contributor` 对 `token_delta` 求和；结果中的键按字节序排序。

## 输出（退出码 0 时）

`events` 为读入的记录行数，`duplicates` 为其中作为完全相同的重复而跳过的行数。下例为 `duplicate.jsonl`（5 行，其中 1 行重复）的输出：

```json
{
  "ok": true,
  "events": 5,
  "duplicates": 1,
  "total": 13,
  "by_project": {"DEMO-05": 13},
  "by_module": {"ledger": 9, "report": 4},
  "by_spec": {"spec/DEMO-05.md@v1": 13},
  "by_contributor": {"@impl:example.org": 12, "@review:example.org": 1}
}
```

可以包含其他字段；上述字段必须一致。

失败时：`{"ok": false, "conflicts": [...], "errors": [...]}`。`conflicts` 每条含 `event_id` 和 `lines`（两处行号，升序）；`errors` 每条含 `line` 和 `code`，`code` 取 `bad_signature`（签名或摘要不符）、`unknown_key`（无对应公钥）、`non_integer`（浮点数）、`invalid_field`（字段缺失或类型不对）、`invalid_json` 之一。

## 测试

合法、重复、冲突、浮点数、签名被改动的夹具；`cargo test --locked` 通过。

## 验收

`acceptance/fixtures/DEMO-05/` 中：`valid.jsonl` 的输出与 `valid.expected.json` 一致；`duplicate.jsonl` 输出与 `duplicate.expected.json` 一致；`conflict.jsonl`、`tampered.jsonl`、`float.jsonl` 退出码为 `1`，报告的冲突和错误行号与 `failures.expected.json` 一致。公钥在 `keys/`。另由 `./acceptance/verify-ledger.sh DEMO-05` 复算系统自身产生的事件。

镜像中可执行文件 `ledger` 在 PATH 上；验收方用 `acceptance/verify-blackbox.sh DEMO-05` 在部署的镜像中运行它，夹具目录只读挂载在 `/fixtures`（用例见 `cases.json`）。
