# Codex Switch POC 与最终方案

版本：v0.1
日期：2026-09-08
状态：方案确认，POC v0.1 已实现，待真实 Codex 请求验证

## 1. 结论

最终产品采用：

```text
Tauri 可视化控制面
    +
本地 Rust Proxy
    +
SQLite 配置与绑定数据
    +
Codex Session 元数据扫描器
```

产品本质上是一个 **Codex Session Router**。

它在网络层是本地反向代理，但核心能力不是单纯转发，而是：

- 管理 Profile
- 扫描和展示 Codex Session 元数据
- 把不同 Session 绑定到不同 Profile
- 支持多个 Session 并发使用不同 Profile
- 支持全局默认 Profile
- 不负责展示 Codex 对话内容
- 不负责替代 Codex 执行任务

## 2. 为什么不能继续使用全局切换

当前 `codex-switch` 的模式是修改：

```text
~/.codex/config.toml
~/.codex/auth.json
```

这种方式适合“整个 Codex 客户端切换 Profile”，不适合以下场景：

```text
Session A -> aihezu
Session B -> her
Session C -> sakura
```

因为多个 Session 共享同一份全局配置。运行期间修改配置会产生：

- 已运行 Session 互相影响
- 请求发往错误的上游
- OAuth/API Key 账号边界混淆
- 并发切换时出现竞态
- 用户无法判断某一轮请求实际使用了哪个 Profile

因此，最终方案不能继续依赖“请求前修改全局配置文件”作为主要路由机制。

## 3. cc-switch 调研结论

参考项目：

```text
cc-switch
```

### 3.1 可以直接借鉴的部分

| 模块 | 借鉴内容 |
|---|---|
| Tauri 2 | macOS 应用壳、窗口、DMG 打包 |
| React + TypeScript | Profile 和 Session 管理页面 |
| Axum/Hyper Proxy | 本地 HTTP 代理和流式响应转发 |
| `proxy/session.rs` | 从 Header 和请求体提取 Session ID |
| Codex Session Scanner | 扫描 JSONL、提取标题和元数据 |
| `state_5.sqlite` 查询 | 获取 Codex 标题和线程信息 |
| SQLite DAO | 配置、Profile、绑定关系持久化 |
| 配置原子写入 | 接管 Codex 配置时的带日期备份、取消接管、权限处理 |
| Terminal Launcher | 可选的外部终端打开能力 |

参考文件：

- `src-tauri/src/proxy/server.rs`
- `src-tauri/src/proxy/session.rs`
- `src-tauri/src/proxy/provider_router.rs`
- `src-tauri/src/session_manager/providers/codex.rs`
- `src-tauri/src/session_manager/mod.rs`
- `src-tauri/src/codex_config.rs`
- `src-tauri/tauri.conf.json`

### 3.2 不能直接复用的部分

cc-switch 当前的 Provider 路由仍然是：

```text
app_type -> current_provider
```

其 Provider 选择接口主要接收 `app_type`，没有接收 `session_id`。

它虽然提取了 Session ID，但当前主要用于：

- 日志
- 用量统计
- 缓存
- 历史关联

并没有实现：

```text
session_id -> profile
```

所以不能直接使用它的全局 Provider 切换逻辑来解决本项目需求。

## 4. POC 目标

POC 只验证一个架构闸门：

> 两个或多个 Codex Session 同时请求本地 Proxy 时，Proxy 能否稳定识别 Session，并把请求分别转发到不同 Profile。

POC 不做以下内容：

- 不做 Tauri UI
- 不做 DMG
- 不做完整 SQLite 数据层
- 不做完整 Codex 配置接管
- 不做对话内容查看
- 不做消息恢复
- 不做自动故障转移
- 不做 MCP、Skills、云同步
- 不做生产级密钥存储

POC 的目标是尽快确认“按 Session 路由”是否成立，而不是提前构建完整产品。

## 5. POC 功能范围

### 5.1 本地代理

监听地址默认：

```text
127.0.0.1:<port>
```

代理需要支持：

- `POST /responses`
- `POST /v1/responses`
- `POST /responses/compact`
- `GET /models`
- 其他请求路径的通用转发

POC 不需要理解完整 Codex 协议，只需要：

1. 读取请求 Header
2. 读取 JSON 请求体
3. 提取 Session ID
4. 选择 Profile
5. 转发请求
6. 记录路由结果

### 5.2 Session ID 提取顺序

优先级：

```text
1. session_id Header
2. x-session-id Header
3. body.metadata.session_id
4. body.session_id
5. 没有 Session ID
```

没有提取到 Session ID 时：

```text
使用全局默认 Profile
```

POC 不生成随机 Session ID 用于路由。随机 ID 只能用于日志关联，不能作为持久绑定依据。

### 5.3 Profile 配置

POC 先使用 JSON 配置，避免引入数据库和 UI。

示例：

```json
{
  "listen": "127.0.0.1:8787",
  "default_profile": "sakura",
  "profiles": {
    "aihezu": {
      "base_url": "http://127.0.0.1:9101",
      "headers": {
        "x-poc-upstream": "aihezu"
      }
    },
    "her": {
      "base_url": "http://127.0.0.1:9102",
      "headers": {
        "x-poc-upstream": "her"
      }
    },
    "sakura": {
      "base_url": "http://127.0.0.1:9103",
      "headers": {
        "x-poc-upstream": "sakura"
      }
    }
  },
  "bindings": {
    "session-a": {
      "mode": "fixed",
      "profile": "aihezu"
    },
    "session-b": {
      "mode": "fixed",
      "profile": "her"
    },
    "session-c": {
      "mode": "global"
    }
  }
}
```

### 5.4 路由规则

```text
session binding.mode == fixed
    -> 使用绑定的 Profile

session binding.mode == global
    -> 使用当前全局默认 Profile

session 没有绑定记录
    -> 使用当前全局默认 Profile

请求没有 Session ID
    -> 使用当前全局默认 Profile
```

POC 在进程启动时加载配置，避免每个请求重复读文件。

测试期间修改配置后需要重启 POC。最终版本改为 SQLite，并通过应用界面热更新内存中的路由配置。

### 5.5 路由日志

只记录元数据，不记录请求体、响应体和密钥：

```json
{
  "timestamp": "2026-09-08T12:00:00Z",
  "method": "POST",
  "path": "/responses",
  "session_id": "session-a",
  "session_id_source": "header",
  "profile_id": "aihezu",
  "upstream": "http://127.0.0.1:9101",
  "status": 200
}
```

日志用于验证：

- Session ID 是否稳定
- 不同 Session 是否被正确分流
- 无 Session 请求是否走默认 Profile
- Profile 变更后是否按预期生效

## 6. POC 验收标准

### 6.1 基本路由

- [ ] `session-a` 固定到 `aihezu`
- [ ] `session-b` 固定到 `her`
- [ ] 未绑定的 `session-c` 使用全局默认 `sakura`
- [ ] 没有 Session ID 的 `/models` 使用全局默认 Profile
- [ ] 不存在的 Profile 配置会返回明确错误
- [ ] 未知绑定关系不会静默路由到错误 Profile

### 6.2 并发路由

至少并发发送：

```text
10 个 session-a 请求
10 个 session-b 请求
10 个 session-c 请求
```

验收要求：

- [ ] `session-a` 的请求全部到 `aihezu`
- [ ] `session-b` 的请求全部到 `her`
- [ ] `session-c` 的请求全部到默认 Profile
- [ ] 不出现请求串 Profile
- [ ] 不依赖全局切换锁

### 6.3 Session ID 覆盖率

必须用真实 Codex CLI 请求验证以下场景：

- [ ] 新建 Session
- [ ] 同一 Session 连续多轮请求
- [ ] `codex resume`
- [ ] compact
- [ ] 图片请求
- [ ] `/models`
- [ ] 两个 Session 并发运行

POC 只有在真实 Codex 请求中确认 Session ID 足够稳定后，才能进入产品实现阶段。

## 7. 关键风险与处理策略

### 7.1 Session ID 不一定出现在每个请求中

这是最大风险。

cc-switch 当前支持从以下位置提取：

- `session_id`
- `x-session-id`
- `metadata.session_id`

但仍需通过 Codex CLI 实际请求确认：

- 是否每轮都有
- 是否跨 resume 保持
- compact、图片和模型请求是否携带
- 并发时是否不会复用错误 ID

如果 Session ID 覆盖不足，备用方案按优先级排列：

1. 为每个 Session 分配独立本地端口
2. 通过启动 wrapper 注入路由标识
3. 为每个 Codex 进程使用独立 `CODEX_HOME`

在未完成真实请求验证前，不承诺纯共享端口一定能够实现完整路由。

### 7.2 Profile 不等于完整 `config.toml`

最终 Profile 应该定义为“上游连接 Profile”，而不是简单保存一份完整 Codex 配置文件。

建议拆分：

```text
客户端配置
- Codex 行为配置
- 模型选择
- 本地工具设置

上游连接配置
- base_url
- API Key 或 OAuth 凭据
- 上游请求 Header
- Provider 类型
- 认证方式
```

Proxy 只负责上游连接配置。

不能假设任意 `config.toml` 字段都能通过 Proxy 动态生效。

### 7.3 认证信息不能直接无差别转发

不同 Profile 可能使用：

- API Key
- Bearer Token
- ChatGPT OAuth
- 其他 Provider 的自定义 Header

最终版本必须做到：

- 不在 UI 中明文展示密钥
- 不把密钥写入路由日志
- 不把一个 Profile 的 Authorization 复用给另一个 Profile
- 文件权限保持 `0600`
- 后续优先迁移到 macOS Keychain

### 7.4 `/models` 等请求可能没有 Session ID

没有 Session ID 的请求默认使用全局 Profile。

如果未来发现某些请求虽然没有 Session ID，但必须使用具体 Session 的 Profile，需要增加：

- Session 注册
- 进程级路由标识
- 独立本地端口

POC 阶段先记录这些请求，不做猜测绑定。

### 7.5 “固定 Profile”不是绕过 Proxy 后仍然有效

固定绑定只对经过 Codex Switch Proxy 的请求生效。

如果用户绕过 Proxy，直接执行原始 Codex CLI，则无法保证使用固定 Profile。

最终产品需要选择一种接入方式：

1. 一次性把 Codex 的 API 地址接管到本地 Proxy
2. 提供 `codex-switch` wrapper
3. 由应用管理 Codex 的启动入口

推荐第一种作为基础，wrapper 作为兜底。

## 8. 最终数据模型

### 8.1 profiles

```text
profiles
- id                TEXT PRIMARY KEY
- name              TEXT NOT NULL
- provider_type     TEXT NOT NULL
- base_url          TEXT NOT NULL
- auth_type         TEXT NOT NULL
- credential_ref    TEXT
- extra_headers     TEXT
- is_default        INTEGER NOT NULL DEFAULT 0
- created_at        INTEGER NOT NULL
- updated_at        INTEGER NOT NULL
```

`credential_ref` 不直接保存明文密钥，后续可指向 Keychain。

### 8.2 session_bindings

```text
session_bindings
- session_id        TEXT PRIMARY KEY
- mode              TEXT NOT NULL
- profile_id        TEXT
- created_at        INTEGER NOT NULL
- updated_at        INTEGER NOT NULL
```

约束：

```text
mode = global -> profile_id 可以为空
mode = fixed  -> profile_id 必须存在
```

### 8.3 session 元数据

Session 本身不需要复制到数据库作为事实源。

扫描器实时读取 Codex 文件系统和状态库，展示：

```text
- session_id
- title
- created_at
- last_active_at
- project_dir
- size_bytes
- source_path
```

数据库只持久化用户主动设置的绑定关系。

这样可以避免：

- 复制巨量 JSONL
- Session 元数据过期
- 数据库和 Codex 文件系统双向同步

## 9. 最终界面范围

### 9.1 Profile 页面

支持：

- 列出 Profile
- 新增 Profile
- 编辑 Profile
- 删除 Profile
- 设置默认 Profile
- 测试连接
- 显示认证类型
- 显示当前是否可用

Profile 建议称为：

```text
配置档
```

内部可以使用 `profile`，避免和 Codex 原生配置概念混淆。

### 9.2 Session 页面

列表字段：

- Session ID
- 标题
- 创建时间
- 最后使用时间
- 工作目录
- Session 大小
- 当前 Profile
- 绑定模式

默认排序：

```text
最后使用时间倒序
```

支持排序：

- 创建时间
- 最后使用时间

每行操作：

- 跟随全局
- 固定到指定 Profile
- 取消绑定

不在应用内展示 Session 消息内容。

### 9.3 Proxy 状态

显示：

- Proxy 是否运行
- 监听地址
- 当前全局 Profile
- 最近请求时间
- 最近错误
- 当前路由数量

不显示：

- API Key
- OAuth Token
- 请求体
- 响应内容

## 10. 实现顺序

### 阶段 0：POC

目标：确认 Session ID 和并发路由可行。

内容：

1. 标准库本地 HTTP Proxy
2. JSON Profile 配置
3. JSON Session 绑定
4. Session ID 提取
5. 上游转发
6. 路由日志
7. 并发测试
8. 真实 Codex 抓包验证

完成条件：通过第 6 节全部验收项。

### 阶段 1：核心服务

目标：把 POC 逻辑变成稳定的 Rust 服务。

内容：

1. Tauri 2 工程
2. Rust Proxy
3. SQLite
4. Profile DAO
5. Session Binding DAO
6. 路由服务
7. 配置接管与备份
8. 真实 Codex 流式响应
9. 认证类型处理

此阶段不做完整 UI，只保留最小控制命令或调试页面。

### 阶段 2：可视化页面

目标：完成可用的 macOS 应用。

内容：

1. Profile 管理页面
2. Session 列表页面
3. Session 绑定操作
4. 默认 Profile 设置
5. Proxy 状态
6. 外部终端打开能力
7. 配置异常提示

### 阶段 3：打包和稳定性

目标：交付 DMG。

内容：

1. macOS 应用签名
2. DMG 打包
3. 自动启动 Proxy
4. 退出时取消接管配置
5. 外部修改检测
6. 权限和 Keychain
7. 升级迁移
8. 多 Session 长时间运行测试

## 11. POC 成功后的技术决策

### 如果 Session ID 稳定

采用共享本地端口的 Session Router：

```text
session_id -> binding -> profile -> upstream
```

这是首选方案，结构简单，用户体验最好。

### 如果 Session ID 不稳定

不继续堆叠请求体启发式判断，改用进程级隔离：

```text
Codex process A -> local port A -> Profile A
Codex process B -> local port B -> Profile B
```

这会增加启动、端口、配置和清理逻辑，但比不可靠地猜 Session 更安全。

### Codex 配置接管闭环

桌面版采用一键接管：

1. 接管前把当前 `~/.codex/config.toml` 复制为 `config.toml.codex-switch-backup-YYYYMMDD-HHMMSS`。
2. 当前配置已经指向本地 Proxy 时，直接视为已接管，不要求用户先恢复。
3. 取消接管时使用最新一份 `config.toml.codex-switch-backup*`；旧版固定备份名 `config.toml.codex-switch-backup` 继续兼容。
4. 如果当前配置已经不是本地 Proxy，取消接管只清理接管标记，不覆盖用户当前配置。
5. 菜单栏操作成功或失败都要拉起主窗口并显示 2 秒提示。

## 12. POC 之后明确不做的事情

以下能力暂不进入第一版：

- 代理层自动修改 Session 内容
- 应用内对话查看
- 自动总结 Session
- Provider 自动选择
- 自动重试到另一个 Profile
- 复杂故障转移
- 云端同步
- 多 AI 客户端统一管理
- MCP 和 Skills 管理
- 统计分析大屏

这些能力会扩大数据、权限和路由风险，不影响当前核心目标。

## 13. 当前执行项

下一步按以下顺序执行：

1. 使用真实 Codex CLI 请求验证 Session ID
2. 根据验证结果锁定共享端口或独立端口方案
3. 再开始 Tauri 核心服务实现

当前桌面版已完成：

- Rust standalone Cargo 工程
- Axum 本地 HTTP Proxy
- Profile 和 Session Binding JSON 配置
- Header、`metadata.session_id`、顶层 `session_id` 提取
- Codex `x-codex-turn-metadata.threadId` 提取
- 固定 Profile、全局默认 Profile 路由
- 流式响应体转发
- 本地假上游并发测试
- `/healthz` 真实入口冒烟
- Tauri 2 macOS App
- Profile 新建、编辑、删除、设为全局默认
- Session 列表分页、搜索、按最后使用/创建时间/大小降序排序
- `~/.codex` 配置一键接管、带日期备份、取消接管
- 菜单栏 Proxy / Codex 操作和窗口 toast 提示

尚未完成：

- Codex 各请求类型的 Session ID 覆盖率统计
- SQLite 持久化
- DMG
- Keychain 存储
- 长时间多 Session 稳定性测试
