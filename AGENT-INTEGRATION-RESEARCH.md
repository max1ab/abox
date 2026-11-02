# Agent 接入层复用调查

2026-10-30。目标是把 Codex、Claude 及后续 Agent 的接口适配、升级兼容交给活跃上游维护。本轮为官方仓库/API/协议文档核查，没有安装或实测以下候选。

## 维护快照

GitHub REST API 查询默认分支最近 30 条提交与 release；日期按 UTC 日列示。

| 项目 | Star | 最近提交/发布 | 判断 |
| --- | ---: | --- | --- |
| agentclientprotocol/agent-client-protocol | 4,542 | 10-30 提交；v1.13.0 10-27 发布 | 协议生态活跃；v1 为稳定协议，v2 仍为 draft |
| agentclientprotocol/codex-acp | 497 | 10-29 提交；v2.5.0 10-29 发布 | 最近 30 条跨约 6 天，活跃 |
| agentclientprotocol/claude-agent-acp | 2,707 | 10-29 提交；v0.99.1 10-29 发布 | 最近 30 条跨约 3 天，活跃 |
| giuliastro/harness-remote | 430 | 10-29 提交及 v3.7.0 发布 | 最近 30 条跨约 5 天，活跃 |
| digitARTI/turnlink | 1 | 10-15 提交，无 release | 有近期适配开发，但依赖维护者单点、社区验证少 |
| coder/agentapi | 1,496 | 09-13 归档，README 明确弃用 | 排除 |
| twaldin/harness | 29 | 已归档，最近提交 09-10 | 排除 |
| beyond5959/acp-adapter | 20 | 最近提交 04-14 | 不列优先依赖 |

## 优先方向：ACP + 现成适配器

ACP 统一会话创建/加载、prompt、流式事件、工具权限、取消等接口。Rust 有 `agent-client-protocol` crate，可实现 Client 端。邮箱收到消息后，节点程序向受控会话发送 `session/prompt`，由适配器驱动 Agent 继续一轮；无需让模型持续思考或轮询。

`codex-acp` 启动 Codex App Server 并转换请求/事件；npm 包自带兼容的 `@openai/codex` 依赖，也允许 CODEX_PATH 指定二进制。`claude-agent-acp` 使用 Claude Agent SDK。两者位于 ACP 组织并持续维护，不应误称为两家模型厂商保证的通用原会话接管接口。

适配器承接多数后端差异。我们仍需维护统一 Client、消息到会话映射、权限请求处理和有限兼容验收，不能承诺从此无需升级。新增已有 ACP 后端通常可复用同一 Client，但应协商能力，不能假设所有扩展都通用。

关键边界：默认架构由 Client 启动适配器/后端进程。`session/load`/resume 可以恢复历史，前提是后端支持；恢复历史不等于连接另一个正在运行的 CLI/桌面 App 进程，更不证明两个 UI 可安全同时控制同一会话。Codex 桌面原聊天投递仍需单独验证。

来源：
- https://agentclientprotocol.com/get-started/architecture
- https://agentclientprotocol.com/protocol/v1/session-setup
- https://agentclientprotocol.com/protocol/v1/prompt-turn
- https://agentclientprotocol.com/libraries/rust
- https://github.com/agentclientprotocol/codex-acp
- https://github.com/agentclientprotocol/claude-agent-acp

## 可复用的网关：Harness Remote

它已经在 ACP 上实现多机器网关、原生会话发现/恢复、权限与能力发现，支持 Codex、Claude、Copilot、OpenCode、MiMo、OMP、PI 等。Apache-2.0。

README 声称能发现普通 CLI 建立的原生会话并继续，且明确：运行中的 turn 在网关启动的 harness 进程内执行，网关重启会中断 turn。这意味着原生历史的延续与正在运行的任意会话进程接管必须区分。

它是带桌面/Web/Android 的完整控制平面，不是一个已验证可嵌入的轻量库；需要核查网关 API 是否适合 agent-mail 独立复用，不能因它支持多 Agent 就直接引入完整产品。

来源：https://github.com/giuliastro/harness-remote

## 已有会话投递方向：Turnlink

明确目标是跨 host 给既有会话发送消息并唤醒。Codex 官方 VS Code 扩展走同进程 App Server stdio 代理，闲置 `turn/start`、忙时 `turn/steer`；Claude 走 Channels MCP；OpenCode 走插件。README 提供 VS Code Windows 的远程 Codex 活模型证据。

这一路线更贴近保留现有会话，但当前仅 1 star，无 release；部分其他后端的验证仍有限。可作适配参考或实验候选，不适合承诺把长期兼容维护全交给它。它的 Codex 文档明确覆盖 VS Code 扩展，不应扩大解释为 Codex 桌面 App 已支持。

来源：https://github.com/digitARTI/turnlink

## 待用户澄清的选型分界

已提问：是否接受统一入口承载 Agent，还是必须向已打开的 CLI/桌面 App 原会话送消息，或两者都要。前者优先 ACP 适配器；后者需已有会话的原生接入，暂无本轮证据支持一个高成熟度、跨所有宿主通用的现成接管层。
