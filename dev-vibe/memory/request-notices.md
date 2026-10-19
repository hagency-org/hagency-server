# 请求状态提醒

2026-10-20：配合 Hagency Client 修正重复引用及固定中文/不实等待提示。

- workers request_notice_content 不再为通知添加 m.in_reply_to；线程保留 m.thread 及 is_falling_back=false，org.hagency.request_event_id 关联原请求。m.notice 不触发其他 Agent。
- org.hagency.request_notice v1 附 kind、deviceState、deviceName、observedAtMs、requestEventId，Client按查看者语言本地渲染；body为英文兼容回退。Client同时兼容旧中文提醒。
- RequestNotice SELECT 在 claim 时联结 Agent当前执行设备、其有效session/generation、15秒configuration_presence及当前execution_leases，区分 unassigned/offline/unobserved/not_running/ready。设备心跳只说明连接，活动lease也不证明该请求已开始；等待文案只承诺入队事实并说明可能超时，不猜测其他任务、不保证恢复执行。无数据库schema变更。
- 新PostgreSQL回归验证无心跳、旧心跳、新心跳无runtime及活动lease四状态；保留离线排队、过期、授权拒绝、状态通知幂等与公平性覆盖。更新通知关联断言，确保移除引用后仍能定位请求。全包75通过/0失败/0忽略，含隔离数据库测试（D:/BuildCache/hagency-client/server-feedback-final-tests.log，39.86s）。
- 所有构建产物D:/BuildCache/hagency-server/target，Cargo缓存与临时使用D:/BuildCache/hagency-client，独立PostgreSQL18数据在D:/BuildCache/hagency-server/pgdata、仅127.0.0.1:55439、测试后已停止。未使用Docker构建，未改生产数据库或schema.rs。未部署生产。
- 用户随后明确要求Client和Server各自推送远端main。fetch确认本仓库与origin/main无分叉；本轮完整修改及记忆一起提交，修改文件经rustfmt并通过cargo fmt --all -- --check。功能验证沿用75项通过结果，推送不表示生产部署完成。
- 提交前cargo clippy --locked -p hagency-agent-service --all-targets -- -D warnings通过（request-notices-push-clippy.log，40.86s），metadata再次确认实际target在D盘。
