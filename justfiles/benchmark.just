[doc('Run deterministic function-level benchmarks and print fastest/slowest rankings')]
[group('benchmark')]
bench-fns:
    cargo bench -p zb_bench --bench workspace_hotspots
    cargo run --quiet -p zb_bench

[doc('Run benchmark comparing zerobrew vs homebrew')]
[group('benchmark')]
[positional-arguments]
[script]
bench *args:
    # Package lists defined in Justfile variables
    read -ra PACKAGES <<< "{{BENCH_PACKAGES}}"
    read -ra QUICK_PACKAGES <<< "{{BENCH_QUICK_PACKAGES}}"

    FORMAT=""
    OUTPUT=""
    COUNT=""
    RUNS=3
    QUICK=true
    FULL=false
    FULL_OUTPUT_DIR=""
    NO_COLOR=false
    LOG_FILE=""
    DRY_RUN=false

    need_arg() { [[ -n "$2" && "$2" != --* ]] || { echo "Error: $1 requires a value" >&2; exit 1; }; }

    while [[ $# -gt 0 ]]; do
        case $1 in
            --format)  need_arg "$1" "$2"; FORMAT="$2"; shift 2 ;;
            -c|--count)   need_arg "$1" "$2"; COUNT="$2"; shift 2 ;;
            --runs)    need_arg "$1" "$2"; RUNS="$2"; shift 2 ;;
            --quick)   QUICK=true; FULL=false; shift ;;
            --full)    FULL=true; QUICK=false;
                       # Check if next arg is a directory path (not a flag)
                       if [[ $# -gt 1 && "$2" != -* ]]; then
                           FULL_OUTPUT_DIR="$2"
                           shift 2
                       else
                           shift
                       fi ;;
            --no-color) NO_COLOR=true; shift ;;
            --log)     need_arg "$1" "$2"; LOG_FILE="$2"; shift 2 ;;
            -o|--output) need_arg "$1" "$2"; OUTPUT="$2"; shift 2 ;;
            --dry-run) DRY_RUN=true; shift ;;
            -h|--help)
                echo "Usage: just bench [options]"
                echo ""
                echo "Options:"
                echo "  --quick              Test all quick packages (default, 22 packages)"
                echo "  --full [DIR]         Test all 100 top Homebrew packages"
                echo "                       Optionally specify DIR to output all formats to directory"
                echo "  -c, --count N        Test first N packages from selected list"
                echo "                       (quick packages by default, or from --full list)"
                echo "  --runs N             Hyperfine runs per command (default: 3)"
                echo "  --format FORMAT      Output format: text (default), json, csv, or html"
                echo "  -o, --output FILE    Write output to file instead of stdout (format inferred from extension)"
                echo "  --no-color           Disable colored output"
                echo "  --log FILE           Write install command logs to file"
                echo "  --dry-run            Show what would be tested without running benchmarks"
                echo "  -h, --help           Show this help message"
                exit 0 ;;
            *) echo "Unknown option: $1" >&2; exit 1 ;;
        esac
    done

    # Infer format from output file extension if not explicitly set
    if [[ -z "$FORMAT" && -n "$OUTPUT" ]]; then
        case "$OUTPUT" in
            *.json) FORMAT="json" ;;
            *.csv)  FORMAT="csv" ;;
            *.html) FORMAT="html" ;;
            *)      FORMAT="text" ;;
        esac
    elif [[ -z "$FORMAT" ]]; then
        FORMAT="text"
    fi

    [[ "$FORMAT" =~ ^(text|json|csv|html)$ ]] || { echo "Error: format must be text, json, csv, or html" >&2; exit 1; }
    [[ "$RUNS" =~ ^[1-9][0-9]*$ ]] || { echo "Error: --runs must be a positive integer" >&2; exit 1; }

    # Pre-run validation
    missing=()
    command -v brew &>/dev/null || missing+=("brew")
    command -v zb &>/dev/null || missing+=("zb")
    command -v hyperfine &>/dev/null || missing+=("hyperfine")
    command -v python3 &>/dev/null || missing+=("python3")
    command -v bc &>/dev/null || missing+=("bc")
    command -v sed &>/dev/null || missing+=("sed")

    if [[ ${#missing[@]} -gt 0 ]]; then
        echo "Error: Missing required commands: ${missing[*]}" >&2
        echo "Please install them before running benchmarks." >&2
        exit 1
    fi

    export HOMEBREW_NO_AUTO_UPDATE=1
    export HOMEBREW_NO_ANALYTICS=1
    export HOMEBREW_NO_ENV_HINTS=1

    brew_prefix=$(brew --prefix)
    if [[ ! -w "$brew_prefix" ]]; then
        echo "Error: Homebrew prefix is not writable: $brew_prefix" >&2
        echo "This will cause brew to prompt for sudo during benchmarks." >&2
        echo "Fix: sudo chown -R \"$(whoami)\" \"$brew_prefix\"" >&2
        exit 1
    fi

    # Check if zerobrew directories have files not owned by current user
    # This would cause zb reset to prompt for sudo during benchmarks
    current_user=$(whoami)
    needs_chown=false

    for dir in "$ZEROBREW_ROOT" "$ZEROBREW_PREFIX"; do
        if [[ -d "$dir" ]]; then
            # Find any files/dirs not owned by current user (limit to 1 for speed)
            not_owned=$(find "$dir" ! -user "$current_user" -print -quit 2>/dev/null)
            if [[ -n "$not_owned" ]]; then
                needs_chown=true
                break
            fi
        fi
    done

    if [[ "$needs_chown" == "true" ]]; then
        echo -e "${YELLOW}==> Some files in zerobrew directories are not owned by you.${NORMAL}" >&2
        echo -e "${YELLOW}    This will cause password prompts during benchmarks.${NORMAL}" >&2
        echo -e "${YELLOW}    Fixing ownership now (requires sudo once)...${NORMAL}" >&2
        if [[ -d "$ZEROBREW_ROOT" ]]; then
            {{SUDO}} chown -R "$current_user" "$ZEROBREW_ROOT" || { echo "Error: Failed to fix ownership of $ZEROBREW_ROOT" >&2; exit 1; }
        fi
        if [[ -d "$ZEROBREW_PREFIX" && "$ZEROBREW_PREFIX" != "$ZEROBREW_ROOT"* ]]; then
            {{SUDO}} chown -R "$current_user" "$ZEROBREW_PREFIX" || { echo "Error: Failed to fix ownership of $ZEROBREW_PREFIX" >&2; exit 1; }
        fi
        echo -e "${GREEN}    Ownership fixed!${NORMAL}" >&2
    fi

    echo "Hyperfine runs per command: $RUNS" >&2

    # Determine which packages to test
    # Default: all quick packages (22)
    # --quick: all quick packages
    # --full: all 100 packages
    # --count N: limit currently selected list to N
    if [[ "$FULL" == "true" ]]; then
        # --full: use all 100 packages
        PACKAGES=("${PACKAGES[@]}")
    else
        # Default or --quick: use all quick packages
        PACKAGES=("${QUICK_PACKAGES[@]}")
    fi

    # Apply --count limit if specified
    if [[ -n "$COUNT" ]]; then
        PACKAGES=("${PACKAGES[@]:0:$COUNT}")
    fi

    if [[ "$NO_COLOR" == "true" ]]; then
        RED="" GREEN="" YELLOW="" BLUE="" CYAN="" NORMAL="" BOLD=""
    else
        RED="{{RED}}" GREEN="{{GREEN}}" YELLOW="{{YELLOW}}"
        BLUE="{{BLUE}}" CYAN="{{CYAN}}" NORMAL="{{NORMAL}}" BOLD="{{BOLD}}"
    fi

    if [[ -n "$LOG_FILE" ]]; then
        : > "$LOG_FILE"
        echo -e "${BLUE}Debug logging to: $LOG_FILE${NORMAL}" >&2
    fi

    log_msg() {
        [[ -n "$LOG_FILE" ]] || return 0
        printf "[%s] %s\n" "$(date -u +'%Y-%m-%dT%H:%M:%SZ')" "$*" >> "$LOG_FILE"
    }

    format_duration() {
        python3 -c "ms=int('$1'); print(f'{ms/1000:.2f}s' if ms>=1000 else f'{ms}ms')"
    }

    log_msg "bench start: format=$FORMAT full=$FULL quick=$QUICK count=${COUNT:-all} packages=${#PACKAGES[@]}"
    if [[ -n "$FULL_OUTPUT_DIR" ]]; then
        log_msg "output dir: $FULL_OUTPUT_DIR"
    elif [[ -n "$OUTPUT" ]]; then
        log_msg "output file: $OUTPUT"
    fi

    if [[ "$DRY_RUN" == "true" ]]; then
        echo -e "${CYAN}=== Dry Run ===${NORMAL}" >&2
        echo "Would test ${#PACKAGES[@]} packages:" >&2
        for pkg in "${PACKAGES[@]}"; do
            echo "  - $pkg" >&2
        done
        echo "" >&2
        if [[ -n "$FULL_OUTPUT_DIR" ]]; then
            echo "Output directory: $FULL_OUTPUT_DIR" >&2
            echo "  Will create benchmark.{txt,json,csv,html}" >&2
        else
            echo "Format: $FORMAT" >&2
            [[ -n "$OUTPUT" ]] && echo "Output file: $OUTPUT" >&2
        fi
        [[ -n "$LOG_FILE" ]] && echo "Log file: $LOG_FILE" >&2
        echo "" >&2
        echo "Each package would be tested with:" >&2
        echo "  1. brew install <package>" >&2
        echo "  2. zb install <package> (cold cache)" >&2
        echo "  3. zb install <package> (warm cache)" >&2
        exit 0
    fi

    shell_quote() {
        printf "%q" "$1"
    }

    shell_join() {
        local out=""
        for arg in "$@"; do
            if [[ -n "$out" ]]; then
                out+=" "
            fi
            out+="$(shell_quote "$arg")"
        done
        printf "%s" "$out"
    }

    append_log_redirect() {
        local command="$1"
        if [[ -n "$LOG_FILE" ]]; then
            printf "%s >> %s 2>&1" "$command" "$(shell_quote "$LOG_FILE")"
        else
            printf "%s >/dev/null 2>&1" "$command"
        fi
    }

    run_hyperfine_install() {
        local package="$1"
        local export_json="$2"
        shift 2

        local names=()
        local prepares=()
        local commands=()

        while [[ $# -gt 0 ]]; do
            names+=("$1")
            prepares+=("$2")
            commands+=("$3")
            shift 3
        done

        local args=(--runs "$RUNS" --warmup 0 --time-unit millisecond --style basic --export-json "$export_json")
        [[ "$NO_COLOR" == "true" ]] && args=(--runs "$RUNS" --warmup 0 --time-unit millisecond --style nocolor --export-json "$export_json")

        for name in "${names[@]}"; do
            args+=(-n "$name")
        done
        for prepare in "${prepares[@]}"; do
            args+=(--prepare "$prepare")
        done
        for command in "${commands[@]}"; do
            args+=("$command")
        done

        echo -e "  ${YELLOW}-> Hyperfine: $package...${NORMAL}" >&2
        log_msg "hyperfine start: $package"
        if [[ -n "$LOG_FILE" ]]; then
            hyperfine "${args[@]}" >> "$LOG_FILE" 2>&1
        else
            hyperfine "${args[@]}" >&2
        fi
        local status=$?
        log_msg "hyperfine end: $package status=$status"
        return "$status"
    }

    mean_ms_from_json() {
        python3 -c 'import json, sys; data = json.load(open(sys.argv[1], encoding="utf-8")); command = sys.argv[2]; matches = [result for result in data.get("results", []) if result.get("command") == command]; print(int(round(float(matches[0]["mean"]) * 1000))) if matches else sys.exit(f"missing hyperfine command: {command}")' "$1" "$2"
    }

    declare -a NAMES=() BREW_TIMES=() ZB_COLD_TIMES=() ZB_WARM_TIMES=() SPEEDUPS_COLD=() SPEEDUPS_WARM=() FAILED_NAMES=() FAILED_REASONS=()
    PASSED=0
    FAILED=0

    for i in "${!PACKAGES[@]}"; do
        pkg="${PACKAGES[$i]}"
        idx=$((i + 1))
        echo -e "${CYAN}[$idx/${#PACKAGES[@]}] Testing: $pkg${NORMAL}" >&2
        log_msg "package start: $pkg ($idx/${#PACKAGES[@]})"

        export_json=$(mktemp "${TMPDIR:-/tmp}/zerobrew-hyperfine-${pkg//[^A-Za-z0-9_.-]/_}.XXXXXX")
        brew_prepare="$(shell_join brew uninstall --ignore-dependencies "$pkg") >/dev/null 2>&1 || true; $(shell_join zb uninstall "$pkg") >/dev/null 2>&1 || true; $(shell_join zb reset -y) >/dev/null 2>&1 || true"
        zb_cold_prepare="$(shell_join brew uninstall --ignore-dependencies "$pkg") >/dev/null 2>&1 || true; $(shell_join zb uninstall "$pkg") >/dev/null 2>&1 || true; $(shell_join zb reset -y) >/dev/null 2>&1 || true"
        zb_warm_setup="$(shell_join brew uninstall --ignore-dependencies "$pkg") >/dev/null 2>&1 || true; $(shell_join zb uninstall "$pkg") >/dev/null 2>&1 || true; $(shell_join zb reset -y) >/dev/null 2>&1 || true; $(shell_join zb install "$pkg") >/dev/null 2>&1"
        zb_warm_prepare="$(shell_join zb uninstall "$pkg") >/dev/null 2>&1 || true; $zb_warm_setup; $(shell_join zb uninstall "$pkg") >/dev/null 2>&1 || true"

        brew_cmd=$(append_log_redirect "$(shell_join brew install "$pkg")")
        zb_cmd=$(append_log_redirect "$(shell_join zb install "$pkg")")

        if run_hyperfine_install "$pkg" "$export_json"             "Homebrew" "$brew_prepare" "$brew_cmd"             "Zerobrew (cold)" "$zb_cold_prepare" "$zb_cmd"             "Zerobrew (warm)" "$zb_warm_prepare" "$zb_cmd"; then
            BREW_MS=$(mean_ms_from_json "$export_json" "Homebrew")
            ZB_COLD_MS=$(mean_ms_from_json "$export_json" "Zerobrew (cold)")
            ZB_WARM_MS=$(mean_ms_from_json "$export_json" "Zerobrew (warm)")
            echo -e "    Homebrew: ${GREEN}$(format_duration "$BREW_MS")${NORMAL}" >&2
            echo -e "    ZB cold:  ${GREEN}$(format_duration "$ZB_COLD_MS")${NORMAL}" >&2
            echo -e "    ZB warm:  ${GREEN}$(format_duration "$ZB_WARM_MS")${NORMAL}" >&2
        else
            echo -e "    ${RED}FAILED${NORMAL}" >&2
            FAILED_NAMES+=("$pkg")
            FAILED_REASONS+=("hyperfine benchmark failed")
            ((FAILED++)) || true
            log_msg "package fail: $pkg (hyperfine failed)"
            rm -f "$export_json"
            continue
        fi
        rm -f "$export_json"

        SPEEDUP_COLD=$( [[ $ZB_COLD_MS -gt 0 ]] && echo "scale=2; $BREW_MS / $ZB_COLD_MS" | bc -l | sed 's/^\./0./' || echo "0" )
        SPEEDUP_WARM=$( [[ $ZB_WARM_MS -gt 0 ]] && echo "scale=2; $BREW_MS / $ZB_WARM_MS" | bc -l | sed 's/^\./0./' || echo "0" )

        NAMES+=("$pkg")
        BREW_TIMES+=("$BREW_MS")
        ZB_COLD_TIMES+=("$ZB_COLD_MS")
        ZB_WARM_TIMES+=("$ZB_WARM_MS")
        SPEEDUPS_COLD+=("$SPEEDUP_COLD")
        SPEEDUPS_WARM+=("$SPEEDUP_WARM")
        ((PASSED++)) || true
        log_msg "package done: $pkg"

        zb uninstall "$pkg" &>/dev/null || true
        echo >&2
    done

    echo "Cleaning up..." >&2
    log_msg "cleanup start"
    for pkg in "${PACKAGES[@]}"; do
        brew uninstall --ignore-dependencies "$pkg" 2>/dev/null || true
        zb uninstall "$pkg" 2>/dev/null || true
    done
    zb uninstall 2>/dev/null || true

    TOTAL_BREW=0
    TOTAL_ZB_COLD=0
    TOTAL_ZB_WARM=0

    for i in "${!NAMES[@]}"; do
        TOTAL_BREW=$((TOTAL_BREW + BREW_TIMES[i]))
        TOTAL_ZB_COLD=$((TOTAL_ZB_COLD + ZB_COLD_TIMES[i]))
        TOTAL_ZB_WARM=$((TOTAL_ZB_WARM + ZB_WARM_TIMES[i]))
    done

    if [[ $PASSED -gt 0 ]]; then
        AVG_SPEEDUP_COLD=$( [[ $TOTAL_ZB_COLD -gt 0 ]] && echo "scale=2; $TOTAL_BREW / $TOTAL_ZB_COLD" | bc -l | sed 's/^\./0./' || echo "0" )
        AVG_SPEEDUP_WARM=$( [[ $TOTAL_ZB_WARM -gt 0 ]] && echo "scale=2; $TOTAL_BREW / $TOTAL_ZB_WARM" | bc -l | sed 's/^\./0./' || echo "0" )
    else
        AVG_SPEEDUP_COLD="0"
        AVG_SPEEDUP_WARM="0"
    fi
    log_msg "bench done: passed=$PASSED failed=$FAILED"

    output_text() {
        echo "=== Benchmark Summary ==="
        echo "Tested: ${#PACKAGES[@]} packages"
        echo "Passed: $PASSED"
        echo "Failed: $FAILED"
        echo ""
        echo "Performance:"
        echo "  Average speedup (cold): ${AVG_SPEEDUP_COLD}x"
        echo "  Average speedup (warm): ${AVG_SPEEDUP_WARM}x"
        echo ""
        echo "Results:"
        echo "Package             Homebrew      ZB Cold      ZB Warm   Speed (cold/warm)"
        echo "--------------------------------------------------------------------------"
        for i in "${!NAMES[@]}"; do
            printf "%-15s %12s %12s %12s %10sx / %sx\n" "${NAMES[i]}" "$(format_duration "${BREW_TIMES[i]}")" "$(format_duration "${ZB_COLD_TIMES[i]}")" "$(format_duration "${ZB_WARM_TIMES[i]}")" "${SPEEDUPS_COLD[i]}" "${SPEEDUPS_WARM[i]}"
        done
        echo
        if [[ $FAILED -gt 0 ]]; then
            echo "Failed Packages:"
            for i in "${!FAILED_NAMES[@]}"; do
                echo "  ${FAILED_NAMES[i]} - ${FAILED_REASONS[i]}"
            done
        fi
        echo "Done."
    }

    output_json() {
        printf '{"results":['
        first=1
        for i in "${!NAMES[@]}"; do
            [[ $first -eq 0 ]] && printf ","
            first=0
            printf '{"name":"%s","homebrew_ms":%s,"zerobrew_cold_ms":%s,"zerobrew_warm_ms":%s,"speedup_cold":%s,"speedup_warm":%s}' "${NAMES[i]}" "${BREW_TIMES[i]}" "${ZB_COLD_TIMES[i]}" "${ZB_WARM_TIMES[i]}" "${SPEEDUPS_COLD[i]}" "${SPEEDUPS_WARM[i]}"
        done
        printf '],"failures":['
        first=1
        for i in "${!FAILED_NAMES[@]}"; do
            [[ $first -eq 0 ]] && printf ","
            first=0
            printf '{"name":"%s","reason":"%s"}' "${FAILED_NAMES[i]}" "${FAILED_REASONS[i]}"
        done
        printf '],"summary":{"tested":%d,"passed":%d,"failed":%d,"avg_speedup_cold":%s,"avg_speedup_warm":%s}}\n' "${#PACKAGES[@]}" "$PASSED" "$FAILED" "$AVG_SPEEDUP_COLD" "$AVG_SPEEDUP_WARM"
    }

    output_csv() {
        echo "package,homebrew_ms,zerobrew_cold_ms,zerobrew_warm_ms,speedup_cold,speedup_warm"
        for i in "${!NAMES[@]}"; do
            echo "${NAMES[i]},${BREW_TIMES[i]},${ZB_COLD_TIMES[i]},${ZB_WARM_TIMES[i]},${SPEEDUPS_COLD[i]},${SPEEDUPS_WARM[i]}"
        done
    }

    output_html() {
        echo '<!DOCTYPE html>'
        echo '<html>'
        echo '<head>'
        echo '    <title>Zerobrew Benchmark Results</title>'
        echo '    <style>'
        echo '        body { font-family: -apple-system, BlinkMacSystemFont, '"'"'Segoe UI'"'"', Roboto, sans-serif; margin: 40px; background: #f5f5f5; }'
        echo '        .container { max-width: 1000px; margin: 0 auto; background: white; padding: 30px; border-radius: 8px; box-shadow: 0 2px 4px rgba(0,0,0,0.1); }'
        echo '        h1 { color: #333; border-bottom: 2px solid #0066cc; padding-bottom: 10px; }'
        echo '        .summary { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: 20px; margin: 20px 0; }'
        echo '        .stat { background: #f8f9fa; padding: 20px; border-radius: 8px; text-align: center; }'
        echo '        .stat-value { font-size: 2em; font-weight: bold; color: #0066cc; }'
        echo '        .stat-label { color: #666; margin-top: 5px; }'
        echo '        table { width: 100%; border-collapse: collapse; margin: 20px 0; }'
        echo '        th, td { padding: 12px; text-align: left; border-bottom: 1px solid #ddd; }'
        echo '        th { background: #0066cc; color: white; }'
        echo '        tr:hover { background: #f5f5f5; }'
        echo '        .speedup { font-weight: bold; color: #28a745; }'
        echo '        .failed { color: #dc3545; }'
        echo '        .timestamp { color: #999; font-size: 0.9em; margin-top: 20px; }'
        echo '    </style>'
        echo '</head>'
        echo '<body>'
        echo '    <div class="container">'
        echo '        <h1>Zerobrew Benchmark Results</h1>'
        echo '        <div class="summary">'
        echo '            <div class="stat">'
        echo "                <div class=\"stat-value\">${#PACKAGES[@]}</div>"
        echo '                <div class="stat-label">Packages Tested</div>'
        echo '            </div>'
        echo '            <div class="stat">'
        echo "                <div class=\"stat-value\">$PASSED</div>"
        echo '                <div class="stat-label">Passed</div>'
        echo '            </div>'
        echo '            <div class="stat">'
        echo "                <div class=\"stat-value\">${AVG_SPEEDUP_COLD}x</div>"
        echo '                <div class="stat-label">Avg Cold Speedup</div>'
        echo '            </div>'
        echo '            <div class="stat">'
        echo "                <div class=\"stat-value\">${AVG_SPEEDUP_WARM}x</div>"
        echo '                <div class="stat-label">Avg Warm Speedup</div>'
        echo '            </div>'
        echo '        </div>'
        echo '        <h2>Results</h2>'
        echo '        <table>'
        echo '            <thead>'
        echo '                <tr>'
        echo '                    <th>Package</th>'
        echo '                    <th>Homebrew</th>'
        echo '                    <th>ZB Cold</th>'
        echo '                    <th>ZB Warm</th>'
        echo '                    <th>Speedup (cold/warm)</th>'
        echo '                </tr>'
        echo '            </thead>'
        echo '            <tbody>'
        for i in "${!NAMES[@]}"; do
            echo "                <tr>"
            echo "                    <td>${NAMES[$i]}</td>"
            echo "                    <td>$(format_duration "${BREW_TIMES[$i]}")</td>"
            echo "                    <td>$(format_duration "${ZB_COLD_TIMES[$i]}")</td>"
            echo "                    <td>$(format_duration "${ZB_WARM_TIMES[$i]}")</td>"
            echo "                    <td class=\"speedup\">${SPEEDUPS_COLD[$i]}x / ${SPEEDUPS_WARM[$i]}x</td>"
            echo "                </tr>"
        done
        echo '            </tbody>'
        echo '        </table>'
        if [[ $FAILED -gt 0 ]]; then
            echo '        <h2>Failed Packages</h2>'
            echo '        <ul>'
            for i in "${!FAILED_NAMES[@]}"; do
                echo "            <li class=\"failed\"><strong>${FAILED_NAMES[$i]}</strong>: ${FAILED_REASONS[$i]}</li>"
            done
            echo '        </ul>'
        fi
        TIMESTAMP=$(date)
        echo "        <div class=\"timestamp\">Generated: $TIMESTAMP</div>"
        echo '    </div>'
        echo '</body>'
        echo '</html>'
    }

    output_result() {
        case "$FORMAT" in
            text) output_text ;;
            json) output_json ;;
            csv)  output_csv ;;
            html) output_html ;;
        esac
    }

    if [[ -n "$FULL_OUTPUT_DIR" ]]; then
        # Output all formats to the specified directory
        mkdir -p "$FULL_OUTPUT_DIR"

        BASE_NAME="benchmark"

        echo -e "${CYAN}Writing all formats to: $FULL_OUTPUT_DIR${NORMAL}" >&2

        FORMAT="text" output_result > "$FULL_OUTPUT_DIR/${BASE_NAME}.txt"
        echo -e "${GREEN}  ✓ ${BASE_NAME}.txt${NORMAL}" >&2

        FORMAT="json" output_result > "$FULL_OUTPUT_DIR/${BASE_NAME}.json"
        echo -e "${GREEN}  ✓ ${BASE_NAME}.json${NORMAL}" >&2

        FORMAT="csv" output_result > "$FULL_OUTPUT_DIR/${BASE_NAME}.csv"
        echo -e "${GREEN}  ✓ ${BASE_NAME}.csv${NORMAL}" >&2

        FORMAT="html" output_result > "$FULL_OUTPUT_DIR/${BASE_NAME}.html"
        echo -e "${GREEN}  ✓ ${BASE_NAME}.html${NORMAL}" >&2

        echo -e "${GREEN}All results written to: $FULL_OUTPUT_DIR${NORMAL}" >&2
    elif [[ -n "$OUTPUT" ]]; then
        output_result > "$OUTPUT"
        echo -e "${GREEN}Results written to: $OUTPUT${NORMAL}" >&2
    else
        output_result
    fi
