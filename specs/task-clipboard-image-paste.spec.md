spec: task
name: "Ctrl+V 剪贴板位图粘贴为轮次媒体"
inherits: project
tags: [tui, composer, media, clipboard, vision]
estimate: 1.5d
---

## 意图

现有图片附件只有 Route-1 一条路：prompt 文本里出现图片**路径**（拖放、粘贴路径、
`@` picker）时挂为 `Message.media`。终端的 bracketed paste 递不出图片**字节**，
所以截图后按 Ctrl+V 目前什么也不发生。本任务补上第二条路：Ctrl+V 时直接读系统
剪贴板的位图，落盘为临时 PNG，再汇入既有 media 通道，让"截图→粘贴→提问"在
终端里成立。

## 已定决策

- 剪贴板位图通过外部命令读取，不新增 crate 依赖：macOS 用 `osascript`
  导出 PNG，Wayland 用 `wl-paste --type image/png`，X11 用 `xclip -selection
  clipboard -t image/png -o`。探测顺序按平台固定，首个可用者胜出。
- 读取实现藏在 `ClipboardImageSource` trait 后面，`src/clipboard.rs` 提供真实
  实现；落盘命名、体积校验、media 组装是纯函数，用假实现单测。
- 落盘目录 `~/.octos/tmp/paste/`，文件名 `paste-<内容 sha256 前 16 位>.png`；
  同一位图重复粘贴命中同名文件时复用，不重复写盘。
- 复用 `store.rs` 既有上限，不新增常量：单轮最多 `MAX_TURN_IMAGES` = 4 张图，
  单文件不超过 `MAX_IMAGE_BYTES` = 20 MiB。
- 剪贴板里没有位图时，Ctrl+V 落回既有文本粘贴路径，按键不被吞掉。
- 外部命令缺失、非零退出或输出为空时降级为文本粘贴，并在状态栏给一行提示；
  任何路径都不 panic、不阻塞事件循环超过 2 秒（读取带超时）。

## 边界

### Allowed Changes
- src/clipboard.rs
- src/model.rs
- src/store.rs
- src/keymap.rs
- locales/en.yml
- locales/zh.yml
- specs/**

### Forbidden
- 不新增 crate 依赖。
- 不改变 Route-1（prompt 内图片路径）的既有挂载语义。
- 不改动 `MAX_TURN_IMAGES` 与 `MAX_IMAGE_BYTES` 的取值。
- 不把剪贴板字节写进命令历史或日志。

## 排除范围

- 剪贴板里的非图片二进制（PDF、视频）。
- 服务器侧的图片编码与 provider 适配。
- Windows 剪贴板位图读取。

## 完成条件

场景: 剪贴板含 PNG 位图时 Ctrl+V 落盘并挂为轮次媒体
  测试: ctrl_v_stages_clipboard_bitmap_as_turn_media
  假设 假剪贴板源返回一张 4 KiB 的 PNG 位图
  当 用户在 composer 按下 Ctrl+V
  那么 位图写入 `~/.octos/tmp/paste/` 下的一个 `.png` 文件
  并且 该文件以 `image/png` 进入本轮 `Message.media`
  并且 composer 文本不变

场景: 剪贴板无位图时 Ctrl+V 落回文本粘贴且不吞按键
  测试: ctrl_v_without_clipboard_image_falls_back_to_text_paste
  层级: 单元
  替身: fake `ClipboardImageSource`
  假设 `ClipboardImageSource` 假实现报告剪贴板里没有位图
  当 用户按下 Ctrl+V
  那么 本轮 media 为空
  并且 该按键未被吞掉，落回既有文本粘贴路径

场景: 超过体积上限的剪贴板位图被拒绝
  测试: oversized_clipboard_image_is_rejected_with_status
  假设 假剪贴板源返回一张 21 MiB 的 PNG 位图
  当 用户按下 Ctrl+V
  那么 本轮 media 为空
  并且 状态栏显示体积超限提示
  并且 `~/.octos/tmp/paste/` 下不新增文件

场景: 已挂满 4 张图后继续 Ctrl+V 不再新增
  测试: clipboard_image_paste_respects_turn_image_cap
  假设 本轮已挂载 4 张图片
  当 用户再次按下 Ctrl+V 且假剪贴板源返回一张新位图
  那么 本轮 media 仍为 4 项
  并且 状态栏显示已达单轮图片上限

场景: 外部读取命令缺失或非零退出时降级为文本粘贴且不 panic
  测试: missing_clipboard_helper_degrades_to_text_paste
  层级: 单元
  替身: fake `ClipboardImageSource`
  假设 `ClipboardImageSource` 的外部命令以"命令不存在"错误失败
  当 用户按下 Ctrl+V
  那么 本轮 media 为空
  并且 状态栏显示剪贴板图片读取不可用
  并且 文本粘贴路径收到该按键

场景: 外部读取命令输出为空时降级为文本粘贴并给状态栏提示
  测试: empty_clipboard_helper_output_degrades_to_text_paste
  层级: 单元
  替身: fake `ClipboardImageSource`
  假设 `ClipboardImageSource` 的外部命令以退出码 0 返回 0 字节输出
  当 用户按下 Ctrl+V
  那么 本轮 media 为空
  并且 状态栏显示剪贴板图片读取不可用
  并且 文本粘贴路径收到该按键

场景: 相同位图重复粘贴复用同一临时文件
  测试: identical_clipboard_bitmap_reuses_staged_temp_file
  层级: 单元
  替身: fake `ClipboardImageSource` + 临时目录
  假设 假剪贴板源两次返回字节完全相同的位图
  当 用户连续按下两次 Ctrl+V
  那么 `~/.octos/tmp/paste/` 下只存在一个 `.png` 文件
  并且 本轮 media 的两项指向同一路径
