[doc('Run all tests')]
[group('test')]
test:
    cargo test --workspace -- --include-ignored

[doc('Run upstream Homebrew tests against the local zb binary')]
[group('test')]
[positional-arguments]
[script]
homebrew-compat *args:
    source_repo="${HOMEBREW_COMPAT_SOURCE_REPO:-${XDG_CACHE_HOME:-$HOME/.cache}/zerobrew/homebrew-src}"
    work_root="$PWD/target/homebrew-compat"
    worktree="$work_root/brew-worktree"
    logs_dir="$work_root/logs"
    zb_root="${HOMEBREW_COMPAT_ZB_ROOT:-/tmp/zbcompat}"
    zb_prefix="${HOMEBREW_COMPAT_ZB_PREFIX:-$zb_root}"

    mkdir -p "$(dirname "$source_repo")" "$work_root" "$logs_dir"

    if [[ -d "$source_repo/.git" ]]; then
        echo "==> Updating Homebrew source: $source_repo"
        git -C "$source_repo" fetch --prune origin
    else
        echo "==> Cloning Homebrew source: $source_repo"
        git clone https://github.com/Homebrew/brew.git "$source_repo"
    fi

    if [[ -n "${HOMEBREW_COMPAT_REF:-}" ]]; then
        brew_ref="$HOMEBREW_COMPAT_REF"
    else
        brew_ref="$(git -C "$source_repo" symbolic-ref --quiet --short refs/remotes/origin/HEAD 2>/dev/null || true)"
        brew_ref="${brew_ref:-origin/main}"
    fi
    brew_commit="$(git -C "$source_repo" rev-parse "$brew_ref")"

    reuse_worktree=0
    if [[ "${HOMEBREW_COMPAT_REFRESH_WORKTREE:-0}" != "1" ]] \
        && [[ -d "$worktree" ]] \
        && [[ "$(git -C "$worktree" rev-parse --is-inside-work-tree 2>/dev/null || true)" == "true" ]]; then
        worktree_commit="$(git -C "$worktree" rev-parse HEAD 2>/dev/null || true)"
        if [[ "$worktree_commit" == "$brew_commit" ]] \
            && grep -q 'HOMEBREW_ZEROBREW_COMPAT_BIN' "$worktree/Library/Homebrew/test/support/helper/spec/shared_context/integration_test.rb"; then
            reuse_worktree=1
        fi
    fi

    if [[ "$reuse_worktree" == "1" ]]; then
        echo "==> Reusing Homebrew worktree: $brew_commit"
    else
        if [[ -e "$worktree" ]]; then
            git -C "$source_repo" worktree remove --force "$worktree" >/dev/null 2>&1 || rm -rf "$worktree"
        fi
        git -C "$source_repo" worktree prune
        echo "==> Preparing Homebrew worktree: $brew_commit"
        git -C "$source_repo" worktree add --detach "$worktree" "$brew_commit" >/dev/null
        git -C "$worktree" apply --unidiff-zero - <<'PATCH'
    diff --git a/Library/Homebrew/test/support/helper/spec/shared_context/integration_test.rb b/Library/Homebrew/test/support/helper/spec/shared_context/integration_test.rb
    index 97a89d8a03..c9e9e732b4 100644
    --- a/Library/Homebrew/test/support/helper/spec/shared_context/integration_test.rb
    +++ b/Library/Homebrew/test/support/helper/spec/shared_context/integration_test.rb
    @@ -90,0 +91,15 @@ RSpec.shared_context "integration test" do # rubocop:disable RSpec/ContextWordin
    +    if (zerobrew_bin = ENV["HOMEBREW_ZEROBREW_COMPAT_BIN"])
    +      env.merge!(
    +        "ZEROBREW_ROOT"      => ENV.fetch("HOMEBREW_ZEROBREW_COMPAT_ROOT"),
    +        "ZEROBREW_PREFIX"    => ENV.fetch("HOMEBREW_ZEROBREW_COMPAT_PREFIX"),
    +        "ZEROBREW_AUTO_INIT" => "true",
    +      )
    +
    +      return Bundler.with_unbundled_env do
    +        stdout, stderr, status = Open3.capture3(env, zerobrew_bin, *args)
    +        $stdout.print stdout
    +        $stderr.print stderr
    +        status
    +      end
    +    end
    +
    @@ -132 +147,10 @@ RSpec.shared_context "integration test" do # rubocop:disable RSpec/ContextWordin
    -      brew_sh_path = env.delete("HOMEBREW_BREW_SH") || "#{ENV.fetch("HOMEBREW_PREFIX")}/bin/brew"
    +      brew_sh_path = ENV["HOMEBREW_ZEROBREW_COMPAT_BIN"] ||
    +                     env.delete("HOMEBREW_BREW_SH") ||
    +                     "#{ENV.fetch("HOMEBREW_PREFIX")}/bin/brew"
    +      if ENV["HOMEBREW_ZEROBREW_COMPAT_BIN"]
    +        env.merge!(
    +          "ZEROBREW_ROOT"      => ENV.fetch("HOMEBREW_ZEROBREW_COMPAT_ROOT"),
    +          "ZEROBREW_PREFIX"    => ENV.fetch("HOMEBREW_ZEROBREW_COMPAT_PREFIX"),
    +          "ZEROBREW_AUTO_INIT" => "true",
    +        )
    +      end
           stdout, stderr, status = Open3.capture3(
             env,
             brew_sh_path,
    PATCH
    fi

    if [[ -n "${HOMEBREW_COMPAT_ZB_BIN:-}" ]]; then
        zb_bin="$HOMEBREW_COMPAT_ZB_BIN"
    else
        echo "==> Building zb release binary"
        cargo build --release --bin zb
        zb_bin="$PWD/target/release/zb"
    fi

    if [[ "${HOMEBREW_COMPAT_KEEP_ROOT:-0}" != "1" ]]; then
        echo "==> Resetting isolated zerobrew root: $zb_root"
        rm -rf "$zb_root"
        if [[ "$zb_prefix" != "$zb_root" ]]; then
            rm -rf "$zb_prefix"
        fi
    fi

    compat_state="$work_root/zerobrew-state"
    mkdir -p "$compat_state/bin" "$compat_state/config"
    ZEROBREW_DIR="$compat_state/config" ZEROBREW_BIN="$compat_state/bin" \
        "$zb_bin" --root "$zb_root" --prefix "$zb_prefix" init --no-modify-path >/dev/null

    test_args=("$@")
    if [[ "${HOMEBREW_COMPAT_NO_PARALLEL:-1}" != "0" ]]; then
        has_no_parallel=0
        for arg in "${test_args[@]}"; do
            [[ "$arg" == "--no-parallel" ]] && has_no_parallel=1
        done
        if [[ "$has_no_parallel" == "0" ]]; then
            test_args=("--no-parallel" "${test_args[@]}")
        fi
    fi

    timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
    log_file="$logs_dir/homebrew-compat-$timestamp.log"
    failed_file="$logs_dir/homebrew-compat-$timestamp.failed"

    echo "==> Running Homebrew tests"
    echo "    Homebrew: $brew_commit"
    echo "    zb:       $zb_bin"
    echo "    root:     $zb_root"
    echo "    prefix:   $zb_prefix"
    echo "    args:     ${test_args[*]:-(none)}"
    echo "    log:      $log_file"

    set +e
    HOMEBREW_NO_AUTO_UPDATE=1 \
    HOMEBREW_NO_ANALYTICS=1 \
    HOMEBREW_NO_ENV_HINTS=1 \
    HOMEBREW_DEVELOPER=1 \
    HOMEBREW_ZEROBREW_COMPAT_BIN="$zb_bin" \
    HOMEBREW_ZEROBREW_COMPAT_ROOT="$zb_root" \
    HOMEBREW_ZEROBREW_COMPAT_PREFIX="$zb_prefix" \
        "$worktree/bin/brew" tests "${test_args[@]}" 2>&1 | tee "$log_file"
    status=${PIPESTATUS[0]}
    set -e

    grep '^rspec ' "$log_file" > "$failed_file" || true
    ln -sf "$log_file" "$logs_dir/latest.log"
    ln -sf "$failed_file" "$logs_dir/latest.failed"

    summary="$(awk '/^[0-9]+ examples?,/ { line=$0 } END { print line }' "$log_file")"
    if [[ -n "$summary" ]]; then
        echo "==> Summary"
        echo "    $summary"
        awk '
            /^[0-9]+ examples?,/ { summary=$0 }
            END {
                if (summary == "") exit
                split(summary, parts, ",")
                total = parts[1] + 0
                failures = parts[2] + 0
                pending = parts[3] + 0
                passed = total - failures - pending
                printf "    passed: %d\n    failures: %d\n    pending: %d\n", passed, failures, pending
            }
        ' "$log_file"
    fi
    echo "    failed examples: $failed_file"
    exit "$status"

