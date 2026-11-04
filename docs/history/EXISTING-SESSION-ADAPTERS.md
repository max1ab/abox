# 保留原生 Agent 界面的接入轮子

核查日期：2026-10-30。AnySearch 检索，GitHub 官方 API 核查 star、默认分支最近 30 条提交、最近 2 个 release；读取官方 README、正式 tag 文档及部分适配代码。本轮未安装或实测候选。提交日期采用 UTC 日。

后续实测更新（2026-11-02）：agmsg v1.7.0 的 Claude Code、Codex CLI 本机空闲原会话收信和自动回信已分别通过；Codex 重测连续两轮约 17 秒回信。本轮实测范围为本机 CLI 原会话。以下保留调研时点资料。

## 重点候选

| 项目 | Star | 更新与发布 | 接入边界 |
| --- | ---: | --- | --- |
| fujibee/agmsg | 1,571 | 10-30 代码提交；v1.7.0 于 10-27 发布；最近 30 条跨约一周 | 原生 CLI 中安装 skill、入组；需要首次重启加载。agmsgd 通过 Codex queue / Claude 会话 socket 发未读提醒；仍标 beta |
| aannoo/hcom | 614 | 10-21 提交及 v0.7.31 发布；此前 10-15 发 v0.7.30，最近 30 条覆盖 10-20 至 10-21 | `hcom codex` / `hcom claude` 启动原生 CLI；PTY 与 hook 处理唤醒，MQTT relay 跨机器。不能据此认定能接管任意已开进程 |
| asheshgoplani/agent-deck | 1,183 | 10-25 提交及 v1.16.34 发布；最近 30 条覆盖 10-24 至 10-25 | tmux 会话管理层；支持给会话发送/排队消息，远程 SSH 管理；属于更完整的会话管理器 |
| zm2231/agenthail | 2 | 10-29 提交及 v0.2.92 发布；最近 30 条覆盖 10-27 至 10-29 | 声称发现已有 Claude socket、连接既有 Codex 会话；活跃但社区验证极少，不优先承担长期兼容依赖 |

## agmsg

符合“安装接入层、照常在原生 Agent 对话”的方向。v1.7.0 tag 的 README 已包含 agmsgd beta，不是只有 main 上的未发布承诺。

- Claude Code、Codex、Gemini、Copilot、OpenCode 等独立 peer 会话通过 SQLite 存储消息；各 Agent 的适配驱动独立。
- 可选 agmsgd 常驻程序：Codex 调用 queue，Claude 向其当前会话跨会话消息 socket 发“检查未读”提醒；正文仍由 Agent 读取。
- 安装/更新不会自动启用 daemon。需首次选择投递模式、登记实际会话目的地；已有 bridge 会话保持原方式，切换需要重启。不能声称装上后所有已经打开的窗口立即可达。
- 不支持直接投递时保留旧 Monitor/hook 路线；`turn` 模式只在交互边界查信，不能当作闲置唤醒。
- Codex App `where` 驱动只识别桌面位置，明确“不改变投递，也不唤醒桌面闲置聊天”。因此 CLI 兼容不能扩大为桌面原聊天兼容。
- 跨机器有参考服务和远程同步文档；服务使用 PostgreSQL，当前参考部署无认证，要求置于受控网络/SSH 转发等边界内。不是一个开箱即可公开暴露的公网 mailbox 服务。
- MIT；本体大量 shell 脚本，agmsgd 为 Node 运行时。我们可直接使用，或复用其投递层，但不属于纯 Rust 单二进制。

来源：
- https://github.com/fujibee/agmsg
- https://github.com/fujibee/agmsg/blob/v1.7.0/README.md
- https://github.com/fujibee/agmsg/blob/main/docs/codex-monitor-beta.md
- https://github.com/fujibee/agmsg/blob/main/scripts/drivers/terminals/codex-app/README.md
- https://github.com/fujibee/agmsg/blob/main/docs/remote-setup.md

## hcom

Rust 实现的 CLI，MIT。保留原生 Claude/Codex 终端交互，不需要用 ACP 自己做聊天界面；代价是在启动命令前加 `hcom`，让它持有 PTY 与会话绑定。

- 源码 `src/hooks/codex.rs` / `src/hooks/claude.rs` 实现独立后端 hook 适配；Claude 的 PTY 模式由 wrapper 注入唤醒标记并在 hook 中提供正文，避免只靠 Stop hook 无法闲置唤醒的问题。
- README 列出 Claude、Codex、OpenCode、Gemini、Copilot、Pi/OMP、Cursor、Kimi、Hermes 等自动接入项；其他工具可手动 listen，但不能把手动通道算成已维护的自动适配。
- 有 `send`、活动观察、事件订阅、fork/resume 等接口，适合用作节点层，或整个通信工具直接采用。
- 跨机器采用 MQTT relay：各机器使用 relay token 连接。自托管选择、可靠投递细节仍需进一步检查或测试。
- `hcom start` 不足以证明可以无条件接管一个没有经过 wrapper 启动的 Codex 进程。现有 CLI 可考虑重启/恢复历史后接入；已有桌面 App 原会话无本轮支持证据。

来源：
- https://github.com/aannoo/hcom
- https://github.com/aannoo/hcom/blob/v0.7.31/README.md
- https://github.com/aannoo/hcom/blob/main/src/hooks/codex.rs
- https://github.com/aannoo/hcom/blob/main/src/hooks/claude.rs
- https://github.com/aannoo/hcom/blob/main/Cargo.toml

## Agent Deck

Go + tmux，支持原生 CLI、send/queue、状态观察和远程 SSH。它的维护者承担多种 CLI 的终端适配。适合接受把会话放入 tmux 管理的人；不应把 tmux 会话发送能力说成能接管任意外部窗口。主菜单 composer 当前只支持本机 session 行，远程路径需按独立命令核验。

来源：https://github.com/asheshgoplani/agent-deck

## 低优先级或排除

- xhluca/agent-talk：201 star；最近 09-10 更新，更新稀疏。Claude 有自动接收，Codex 主要依靠交互边界 hook，不能凭此认定能闲置唤醒。
- yigitkonur/gossip：2 star，最近 05-07 提交，不选为维护底座。
- d4j3y2k/oxtail：1 star，最近 07-29 提交，且聚焦单机单项目，低优先级。
- heisenbear-rebirth/codex-claude-bridge：3 star，10-08 更新，无 release；明确覆盖 Codex 桌面已有会话与 VS Code Claude，但主要 Windows 部署、社区验证少。留作实现参考。
- agenthail：已有会话发现方向很贴近；2 star 无法支撑“长期放心把全部适配交出去”的判断，且未实测。

## 当前判断

优先检验 agmsg 的原生 CLI 安装路径及 agmsgd；若希望复用 Rust 单程序和更多后端适配，并能接受启动前加 `hcom`，则检验 hcom。它们比从零编写适配器更可能省工作，但需要一次真实闲置收信/原会话回复验收，不能因文档写支持就直接定案。

严格要求“现在已经打开、未配置接入的 Codex 桌面聊天，无需重启或改变入口即可被控制”时，本轮仍无高成熟度方案可确认。安装后重启一次、此后保留原生界面使用，与即时接管所有旧进程必须分开。
