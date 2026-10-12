# Hagency frontend

[中文](README.zh-CN.md) · [Project quick start](../../README.md)

Dioxus 0.7.5/WASM frontend combining Padmin and Hagency pages, hosted by the Rust backend.

Build or watch from the repository root:

```sh
just prepare-frontend
just dev
```

Generated assets live in `resources/frontend/public` and are ignored by Git.
See the [development guide](../../docs/guide.md#development) for configuration
and the root [NOTICE](../../NOTICE) for upstream source and license references.

The current Hagency pages are `/hagency/projects` and `/hagency/agents`. Users sign
in with their personal Matrix account through Pasion. Generic Palpo/Pasion admin
pages and their upstream APIs retain their existing role checks.

Browser authentication uses only `/hagency/api/login/token`, `/hagency/api/session`, and
`/hagency/api/logout`. Agent management goes through the closed cookie/CSRF BFF at
`/hagency/api/browser/v1`; the BFF derives identity from the stored personal OAuth
grant and obtains a short, separately verified server authorization for each
operation. It never exposes native session credentials or offers device/execution
operations. The native `/hagency/api/v1` keeps rejecting browser Cookie/Origin.

Projects register existing Matrix Spaces and Rooms and edit creation policy with
a revision check. Matrix Space administration rights are checked on the server.
My agents lists the permanent owner's puppets, creates them in a Room, adds other
Project/Room bindings, and pauses/resumes agents or bindings. Resource budgets,
Codex configuration and risky tool policy belong in the local hagency-client.

Fleet/Hafleet, resource allocation/Engagement approvals, custom account approval
and native password-login pages have been removed. There are no old route aliases.

Browser sessions are revalidated through `/hagency/api/session` after reload. Project and
Agent pages use the restricted cookie/CSRF BFF without storing OAuth bearers in
browser persistent storage. Generic Palpo/Pasion administrative pages request a
fresh native Pasion PKCE grant when their memory-only bearer is absent.

Projects also expose independent Room creation policies and administrator service
pause controls. Clearing a Project/Room administrator pause requires owners to
resume their own bindings; it never resumes execution on their behalf.
