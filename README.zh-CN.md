# LimitDeck

[English](README.md)

一个极简、隐私安全的 AI Coding Plan 剩余额度终端面板。

![LimitDeck 演示](assets/limitdeck.gif)

```text
› Codex    Codex    7d  ━━━━━━━━━━━━━━──────────  61%
           Spark    5h  ━━━━━━━━━━━━━━━━━━━━━━━━ 100%
           Spark    7d  ━━━━━━━━━━━━━━━━━━━━━━━━ 100%
```

LimitDeck 优先复用 Provider 的官方本地登录入口。它不会复制凭证、读取浏览器 Cookie、抓取订阅网页，也不会直接读取 Codex 的 `auth.json`。

## 数据源支持

| Coding Plan | 数据入口 | 状态 |
| --- | --- | --- |
| Codex | 官方 Codex App Server：`account/rateLimits/read` | 内置 |
| Claude | 官方 Claude Code status line JSON | 内置 |
| Codex 兼容回退 | OMP 脱敏 usage 输出 | 可选 |

Codex 官方 App Server 与 OMP 同时可用时，LimitDeck 优先使用官方入口。

## 安装

### Homebrew

```bash
brew install rockythink/tap/limitdeck
```

### Cargo

需要 Rust 1.88 或更高版本：

```bash
cargo install limitdeck
```

### 预编译二进制

从 [GitHub 最新 Release](https://github.com/rockythink/limitdeck/releases/latest) 下载 macOS 或 Linux 压缩包及 `SHA256SUMS`。

安装后可在任意终端运行：

```bash
limitdeck
```

LimitDeck 会自动发现已安装、已登录的 Codex CLI，不需要再次登录 LimitDeck。

## 接入 Claude

Claude Code 通过官方 status line 输入提供订阅额度。将以下配置加入 `~/.claude/settings.json`：

```json
{
  "statusLine": {
    "type": "command",
    "command": "limitdeck ingest claude",
    "refreshInterval": 60
  }
}
```

命令只会保存额度白名单快照。缓存路径为 `$XDG_CACHE_HOME/limitdeck/claude.json`；未设置 `XDG_CACHE_HOME` 时使用 `~/.cache/limitdeck/claude.json`。Claude Code 只会为符合条件的订阅提供 `rate_limits`，且通常要等当前会话完成第一次 API 响应后才出现。

这项配置会替换已有的 Claude Code 自定义状态行。如果你已经有状态行脚本，请在原脚本中调用 `limitdeck ingest claude`，并通过 stdin 传入原始 JSON。

## 操作

| 按键 | 操作 |
| --- | --- |
| `↑` / `↓`、`k` / `j` | 选择 Plan |
| `Enter` | 打开或关闭详情 |
| `Esc` | 返回列表，再次按下退出 |
| `r` | 刷新 |
| `q` | 退出 |

窄终端会自动降级为紧凑百分比；Plan 数量超过窗口高度时，列表会跟随选中项滚动。

打开 Plan 详情后，终端行数充足时会显示本地采样的额度历史。当前重置周期内至少有 3 个样本且额度发生变化时，界面会绘制紧凑的 Braille 微型折线，并标注真实时间跨度、起止值和变化量；历史持平或样本稀疏时只显示文字摘要，避免画出误导性的粗条。额度上升时开启新的视觉周期。历史数据保存在 `$XDG_CACHE_HOME/limitdeck/history.json`；未设置 `XDG_CACHE_HOME` 时使用 `~/.cache/limitdeck/history.json`。每个额度窗口最多保留 30 天和 2,048 个样本；取得前两个真实样本后，未变化的值最多每 15 分钟采样一次。

## 故障诊断

数据源刷新失败后不会从列表消失：已有快照时标记为“缓存”，没有快照时显示“不可用 · Enter 查看原因”。按 `Enter` 可查看安全的失败分类和下一步操作：

| 原因 | 处理 |
| --- | --- |
| 找不到来源命令 | 安装对应 CLI，确认命令已加入 `PATH`，再按 `r` |
| 来源尚未登录 | 登录对应来源，再按 `r` |
| 请求超时 | 检查网络，再按 `r` 重试 |
| Provider 协议变化 | 升级 LimitDeck；仍失败时提交 issue |
| 缺少 Claude 快照 | 将 `limitdeck ingest claude` 配置为 Claude Code status line |
| 缓存快照过期 | 按 `r` 刷新来源 |

诊断状态只保留并展示固定的数据源标签与失败分类。原始 stderr、Provider 响应、凭证、账户字段、邮箱和本地路径都不会进入应用状态。


## 隐私模型

LimitDeck 只保留：

- Provider、Plan 和额度窗口标识
- 展示标签与窗口周期
- 剩余百分比与重置时间
- 额度历史的时间戳、剩余百分比与可用状态

它会主动忽略邮箱、account ID、组织、套餐等级、账单信息、原始凭证和 Provider 原始响应。Parser 输入与子进程输出均有大小限制；子进程有明确超时，并在退出时回收。

## 开发

```bash
cargo fmt --check
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo build --release
```

架构保持小而明确：

```text
官方协议 / 安全快照 / 可选回退
               |
          PlanAdapter
               |
    CodingPlan -> UsageWindow
               |
      App 状态 + 本地历史 -> TUI
```

Adapter 负责 Provider 特定的解析与超时；领域模型和 UI 不依赖 Codex、Claude 或 OMP 的响应格式。

## 声明

LimitDeck 是独立开源项目，与 OpenAI、Anthropic 或 OMP 不存在隶属或背书关系。Provider 接口和订阅额度可能发生变化。

## 许可证

[MIT](LICENSE)
