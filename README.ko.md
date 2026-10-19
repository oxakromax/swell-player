<div align="center">

[English](README.md) · [Español](README.es.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [Italiano](README.it.md) · [Português (Brasil)](README.pt_BR.md) · [Русский](README.ru.md) · [日本語](README.ja.md) · **한국어** · [简体中文](README.zh_CN.md) · [繁體中文](README.zh_TW.md)

<img src="data/icons/scalable/apps/io.github.oxakromax.SwellPlayer.svg" width="112" alt="">

# Swell

**비트퍼펙트 Hi-Res 재생을 지원하는 빠른 네이티브 TIDAL 클라이언트.**

[![CI](https://github.com/oxakromax/swell-player/actions/workflows/ci.yml/badge.svg)](https://github.com/oxakromax/swell-player/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/oxakromax/swell-player)](https://github.com/oxakromax/swell-player/releases/latest)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](COPYING)

![앨범을 표시한 Swell. 오른쪽에 지금 재생 중 패널이 고정되어 있음](docs/screenshots/album.png)

</div>

Swell은 Rust로 작성되었으며 [gtk4-rs](https://gtk-rs.org),
[libadwaita](https://gnome.pages.gitlab.gnome.org/libadwaita/),
[GStreamer](https://gstreamer.freedesktop.org/)를 사용합니다.
라이브러리는 로컬 캐시에서 즉시 열리고, Hi-Res FLAC은 손대지 않은 상태로 사운드 서버에 전달되며,
인터페이스는 휴대전화 크기의 창부터 넓은 데스크톱까지 알맞게 바뀌고,
다른 클라이언트의 일부에 불과한 메모리만 사용합니다
([측정 결과](#측정-결과)).

> [!NOTE]
> Swell은 TIDAL과 제휴 관계가 없으며, TIDAL의 승인이나 지원을 받지 않았습니다.
> TIDAL의 비공식 API를 사용하므로 예고 없이 바뀔 수 있으며,
> TIDAL 유료 구독이 필요합니다.

## 목차

- [기능](#기능)
- [스크린샷](#스크린샷)
- [설치](#설치)
- [로그인과 음질](#로그인과-음질)
- [비트퍼펙트 재생](#비트퍼펙트-재생)
- [언어](#언어)
- [개인 정보 보호](#개인-정보-보호)
- [측정 결과](#측정-결과)
- [개발](#개발)
- [라이선스](#라이선스)

## 기능

**둘러보기**
- 홈: 맞춤형 TIDAL 피드(바로 가기, 믹스, 추천, 최근 재생)이며
  스크롤하면 더 불러옵니다. 둘러보기: 에디토리얼 페이지, 장르, 분위기, 연대.
- 입력하는 즉시 검색되며 필터(곡, 앨범, 아티스트, 재생 목록), 최상위 결과,
  최근 검색, 페이지 넘기기를 지원합니다. TIDAL 링크를 붙여 넣으면 해당 페이지가 열립니다.
- 앨범, 아티스트, 재생 목록, 믹스, 라디오 페이지에서 크레디트, 리뷰, 약력,
  비슷한 아티스트, “모두 보기” 페이지를 볼 수 있습니다.
- `tidal://` 및 `https://tidal.com/…` 링크가 Swell에서 열립니다.

**컬렉션**
- 수천 곡의 즐겨찾는 곡을 가상화하여 표시하고, 필터와 정렬이 즉시 적용됩니다.
  즐겨찾는 앨범, 아티스트, 재생 목록, 믹스와 최근 재생도 지원합니다.
- 모든 항목(곡, 앨범, 아티스트, 재생 목록, 믹스)에 하트를 누를 수 있으며,
  라이브러리 전체를 요청 한 번으로 가져옵니다.
- 재생 목록을 만들고, 이름을 바꾸고, 설명을 달고, 공개하고, 삭제할 수 있습니다. 메뉴에서 곡을 추가하거나
  사이드바의 재생 목록으로 끌어다 놓을 수 있고, 제거와 순서 변경도 실행 취소와 함께 지원합니다.

**재생**
- Hi-Res Lossless(최대 24비트/192 kHz FLAC), [비트퍼펙트](#비트퍼펙트-재생).
- 끊김 없는 재생, 선택 사항인 ReplayGain(트랙 또는 앨범), 출력 장치 선택.
- 제대로 된 대기열: 다음에 재생, 대기열에 추가, 건너뛰기, 제거, 순서 변경,
  실행 취소가 가능한 비우기. 셔플, 전체/한 곡 반복, 기록.
- 네트워크가 끊기거나 스트림 URL이 만료되어도 같은 위치에서 이어서 재생합니다.
- 노래에 맞춰 따라가며 클릭하면 해당 위치로 이동하는 동기화 가사.
- 시작할 때 대기열과 재생 위치를 복원합니다. 수면 타이머, 데스크톱 알림,
  재생하는 동안 시스템 절전을 막습니다.
- Linux에서는 MPRIS를 지원하여 미디어 키, 데스크톱 위젯, `playerctl`이 작동합니다.

**인터페이스**
- 시스템의 밝은/어두운 스타일과 강조 색상을 따르는 libadwaita 디자인이며,
  지금 재생 중 패널을 고정한 넓은 레이아웃, 일반 레이아웃,
  하단 시트와 미니 플레이어를 갖춘 좁은 레이아웃의 세 가지 레이아웃을 제공합니다.
- 완전한 키보드 조작(Space, Ctrl+←/→, Ctrl+F, Ctrl+L, F6…, Ctrl+?로 모든 단축키 목록 표시)과
  모든 컨트롤의 접근성 이름.
- 영어와 스페인어.

## 스크린샷

| 홈 | 검색 |
|---|---|
| ![바로 가기, 믹스, 최근 재생이 표시된 홈 피드](docs/screenshots/home.png) | ![“yoasobi” 검색 결과. 최상위 결과, 곡, 아티스트가 표시됨](docs/screenshots/search.png) |
| **동기화 가사** | **밝은 스타일** |
| ![재생 대기열을 보여 주는 지금 재생 중 패널](docs/screenshots/now-playing.png) | ![밝은 스타일의 앨범 페이지](docs/screenshots/light.png) |

<p align="center">
  <img src="docs/screenshots/narrow.png" width="300" alt="미니 플레이어와 하단 탐색 표시줄이 있는 좁은 레이아웃">
</p>

## 설치

각 릴리스의 패키지는
[Releases 페이지](https://github.com/oxakromax/swell-player/releases)에 있습니다.
Swell에는 GTK ≥ 4.18, libadwaita ≥ 1.7, 그리고 base 및 good 플러그인이 포함된 GStreamer가 필요하며,
패키지를 설치하면 함께 설치됩니다.

### Arch Linux (AUR)

AUR 패키지는 준비 중입니다. 그때까지는 이 저장소의 같은 PKGBUILD로 직접 빌드하세요.

```bash
git clone https://github.com/oxakromax/swell-player.git
cd swell-player/packaging/aur/swell-player   # or swell-player-git
makepkg -si
```

AUR에 올라가면: `yay -S swell-player`(또는 `swell-player-git`).

### Flatpak

Releases 페이지에서 `.flatpak` 번들을 다운로드하여 설치하세요
(Flathub의 GNOME 런타임을 사용합니다). Flathub 등록도 계획 중입니다.

```bash
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user ./swell-player-*.flatpak
flatpak run io.github.oxakromax.SwellPlayer
```

### Debian 및 Ubuntu

Debian 13(trixie), Ubuntu 25.04 이상이 필요합니다(GTK ≥ 4.18 및
libadwaita ≥ 1.7). Releases 페이지에서 `.deb`를 다운로드하세요.

```bash
sudo apt install ./swell-player_*.deb
```

### Fedora

Releases 페이지에서 `.rpm`을 다운로드하세요.

```bash
sudo dnf install ./swell-player-*.rpm
```

### Windows

Releases 페이지에서 설치 프로그램(또는 휴대용 `.zip`)을 다운로드하세요.
빌드에는 아직 코드 서명이 없어서 SmartScreen이 인식되지 않은 앱이라고 경고할 수 있습니다.
이 경우 **추가 정보 → 실행**을 선택하세요.

### macOS

Releases 페이지에서 `.dmg`를 다운로드하고 Swell을 응용 프로그램 폴더로 끌어다 놓으세요.
앱은 아직 공증되지 않았으므로 처음에는 마우스 오른쪽 버튼(또는 Control 키를 누른 채 클릭)으로
클릭하고 **열기**를 선택하세요. macOS 15 이상에서는 한 번 열어 본 다음
**시스템 설정 → 개인 정보 보호 및 보안 → 그래도 열기**에서 허용하세요.
각 릴리스는 애드혹 서명이므로 업데이트 후 macOS가 Swell의 키체인 항목 접근을
다시 물을 수 있습니다. 이 경우 **항상 허용**을 선택하세요.
macOS에서는 `tidal://` 링크가 아직 Swell에서 열리지 않으며, Intel(x86_64) 디스크 이미지는
최선의 노력으로 제공되므로 일부 릴리스에는 없을 수 있습니다.

Windows와 macOS 빌드는 Linux 빌드보다 새로우며, MPRIS와 아래의 비트퍼펙트 관련 내용은 Linux 전용입니다.

### 소스에서 빌드

Rust ≥ 1.96(배포판의 버전이 오래되었다면 [rustup](https://rustup.rs)),
GTK ≥ 4.20 및 libadwaita ≥ 1.8(또는 `--no-default-features`를 지정한 GTK 4.18 및 libadwaita 1.7),
GStreamer ≥ 1.24, SQLite, GLib의 `glib-compile-resources`가 필요합니다.

<details>
<summary>배포판별 빌드 의존성</summary>

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

`gst-plugins-good`은 Hi-Res 스트림에 사용되는 DASH 디먹서(`dashdemux2`)를 제공합니다.

</details>

```bash
make                  # cargo build --release --locked
sudo make install     # binary, desktop entry, icon, metainfo under /usr/local
```

`make install PREFIX=/usr DESTDIR=…`는 패키징을 위해 설치 결과를 지정한 디렉터리에 구성하며,
`sudo make uninstall`로 제거할 수 있습니다. 아이콘과 번역이 내장되어 있으므로
바이너리는 `target/release/swell-player`에서 바로 실행할 수도 있습니다.

### 업데이트

Swell은 시작한 직후와 그 이후 하루에 한 번 GitHub에서 새 릴리스를 확인합니다.
**환경 설정 → 일반 → 업데이트**에서 이 확인을 끄거나 바로 확인할 수 있습니다.
새 릴리스가 어떻게 처리되는지는 Swell을 설치한 방법에 따라 다릅니다.

| 설치 방법 | 새 릴리스 |
|---|---|
| Windows 설치 프로그램, 현재 사용자 전용(기본값) | 다운로드한 뒤 릴리스의 `SHA256SUMS`로 검증하고 Swell을 종료할 때 설치하며, **다시 시작**을 누르면 즉시 설치합니다 |
| macOS `.dmg` | Swell이 앱이 있는 폴더에 쓸 수 있으면 같습니다. 그렇지 않으면 새 디스크 이미지를 다운로드하고 열지 묻습니다 |
| Linux 패키지, Flatpak, tarball, Windows 휴대용 `.zip`, 모든 사용자용 Windows 설치 프로그램 | 릴리스 링크가 담긴 알림이 표시됩니다. 설치한 방식 그대로 업데이트하세요 |

업데이트를 자체적으로 제공하는 배포판은 확인 기능 없이 Swell을 빌드할 수 있으며
([packaging/README.md](packaging/README.md#update-checks) 참조),
소스 트리에서 실행한 빌드는 확인하지 않습니다.

## 로그인과 음질

TIDAL은 서드파티 앱이 자체 OAuth 클라이언트를 등록하는 것을 허용하지 않습니다.
그래서 Swell은 모든 오픈 소스 TIDAL 클라이언트처럼 TIDAL 자체 앱 중 하나로 로그인합니다.
선택한 로그인 방식에 따라 어느 앱이 될지 정해지며, 그에 따라 음질의 상한이 결정됩니다.

| 로그인 | 작동 방식 | 최고 음질 |
|---|---|---|
| **브라우저**(권장) | Swell이 브라우저에서 TIDAL 로그인 페이지를 엽니다. 로그인하면 TIDAL이 “Oops”라고 표시된 페이지로 이동하는데, 그 페이지의 주소를 복사하여 Swell에 붙여 넣습니다. | **Hi-Res Lossless**: 최대 24비트/192 kHz FLAC |
| **기기 코드** | Swell이 짧은 코드를 보여 줍니다. 아무 기기에서나 `link.tidal.com`을 열어 승인하세요. | **Lossless**: 16비트/44.1 kHz FLAC |

스트리밍 품질은 **환경 설정 → 재생**에서 선택합니다.
플레이어 표시줄의 형식 배지에는 실제로 전달되고 있는 형식이 표시됩니다.

## 비트퍼펙트 재생

정규화를 끄고 볼륨을 100 %로 설정하면 Swell은 스트림의 형식을 그대로 유지한 채 PipeWire까지 전달합니다.
24비트/96 kHz 곡은 24비트·96 kHz 그대로 도착합니다.
플레이어 표시줄의 형식 배지를 누르면 팝오버가 열려, 출력이 비트퍼펙트인지,
아니라면 그 이유가 무엇인지 알려 줍니다.

사운드 카드로 무엇이 전달될지는 그 뒤에서 PipeWire가 결정합니다.
기본적으로 PipeWire는 하나의 샘플 레이트(대개 48 kHz)로 동작하며 나머지는 모두 리샘플링합니다.
각 곡의 레이트를 따르게 하려면 흔히 쓰는 레이트를 허용하세요.
예를 들어 `~/.config/pipewire/pipewire.conf.d/10-rates.conf`에 다음과 같이 씁니다.

```
context.properties = {
    default.clock.allowed-rates = [ 44100 48000 88200 96000 176400 192000 ]
}
```

그런 다음 PipeWire를 다시 시작하세요(`systemctl --user restart pipewire`).
`pw-top`으로 장치에 무엇이 전달되는지 확인할 수 있습니다.

## 언어

Swell은 **영어**와 **스페인어**를 지원합니다. 시스템 언어를 자동으로 따르며
(gettext처럼 `LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG` 참조),
**환경 설정**에서 직접 바꿀 수도 있습니다. TIDAL 자체 콘텐츠(에디토리얼 글, 페이지 제목)도
같은 언어로 요청하므로 한 화면에 두 언어가 섞이지 않습니다.

Swell을 내 언어로 사용하고 싶으신가요? [docs/translating.md](docs/translating.md)를 참고하세요.

## 개인 정보 보호

- **토큰**은 시스템 키링(Linux에서는 Secret Service: GNOME 키링,
  KWallet, KeePassXC…, Flatpak에서는 시크릿 포털)에 저장되며
  디스크에 일반 텍스트로 기록되지 않습니다. 만료되기 직전에 갱신되고,
  네트워크 문제로 로그아웃되는 일은 없습니다.
- **컴퓨터에 캐시되는 것:** TIDAL의 응답과 라이브러리
  (`~/.cache/swell-player/library.sqlite3`), 아트워크, 재생했거나 곧 재생할 곡의 처음 몇 초(`~/.cache/swell-player/audio/`, 오디오만, 최대 512MB), 설정, 저장된 대기열, 최근 검색.
  환경 설정에서 캐시를 지울 수 있으며, 로그아웃하면 토큰, 캐시, 저장된 대기열,
  검색 기록이 삭제됩니다.
- **원격 측정 없음.** Swell은 사용자가 표시하거나 재생하라고 요청한 내용을 위해 TIDAL 서버에 연결하고,
  하루에 한 번 GitHub에 연결해 새 릴리스를 확인하는데, 이때 보내는 것은 Swell의 버전뿐입니다
  ([업데이트](#업데이트) 참조).
  분석, 충돌 보고, 그 밖의 서드파티 서비스는 없습니다.

## 측정 결과

같은 컴퓨터에서 Swell과 [High Tide](https://github.com/Nokse22/high-tide) 1.6
(Python + PyGObject)을 비교했습니다. 둘 다 헤드리스 KWin 세션
(`kwin_wayland --virtual`, 1280×860)에서 저장된 세션으로 실행했습니다.
세 번 실행한 중앙값이며, 메모리는 `/proc`에서 읽었습니다(RssAnon은 앱 자체의 힙이고,
RSS에는 공유 라이브러리와 이 환경의 모든 GTK 4 앱에 있는 약 250 MB의 GPU 드라이버 매핑도 포함됩니다).
유휴 수치는 아무것도 재생하지 않는 상태에서 10초간 평균한 값입니다.

| | Swell | High Tide |
|---|---|---|
| 홈 표시: RssAnon / RSS | **61 MB / 360 MB** | 131 MB / 504 MB |
| 즐겨찾는 곡 1,295곡을 캐시에서 화면에 표시 | 실행 후 **0.42초**(TIDAL과 동기화는 0.50초) | — ¹ |
| … Swell 캐시 없이 | 처음 100곡은 **0.59초**, 전체는 1.2~1.9초 | — ¹ |
| 즐겨찾기 보기: RssAnon / RSS | 85 MB / 371 MB | — ¹ |
| 유휴 CPU / 메인 스레드 깨어남 | 약 0 % / 초당 1.1회 | 약 0 % / 초당 0.1회 |
| 스레드 | 15 | 19 |
| Hi-Res 곡 선택 → 소리 출력 | **0.67초**(24비트/96 kHz, S24_32 96 kHz로 PipeWire에 도달) | — |

¹ High Tide는 즐겨찾기를 캐시하지 않으며 즐겨찾기 목록을 바로 여는 방법도 없습니다.
테스트 하네스를 사용한 Swell의 개념 증명에서 같은 목록을 표시하는 데
High Tide는 13.3초, Swell은 0.3초가 걸렸습니다.

컴퓨터: Arch Linux, KDE Plasma 6(Wayland), GTK 4.24, libadwaita 1.10,
GStreamer 1.28, Rust 1.99, 릴리스 빌드.

## 개발

```bash
cargo run --release
cargo test --release
cargo clippy --release --all-targets
```

[CONTRIBUTING.md](CONTRIBUTING.md)에서 빌드, 테스트, 디버깅용 환경 변수
(페이지 열기, 곡 재생, 디스플레이 없이 창을 PNG로 렌더링)와
변경 사항을 보내는 방법을 설명합니다.
[docs/architecture.md](docs/architecture.md)는 코드 구성을,
[docs/features.md](docs/features.md)는 기능 범위를,
[docs/design.md](docs/design.md)는 인터페이스 명세(스페인어)를 설명합니다.
릴리스 노트는 [CHANGELOG.md](CHANGELOG.md)에 있습니다.

보안 문제를 발견하셨나요? 비공개로 알려 주세요:
[SECURITY.md](SECURITY.md)를 참고하세요.

## 라이선스

Swell은 자유 소프트웨어입니다. 자유 소프트웨어 재단이 발표한
GNU 일반 공중 사용 허가서 버전 3 또는 (선택에 따라) 그 이후 버전의 조건에 따라
재배포하거나 수정할 수 있습니다. [COPYING](COPYING)을 참고하세요.

TIDAL은 해당 소유자의 상표이며, 여기서는 Swell이 무엇과 함께 작동하는지 밝히기 위해서만 사용합니다.
