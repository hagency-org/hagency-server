# 当前Server/Client原始文件联合验证

本批新增独立example agent_file_fixture，不修改产品执行、权限或投递代码。
它连接真实Store/Domain/Transport/PostgreSQL及当前HTTP API、Verifier、Appservice、
readiness和投递worker；Pasion身份及Matrix状态/媒体/消息服务明确为loopback替身。
产品backend不挂载fixture路由，不涉及生产部署或向真实用户发消息。

## 隔离与启动

构建：

```sh
cargo +1.99.0 build -p hagency-agent-service --example agent_file_fixture
```

运行时设置HAGENCY_AGENT_TEST_DATABASE_URL、HAGENCY_FILE_FIXTURE_PORT、
HAGENCY_FILE_FIXTURE_INFO。数据库host必须127.0.0.1，库名必须agent_files_joint_
前缀；HTTP只监听127.0.0.1。必须新建专用数据库及私有临时目录。同一原场景重启
保持端口和库不变：首次写入issuer后更换origin会被产品部署身份保护拒绝。
fixture凭据均为明确静态测试值，不能用于真实服务。Ctrl-C停止自有worker和服务。

fixture采用当前create_agent、confirm_identity_provisioned、bind_room及
activate_binding流程；身份经过真实HTTP introspection/whoami Verifier。Matrix
成员和权限是显式测试事实；启动探针经过真实AS HTTP回环，不直接设ready。

## 已运行的联合证据

当前Client直接连接此示例。原输入131073字节，输出追加16字节后131089字节，
覆盖真实64KiB分块、原Agent sender/Room/thread、HTTP实际媒体字节与原事件回执。
正常与queue已提交但响应丢失两种场景最终各运行通过，分别保留Completed/Unknown
原审批审计。原Server及Client重启后历史刷新成功，上传、Room消息和模型调用未
重放，原费用不变。替身provider的5 Token是合成值，不能算真实模型证据。
最终日志：/tmp/hagency-joint-wire5.log、/tmp/hagency-joint-lost5.log。

另经Client公开NativeOwner独立示例运行真实Codex编程，得到同样131089字节及
原sent回执；matrix.receive_file、matrix.send_file和两个codex.command审计均
Completed。实际用量输入38549、输出401，共38950；缓存30976已包含输入，不能
重复相加。调用1，pending/unreported/reserved均0，原停止范围whole_tree。
该首轮驱动在实际执行后因停止请求漏传bindingId而退出400，仍记录为驱动失败。
修正后的显式VERIFY_EXISTING模式只核验同一原结果和checked shutdown，无重新
推理、acquire、上传或发送；不能宣称首轮驱动或新付费包装器通过。

最终原结果位于/private/tmp/hagency-joint-real-example1/real-codex-files-result.json，
verifiedExisting=true；核验日志/tmp/hagency-joint-public-real1-final-evidence.log。
真实homeserver、完整App SDK、桌面人工及Windows文件工具仍未联合验收。

## 检查与保留失败

当前示例构建、Agent-service严格all-targets Clippy、workspace格式、拼写及
OpenAPI83个owner/4个AS路由和22项负向守卫通过。产品代码与0a1ef55相同；该提交
CI40406382037成功，含107项PG/HTTP/AS测试，本批不将旧测试冒记为新执行。
Client b5206bf7的完整CI40409746628三个平台任务均success；不替代外部联合验收。

早期编译误用cfg(test)分配helper、私有Identity构造和activate返回值，已改为当前
生命周期及Verifier，不放宽产品可见性。短测试HS token、更换端口复用旧库、
provider-home目录不一致及主动停止后的错误unwrap失败均保留。真实libtest不具备
编程guardian入口，已改由独立public NativeOwner示例提供；未禁用编程或进程托管。
Client侧完整过程和命令见其同名联合验证文档；A2、A4/A3和总体目标继续进行。
