<div align="center">

[English](README.md) · [Español](README.es.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [Italiano](README.it.md) · **Português (Brasil)** · [Русский](README.ru.md) · [日本語](README.ja.md) · [한국어](README.ko.md) · [简体中文](README.zh_CN.md) · [繁體中文](README.zh_TW.md)

<img src="data/icons/scalable/apps/io.github.oxakromax.SwellPlayer.svg" width="112" alt="">

# Swell

**Um cliente nativo e rápido do TIDAL, com reprodução Hi-Res bit-perfect.**

[![CI](https://github.com/oxakromax/swell-player/actions/workflows/ci.yml/badge.svg)](https://github.com/oxakromax/swell-player/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/oxakromax/swell-player)](https://github.com/oxakromax/swell-player/releases/latest)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](COPYING)

![Swell mostrando um álbum, com o painel de reprodução acoplado à direita](docs/screenshots/album.png)

</div>

O Swell é escrito em Rust com [gtk4-rs](https://gtk-rs.org),
[libadwaita](https://gnome.pages.gitlab.gnome.org/libadwaita/) e
[GStreamer](https://gstreamer.freedesktop.org/). Sua biblioteca abre
instantaneamente a partir de um cache local, o FLAC Hi-Res chega ao servidor
de som sem ser alterado, e a interface se adapta de janelas do tamanho de um
celular a desktops largos, usando uma fração da memória de outros clientes
([medições](#medições)).

> [!NOTE]
> O Swell não é afiliado, endossado nem suportado pelo TIDAL. Ele usa as
> APIs não oficiais do TIDAL, que podem mudar sem aviso, e requer uma
> assinatura paga do TIDAL.

## Índice

- [Recursos](#recursos)
- [Capturas de tela](#capturas-de-tela)
- [Instalação](#instalação)
- [Login e qualidade de áudio](#login-e-qualidade-de-áudio)
- [Reprodução bit-perfect](#reprodução-bit-perfect)
- [Idiomas](#idiomas)
- [Privacidade](#privacidade)
- [Medições](#medições)
- [Desenvolvimento](#desenvolvimento)
- [Licença](#licença)

## Recursos

**Navegar**
- Início: seu feed personalizado do TIDAL (atalhos, mixes, recomendações,
  reproduzidas recentemente), que carrega mais conforme você rola. Explorar:
  páginas editoriais, gêneros, climas e décadas.
- Pesquisa enquanto você digita, com filtros (faixas, álbuns, artistas,
  playlists), um melhor resultado, pesquisas recentes e paginação; colar um
  link do TIDAL o abre.
- Páginas de álbum, artista, playlist, mix e rádio, com créditos, resenhas,
  biografias, artistas semelhantes e páginas “Ver tudo”.
- Links `tidal://` e `https://tidal.com/…` abrem no Swell.

**Sua coleção**
- Milhares de faixas favoritas, virtualizadas, com filtro e ordenação
  instantâneos; álbuns, artistas, playlists e mixes favoritos; reproduzidas
  recentemente.
- Corações em tudo (faixas, álbuns, artistas, playlists, mixes), a partir de
  uma única requisição para a biblioteca inteira.
- Crie, renomeie, descreva, publique e exclua playlists; adicione faixas por
  um menu ou arrastando-as para uma playlist da barra lateral; remova e
  reordene, com opção de desfazer.

**Reprodução**
- Hi-Res Lossless (FLAC de até 24 bits/192 kHz), [bit-perfect](#reprodução-bit-perfect).
- Reprodução sem pausas, ReplayGain opcional (faixa ou álbum), escolha do
  dispositivo de saída.
- Uma fila de verdade: reproduzir em seguida, adicionar à fila, pular,
  remover, reordenar, limpar com opção de desfazer; aleatório, repetir
  tudo/uma, histórico.
- Sobrevive a quedas de rede e a URLs de stream expiradas, retomando na
  mesma posição.
- Letras sincronizadas que acompanham a música e permitem pular com um
  clique.
- Restaura a fila e a posição ao iniciar; temporizador de sono; notificações
  da área de trabalho; mantém o sistema ativo durante a reprodução.
- MPRIS no Linux: teclas de mídia, widgets da área de trabalho e `playerctl`
  funcionam.

**Interface**
- Design libadwaita que segue o estilo claro/escuro e a cor de destaque do
  sistema, com três layouts: largo com um painel de reprodução acoplado,
  normal e estreito com uma folha inferior e um mini player.
- Controle total pelo teclado (Espaço, Ctrl+←/→, Ctrl+F, Ctrl+L, F6…; Ctrl+?
  lista todos os atalhos) e nomes acessíveis em todos os controles.
- Inglês, espanhol, francês, alemão, italiano, português (Brasil), russo,
  japonês, coreano e chinês (simplificado e tradicional).

## Capturas de tela

| Início | Pesquisa |
|---|---|
| ![Feed do Início com atalhos, mixes e reproduzidas recentemente](docs/screenshots/home.png) | ![Resultados da pesquisa por “yoasobi” com um melhor resultado, faixas e artistas](docs/screenshots/search.png) |
| **Letras sincronizadas** | **Estilo claro** |
| ![Painel de reprodução mostrando a fila](docs/screenshots/now-playing.png) | ![A página de um álbum no estilo claro](docs/screenshots/light.png) |

<p align="center">
  <img src="docs/screenshots/narrow.png" width="300" alt="Layout estreito com o mini player e a barra de navegação inferior">
</p>

## Instalação

Os pacotes de cada versão estão na
[página de Releases](https://github.com/oxakromax/swell-player/releases).
O Swell requer GTK ≥ 4.18, libadwaita ≥ 1.7 e GStreamer com os plugins base e
good; os pacotes instalam essas dependências.

### Arch Linux (AUR)

O pacote do AUR está a caminho. Até lá, compile o mesmo PKGBUILD a partir
deste repositório:

```bash
git clone https://github.com/oxakromax/swell-player.git
cd swell-player/packaging/aur/swell-player   # or swell-player-git
makepkg -si
```

Quando estiver no AUR: `yay -S swell-player` (ou `swell-player-git`).

### Flatpak

Baixe o pacote `.flatpak` na página de Releases e instale-o (ele usa o
runtime do GNOME do Flathub). Uma página no Flathub está nos planos.

```bash
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user ./swell-player-*.flatpak
flatpak run io.github.oxakromax.SwellPlayer
```

### Debian e Ubuntu

Requer Debian 13 (trixie), Ubuntu 25.04 ou mais recente (GTK ≥ 4.18 e
libadwaita ≥ 1.7). Baixe o `.deb` na página de Releases:

```bash
sudo apt install ./swell-player_*.deb
```

### Fedora

Baixe o `.rpm` na página de Releases:

```bash
sudo dnf install ./swell-player-*.rpm
```

### Windows

Baixe o instalador (ou o `.zip` portátil) na página de Releases. As builds
ainda não são assinadas, então o SmartScreen pode avisar sobre um app não
reconhecido: escolha **Mais informações → Executar assim mesmo**.

### macOS

Baixe o `.dmg` na página de Releases e arraste o Swell para Aplicativos. O
app ainda não é notarizado, então, na primeira vez, clique com o botão
direito (ou com Control) nele e escolha **Abrir**; no macOS 15 e posteriores,
abra-o uma vez e depois permita-o em **Ajustes do Sistema → Privacidade e
Segurança → Abrir Mesmo Assim**. Como cada versão é assinada ad hoc, o macOS
pode pedir novamente acesso ao item do Chaveiro do Swell após uma
atualização: escolha **Sempre Permitir**. Os links `tidal://` ainda não
abrem o Swell no macOS, e a imagem de disco Intel (x86_64) é feita na base
do melhor esforço, então uma versão pode não tê-la.

As builds para Windows e macOS são mais novas que a do Linux: o MPRIS e as
notas sobre bit-perfect abaixo são específicos do Linux.

### A partir do código-fonte

Você precisa do Rust ≥ 1.96 ([rustup](https://rustup.rs) se o da sua
distribuição for mais antigo), GTK ≥ 4.20 e libadwaita ≥ 1.8 (ou GTK 4.18 e
libadwaita 1.7 com `--no-default-features`), GStreamer ≥ 1.24, SQLite e o
`glib-compile-resources` da GLib.

<details>
<summary>Dependências de compilação por distribuição</summary>

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

O `gst-plugins-good` fornece o demuxer DASH (`dashdemux2`) usado nos streams
Hi-Res.

</details>

```bash
make                  # cargo build --release --locked
sudo make install     # binary, desktop entry, icon, metainfo under /usr/local
```

`make install PREFIX=/usr DESTDIR=…` prepara uma instalação para
empacotamento, e `sudo make uninstall` a remove. O binário também roda
diretamente de `target/release/swell-player`, pois os ícones e as traduções
são embutidos.

## Login e qualidade de áudio

O TIDAL não permite que apps de terceiros registrem seus próprios clientes
OAuth, então o Swell, como todo cliente TIDAL de código aberto, faz login
como um dos apps do próprio TIDAL. O tipo de login que você escolhe define
qual deles, e isso limita a qualidade:

| Login | Como funciona | Melhor qualidade |
|---|---|---|
| **Navegador** (recomendado) | O Swell abre a página de login do TIDAL no seu navegador. Depois que você entra, o TIDAL cai em uma página que diz “Oops”; copie o endereço dessa página de volta para o Swell. | **Hi-Res Lossless**: FLAC de até 24 bits/192 kHz |
| **Código de dispositivo** | O Swell mostra um código curto; aprove-o em `link.tidal.com` a partir de qualquer dispositivo. | **Lossless**: FLAC 16 bits/44,1 kHz |

Escolha a qualidade do streaming em **Preferências → Reprodução**. O selo de
formato na barra do player mostra o que está de fato sendo entregue.

## Reprodução bit-perfect

Com a normalização desativada e o volume em 100 %, o Swell mantém o formato
do stream até o PipeWire: uma faixa de 24 bits/96 kHz chega a ele como
24 bits a 96 kHz. O selo de formato na barra do player abre um popover que
diz se a saída é bit-perfect e, se não for, por quê.

O PipeWire então decide o que a placa de som recebe. Por padrão, ele roda em
uma única taxa (geralmente 48 kHz) e reamostra todo o resto. Para que ele
acompanhe a taxa de cada faixa, permita as taxas comuns, por exemplo em
`~/.config/pipewire/pipewire.conf.d/10-rates.conf`:

```
context.properties = {
    default.clock.allowed-rates = [ 44100 48000 88200 96000 176400 192000 ]
}
```

depois reinicie o PipeWire (`systemctl --user restart pipewire`). O `pw-top`
mostra o que chega ao dispositivo.

## Idiomas

O Swell fala **inglês**, **espanhol**, **francês**, **alemão**,
**italiano**, **português (Brasil)**, **russo**, **japonês**, **coreano** e
**chinês** (simplificado e tradicional). Ele segue o idioma do sistema
automaticamente (`LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`, como o
gettext), e você pode alterá-lo nas **Preferências**. O conteúdo do próprio
TIDAL (textos editoriais, títulos de páginas) é solicitado no mesmo idioma,
então uma tela nunca mistura dois.

Quer o Swell no seu idioma? Veja [docs/translating.md](docs/translating.md).

## Privacidade

- Os **tokens** ficam no chaveiro do seu sistema (no Linux, o Secret
  Service: GNOME Keyring, KWallet, KeePassXC…, ou o portal de segredos no
  Flatpak), nunca em texto simples no disco. Eles são renovados pouco antes
  de expirar, e problemas de rede nunca encerram sua sessão.
- **Em cache no seu computador:** as respostas do TIDAL e sua biblioteca
  (`~/.cache/swell-player/library.sqlite3`), capas, os primeiros segundos
  das faixas que você ouviu ou ia ouvir (`~/.cache/swell-player/audio/`, só
  áudio, no máximo 512 MB), suas configurações, a fila salva e as pesquisas
  recentes. As Preferências podem limpar o cache; desconectar apaga os
  tokens, o cache, a fila salva e o histórico de pesquisas.
- **Sem telemetria.** O Swell se conecta apenas aos servidores do TIDAL, para
  o que você pede que ele mostre ou reproduza. Não há análises, relatórios de
  falhas nem outros serviços de terceiros.

## Medições

O Swell contra o [High Tide](https://github.com/Nokse22/high-tide) 1.6
(Python + PyGObject) na mesma máquina, ambos em uma sessão KWin headless
(`kwin_wayland --virtual`, 1280×860) com uma sessão salva. Medianas de três
execuções; memória lida de `/proc` (RssAnon é o heap do próprio app; o RSS
também conta as bibliotecas compartilhadas e cerca de 250 MB de mapeamentos
do driver de GPU que qualquer app GTK 4 tem aqui). Os valores em repouso são
médias de 10 s sem nada tocando.

| | Swell | High Tide |
|---|---|---|
| Início na tela: RssAnon / RSS | **61 MB / 360 MB** | 131 MB / 504 MB |
| 1.295 faixas favoritas na tela, a partir do cache | **0,42 s** após a abertura (sincronizado com o TIDAL em 0,50 s) | — ¹ |
| … sem o cache do Swell | as primeiras 100 em **0,59 s**, todas em 1,2–1,9 s | — ¹ |
| Tela de favoritos: RssAnon / RSS | 85 MB / 371 MB | — ¹ |
| CPU em repouso / despertares da thread principal | ~0 % / 1,1 por s | ~0 % / 0,1 por s |
| Threads | 15 | 19 |
| Faixa Hi-Res escolhida → áudio | **0,67 s** (24 bits/96 kHz, chega ao PipeWire como S24_32 a 96 kHz) | — |

¹ O High Tide não coloca os favoritos em cache e não tem como abrir sua lista
de favoritos diretamente; com um ambiente de teste, a prova de conceito do
Swell mediu 13,3 s para o High Tide mostrar a mesma lista, contra 0,3 s para
o Swell.

Máquina: Arch Linux, KDE Plasma 6 (Wayland), GTK 4.24, libadwaita 1.10,
GStreamer 1.28, Rust 1.99, build de release.

## Desenvolvimento

```bash
cargo run --release
cargo test --release
cargo clippy --release --all-targets
```

O [CONTRIBUTING.md](CONTRIBUTING.md) explica como compilar, testar, as
variáveis de ambiente de depuração (abrir uma página, reproduzir uma faixa
ou renderizar a janela em um PNG sem monitor) e como enviar alterações.
O [docs/architecture.md](docs/architecture.md) explica como o código está
organizado; o [docs/features.md](docs/features.md) é o escopo de recursos e
o [docs/design.md](docs/design.md) a especificação da interface (em
espanhol). As notas de versão estão no [CHANGELOG.md](CHANGELOG.md).

Encontrou um problema de segurança? Reporte-o em particular: veja o
[SECURITY.md](SECURITY.md).

## Licença

O Swell é software livre: você pode redistribuí-lo e/ou modificá-lo sob os
termos da Licença Pública Geral GNU, conforme publicada pela Free Software
Foundation, na versão 3 da Licença ou (a seu critério) em qualquer versão
posterior. Veja [COPYING](COPYING).

TIDAL é uma marca registrada de seu proprietário, usada aqui apenas para
dizer com o que o Swell funciona.
