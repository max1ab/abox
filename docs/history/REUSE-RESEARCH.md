# Agent 通信组件复用调查

实测更新（2026-10-30）：Agent Relay 的本机闲置投递尚未通过，不能标为可直接采用；Relaycast 的离线队列、频道历史和静音/@mention 行为已验证。下文是实测前的候选评估。

核查日期：2026-10-30。通过 GitHub REST API 读取仓库元数据、默认分支最近最多 100 条提交、最近 3 次 release，并核对官方 README、许可证和自托管文档。日期按 API UTC 日期列示，star 是本次快照。未安装或运行候选，不将 README 宣称视为端到端实测结果。

## 维护状态

| 项目 | Star | 默认分支最新提交 | 最近发布 | 提交节奏与判断 |
| --- | ---: | --- | --- | --- |
| AgentWorkforce/relay | 1,059 | 10-30 | probe-v15.0.1，10-30 | 最近 100 条覆盖 10-26 至 10-30，含合并提交；持续活跃 |
| AgentWorkforce/relaycast | 14 | 10-29 | v12.2.1，10-29 | 最近 100 条覆盖 10-12 至 10-29；活跃，不能因单独仓库 star 少而判死 |
| Dicklesworthstone/mcp_agent_mail_rust | 224 | 10-30 | v0.3.39，10-13 | 最近 100 条覆盖 10-27 至 10-30；持续活跃 |
| Dicklesworthstone/mcp_agent_mail | 2,228 | 10-26 | v0.3.2，04-16 | README 明确已被 Rust 版取代、不再维护；排除新部署 |
| alexfrmn/murmur | 20 | 10-21 | v2.12.1，10-17 | 最近 100 条覆盖 09-29 至 10-21；活跃但社区小 |
| jahwag/agentbus | 2 | 09-19 | v0.1.0，07-26 | 默认分支仅 14 条；近期主要依赖和文档，不列首选 |
| erichexter/agentmail | 6 | 09-11 | v0.5.1，09-11 | 总计 18 条提交；已约 7 周无提交，不列首选，不能据此断言死亡 |
| iambashar/agent-intercom | 1 | 10-18 | v0.2.5，10-18 | 18 条提交集中在一天，缺持续维护证据；只作适配参考 |
| nats-io/nats-server | 20,968 | 10-29 | v2.15.2，10-26 | 最近 100 条覆盖 10-15 至 10-29；成熟底座 |
| stalwartlabs/stalwart | 15,510 | 10-28 | v1.0.1，10-28 | 最近 100 条覆盖 09-04 至 10-28；活跃的传统邮件服务器 |

注意：`pushed_at` 不等于默认分支代码更新时间。例如 AgentBus 的 pushed_at 是 10-25，但默认分支最新提交仍为 09-19。本表采用提交记录，不用 pushed_at 冒充开发活跃度。

## 首选：Agent Relay + Relaycast

- Relay 提供 Codex/Claude 等工具接入，支持不同机器共享工作空间、私信、频道、线程和实时事件。
- Relaycast 是消息存储与路由引擎，可自托管为 Node + SQLite 单进程，不依赖 Cloudflare；有 REST/WebSocket 和官方 Rust SDK。Relay 仓库也包含 Rust 组件，但整个系统并非纯 Rust。
- 自托管消息、频道、私信和本地节点不要求 Cloud 账号；云沙箱、托管看板、共享会话等另有 Cloud 边界。
- 官方 broker 方案包含启动 Agent 和 PTY 注入；另有可选 harness-driver。不能直接推定它可唤醒任意已打开的 Codex 桌面聊天。需实测正常启动的现有会话能否接入、接收时是否需要托管其进程。
- 留言板可用频道历史实现，但默认消息频道有推送。静音普通频道消息会取消普通持久投递和实时推送，显式 @mention 仍产生投递。严格实现「留言板完全不通知」需要入口约束或适配过滤。
- Agent 名称需要唯一；我们的「允许重名＋唯一 ID」需用内部唯一寻址名加展示名映射。30 天按主动活动注销及删除全部留言的规则也需补适配。
- Apache-2.0。优先作为现成系统验证；若运行时适配不适合，则复用 Relaycast 服务和 Rust SDK，自己写薄适配层。

来源：
- https://github.com/AgentWorkforce/relay
- https://github.com/AgentWorkforce/relay/blob/main/docs/self-hosting/README.md
- https://github.com/AgentWorkforce/relay/blob/main/packages/harness-driver/README.md
- https://github.com/AgentWorkforce/relaycast
- https://github.com/AgentWorkforce/relaycast/blob/main/packages/sdk-rust/README.md

## Rust 成品候选：MCP Agent Mail Rust

- 已有身份登记、私信、回复、读/确认状态、主题读取、项目邮箱、搜索、CLI、Web UI 和 TUI。
- 身份与项目路径绑定，跨项目有联系人策略；项目邮箱和主题历史接近公共讨论空间，但不等同于我们定义的独立公共留言板。
- 项目邮箱消息可进入既有 Agent 的收件箱，不能直接当成严格静默留言板。
- 在线原会话唤醒尚需独立核验，不能从 MCP 兼容推断闲置唤醒已解决。
- 技术栈使用 Asupersync、FrankenSQLite 等，并非我们原定 Tokio + rusqlite 的轻量骨架。功能和维护复杂度较高。
- LICENSE 为 MIT with OpenAI/Anthropic Rider，含针对 OpenAI、Anthropic 及相关主体的附加限制，不应标成标准 MIT；复用需核对具体适用范围。

来源：
- https://github.com/Dicklesworthstone/mcp_agent_mail_rust
- https://github.com/Dicklesworthstone/mcp_agent_mail_rust/releases
- https://github.com/Dicklesworthstone/mcp_agent_mail_rust/blob/main/LICENSE
- https://github.com/Dicklesworthstone/mcp_agent_mail （Python 版停止维护声明）

## 次选与基础组件

Murmur：MIT，跨机器加密通信，NATS 可选 JetStream，已有 Claude asyncRewake、Codex app-server/queue 相关适配。README 对部分平台和模式的唤醒仍明确保留限制，功能迭代多，20 star 的外部验证有限。可作实际接入代码的参考，优先级低于 Relay。

NATS + JetStream：Apache-2.0，持久化、离线重放、确认和重投都有，支持 Rust 客户端。Core NATS 单独使用不保存离线消息。需要自行实现 Agent 通讯录、消息对象、留言查询、生命周期、MCP 和运行时唤醒；适合坚持自己实现应用层时使用，不能当现成 Agent 邮箱。

Stalwart：Rust 传统邮件及协作服务器，提供 SMTP/IMAP/JMAP 等协议；邮箱和离线存信成熟，但不提供 Codex/Claude 会话唤醒及我们的 Agent 公共留言板。仅为了 Agent 内部通信引入整套邮件服务不划算；若要与真实 email 地址互通，再评估。

来源：
- https://github.com/alexfrmn/murmur
- https://github.com/nats-io/nats-server
- https://docs.nats.io/concepts/jetstream
- https://github.com/stalwartlabs/stalwart

## 下一步验证建议

先验证 Relay/Relaycast 的现成链路：两台机器的 Codex 与 Claude 各一个正常会话，闲置时收到私信并读信回复；接收方退出后保存信，上线后补收；公共板只主动读，即使出现 @ 也不唤醒；同名会话互不串信。记录进程是否由 Relay 托管、是否原会话、是否真实产生读信/回复回执。通过后再决定直接采用还是用 Rust SDK 补薄适配层。
