<div align="center">

[English](README.md) · [Español](README.es.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [Italiano](README.it.md) · [Português (Brasil)](README.pt_BR.md) · [Русский](README.ru.md) · [日本語](README.ja.md) · [한국어](README.ko.md) · **简体中文** · [繁體中文](README.zh_TW.md)

<img src="data/icons/scalable/apps/io.github.oxakromax.SwellPlayer.svg" width="112" alt="">

# Swell

**快速的原生 TIDAL 客户端，支持 bit-perfect 的 Hi-Res 播放。**

[![CI](https://github.com/oxakromax/swell-player/actions/workflows/ci.yml/badge.svg)](https://github.com/oxakromax/swell-player/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/oxakromax/swell-player)](https://github.com/oxakromax/swell-player/releases/latest)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](COPYING)

![Swell 显示一张专辑，右侧停靠着正在播放面板](docs/screenshots/album.png)

</div>

Swell 使用 Rust 编写，基于 [gtk4-rs](https://gtk-rs.org)、
[libadwaita](https://gnome.pages.gitlab.gnome.org/libadwaita/) 和
[GStreamer](https://gstreamer.freedesktop.org/)。你的资料库从本地缓存中即时打开，
Hi-Res FLAC 原封不动地送达音频服务器，界面可从手机大小的窗口自适应到宽屏桌面，
而占用的内存只有其他客户端的一小部分（见[性能测量](#性能测量)）。

> [!NOTE]
> Swell 与 TIDAL 没有任何关联，也未获得 TIDAL 的认可或支持。它使用的是
> TIDAL 的非官方 API，这些 API 可能随时更改，并且需要付费的 TIDAL 订阅。

## 目录

- [功能](#功能)
- [截图](#截图)
- [安装](#安装)
- [登录与音质](#登录与音质)
- [Bit-perfect 播放](#bit-perfect-播放)
- [语言](#语言)
- [隐私](#隐私)
- [性能测量](#性能测量)
- [开发](#开发)
- [许可证](#许可证)

## 功能

**浏览**
- 主页：你的个性化 TIDAL 信息流（快捷入口、Mix、推荐、最近播放），
  滚动时继续加载更多。探索：编辑精选页面、流派、情绪和年代。
- 边输入边搜索，支持筛选（曲目、专辑、艺人、播放列表）、最佳匹配、最近搜索
  和分页；粘贴 TIDAL 链接即可打开。
- 专辑、艺人、播放列表、Mix 和电台页面，包含制作人员信息、乐评、简介、
  相似艺人以及“查看全部”页面。
- `tidal://` 和 `https://tidal.com/…` 链接会在 Swell 中打开。

**你的收藏**
- 数以千计的喜欢曲目，采用虚拟化列表，筛选和排序即时响应；
  喜欢的专辑、艺人、播放列表和 Mix；最近播放。
- 所有内容（曲目、专辑、艺人、播放列表、Mix）都带有心形按钮，
  整个资料库的状态只需一次请求即可获取。
- 创建、重命名、添加描述、公开和删除播放列表；通过菜单添加曲目，
  或将曲目拖到侧边栏的播放列表上；移除和重新排序，均可撤销。

**播放**
- Hi-Res Lossless（FLAC，最高 24 位/192 kHz），[bit-perfect](#bit-perfect-播放)。
- 无缝播放，可选的 ReplayGain（按曲目或按专辑），可选择输出设备。
- 真正的队列：下一首播放、添加到队列、跳转、移除、重新排序、清空（可撤销）；
  随机播放、全部循环/单曲循环、播放历史。
- 网络中断或流媒体 URL 过期后可从原位置继续播放。
- 同步歌词，跟随歌曲滚动，点击即可跳转。
- 启动时恢复你的队列和播放位置；睡眠定时器；桌面通知；播放时保持系统唤醒。
- Linux 上支持 MPRIS：媒体键、桌面小部件和 `playerctl` 均可使用。

**界面**
- libadwaita 设计，跟随系统的浅色/深色样式和强调色，
  提供三种布局：宽屏（停靠正在播放面板）、常规，以及带底部面板和迷你播放器的窄屏。
- 完整的键盘操作（Space、Ctrl+←/→、Ctrl+F、Ctrl+L、F6…；Ctrl+? 列出所有快捷键），
  每个控件都有无障碍名称。
- 提供 English、Español、Français、Deutsch、Italiano、Português (Brasil)、
  Русский、日本語、한국어、简体中文 和 繁體中文 界面。

## 截图

| 主页 | 搜索 |
|---|---|
| ![包含快捷入口、Mix 和最近播放的主页信息流](docs/screenshots/home.png) | ![“yoasobi”的搜索结果，包含最佳匹配、曲目和艺人](docs/screenshots/search.png) |
| **同步歌词** | **浅色样式** |
| ![显示播放队列的正在播放面板](docs/screenshots/now-playing.png) | ![浅色样式下的专辑页面](docs/screenshots/light.png) |

<p align="center">
  <img src="docs/screenshots/narrow.png" width="300" alt="带有迷你播放器和底部导航栏的窄屏布局">
</p>

## 安装

每个版本的安装包都在
[Releases 页面](https://github.com/oxakromax/swell-player/releases)。
Swell 需要 GTK ≥ 4.18、libadwaita ≥ 1.7，以及包含 base 和 good 插件的
GStreamer；安装包会自动引入这些依赖。

### Arch Linux (AUR)

AUR 软件包即将上线。在此之前，可以用本仓库中的同一个 PKGBUILD 自行构建：

```bash
git clone https://github.com/oxakromax/swell-player.git
cd swell-player/packaging/aur/swell-player   # or swell-player-git
makepkg -si
```

上线 AUR 后：`yay -S swell-player`（或 `swell-player-git`）。

### Flatpak

从 Releases 页面下载 `.flatpak` 安装包并安装（它使用 Flathub 上的
GNOME 运行时）。计划登陆 Flathub。

```bash
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user ./swell-player-*.flatpak
flatpak run io.github.oxakromax.SwellPlayer
```

### Debian 和 Ubuntu

需要 Debian 13 (trixie)、Ubuntu 25.04 或更新版本（GTK ≥ 4.18 和
libadwaita ≥ 1.7）。从 Releases 页面下载 `.deb`：

```bash
sudo apt install ./swell-player_*.deb
```

### Fedora

从 Releases 页面下载 `.rpm`：

```bash
sudo dnf install ./swell-player-*.rpm
```

### Windows

从 Releases 页面下载安装程序（或便携版 `.zip`）。
这些构建尚未进行代码签名，因此 SmartScreen 可能会提示无法识别的应用：
请选择**更多信息 → 仍要运行**。

### macOS

从 Releases 页面下载 `.dmg`，并将 Swell 拖到“应用程序”中。该应用尚未经过公证，
因此首次打开时，请右键点按（或按住 Control 点按）它并选择**打开**；
在 macOS 15 及更高版本中，请先打开一次，然后在**系统设置 → 隐私与安全性 →
仍要打开**中允许它。由于每个版本都使用临时（ad hoc）签名，更新后 macOS 可能会
再次询问是否允许访问 Swell 的钥匙串项目：请选择**始终允许**。
`tidal://` 链接在 macOS 上暂时无法打开 Swell，而 Intel (x86_64) 磁盘映像
只是尽力提供，因此某个版本可能没有它。

Windows 和 macOS 版本比 Linux 版本更新：下文的 MPRIS 和
bit-perfect 说明仅适用于 Linux。

### 从源码构建

你需要 Rust ≥ 1.96（如果发行版自带的版本较旧，请使用 [rustup](https://rustup.rs)）、
GTK ≥ 4.20 和 libadwaita ≥ 1.8（或使用 GTK 4.18 和 libadwaita 1.7 并加上
`--no-default-features`）、GStreamer ≥ 1.24、SQLite，以及 GLib 的
`glib-compile-resources`。

<details>
<summary>各发行版的构建依赖</summary>

```bash
# Arch Linux
sudo pacman -S --needed rust pkgconf gtk4 libadwaita sqlite \
    gstreamer gst-plugins-base gst-plugins-good

# Debian 13 / Ubuntu: their rustc is too old, so install Rust with rustup,
# and build with `make CARGOFLAGS=--no-default-features` where GTK is
# older than 4.20 (Debian 13 ships 4.18)
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
    libsqlite3-dev libglib2.0-dev-bin libgstreamer1.0-dev \
    libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-good

# Fedora
sudo dnf install cargo pkgconf-pkg-config gtk4-devel libadwaita-devel sqlite-devel \
    glib2-devel gstreamer1-devel gstreamer1-plugins-base-devel gstreamer1-plugins-good
```

`gst-plugins-good` 提供 Hi-Res 流所用的 DASH 解复用器（`dashdemux2`）。

</details>

```bash
make                  # cargo build --release --locked
sudo make install     # binary, desktop entry, icon, metainfo under /usr/local
```

`make install PREFIX=/usr DESTDIR=…` 会为打包准备一个安装目录，
`sudo make uninstall` 则将其卸载。由于图标和翻译已内嵌，二进制文件也可以直接从
`target/release/swell-player` 运行。

### 更新

Swell 会在启动后不久检查 GitHub 上是否有新版本，之后每天检查一次；
在**首选项 → 常规 → 更新**中可以关闭检查，或立即检查。
新版本如何处理，取决于 Swell 的安装方式：

| 安装方式 | 新版本 |
|---|---|
| Windows 安装程序，仅为当前用户安装（默认） | 下载后用该版本的 `SHA256SUMS` 校验，并在退出 Swell 时安装，也可以点按**重启**立即安装 |
| macOS `.dmg` | 如果 Swell 有权写入应用所在的文件夹，则同上；否则 Swell 会下载新的磁盘映像并提示打开 |
| Linux 安装包、Flatpak、tarball、Windows 便携版 `.zip`、面向所有用户的 Windows 安装程序 | 一条带有版本链接的通知：请用原来的安装方式更新 |

自行分发更新的发行版可以在构建 Swell 时去掉这些检查
（参见 [packaging/README.md](packaging/README.md#update-checks)）；
从源码树中运行的构建不会检查更新。

## 登录与音质

TIDAL 不允许第三方应用注册自己的 OAuth 客户端，因此 Swell 和所有开源 TIDAL
客户端一样，以 TIDAL 自家应用之一的身份登录。你选择的登录流程决定了使用哪一个，
也就决定了音质上限：

| 登录方式 | 工作原理 | 最高音质 |
|---|---|---|
| **浏览器**（推荐） | Swell 在浏览器中打开 TIDAL 的登录页面。登录后，TIDAL 会跳转到一个写着“Oops”的页面；把该页面的地址复制回 Swell。 | **Hi-Res Lossless**：FLAC 最高 24 位/192 kHz |
| **设备代码** | Swell 显示一个简短的代码；在任意设备上通过 `link.tidal.com` 批准它。 | **Lossless**：FLAC 16 位/44.1 kHz |

在**首选项 → 播放**中选择流媒体音质。播放器栏中的格式标记会显示实际传输的内容。

## Bit-perfect 播放

关闭标准化且音量为 100 % 时，Swell 会将流的格式一直保持到 PipeWire：
一首 24 位/96 kHz 的曲目会以 24 位、96 kHz 抵达 PipeWire。点击播放器栏中的格式标记会打开一个弹出窗口，
告诉你输出是否为 bit-perfect，如果不是，会说明原因。

之后由 PipeWire 决定声卡得到什么。默认情况下，它以单一采样率（通常是 48 kHz）运行，
并对其他一切进行重采样。若要让它跟随每首曲目的采样率，请允许常见的采样率，
例如在 `~/.config/pipewire/pipewire.conf.d/10-rates.conf` 中：

```
context.properties = {
    default.clock.allowed-rates = [ 44100 48000 88200 96000 176400 192000 ]
}
```

然后重启 PipeWire（`systemctl --user restart pipewire`）。`pw-top` 会显示抵达设备的内容。

## 语言

Swell 提供 **English**、**Español**、**Français**、**Deutsch**、**Italiano**、
**Português (Brasil)**、**Русский**、**日本語**、**한국어**、**简体中文** 和 **繁體中文**
界面。它会自动跟随系统语言（`LANGUAGE`、`LC_ALL`、`LC_MESSAGES`、`LANG`，与 gettext 相同），
你也可以在**首选项**中覆盖它。TIDAL 自身的内容（编辑精选文字、页面标题）会以同样的语言请求，
因此同一个界面不会混用两种语言。

想让 Swell 支持你的语言？请参阅 [docs/translating.md](docs/translating.md)。

## 隐私

- **令牌**保存在系统密钥环中（在 Linux 上是 Secret Service：
  GNOME Keyring、KWallet、KeePassXC…，在 Flatpak 中则是 secret portal），
  绝不会以明文形式写入磁盘。它们会在过期前不久刷新，网络问题也不会让你退出登录。
- **缓存在你的电脑上的内容：**TIDAL 的应答和你的资料库
  （`~/.cache/swell-player/library.sqlite3`）、封面图片、播放过或即将播放的曲目开头的几秒（`~/.cache/swell-player/audio/`，仅音频，最多 512 MB）、你的设置、
  已保存的队列和最近搜索。可以在首选项中清除缓存；退出登录会删除令牌、缓存、
  已保存的队列和搜索历史。
- **没有遥测。**Swell 只会连接 TIDAL 的服务器，用于显示或播放你要求的内容；
  另外每天会连接一次 GitHub 检查新版本，只发送 Swell 的版本号（参见[更新](#更新)）。
  它没有统计分析、崩溃报告或其他第三方服务。

## 性能测量

在同一台机器上，将 Swell 与 [High Tide](https://github.com/Nokse22/high-tide) 1.6
（Python + PyGObject）进行对比，两者都运行在无头的 KWin 会话
（`kwin_wayland --virtual`，1280×860）中，并使用已保存的会话。取三次运行的中位数；
内存读取自 `/proc`（RssAnon 是应用自身的堆；RSS 还包括共享库，
以及这里任何 GTK 4 应用都有的约 250 MB GPU 驱动映射）。空闲数据取自
无任何播放时 10 秒内的平均值。

| | Swell | High Tide |
|---|---|---|
| 主页显示时：RssAnon / RSS | **61 MB / 360 MB** | 131 MB / 504 MB |
| 从缓存显示 1,295 首喜欢的曲目 | 启动后 **0.42 s**（与 TIDAL 同步后为 0.50 s） | — ¹ |
| … 不使用 Swell 的缓存 | 前 100 首 **0.59 s**，全部 1.2–1.9 s | — ¹ |
| 喜欢视图：RssAnon / RSS | 85 MB / 371 MB | — ¹ |
| 空闲 CPU / 主线程唤醒 | ~0 % / 每秒 1.1 次 | ~0 % / 每秒 0.1 次 |
| 线程数 | 15 | 19 |
| 选中 Hi-Res 曲目 → 出声 | **0.67 s**（24 位/96 kHz，以 S24_32、96 kHz 抵达 PipeWire） | — |

¹ High Tide 不缓存喜欢的内容，也无法直接打开它的喜欢列表；
借助测试工具，Swell 的概念验证测得 High Tide 显示同一份列表需要 13.3 s，
而 Swell 只需 0.3 s。

机器：Arch Linux、KDE Plasma 6 (Wayland)、GTK 4.24、libadwaita 1.10、
GStreamer 1.28、Rust 1.99，release 构建。

## 开发

```bash
cargo run --release
cargo test --release
cargo clippy --release --all-targets
```

[CONTRIBUTING.md](CONTRIBUTING.md) 介绍了构建、测试、调试用的环境变量
（打开某个页面、播放曲目，或在没有显示器的情况下将窗口渲染为 PNG）
以及如何提交更改。
[docs/architecture.md](docs/architecture.md) 说明代码的组织方式；
[docs/features.md](docs/features.md) 是功能范围，
[docs/design.md](docs/design.md) 是界面规范（西班牙语）。
发行说明见 [CHANGELOG.md](CHANGELOG.md)。

发现了安全问题？请私下报告：参见 [SECURITY.md](SECURITY.md)。

## 许可证

Swell 是自由软件：你可以根据自由软件基金会发布的 GNU 通用公共许可证第 3 版，
或（由你选择的）任何更高版本的条款，重新分发和/或修改它。参见 [COPYING](COPYING)。

TIDAL 是其所有者的商标，在此仅用于说明 Swell 可与什么配合使用。
