# 岛仔 macOS 版

这是岛仔的 macOS 移植分支。界面、页面状态、待办、健康提醒、邮件、日历、天气、音乐、AI 额度和插件执行逻辑沿用上游项目，平台外壳换成 macOS 原生实现。

项目与发布页：[wylcpu/daozi-macos](https://github.com/wylcpu/daozi-macos)。

## 构建

需要 macOS、Xcode Command Line Tools、Rust stable 与 CMake。

```bash
cmake -S macos/mediaremote-adapter -B macos/mediaremote-adapter/build -DCMAKE_BUILD_TYPE=Release
cmake --build macos/mediaremote-adapter/build --config Release -j 6

cargo build --release --bin daozai --features sdk-js,sdk-wasm
./scripts/package-macos.sh release
```

输出：

```text
dist/岛仔.app
dist/Daozai-macOS-arm64.zip
```

当前包使用临时（ad-hoc）签名，适合本机运行。对外分发前仍应使用 Apple Developer ID 签名和公证。

## macOS 平台实现

- 状态栏菜单：打开设置、打开日历、吉祥物、开机启动、数据目录和退出。
- 透明置顶岛体、运行中拖动定位、四边停靠、多显示器识别和岛体大小缩放。
- 透明画布按实际命中区切换鼠标事件，岛外区域允许点击穿透。
- macOS“正在播放”通过 MediaRemote 兼容层读取，覆盖网易云、QQ 音乐、Spotify、Apple Music 等接入系统媒体中心的播放器。
- 主音量、静音、播放、暂停、上一首、下一首和进度跳转。
- 电池、续航和电源状态通过系统状态获取。
- 蓝牙名称与电量从 `system_profiler` 读取。
- 系统性能从 Mach 主机 CPU 计数器、IOAccelerator、`ps`、`vm_stat`、`netstat`、`df` 采样；总 CPU、逐核负载、GPU、内存、网络和磁盘吞吐都在后台线程更新。
- 剪贴板文字历史使用 `pbpaste` 监听、`pbcopy` 写回。
- 邮件与 CalDAV 凭据进入 macOS 钥匙串。
- Gemini、豆包、WorkBuddy 等登录会话使用随机 AES-256-GCM 密钥加密，密钥保存在钥匙串。
- 本地通知由 `osascript` 发出，提示音使用应用自带 WAV。

macOS 暂不提供读取其他应用本地通知和 2.4G USB 接收器状态的能力，因此不会把这些功能伪装成可用。macOS 上仍可接收邮件验证码、日程、额度、天气和岛仔自身提醒。

## 数据目录

```text
~/Library/Application Support/Daozai
```

设置 `DAOZAI_DATA_DIR` 可以覆盖数据目录。

## MediaRemote 兼容层

`macos/mediaremote-adapter` 来自 `ungive/mediaremote-adapter`，使用 BSD 3-Clause License。它只负责读取 macOS 系统媒体中心和发送标准媒体命令；框架随应用一起打包，不链接进主程序。

## 验证

```bash
cargo test --lib --features sdk-js,sdk-wasm -- --test-threads=1
```

实机已验证岛体展开、设置、音乐与封面、歌词、天气、日历、剪贴板、菜单栏和透明窗口定位。
