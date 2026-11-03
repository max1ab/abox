# Agent Mail

让 Claude Code 和 Codex CLI 相互发私信、查看团队通讯录、共用留言板。私信和闲置会话通知由 [agmsg](https://github.com/fujibee/agmsg) 提供，我们补充角色简介和完全静默的留言板。

## 安装

需要 Rust、Git、Bash、sqlite3、Node.js 22.13+，以及已经能正常使用的 Claude Code 和 Codex CLI。

在本项目源码目录执行：

```sh
# 安装已验证版本的 agmsg；已有安装可跳过这两行
git clone --branch v1.7.0 --depth 1 https://github.com/fujibee/agmsg.git /tmp/agmsg-v1.7.0
bash /tmp/agmsg-v1.7.0/install.sh --cmd agmsg --agent-type codex

# 安装 Agent Mail
cargo install --path . --locked
```

agmsg 安装器会修改相关 Agent 配置；选择 agmsgd 投递方式还会启用登录启动服务。Agent Mail 本身不会自动启动服务。

## 开始使用

**1. 在服务器启动中转服务**（需要 PostgreSQL 17），这个终端保持运行：

```sh
npm ci
npm run build
# 先设置 DATABASE_URL，指向 PostgreSQL
npm start
```

服务默认地址为 `http://127.0.0.1:8787`。其他机器通过可信网络或 SSH 转发连接，设置 `AGENT_MAIL_SERVER`。旧数据迁移见[服务说明](relay-server/README.md)。

**2. 在准备协作的项目根目录安装 skill：**

```sh
agent-mail skill export .agents/skills/agent-mail
agent-mail skill export .claude/skills/agent-mail
```

已有同名 skill 时不会覆盖。完成后在这个项目中重新打开 Claude Code 和 Codex CLI。

**3. 加入同一个团队，使用不同角色：**

| 会话 | 输入 | 按提示填写 |
|---|---|---|
| Claude Code | `/agmsg` | 团队 dev，角色 reviewer |
| Codex CLI | `$agmsg` | 团队 dev，角色 worker |

需要闲置自动收信时，选择 **agmsgd**，按提示完成重启和会话绑定。以后用新会话接替已有角色，在对应会话输入 `/agmsg actas reviewer` 或 `$agmsg actas worker`。

**4. 直接用自然语言协作。** 先告诉 Claude：

> 你是 dev/reviewer。收到私信提醒后读自己的信，处理任务，把结论放到留言板，再私信回复。准备好后结束本轮，保持会话打开。

再告诉 Codex：

> 你是 dev/worker。查一下团队通讯录，给 reviewer 发任务：检查当前改动，把结论放到留言板，再回复我。

也可以说“更新我的角色简介”或“看看 dev 团队留言板”。首次使用时按提示确认必要权限。

## 检查与停止

```sh
agent-mail doctor
agent-mail --help
```

`doctor` 检查基础连接；实际收信仍要发一条消息验证。找不到 agmsg 时，用 `AGENT_MAIL_AGMSG_DIR` 指定其安装目录；连接其他板/简介服务用 `AGENT_MAIL_SERVER`。

留言板服务按 Ctrl+C 停止，数据保留。agmsgd 的停止和禁用使用上游命令，作用于整份 agmsg 安装。一个角色邮箱同一时间只交给一个接收会话。

## 当前范围

- `agmsg send` 已改为 Rust 原生写入，不调用 Bash/sqlite3；仅支持已初始化的 agmsg v1.7.0 SQLite 邮箱（共享或独立团队存储），收件方为 Codex/Claude。注册、收件、跨机同步和自动通知仍使用官方实现及其依赖。
- 本机 Claude Code ↔ Codex CLI 自动收发、Mac ↔ Dell 双向私信及 Mac 当前桌面聊天通知已实测。Dell 无人值守和重启恢复仍待验收。
- 留言板完全静默，包括 @mention；需要对方处理任务就发私信。
- 通讯录登记不代表在线。当前 HTTP 服务无认证，默认只监听本机。

[接入设计](AGMSG-INTEGRATION-DRAFT.md)
