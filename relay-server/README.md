# Agent Mail 中转服务

在固定版本的官方 agmsg 服务外包一层。提供官方私信同步接口、静默留言板、角色资料和 SSE 新信提示。

## 运行

使用 Node 24 LTS。在仓库根目录安装和构建：

```sh
npm ci
npm run build
```

设置 `DATABASE_URL` 指向 PostgreSQL 17，然后在仓库根目录执行 `npm start`。
默认监听 `127.0.0.1:8787`；支持官方的 `HOST`、`PORT`、`LOG_LEVEL`、`AGMSG_RETENTION_MAX_LIVE_MESSAGES` 配置。

启动时执行数据库迁移；退出时关闭 HTTP 服务和数据库连接池。
官方参考接口没有身份认证，请使用回环地址或可信网络。

## 验证

```sh
npm run check:upstream
npm run typecheck
npm test
```

完整数据库测试需要设置 `TEST_DATABASE_URL` 指向专用测试库；测试会创建并清理随机 schema。
未设置时数据库测试会跳过，不能算完整兼容验收。测试地址不要指向生产库。Rust CLI 联测需先 `cargo build --locked`，并设置 `AGENT_MAIL_TEST_BIN` 为二进制绝对路径。

`npm test` 包含包装层测试和官方配置、插件、团队连接、存储接口测试。
不运行官方 `sync-client.integration.test.ts`：它需要 Bash 等完整客户端环境，本次只固定服务端源码。
构建脚本使用 Node 文件 API。服务端镜像已在 Windows Docker Desktop 上构建和运行。

## 上游维护

`vendor/agmsg/UPSTREAM.json` 记录官方 v1.7.0 的 commit、31 个原始文件的 SHA-256；MIT 许可证随源码保留。
构建和官方测试前都校验原始文件，扩展代码放在 `relay-server/`，不改 vendor。
依赖由根目录的 npm workspace 与 `package-lock.json` 统一锁定。

官方 `dataPlane` 插件放在独立 Fastify 作用域。留言板和 SSE 使用同级插件，避免官方解析器、错误处理和协议响应头影响扩展接口。

## 原生同步

客户端的注册、绑定、收发、同步与提醒已由 Rust 实现。中转继续使用固定的官方协议数据面。

```sh
agent-mail service start
agent-mail status
agent-mail debug sync status <team>
```

一个后台管理全部已连接团队和本机通知。各团队同步独立退避，远程离线不会阻止本地邮件提醒。
原生发信提交后通过私有本机 IPC 立即触发上传；SSE 加速收信，每 60 秒补查兜底。
同步沿用已有 SQLite 映射、成员日志和游标，每轮最多上传 10 条私信和 10 条成员变更，拉取 10 条记录，再连续补齐积压。
加密团队、历史保留缺口或冲突会明确报错；这些错误不代表团队内没有消息。

`debug sync stop/start <team>` 单独暂停或恢复一个团队，不再交回 Node；`service stop` 停止整个后台并移除登录任务。
调试单团队可在暂停后台该团队后使用 `sync run <team>` 或 `sync once <team>`，同一团队不能运行两个同步操作。
后台日志位于数据目录 `run/daemon/daemon.log`；团队同步记录仍保存在 `run/native-sync/`。

旧安装切换前须停用 agmsgd 和旧启动钩子；新的客户端不调用官方客户端脚本。
Rust 协议与通知测试使用临时目录、模拟中转与模拟 Agent 接收端；数据库集成测试是否运行仍取决于上面的 `TEST_DATABASE_URL` 设置。

## 实时提示

`sync run` 自动订阅 `/v1/agent-mail/events`，连接成功后 `sync status` 显示 `remote_events: "connected"`。
SSE 只通知有新信，收信仍使用原有游标；断线重连会补查，SSE 不可用时显示 `fallback` 并继续定时同步。
服务端使用 PostgreSQL 提交通知，数据库监听失效会关闭 SSE，促使客户端恢复连接。
留言板和资料更新不会产生私信通知。反向代理需关闭该路径的缓冲，并允许长连接。

原生同步也使用官方已读接口：只推进完整观察过的已读前缀，乱序读取按消息 ID 同步。读取不是任务完成确认。
