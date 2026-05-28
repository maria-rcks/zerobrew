set export
set dotenv-load
set unstable
set script-interpreter := ['/bin/bash', '-euo', 'pipefail']

ZEROBREW_ROOT := if env('ZEROBREW_ROOT', '') != '' {
    env('ZEROBREW_ROOT')
} else if path_exists('/opt/zerobrew') == 'true' {
    '/opt/zerobrew'
} else if os() == 'macos' {
    '/opt/zerobrew'
} else {
    env('XDG_DATA_HOME', env('HOME', '~') / '.local' / 'share' ) / 'zerobrew'
}
ZEROBREW_DIR := env('ZEROBREW_DIR', env('HOME', '~') / '.zerobrew')
ZEROBREW_BIN := env('ZEROBREW_BIN', env('HOME', '~') / '.local' / 'bin')
ZEROBREW_PREFIX := if env('ZEROBREW_PREFIX', '') != '' {
    env('ZEROBREW_PREFIX')
} else if os() == 'macos' {
    ZEROBREW_ROOT
} else {
    ZEROBREW_ROOT / 'prefix'
}
ZEROBREW_INSTALLED_BIN := ZEROBREW_BIN / 'zb'

SUDO := if which('doas') != '' {
    'doas'
} else {
    require('sudo')
}

# Package lists for benchmarks
BENCH_PACKAGES := 'ca-certificates openssl@3 xz sqlite readline icu4c@78 python@3.14 awscli node harfbuzz ncurses gh pcre2 libpng zstd glib lz4 gettext libngtcp2 libnghttp3 pkgconf libunistring mpdecimal brotli jpeg-turbo xorgproto ffmpeg cmake libnghttp2 go uv gmp libtiff fontconfig python@3.13 git little-cms2 dav1d openexr c-ares tesseract p11-kit imagemagick zlib libx11 freetype protobuf gnupg openjph libtasn1 ruby gnutls expat libsodium simdjson gemini-cli libarchive pyenv pixman curl opus unbound cairo pango leptonica libxcb jpeg-xl coreutils certifi krb5 docker libheif webp libxext libxau gcc bzip2 libxdmcp abseil xcbeautify libuv giflib utf8proc libxrender m4 graphite2 openjdk uvwasi libffi libdeflate llvm aom lzo libevent libgpg-error libidn2 berkeley-db@5 deno libedit oniguruma'

BENCH_QUICK_PACKAGES := 'jq tree htop bat fd ripgrep fzf wget curl git tmux zoxide openssl@3 sqlite readline pcre2 zstd lz4 node go ruby gh'

alias b := build
alias i := install
alias t := test
alias l := lint
alias f := fmt

import 'justfiles/default.just'
import 'justfiles/build.just'
import 'justfiles/install.just'
import 'justfiles/lint.just'
import 'justfiles/test.just'
import 'justfiles/benchmark.just'
