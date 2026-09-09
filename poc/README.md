# Codex Switch Rust POC

这个 POC 验证本地 Proxy 是否可以根据 Codex Session ID，把并发请求路由到不同 Profile。

## 验证

```bash
cargo test
```

测试会启动：

- 3 个本地假上游
- 1 个本地 Proxy
- 30 个并发请求

并检查：

- `session-a -> aihezu`
- `session-b -> her`
- `session-c -> sakura`
- 未携带 Session ID 的请求 -> `sakura`
- 请求体 `metadata.session_id` 可以识别

## 启动

```bash
cp poc/config.example.json poc/config.json
cargo run -- --config poc/config.json
```

示例配置中的上游地址是占位地址，启动 Proxy 本身不需要立即访问上游；发送请求前需要把它们改成可访问的真实地址。

也可以使用环境变量：

```bash
CODEX_SWITCH_POC_CONFIG=/path/to/config.json cargo run
```

## 当前边界

- 监听地址固定建议使用 `127.0.0.1`
- 不记录请求体、响应体和认证信息
- Profile 配置当前使用 JSON
- 配置在进程启动时加载
- POC 不修改 `~/.codex/config.toml` 或 `~/.codex/auth.json`
- POC 已使用流式响应体转发，但还未验证 Codex 全部请求类型
- 真实 Codex CLI 的 Session ID 覆盖率仍需单独抓取验证
