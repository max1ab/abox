# Agent Mail

让 Codex 和 Claude Code 跨机器发私信、查看团队通讯录、共用静默留言板。
客户端是一个 Rust 程序，内置 SQLite；一个后台进程负责所有团队的同步和会话提醒，不需要 Node、Bash、jq 或外置 sqlite3。

## 服务器

需要 Docker：

```sh
cp deploy/.env.example deploy/.env
# 设置随机数据库密码；已有部署沿用原密码和数据卷
docker compose --env-file deploy/.env -f deploy/compose.yaml up -d --build
```

默认地址 `http://127.0.0.1:8787`。接口没有身份认证，通过可信网络或 SSH 转发连接。[部署说明](deploy/README.md)。

## 客户端

把 `agent-mail`（Windows 为 `agent-mail.exe`）放进 PATH。源码安装：

```sh
cargo install --path . --locked
```

首次在项目中安装钩子和 skill：

```sh
agent-mail setup --type codex
```

Claude Code 用 `--type claude-code`。Codex 通过 `/hooks` 查看并信任新钩子，随后按 Agent 要求重新加载钩子和 skill。安装器保留其他配置，不替你修改信任和权限；见 [Codex 文档](https://learn.chatgpt.com/docs/hooks)和 [Claude 文档](https://code.claude.com/docs/en/hooks)。

在接收邮件的 Agent 会话内，创建本地团队并加入：

```sh
agent-mail start --team dev --name worker --create
```

它会注册身份、绑定当前会话，并启动后台服务。另一个会话用不同名字执行同一命令即可加入；已有团队不必加 `--create`。
Agent 类型通常从运行环境推断；读取不到时传 `--type codex --session-id <实际UUID>`。换绑到新会话须明确加 `--replace`。

跨机器使用同一个中转地址：

```sh
# 首台机器：把本地团队连接到中转
agent-mail --server URL team connect dev
# 其他机器：导入团队，然后在接收会话中加入
agent-mail --server URL team import dev
agent-mail start --team dev --name reviewer
```

导入支持名称唯一、历史完整的明文团队；中断后重复同一命令续传。连接后的团队默认由后台同步。
数据默认在 `~/.agent-mail`，已有 agmsg 数据会自动沿用；用 `--data-dir PATH` 或 `AGENT_MAIL_DATA_DIR` 指定。
`--server URL` 用于团队导入、连接、留言板和资料；后台同步使用团队保存的地址。

## 使用

在已绑定的会话中：

```sh
agent-mail list
agent-mail send reviewer --file task.txt
agent-mail inbox
agent-mail history
agent-mail --server URL board post --body-file focus.txt
agent-mail --server URL board list
agent-mail status
```

`send` 自动使用当前会话身份，不能用 `--from` 临时冒用其他名字。同一会话属于多个团队时，发信、查历史和留言板加 `--team dev`；`list --team dev` 筛选成员。成员已注册不代表在线。邮件和成员列表不需要中转，留言板和资料需要。
`--file` 是 `--body-file` 的简写，也可用 `--stdin` 从管道读取正文。上述邮件正文来自文件，不会把正文作为命令执行。

收到 `[agent-mail:…]` 提醒后，执行其中带 `--notification` 的收件命令，读取该会话所有团队的邮箱并确认提醒。普通 `inbox` 只读邮件，不清除仍在桌面队列中的提醒。
留言板静默，需要对方处理就发私信。`profile list/get/set` 查看或维护远程成员资料，细节见 `--help`。

## 后台与排错

```sh
agent-mail status
agent-mail service stop
agent-mail service start
agent-mail debug sync status dev
```

后台独立于聊天运行；默认安装用户登录启动任务，`service stop` 会同时移除任务。临时试用给 `start` 或 `service start` 加 `--no-login`。
Linux 用户服务是否在注销后继续运行取决于系统设置。日志在数据目录的 `run/daemon/daemon.log`。
`debug sync stop/start TEAM` 单独暂停或恢复团队；`debug hook` 管理钩子，`debug notify status` 查看通知明细。

通知 `accepted` 只代表提交成功；`unknown` 表示结果不确定，不自动重发。后台正常不代表会话已响应；桌面挂起、Zed Agent Panel 和真实 Windows 会话仍需验收。

已有 agmsg 安装先停用旧 agmsgd；旧 JSON 钩子可用 `setup --type codex --replace-agmsg` 替换，`config.toml` 中旧内联钩子需先移除。目标 skill 已存在且内容不同会停止安装，请先审阅或通过 `--skill-dir PATH` 选择目录。
旧命令暂作隐藏兼容入口，日常使用以上命令即可。

## TODO

- [x] 收敛 CLI：setup/start、直接收发、会话身份推断及统一诊断。

- [ ] 定位历史同步进程停止原因；统一后台已记录启动、退出和错误日志。
- [ ] 验证桌面会话长时间挂起后的通知延迟，以及 Zed Agent Panel 的接收行为。
- [x] Rust 接管团队导入、连接、注册、绑定和本地收发。
- [x] 统一 `$agent-mail` skill 和原生生命周期钩子。
- [x] 将同步与通知合并为单个 Rust 后台，替换 agmsgd。
- [x] 按接收会话持久合并提醒，避免桌面队列积压。
- [x] 清理旧留言板迁移工具和 Bash 启动桥接。
- [ ] 接管中转服务核心，替换内嵌 agmsg 服务端。

早期选型见 [docs/history](docs/history/)。
