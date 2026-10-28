# Agent 独立加密身份与后续加密执行

当前目标仍是 A4 的 Agent 独立密钥、加密任务/回复/文件与私人远程审批，随后 A3 多 Agent 协作；不是以身份接口代替执行功能。远程审批已有真实联合证据，详见 Client 对应报告。本批先接通当前执行设备自己的 SDK 身份并实际验证，现有 encrypted_room_requires_client_crypto 拒绝条件保持，直到完整任务链实现。

## 身份与权限

- Agent 使用自己的永久 puppet MXID、按 Agent 永久 ID 与指定执行主机的稳定 Hagency device ID 固定的 Matrix device ID；不复制主人 SDK 私钥、会话或交叉签名私钥。
- Server 通过 Appservice 的设备管理与 device_id masquerading 代理独立设备。AS 凭据仅留在 Server；不执行 /login，不向 Client 发放绕过 Hagency 撤销的 Matrix bearer。注册使用 inhibit_login=true。
- SDK 的 bearer 是已有短期 Hagency execution-device token，仅能访问固定 Agent/generation 前缀。授权代次与密钥身份分离：凭据换代和同主机重开保留 Matrix device ID 与原密钥；旧 bearer 仍被围栏拒绝。Agent 交叉签名公钥按永久 Agent 固定，换执行主机不能静默覆盖。Server 每次验证主人、会话、指定执行设备、设备代次与 Agent 生命周期；持有实际身份和 Agent 行锁至有界网络操作结束。等待行锁后重新采样当前时间；网络阶段再次校验。
- 当前仅允许 provision、versions、whoami、自己的 keys/upload、keys/query、keys/device_signing/upload、keys/signatures/upload。不接受任意 URL、user_id/device_id query、peer key claim/sharing、sync、消息/文件发送、密钥备份、login 或账户管理。
- 首次设备及三种交叉签名公钥摘要在独立事务提交后才进行 HTTP。中断、HTTP 错误、重开不能抹除公钥见证，同一永久设备/Agent 身份不能换钥。重试只允许相同身份的幂等公钥操作；它不是任务/工具/消息重试权限。
- Client 使用独立、加密的 SQLite SDK store；主机传入本地 secret-store passphrase，不上传。私有目录、不可跟随的绑定文件、独占主机锁与真实 store 缺失拒绝，防止混用主人/其他 Agent 或静默重建遗失密钥。SDK 不向调用方暴露原始 Client 或 bearer。
- 实际 homeserver advertised versions 传给 SDK；设备支持通过真实创建和同设备 whoami 探测，不伪造更高协议版本，不自动降级成裸 Matrix 登录。

协议依据：[Matrix Application Service API](https://spec.matrix.org/v1.19/application-service-api/)。设备管理和设备 masquerading 在该规范中明确；实现支持仍须以当前 homeserver 的实际响应验证。

## 必须验证

1. 真实 Server/PG/AS 与两个独立 Agent SDK：设备与密钥不同于主人、彼此独立，实际公钥回读一致，SDK store 重开保留身份。
2. 原 HTTP 失败后公钥仍固定，改钥在网络前拒绝；Server 重开不清除固定值。
3. 同主人的另一设备、外部主人、旧 Agent generation、设备改派/撤销、会话过期及行锁等待后过期均拒绝且不产生新的 Matrix IO。
4. 恶意 endpoint、method、query、Origin/Cookie/Host、不同用户/设备公钥和多余字段不能获得 AS 代理权。
5. Client 私有 store 排他、绑定不符、密钥遗失与 Unix symlink 拒绝；SDK 初始化或网络失败不被记为加密任务成功。

## 后续完整链（本批身份接口不能宣布通过）

- 接入当前 NativeOwner Agent 配置和运行生命周期、主机 secret store、自动续授权/关闭等待及明确身份恢复。
- 主人验证 Agent 实际公钥/交叉签名身份；设备更换/密钥重置需要显式流程（独立于普通登录凭据换代），不能继承旧验证。
- 当前加密 Room 的有界 SDK sync/decrypt，原事件签名/发送者验证及 Server 当前成员/绑定/租约联合授权，原事件接收/ACK 不重复。
- 本机 Codex 精确输入、主人策略/预算/工具审批，原加密回复与文件的发送所有权、撤权/未知/重启不重放。
- 真实 Synapse + SDK + NativeOwner + Codex 加密任务/回复/文件、两端重开、人工桌面、生产 OAuth 和跨平台资格；旧失败及其用量保留。
