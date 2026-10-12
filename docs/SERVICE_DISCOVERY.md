# Hagency service discovery / 服务识别

`GET /hagency/api/v1/discovery` is public native-client metadata. It grants no session or device authority and returns `Cache-Control: no-store`. The request must carry the exact public Host and no browser Origin/Cookie or forwarded-host header.

原生客户端登录前须验证 `product == "hagency-server"`、支持的 `protocolVersion == 3`，以及 `capabilities` 同时包含 `pasion-oauth` 和 `owner-agent-appservice-v1`。普通 Matrix 服务不是 Hagency 服务；不能仅凭 `serviceMxid` 字段存在判定。`version` 是当前 Cargo package version，不是 wire protocol version。HTTPS 认证与 issuer 校验仍不可省略。

新增字段 / Added fields:

```json
{
  "product": "hagency-server",
  "version": "0.1.0",
  "protocolVersion": 3,
  "capabilities": [
    "pasion-oauth",
    "owner-agent-appservice-v1",
    "projects-matrix-spaces-v1",
    "device-execution-v1",
    "execution-history-v1",
    "global-agent-identity-v2",
    "execution-device-v1",
    "owner-direct-v1",
    "processing-reaction-v1"
  ]
}
```

Existing fields remain: `serverName`, `homeserver`, `serviceMxid`, `issuer`, `authorizationEndpoint`, `tokenEndpoint`, `registrationEndpoint`, and `authorizationWindowMs`. Verify `homeserver` against the selected canonical origin and `issuer` against that origin's `/auth/`; the Matrix OAuth metadata must agree. Unknown capabilities may be ignored; missing required capabilities or unsupported protocols must stop login. No Agent E2EE capability is advertised.

本机开发 HTTPS 使用 `https://hagency.local`，对应 Matrix server name `hagency.local`。不能把旧 `localhost:8089` 部署的永久用户标识、Agent owner 和 AS namespace 原地改写为此身份：新开发域使用独立数据库及数据卷，旧部署完整保留。

Regression: PostgreSQL-backed real routing proves unauthenticated metadata retrieval, explicit product/version/capabilities, no issued credential/cookie, protected Projects still unauthorized, and mismatched Host rejection.

Protocol 3 assigns each new Agent to the current authenticated owner device. `executionDeviceId` is nullable for an unassigned identity; one device may execute several Agents. `PUT /hagency/api/v1/agents/{id}/execution-device` accepts only `expectedGeneration`, targets the authenticated device, and uses Agent generation as CAS. The removed execution-instance resource has no compatibility handler. Domain schema version 4 refuses earlier versions at startup; offline maintenance is explicit and backed up.
