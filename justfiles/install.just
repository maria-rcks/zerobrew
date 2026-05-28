[doc('Install zb to $ZEROBREW_BIN')]
[group('install')]
[script]
install: build
    if [[ -d "$ZEROBREW_PREFIX/lib/pkgconfig" ]]; then
        export PKG_CONFIG_PATH="$ZEROBREW_PREFIX/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
    fi
    if [[ -d '/opt/homebrew/lib/pkgconfig' ]] && [[ ! "${PKG_CONFIG_PATH:-}" =~ '/opt/homebrew/lib/pkgconfig' ]]; then
        export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
    fi

    mkdir -p "$ZEROBREW_BIN"
    install -m755 target/release/zb "$ZEROBREW_BIN/zb"
    install -m755 target/release/zbx "$ZEROBREW_BIN/zbx"
    echo "Installed zb to $ZEROBREW_BIN/zb"
    echo "Installed zbx to $ZEROBREW_BIN/zbx"

    "$ZEROBREW_BIN/zb" init

[private]
[script]
_get_zerobrew_configs:
    shell_configs=(
        "${ZDOTDIR:-$HOME}/.zshenv"
        "${ZDOTDIR:-$HOME}/.zshrc"
        "$HOME/.bashrc"
        "$HOME/.bash_profile"
        "$HOME/.profile"
    )

    for config in "${shell_configs[@]}"; do
        if [[ -f "$config" ]] && grep -q '^# zerobrew$' "$config" 2>/dev/null; then
            echo "$config"
        fi
    done

[private]
[script]
_clean_shell_config config:
    tmp_file=$(mktemp)
    sed -e '/^# zerobrew$/,/^}$/d' \
        -e '/_zb_path_append/d' \
        "$config" > "$tmp_file" 2>/dev/null || true
    cat -s "$tmp_file" > "$config"
    rm "$tmp_file"
    echo -e '{{BOLD}}{{GREEN}}✓{{NORMAL}} Cleaned '"$config"''

[doc('Uninstall zb and remove all data')]
[group('install')]
[script]
uninstall:
    configs_to_clean_file=$(mktemp)
    trap 'rm -f "$configs_to_clean_file"' EXIT
    just _get_zerobrew_configs > "$configs_to_clean_file"

    echo 'Running this will remove:'
    echo -en '{{BOLD}}{{RED}}'
    echo -e  "\t$ZEROBREW_INSTALLED_BIN"
    echo -e  "\t$ZEROBREW_DIR"
    echo -e  "\t$ZEROBREW_ROOT"
    while IFS= read -r config; do
        echo -e "\tzerobrew entries in $config"
    done < "$configs_to_clean_file"
    echo -en '{{NORMAL}}'

    read -rp "Continue? [y/N] " confirm
    if [[ ! "$confirm" =~ ^[Yy]$ ]]; then
        exit 0
    fi

    # Clean shell configuration files
    while IFS= read -r config; do
        just _clean_shell_config "$config"
    done < "$configs_to_clean_file"

    [[ -f "$ZEROBREW_INSTALLED_BIN" ]] && rm -- "$ZEROBREW_INSTALLED_BIN"
    [[ -d "$ZEROBREW_DIR" ]] && rm -rf -- "$ZEROBREW_DIR"

    if [[ -d "$ZEROBREW_ROOT" ]]; then
        $SUDO rm -r -- "$ZEROBREW_ROOT"
    fi

    echo ''
    echo -e '{{BOLD}}{{GREEN}}✓{{NORMAL}} zerobrew uninstalled successfully!'
    echo ''
    echo 'Restart your terminal or run: exec $SHELL'

[doc('Reset zerobrew completely (removes data and re-initializes)')]
[group('install')]
[script]
reset:
    configs_to_clean_file=$(mktemp)
    trap 'rm -f "$configs_to_clean_file"' EXIT
    just _get_zerobrew_configs > "$configs_to_clean_file"

    echo -e '{{BOLD}}{{YELLOW}}Warning:{{NORMAL}} This will reset zerobrew completely:'
    echo -en '{{BOLD}}{{RED}}'
    echo -e  "\t$ZEROBREW_DIR"
    echo -e  "\t$ZEROBREW_ROOT"
    while IFS= read -r config; do
        echo -e "\tzerobrew entries in $config"
    done < "$configs_to_clean_file"
    echo -en '{{NORMAL}}'

    read -rp "Continue? [y/N] " confirm
    if [[ ! "$confirm" =~ ^[Yy]$ ]]; then
        exit 0
    fi

    # Clean shell configuration files
    while IFS= read -r config; do
        just _clean_shell_config "$config"
    done < "$configs_to_clean_file"

    [[ -d "$ZEROBREW_DIR" ]] && rm -rf -- "$ZEROBREW_DIR" && echo -e '{{BOLD}}{{GREEN}}✓{{NORMAL}} Removed '"$ZEROBREW_DIR"''

    if [[ -d "$ZEROBREW_ROOT" ]]; then
        $SUDO rm -rf -- "$ZEROBREW_ROOT" && echo -e '{{BOLD}}{{GREEN}}✓{{NORMAL}} Removed '"$ZEROBREW_ROOT"''
    fi

    echo ''
    echo -e '{{BOLD}}{{CYAN}}==>{{NORMAL}} Re-initializing zerobrew...'

    if [[ -f "$ZEROBREW_INSTALLED_BIN" ]]; then
        "$ZEROBREW_INSTALLED_BIN" init
        echo ''
        echo -e '{{BOLD}}{{GREEN}}✓{{NORMAL}} Reset complete!'
    else
        echo -e '{{BOLD}}{{YELLOW}}Note:{{NORMAL}} zb binary not found at $ZEROBREW_INSTALLED_BIN'
        echo -e '{{BOLD}}{{YELLOW}}Note:{{NORMAL}} Run {{BOLD}}just install{{NORMAL}} first to install zerobrew'
    fi

