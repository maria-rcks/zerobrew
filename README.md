<div align="center">

<h2>zerobrew</h2>

[![Lint](https://github.com/maria-rcks/zerobrew/actions/workflows/ci.yml/badge.svg)](https://github.com/maria-rcks/zerobrew/actions/workflows/ci.yml)
[![Test](https://github.com/maria-rcks/zerobrew/actions/workflows/test.yml/badge.svg)](https://github.com/maria-rcks/zerobrew/actions/workflows/test.yml)
[![Release](https://img.shields.io/github/v/release/maria-rcks/zerobrew?display_name=tag)](https://github.com/maria-rcks/zerobrew/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE-MIT.md)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](./LICENSE-APACHE.md)

<img alt="zerobrew demo" src="./assets/zb-demo.gif" />

<p><strong>zerobrew brings uv-style architecture to Homebrew packages on macOS and Linux.</strong></p>

</div>

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/maria-rcks/zerobrew/main/install.sh | bash
```

After install, run the `export` command it prints (or restart your terminal).

Or via Homebrew:

```bash
brew install maria-rcks/zerobrew/zerobrew
```

## Project direction

This repository is maintained as an independent fork of the original
[`lucasgelfond/zerobrew`](https://github.com/lucasgelfond/zerobrew) project.
The fork keeps the original license and contribution history intact while
continuing development under `maria-rcks/zerobrew`.

The current direction is intentionally maintainer-led: the focus is on building
a reliable, agent-friendly package manager workflow before expanding the
project's public contribution surface again. Issues and pull requests are still
welcome, but roadmap and merge decisions are made by the current maintainer.
