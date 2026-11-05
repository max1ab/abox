# 原生后台 review 入口

本轮完成客户端替换，服务器仍使用 Node + PostgreSQL 的固定 agmsg 协议数据面。
现有机器上的安装、钩子、会话和运行服务未切换。

## 主要文件

- `src/notify/transport.rs`：Codex queue、Claude Unix socket / Windows named pipe；成功、失败和未知结果分别记录。
- `src/notify/state.rs`：发送前持久预留 nonce；进程中断转为 unknown，不自动重发。
- `src/notify/worker.rs`：跨团队按接收会话合并提醒；聚合收件确认 nonce，普通单邮箱读取不提前清除排队提醒。
- `src/daemon/`：单一后台、系统登录任务、独立团队同步任务、IPC、日志和状态。
- `src/hooks.rs`：原生钩子安装与会话信息刷新；保留其他配置，不替用户批准 Codex 钩子。
- `src/connect.rs`：本地团队连接中转，稳定身份与重试核对。

旧的 Bash 启动桥接和对应测试已删除。`src/sync/service.rs` 仅用于回收本数据目录旧版 Rust 创建的团队登录任务，不调用上游脚本。
`--data-dir` 为首选参数，旧 `--agmsg-dir` 和 `AGENT_MAIL_AGMSG_DIR` 保留兼容；现有数据库不搬迁。

## 隔离验证

本轮最终回归：62 项测试通过，Clippy（零 warning）、格式检查、差异检查及本机 release 构建通过。9 项需要旧上游客户端脚本的可选兼容测试未在本轮运行，不计入上述结果。

测试使用临时目录、模拟 Codex 可执行文件、临时 Claude socket 和本机模拟中转。没有给真实聊天发测试提醒，也没有安装真实登录任务。

覆盖：

- 原生通知参数、回执核对、超时、socket 鉴别、认证行和消息格式。
- 多团队单提醒、手动读取不产生额外排队通知、未知结果跨重启保留。
- 重复启动排除、团队断网不阻塞本地提醒、正常停止、强制终止后恢复。
- 原生钩子的配置保留、显式替换旧 JSON 钩子、内联 TOML 冲突报告、会话匹配和 Claude clear 刷新。
- 团队创建与连接重试、导入续传、本地收发、读状态和事务回滚。

## 真实环境验收

以下尚未等同于通过：真实 Codex/Claude 自动响应、桌面长时间挂起、Zed Agent Panel、Windows 原生管道、各系统注销/登录与机器重启。当前工具链仅安装了本机 macOS 目标，本轮没有交叉编译 Windows。

建议 review 后在独立客户端目录验证：注册两个会话，分别试通知、聚合收件、关闭聊天、重开会话、断网和后台重启。确认后再停用旧 agmsgd、替换原安装钩子，并启用新后台。新旧通知引擎不能同时接管同一数据目录。

Codex 使用官方 hooks.json / commandWindows 结构，仍需用户审阅并信任；Claude Windows 使用 shell=powershell。参考 [Codex hooks](https://learn.chatgpt.com/docs/hooks) 与 [Claude hooks](https://code.claude.com/docs/en/hooks)。这些配置不会解决宿主自身挂起或不支持 queue/socket 的限制。
