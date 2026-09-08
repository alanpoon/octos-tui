spec: task
name: "/pair 二维码配对第二台设备"
inherits: project
tags: [tui, capability, pairing, websocket, qr]
estimate: 1.5d
---

## 意图

octoscode 已经能通过 WebSocket 连上 `octos serve`，但没有任何把"连哪儿、带
什么令牌"交给第二台设备的路径：想在手机或另一台机器上观察/接管同一会话，
只能手抄 endpoint。本任务加一个能力门控的 `/pair`：向服务器要一次性配对
令牌，把连接 URL 渲染成终端二维码，扫码即可接入同一会话。

## 已定决策

- 新增能力常量 `APPUI_FEATURE_SESSION_PAIR_V1 = "session.pair.v1"`，配对令牌
  由服务器 `session/pair` 返回；客户端绝不自造 token、不猜 endpoint。
- 二维码编码用 `qrcode` crate（纯 Rust、无 unsafe、无传递性重依赖）。自研 QR
  编码器不在预算内，这是本任务唯一允许新增的依赖。
- 渲染用半块字符：两行模块压成一行 `▀` / `▄` / 空格 / 实心块，保证 80 列终端
  容得下 version ≤ 7 的码。
- `/pair` 仅在 WebSocket 连接模式可用；stdio 与 mock 模式渲染 Unsupported。
- 配对 URL 形如 `ws://<host>:<port>/?session=<session_id>&token=<token>`，
  逐字节等于二维码里编码的内容。
- 终端宽度不足以放下二维码时退化为纯文本 URL 加一行提示，不裁剪二维码。
- 令牌只在内存中活到覆盖层关闭；不写配置文件、不进命令历史、不进日志。

## 边界

### Allowed Changes
- src/menu/registry.rs
- src/model.rs
- src/store.rs
- src/app/render.rs
- locales/en.yml
- locales/zh.yml
- Cargo.toml
- specs/**

### Forbidden
- 不在客户端生成或推导配对令牌。
- 不把令牌写入 config、history 或日志。
- 除 `qrcode` 外不新增依赖。
- 不在 stdio / mock 模式下伪造可用状态。

## 排除范围

- 移动端 / 浏览器客户端本身。
- 服务器侧 `session/pair` 的实现与令牌轮换策略。
- 会话接管时的并发写冲突仲裁。

## 完成条件

场景: WebSocket 模式下 /pair 渲染 URL 与二维码
  测试: pair_renders_endpoint_url_and_qr_in_websocket_mode
  假设 mock 服务器广告 `session.pair.v1` 且 `session/pair` 返回 token "t0k3n"
  当 用户在 WebSocket 连接下运行 `/pair`
  那么 覆盖层渲染的 URL 含 "token=t0k3n"
  并且 覆盖层渲染出非空的二维码块字符区

场景: 二维码编码内容与渲染的 URL 逐字节一致
  测试: qr_payload_matches_rendered_url
  假设 `session/pair` 返回 session_id "s1" 与 token "t0k3n"
  当 覆盖层用 `qrcode` crate 编码配对载荷
  那么 编码进二维码的字符串等于渲染出的 URL 字符串

场景: stdio 模式下 /pair 渲染 Unsupported
  测试: pair_renders_unsupported_in_stdio_mode
  假设 客户端以 stdio 方式连接后端
  当 用户运行 `/pair`
  那么 状态区渲染 Unsupported
  并且 不发出 `session/pair` 请求

场景: 能力未广告时 /pair 不出现在命令 popup
  测试: pair_command_hidden_without_pair_capability
  假设 mock 服务器不广告 `APPUI_FEATURE_SESSION_PAIR_V1` 对应的 `session.pair.v1`
  当 用户打开 `/` 命令 popup
  那么 popup 条目中不含 `pair`

场景: 终端宽度不足时退化为纯文本 URL
  测试: narrow_terminal_falls_back_to_plain_url
  假设 终端宽度为 40 列且二维码需要 45 列
  当 覆盖层渲染配对结果
  那么 渲染纯文本 URL 与一行宽度不足提示
  并且 不渲染二维码块字符区

场景: 80 列终端下 version 7 的码用半块字符渲染得下
  测试: qr_half_block_rendering_fits_eighty_columns
  假设 配对 URL 编码出一个 version 7 的二维码
  当 覆盖层以半块字符渲染该二维码
  那么 渲染行宽不超过 80 列
  并且 每行只由 `▀`、`▄`、空格与实心块组成

场景: 配对令牌不写入 config、命令历史或日志
  测试: pair_token_never_persisted_to_disk
  层级: 单元
  替身: 临时 config 目录 + 内存 history
  假设 一次配对已成功并持有 token "t0k3n"
  当 覆盖层关闭且客户端刷写 config 与命令历史
  那么 写出的配置文件内容不含 "t0k3n"
  并且 写出的命令历史内容不含 "t0k3n"
  并且 日志内容不含 "t0k3n"

场景: 配对请求失败时渲染错误且不残留旧令牌
  测试: pair_request_failure_renders_error_without_stale_token
  层级: 单元
  替身: mock 后端返回 session/pair 错误
  假设 上一次配对已成功并持有 token "old"
  当 再次运行 `/pair` 且 `session/pair` 返回错误
  那么 覆盖层渲染错误文案
  并且 渲染内容不含 "old"
