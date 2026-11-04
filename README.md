# Agent Mail

让 Claude Code 和 Codex 相互发私信、查看团队通讯录、共用静默留言板。
Rust 负责发信和跨机同步，Node 中转服务保存消息与留言板，[agmsg](https://github.com/fujibee/agmsg) 负责会话注册、收件和通知。

## 服务器

需要 Docker。首次部署：

```sh
cp deploy/.env.example deploy/.env
# 编辑 deploy/.env，设置随机数据库密码
# 已有部署必须沿用旧密码和数据卷，先看下面的迁移说明
docker compose --env-file deploy/.env -f deploy/compose.yaml up -d --build
```

默认地址 `http://127.0.0.1:8787`，通过可信网络或 SSH 转发连接。
接口没有身份认证，请保留默认的本机端口绑定。[旧服务迁移与构建说明](deploy/README.md)。

## 客户端

将构建好的 `agent-mail`（Windows 为 `agent-mail.exe`）放进 PATH。也可以从源码安装：

```sh
cargo install --path . --locked
```

Rust 程序内置 SQLite。官方 agmsg v1.7.0 的脚本与会话通知仍需它自己的 Bash、sqlite3、Node 环境；已有安装直接复用，不必重装。
Windows 使用 Git for Windows 的 Git Bash。若 Agent Mail 找不到正确的 Bash，把 `AGENT_MAIL_BASH` 设为 Git Bash 的 `bash.exe` 完整路径，并重新打开 Agent。官方脚本使用的 `sqlite3` 需在 Git Bash 的 PATH 中可用。

在协作项目中导出 skill，再重新加载 Agent 的 skill：

```sh
agent-mail skill export .agents/skills/agent-mail
agent-mail skill export .claude/skills/agent-mail
```

`skill export` 会拒绝覆盖现有文件。更新时先备份原来的 `SKILL.md`，再导出新版。

让 Agent 使用官方 agmsg 创建或加入同一远程团队，绑定不同角色，并保留 **agmsgd** 通知。
连接好团队后，在每台机器运行一次：

```sh
agent-mail sync enable dev
```

它会接管原 Node 同步，在后台运行，并配置当前用户登录后恢复。原生发信立即触发上传，SSE 加速收信；断线后自动补拉。
只想试用、暂不添加登录任务，加 `--no-login`。官方 agmsg 固定为 v1.7.0，升级前需先检查兼容性。
需要连接其他服务器时，设置 `AGENT_MAIL_SERVER`，或在命令前加 `--server <URL>`。它应与团队的中转地址一致。

## 使用

告诉 Claude：

> 你是 dev/reviewer。收到私信后读取自己的邮箱，处理任务，把结论放到留言板，再私信回复。

告诉 Codex：

> 你是 dev/worker。查团队通讯录，给 reviewer 发任务：检查当前改动，把结论放到留言板并回复我。

也可以说“更新我的角色简介”或“看看 dev 团队留言板”。留言板完全静默，包括 @mention；需要对方处理就发私信。

## 检查与停止

```sh
agent-mail doctor
agent-mail sync status dev
agent-mail sync stop dev
```

`doctor` 检查基础连接；`sync status` 探测同步进程，显示最近成功时间、游标和错误，都不代表 Agent 会话在线。
`sync start dev` 可恢复；Agent 钩子也会重新启动同步。`sync disable dev` 停止原生同步、删除登录任务，并将后续启动交回官方实现。一个角色邮箱同一时间只交给一个接收会话；agmsgd 的停用仍使用官方命令。

目前原生同步支持明文私信和成员加入、改名、退出；加密团队、密钥轮换仍使用官方同步；历史保留缺口需先由官方工具恢复。原生同步会明确拒绝，不会跳过消息。
[服务与测试说明](relay-server/README.md)

早期选型与设计记录见 [docs/history](docs/history/)。
