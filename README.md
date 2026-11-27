<div align="center">

<img src="assets/daozai-logo.png" width="112" alt="岛仔" />

# 岛仔·macOS

**菜单栏上的桌面信息岛。**

macOS 原生移植版，集中显示音乐、歌词、天气、日程、邮件、AI 额度、系统性能、蓝牙电量和剪贴板。

[下载最新版](https://github.com/wylcpu/daozi-macos/releases/latest) · [macOS 说明](docs/MACOS.md) · [使用指南](docs/USAGE.md) · [更新记录](CHANGELOG.md)

</div>

## 功能

- 透明置顶岛体，支持顶部、底部、左侧和右侧停靠，以及多显示器定位。
- 通过 macOS 系统媒体中心读取正在播放的内容、封面和歌词，并控制播放、上一首、下一首、音量和进度。
- 天气、日历、邮件验证码、待办、专注、饮水和 AI 额度。
- 系统性能页显示总 CPU、逐核负载、GPU、内存、磁盘和网络吞吐。
- 蓝牙设备名称与电量、电源状态、音量和剪贴板历史。
- 插件集市和本地插件安装。

## 安装

1. 从 [Releases](https://github.com/wylcpu/daozi-macos/releases/latest) 下载 `Daozai-macOS-arm64.zip`。
2. 解压后将 `岛仔.app` 移到“应用程序”目录。
3. 首次启动时按 macOS 提示允许所需权限。

要求 macOS 13 或更新版本，当前成品面向 Apple Silicon。

应用使用临时签名，未经过 Apple Developer ID 公证。对外分发前应完成正式签名和公证。

## 从源码构建

需要 macOS、Xcode Command Line Tools、Rust stable 和 CMake。

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

## 数据目录

```text
~/Library/Application Support/Daozai
```

设置环境变量 `DAOZAI_DATA_DIR` 可以覆盖数据目录。

## 更新

“设置 → 关于 → 检查更新”会读取本仓库的最新 GitHub Release。发现新版本后可在浏览器下载 ZIP，解压并替换现有应用。

## 开源许可

项目采用 [MIT License](LICENSE)。Rust 依赖、vendored 代码、第三方图标及素材沿用各自的许可或权利声明，详见 [第三方声明](THIRD_PARTY_NOTICES.txt)。原项目为 [Renly1994/xiaotang-daozai](https://github.com/Renly1994/xiaotang-daozai)。
