# GitFab acceptance events

Offline tools for the event data contract in Annex 3 of the AtomGit agreement: canonical encoding (§8),
the five v1 event types (§7), Ed25519 signatures (§8) and the token ledger (§7.4).

```sh
cargo build --locked -p hagency-gitfab-events --bin gitfab
gitfab keygen --key-id gitfab-1 --dir keys          # keys/gitfab-1.key (0600) and keys/gitfab-1.pub
gitfab sign --key keys/gitfab-1.key --key-id gitfab-1 event.json >> events.jsonl
gitfab verify --keys keys events.jsonl              # schema, then signature where present or required
gitfab ledger events.jsonl                          # token totals by project, module, Spec and contributor
gitfab canonicalize --digest event.json             # payload digest, sha256:<hex>
```

Exit status is 0 when every check passes, 1 when a check fails and 2 for usage or I/O errors. Reports are
JSON on stdout with a stable `code` per failure.

## Rules

- **Canonical encoding.** Keys sorted, no whitespace, UTF-8. Numbers must be integers; a float anywhere is
  refused, so `token_delta` can never be fractional, NaN or infinite.
- **Signed payload.** The event without its top-level `signature`. The signature object carries `algorithm`
  (`ed25519`), `key_id`, `public_key_ref`, `payload_digest` and `signature_base64`.
- **Verification failures are distinct.** `payload_changed` (content edited after signing), `unknown_key`
  and `bad_signature` (wrong public key or altered signature).
- **Key events must be signed.** A task event is a key event when it records task creation (intake
  succeeded), an execution result, a Reviewer verdict (terminal review status) or a release.
- **Schema.** Unknown `schema_version`, missing or mistyped required fields and undeclared fields are
  refused. Timestamps are UTC RFC 3339 ending in `Z`. Strings may not contain credentials in URLs or local
  absolute paths. Declared optional fields: `round`, `findings`, `commit_sha`, `platform_error` and `modules` (the
  Plan's Module breakdown) on task events, `corrects` on memory and
  token events.
- **Ledger.** Idempotent by `event_id` (`release_id` for releases). An identical duplicate counts once; the
  same key with different canonical content fails the ledger with `conflict`. Corrections are new events
  whose `corrects` names an earlier token event. Sums are checked 64-bit integers.
- **Identity events.** `verify` requires every signed key event (and every release record) to have one
  identity event with the same event type, project, task, key, payload digest and signature, and reports
  `missing_identity` or `orphan_identity` otherwise.

## Keys

Private keys are files of a base64 32-byte seed and must be mode 0600; `sign` refuses a key readable by
others. Event signing keys belong to the server and are configured at deployment; Runner deployment
receipts use the Runner's own key. Only public keys are delivered.

## Journal

`journal::Journal` appends events to a JSONL file and is what a writer uses instead of writing lines
itself. Memory, token and release events and key task events are signed on the way in; a key event is
followed by its identity event in the same write and sync. Appending an event key again with the same
content writes nothing; different content is refused with `conflict`. Opening a journal re-validates
every line and refuses a file that already holds a conflicting duplicate.

Not yet covered: the server does not yet call the journal; that comes with the workflow orchestrator.
