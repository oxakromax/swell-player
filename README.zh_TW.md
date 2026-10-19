<div align="center">

[English](README.md) · [Español](README.es.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [Italiano](README.it.md) · [Português (Brasil)](README.pt_BR.md) · [Русский](README.ru.md) · [日本語](README.ja.md) · [한국어](README.ko.md) · [简体中文](README.zh_CN.md) · **繁體中文**

<img src="data/icons/scalable/apps/io.github.oxakromax.SwellPlayer.svg" width="112" alt="">

# Swell

**快速的原生 TIDAL 用戶端，支援 bit-perfect 的 Hi-Res 播放。**

[![CI](https://github.com/oxakromax/swell-player/actions/workflows/ci.yml/badge.svg)](https://github.com/oxakromax/swell-player/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/oxakromax/swell-player)](https://github.com/oxakromax/swell-player/releases/latest)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](COPYING)

![Swell 顯示一張專輯，右側停駐著正在播放面板](docs/screenshots/album.png)

</div>

Swell 以 Rust 撰寫，採用 [gtk4-rs](https://gtk-rs.org)、
[libadwaita](https://gnome.pages.gitlab.gnome.org/libadwaita/) 與
[GStreamer](https://gstreamer.freedesktop.org/)。你的音樂庫從本機快取立即開啟，
Hi-Res FLAC 原封不動地送達音訊伺服器，介面可從手機大小的視窗自動調整到寬螢幕桌面，
而佔用的記憶體只有其他用戶端的一小部分（見[效能量測](#效能量測)）。

> [!NOTE]
> Swell 與 TIDAL 沒有任何關係，也未獲得 TIDAL 的認可或支援。它使用的是
> TIDAL 的非官方 API，這些 API 可能隨時變更，且需要付費的 TIDAL 訂閱。

## 目錄

- [功能](#功能)
- [螢幕截圖](#螢幕截圖)
- [安裝](#安裝)
- [登入與音質](#登入與音質)
- [Bit-perfect 播放](#bit-perfect-播放)
- [語言](#語言)
- [隱私權](#隱私權)
- [效能量測](#效能量測)
- [開發](#開發)
- [授權條款](#授權條款)

## 功能

**瀏覽**
- 首頁：你的個人化 TIDAL 動態（捷徑、Mix、推薦、最近播放），
  捲動時繼續載入更多。探索：編輯精選頁面、曲風、心情與年代。
- 邊輸入邊搜尋，支援篩選（曲目、專輯、藝人、播放清單）、最佳結果、最近搜尋
  與分頁；貼上 TIDAL 連結即可開啟。
- 專輯、藝人、播放清單、Mix 與電台頁面，包含製作人員資訊、樂評、簡介、
  相似藝人，以及「檢視全部」頁面。
- `tidal://` 與 `https://tidal.com/…` 連結會在 Swell 中開啟。

**你的收藏**
- 數以千計的最愛曲目，採用虛擬化清單，篩選與排序即時回應；
  最愛的專輯、藝人、播放清單與 Mix；最近播放。
- 所有內容（曲目、專輯、藝人、播放清單、Mix）都有愛心按鈕，
  整個音樂庫的狀態只需一次請求即可取得。
- 建立、重新命名、加入描述、公開與刪除播放清單；透過選單加入曲目，
  或將曲目拖曳到側邊欄的播放清單上；移除與重新排序，均可復原。

**播放**
- Hi-Res Lossless（FLAC，最高 24 位元/192 kHz），[bit-perfect](#bit-perfect-播放)。
- 無縫播放，可選的 ReplayGain（依曲目或依專輯），可選擇輸出裝置。
- 真正的佇列：下一首播放、加入佇列、跳轉、移除、重新排序、清空（可復原）；
  隨機播放、全部重複／單曲重複、播放記錄。
- 網路中斷或串流 URL 過期後，可從原位置繼續播放。
- 同步歌詞，跟隨歌曲捲動，點一下即可跳轉。
- 啟動時還原你的佇列與播放位置；睡眠計時器；桌面通知；播放時保持系統喚醒。
- Linux 上支援 MPRIS：媒體鍵、桌面小工具與 `playerctl` 都能使用。

**介面**
- libadwaita 設計，跟隨系統的淺色／深色樣式與強調色，
  提供三種版面：寬螢幕（停駐正在播放面板）、一般，以及帶有底部面板與迷你播放器的窄螢幕。
- 完整的鍵盤操作（Space、Ctrl+←/→、Ctrl+F、Ctrl+L、F6…；Ctrl+? 列出所有快速鍵），
  每個控制項都有協助工具名稱。
- 提供 English、Español、Français、Deutsch、Italiano、Português (Brasil)、
  Русский、日本語、한국어、简体中文 與 繁體中文 介面。

## 螢幕截圖

| 首頁 | 搜尋 |
|---|---|
| ![包含捷徑、Mix 與最近播放的首頁動態](docs/screenshots/home.png) | ![「yoasobi」的搜尋結果，包含最佳結果、曲目與藝人](docs/screenshots/search.png) |
| **同步歌詞** | **淺色樣式** |
| ![顯示播放佇列的正在播放面板](docs/screenshots/now-playing.png) | ![淺色樣式下的專輯頁面](docs/screenshots/light.png) |

<p align="center">
  <img src="docs/screenshots/narrow.png" width="300" alt="帶有迷你播放器與底部導覽列的窄螢幕版面">
</p>

## 安裝

每個版本的安裝套件都在
[Releases 頁面](https://github.com/oxakromax/swell-player/releases)。
Swell 需要 GTK ≥ 4.18、libadwaita ≥ 1.7，以及包含 base 與 good 外掛程式的
GStreamer；安裝套件會自動帶入這些相依套件。

### Arch Linux (AUR)

AUR 套件即將上架。在此之前，可以用本儲存庫中的同一個 PKGBUILD 自行建置：

```bash
git clone https://github.com/oxakromax/swell-player.git
cd swell-player/packaging/aur/swell-player   # or swell-player-git
makepkg -si
```

上架 AUR 後：`yay -S swell-player`（或 `swell-player-git`）。

### Flatpak

從 Releases 頁面下載 `.flatpak` 套件並安裝（它使用 Flathub 上的
GNOME 執行環境）。計畫上架 Flathub。

```bash
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user ./swell-player-*.flatpak
flatpak run io.github.oxakromax.SwellPlayer
```

### Debian 與 Ubuntu

需要 Debian 13 (trixie)、Ubuntu 25.04 或更新版本（GTK ≥ 4.18 與
libadwaita ≥ 1.7）。從 Releases 頁面下載 `.deb`：

```bash
sudo apt install ./swell-player_*.deb
```

### Fedora

從 Releases 頁面下載 `.rpm`：

```bash
sudo dnf install ./swell-player-*.rpm
```

### Windows

從 Releases 頁面下載安裝程式（或可攜版 `.zip`）。
這些建置尚未進行程式碼簽署，因此 SmartScreen 可能會提示無法辨識的應用程式：
請選擇**其他資訊 → 仍要執行**。

### macOS

從 Releases 頁面下載 `.dmg`，並將 Swell 拖曳到「應用程式」中。此應用程式尚未經過公證，
因此第一次開啟時，請按右鍵（或按住 Control 點按）它並選擇**打開**；
在 macOS 15 及更新版本中，請先開啟一次，然後在**系統設定 → 隱私權與安全性 →
仍要打開**中允許它。由於每個版本都使用臨時（ad hoc）簽署，更新後 macOS 可能會
再次詢問是否允許存取 Swell 的鑰匙圈項目：請選擇**永遠允許**。
`tidal://` 連結在 macOS 上暫時無法開啟 Swell，而 Intel (x86_64) 磁碟映像檔
只是盡力提供，因此某個版本可能沒有它。

Windows 與 macOS 版本比 Linux 版本更新：下文的 MPRIS 與
bit-perfect 說明僅適用於 Linux。

### 從原始碼建置

你需要 Rust ≥ 1.96（如果發行版內建的版本較舊，請使用 [rustup](https://rustup.rs)）、
GTK ≥ 4.20 與 libadwaita ≥ 1.8（或使用 GTK 4.18 與 libadwaita 1.7 並加上
`--no-default-features`）、GStreamer ≥ 1.24、SQLite，以及 GLib 的
`glib-compile-resources`。

<details>
<summary>各發行版的建置相依套件</summary>

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

`gst-plugins-good` 提供 Hi-Res 串流所用的 DASH 解多工器（`dashdemux2`）。

</details>

```bash
make                  # cargo build --release --locked
sudo make install     # binary, desktop entry, icon, metainfo under /usr/local
```

`make install PREFIX=/usr DESTDIR=…` 會為封裝準備一個安裝目錄，
`sudo make uninstall` 則將其移除。由於圖示與翻譯已內嵌，執行檔也可以直接從
`target/release/swell-player` 執行。

### 更新

Swell 會在啟動後不久檢查 GitHub 上是否有新版本，之後每天檢查一次；
在**偏好設定 → 一般 → 更新**中可以關閉檢查，或立即檢查。
新版本如何處理，取決於 Swell 的安裝方式：

| 安裝方式 | 新版本 |
|---|---|
| Windows 安裝程式，僅為目前使用者安裝（預設） | 下載後以該版本的 `SHA256SUMS` 驗證，並在結束 Swell 時安裝，也可以按下**重新啟動**立即安裝 |
| macOS `.dmg` | 如果 Swell 有權寫入應用程式所在的資料夾，則同上；否則 Swell 會下載新的磁碟映像檔並提示開啟 |
| Linux 套件、Flatpak、tarball、Windows 可攜版 `.zip`、適用所有使用者的 Windows 安裝程式 | 一則附有版本連結的通知：請用原本的安裝方式更新 |

自行發布更新的發行版可以在建置 Swell 時去掉這些檢查
（參見 [packaging/README.md](packaging/README.md#update-checks)）；
從原始碼樹中執行的建置不會檢查更新。

## 登入與音質

TIDAL 不允許第三方應用程式註冊自己的 OAuth 用戶端，因此 Swell 和所有開放原始碼的 TIDAL
用戶端一樣，以 TIDAL 自家應用程式之一的身分登入。你選擇的登入流程決定了使用哪一個，
也就決定了音質上限：

| 登入方式 | 運作方式 | 最高音質 |
|---|---|---|
| **瀏覽器**（建議） | Swell 在瀏覽器中開啟 TIDAL 的登入頁面。登入後，TIDAL 會轉到一個寫著「Oops」的頁面；把該頁面的網址複製回 Swell。 | **Hi-Res Lossless**：FLAC 最高 24 位元/192 kHz |
| **裝置代碼** | Swell 顯示一組簡短的代碼；在任何裝置上透過 `link.tidal.com` 核准它。 | **Lossless**：FLAC 16 位元/44.1 kHz |

在**偏好設定 → 播放**中選擇串流音質。播放器列中的格式標記會顯示實際傳送的內容。

## Bit-perfect 播放

關閉正規化且音量為 100 % 時，Swell 會將串流的格式一路保持到 PipeWire：
一首 24 位元/96 kHz 的曲目會以 24 位元、96 kHz 抵達 PipeWire。點選播放器列中的格式標記會開啟一個彈出視窗，
告訴你輸出是否為 bit-perfect，如果不是，會說明原因。

之後由 PipeWire 決定音效卡得到什麼。預設情況下，它以單一取樣率（通常是 48 kHz）運作，
並對其他一切進行重新取樣。若要讓它跟隨每首曲目的取樣率，請允許常見的取樣率，
例如在 `~/.config/pipewire/pipewire.conf.d/10-rates.conf` 中：

```
context.properties = {
    default.clock.allowed-rates = [ 44100 48000 88200 96000 176400 192000 ]
}
```

然後重新啟動 PipeWire（`systemctl --user restart pipewire`）。`pw-top` 會顯示抵達裝置的內容。

## 語言

Swell 提供 **English**、**Español**、**Français**、**Deutsch**、**Italiano**、
**Português (Brasil)**、**Русский**、**日本語**、**한국어**、**简体中文** 與 **繁體中文**
介面。它會自動跟隨系統語言（`LANGUAGE`、`LC_ALL`、`LC_MESSAGES`、`LANG`，與 gettext 相同），
你也可以在**偏好設定**中覆寫它。TIDAL 自身的內容（編輯精選文字、頁面標題）會以同樣的語言請求，
因此同一個畫面不會混用兩種語言。

想讓 Swell 支援你的語言？請參閱 [docs/translating.md](docs/translating.md)。

## 隱私權

- **權杖**儲存在系統金鑰圈中（在 Linux 上是 Secret Service：
  GNOME Keyring、KWallet、KeePassXC…，在 Flatpak 中則是 secret portal），
  絕不會以明文形式寫入磁碟。它們會在過期前不久更新，網路問題也不會讓你被登出。
- **快取在你電腦上的內容：**TIDAL 的回應與你的音樂庫
  （`~/.cache/swell-player/library.sqlite3`）、封面圖片、播放過或即將播放的曲目開頭的幾秒（`~/.cache/swell-player/audio/`，僅音訊，最多 512 MB）、你的設定、
  已儲存的佇列與最近搜尋。可以在偏好設定中清除快取；登出會刪除權杖、快取、
  已儲存的佇列與搜尋記錄。
- **沒有遙測。**Swell 只會連線到 TIDAL 的伺服器，用於顯示或播放你要求的內容；
  另外每天會連線一次 GitHub 檢查新版本，只傳送 Swell 的版本號（參見[更新](#更新)）。
  它沒有分析統計、當機回報或其他第三方服務。

## 效能量測

在同一台機器上，將 Swell 與 [High Tide](https://github.com/Nokse22/high-tide) 1.6
（Python + PyGObject）進行比較，兩者都在無頭的 KWin 工作階段
（`kwin_wayland --virtual`，1280×860）中執行，並使用已儲存的工作階段。取三次執行的中位數；
記憶體讀取自 `/proc`（RssAnon 是應用程式自身的堆積；RSS 還包括共用函式庫，
以及這裡任何 GTK 4 應用程式都有的約 250 MB GPU 驅動程式對映）。閒置數據取自
沒有任何播放時 10 秒內的平均值。

| | Swell | High Tide |
|---|---|---|
| 首頁顯示時：RssAnon / RSS | **61 MB / 360 MB** | 131 MB / 504 MB |
| 從快取顯示 1,295 首最愛曲目 | 啟動後 **0.42 s**（與 TIDAL 同步後為 0.50 s） | — ¹ |
| … 不使用 Swell 的快取 | 前 100 首 **0.59 s**，全部 1.2–1.9 s | — ¹ |
| 最愛檢視：RssAnon / RSS | 85 MB / 371 MB | — ¹ |
| 閒置 CPU / 主執行緒喚醒 | ~0 % / 每秒 1.1 次 | ~0 % / 每秒 0.1 次 |
| 執行緒數 | 15 | 19 |
| 選取 Hi-Res 曲目 → 出聲 | **0.67 s**（24 位元/96 kHz，以 S24_32、96 kHz 抵達 PipeWire） | — |

¹ High Tide 不快取最愛內容，也無法直接開啟它的最愛清單；
藉助測試工具，Swell 的概念驗證測得 High Tide 顯示同一份清單需要 13.3 s，
而 Swell 只需 0.3 s。

機器：Arch Linux、KDE Plasma 6 (Wayland)、GTK 4.24、libadwaita 1.10、
GStreamer 1.28、Rust 1.99，release 建置。

## 開發

```bash
cargo run --release
cargo test --release
cargo clippy --release --all-targets
```

[CONTRIBUTING.md](CONTRIBUTING.md) 介紹了建置、測試、除錯用的環境變數
（開啟某個頁面、播放曲目，或在沒有顯示器的情況下將視窗轉譯為 PNG）
以及如何提交變更。
[docs/architecture.md](docs/architecture.md) 說明程式碼的組織方式；
[docs/features.md](docs/features.md) 是功能範圍，
[docs/design.md](docs/design.md) 是介面規格（西班牙文）。
發行說明見 [CHANGELOG.md](CHANGELOG.md)。

發現了安全性問題？請私下回報：參見 [SECURITY.md](SECURITY.md)。

## 授權條款

Swell 是自由軟體：你可以依據自由軟體基金會發布的 GNU 通用公共授權條款第 3 版，
或（由你選擇的）任何更新版本的條款，重新散布及／或修改它。參見 [COPYING](COPYING)。

TIDAL 是其所有者的商標，在此僅用於說明 Swell 可與什麼搭配使用。
