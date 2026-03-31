#!/bin/bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CH8_DIR="$ROOT_DIR/tg-rcore-tutorial-ch8"
OUT_ROOT="$ROOT_DIR/target/t4-phase0"

RUN_DIR=""
METRICS_FILE=""
SUMMARY_FILE=""
QEMU_LOG=""
BUILD_SHELL_LOG=""

CHECK_STATUS="pending"
BASE_STATUS="pending"
EXERCISE_STATUS="pending"
SHELL_STATUS="pending"
DOOM_STATUS="pending"

CHECK_DURATION_MS=""
BASE_DURATION_MS=""
EXERCISE_DURATION_MS=""
SHELL_BOOT_MS=""
RACE_ADDER_REPORTED_MS=""
RACE_ADDER_WALL_MS=""
DOOM_LAUNCH_MS=""
DOOM_FIRST_PRESENT_MS=""
DOOM_MODE=""

timestamp() {
    date "+%Y%m%d-%H%M%S"
}

now_ms() {
    python3 -c 'import time; print(time.time_ns() // 1_000_000)'
}

ensure_out_dir() {
    if [[ -n "$RUN_DIR" ]]; then
        mkdir -p "$RUN_DIR"
        return
    fi

    if [[ -n "${T4_PHASE0_RUN_DIR:-}" ]]; then
        RUN_DIR="$T4_PHASE0_RUN_DIR"
    else
        RUN_DIR="$OUT_ROOT/$(timestamp)"
    fi
    mkdir -p "$RUN_DIR"
    METRICS_FILE="$RUN_DIR/metrics.tsv"
    SUMMARY_FILE="$RUN_DIR/summary.md"
    QEMU_LOG="$RUN_DIR/shell-session.log"
    BUILD_SHELL_LOG="$RUN_DIR/build-shell.log"
    if [[ ! -f "$METRICS_FILE" ]]; then
        printf "metric\tvalue\tunit\tnotes\n" > "$METRICS_FILE"
    fi
}

record_metric() {
    ensure_out_dir
    printf "%s\t%s\t%s\t%s\n" "$1" "$2" "${3:-}" "${4:-}" >> "$METRICS_FILE"
}

write_template() {
    ensure_out_dir
    cat > "$RUN_DIR/baseline-template.md" <<EOF
# T4 Phase 0 Baseline

- Generated at: $(date "+%Y-%m-%d %H:%M:%S %Z")
- Repo root: $ROOT_DIR
- Run dir: $RUN_DIR

## Environment

- Git commit: $(git -C "$ROOT_DIR" rev-parse --short HEAD)
- Rustc: $(rustc --version 2>/dev/null || echo "unavailable")
- Cargo: $(cargo --version 2>/dev/null || echo "unavailable")
- QEMU: $(qemu-system-riscv64 --version 2>/dev/null | head -n 1 || echo "unavailable")

## Automated Checks

- cargo check:
- ch8 base test:
- ch8 exercise test:

## Automated Shell Metrics

- shell boot:
- race_adder_mutex_blocking:
- doom launch:
- doom first present:

## Manual Notes

- fb_demo visual correctness:
- doomgeneric visual correctness:
- shell startup:
- extra remarks:

## Metrics

| Item | Result | Notes |
| --- | --- | --- |
| cargo check | pending | |
| ch8 base test | pending | |
| ch8 exercise test | pending | |
| shell boot | pending | |
| race_adder_mutex_blocking | pending | |
| doom launch | pending | |
| doom first present | pending | |
EOF
}

write_summary() {
    ensure_out_dir
    cat > "$SUMMARY_FILE" <<EOF
# T4 Phase 0 Summary

- Generated at: $(date "+%Y-%m-%d %H:%M:%S %Z")
- Run dir: $RUN_DIR

## Status

- cargo check: $CHECK_STATUS
- ch8 base test: $BASE_STATUS
- ch8 exercise test: $EXERCISE_STATUS
- shell metrics: $SHELL_STATUS
- doom test: $DOOM_STATUS

## Durations

- cargo check: ${CHECK_DURATION_MS:-pending} ms
- ch8 base test: ${BASE_DURATION_MS:-pending} ms
- ch8 exercise test: ${EXERCISE_DURATION_MS:-pending} ms
- shell boot: ${SHELL_BOOT_MS:-pending} ms
- race_adder reported: ${RACE_ADDER_REPORTED_MS:-pending} ms
- race_adder wall: ${RACE_ADDER_WALL_MS:-pending} ms
- doom launch: ${DOOM_LAUNCH_MS:-pending} ms
- doom first present: ${DOOM_FIRST_PRESENT_MS:-pending} ms

## Doom

- mode: ${DOOM_MODE:-pending}

## Files

- env: [env.txt]($RUN_DIR/env.txt)
- git status: [git-status.txt]($RUN_DIR/git-status.txt)
- check log: [check.log]($RUN_DIR/check.log)
- base log: [test-base.log]($RUN_DIR/test-base.log)
- exercise log: [test-exercise.log]($RUN_DIR/test-exercise.log)
- shell build log: [build-shell.log]($RUN_DIR/build-shell.log)
- shell session log: [shell-session.log]($RUN_DIR/shell-session.log)
- metrics: [metrics.tsv]($RUN_DIR/metrics.tsv)
EOF
}

capture_metadata() {
    ensure_out_dir
    {
        echo "date: $(date "+%Y-%m-%d %H:%M:%S %Z")"
        echo "repo: $ROOT_DIR"
        echo "git_commit: $(git -C "$ROOT_DIR" rev-parse HEAD)"
        echo "git_commit_short: $(git -C "$ROOT_DIR" rev-parse --short HEAD)"
        echo "rustc: $(rustc --version 2>/dev/null || echo unavailable)"
        echo "cargo: $(cargo --version 2>/dev/null || echo unavailable)"
        echo "qemu: $(qemu-system-riscv64 --version 2>/dev/null | head -n 1 || echo unavailable)"
        echo "uname: $(uname -a 2>/dev/null || echo unavailable)"
    } > "$RUN_DIR/env.txt"

    git -C "$ROOT_DIR" status --short > "$RUN_DIR/git-status.txt"
    write_template
    record_metric "git_commit_short" "$(git -C "$ROOT_DIR" rev-parse --short HEAD)" "" "phase0 baseline commit"
    echo "Phase 0 metadata captured in: $RUN_DIR"
}

run_check() {
    ensure_out_dir
    local start end
    start="$(now_ms)"
    if cargo check --manifest-path "$CH8_DIR/Cargo.toml" --offline 2>&1 | tee "$RUN_DIR/check.log"; then
        CHECK_STATUS="pass"
    else
        CHECK_STATUS="fail"
        write_summary
        return 1
    fi
    end="$(now_ms)"
    CHECK_DURATION_MS="$((end - start))"
    record_metric "cargo_check_duration" "$CHECK_DURATION_MS" "ms" "cargo check --manifest-path tg-rcore-tutorial-ch8/Cargo.toml --offline"
    echo "cargo check log saved to: $RUN_DIR/check.log"
    write_summary
}

run_base_test() {
    ensure_out_dir
    local start end
    start="$(now_ms)"
    if (
        cd "$CH8_DIR"
        bash ./test.sh base
    ) 2>&1 | tee "$RUN_DIR/test-base.log"; then
        BASE_STATUS="pass"
    else
        BASE_STATUS="fail"
        write_summary
        return 1
    fi
    end="$(now_ms)"
    BASE_DURATION_MS="$((end - start))"
    record_metric "ch8_base_test_duration" "$BASE_DURATION_MS" "ms" "bash ./test.sh base"
    echo "base test log saved to: $RUN_DIR/test-base.log"
    write_summary
}

run_exercise_test() {
    ensure_out_dir
    local start end
    start="$(now_ms)"
    if (
        cd "$CH8_DIR"
        bash ./test.sh exercise
    ) 2>&1 | tee "$RUN_DIR/test-exercise.log"; then
        EXERCISE_STATUS="pass"
    else
        EXERCISE_STATUS="fail"
        write_summary
        return 1
    fi
    end="$(now_ms)"
    EXERCISE_DURATION_MS="$((end - start))"
    record_metric "ch8_exercise_test_duration" "$EXERCISE_DURATION_MS" "ms" "bash ./test.sh exercise"
    echo "exercise test log saved to: $RUN_DIR/test-exercise.log"
    write_summary
}

build_shell_image() {
    ensure_out_dir
    local start end
    start="$(now_ms)"
    (
        cd "$CH8_DIR"
        cargo clean
        TG_CH8_DEFAULT_APP=user_shell CHAPTER=0 cargo build --offline
    ) > "$BUILD_SHELL_LOG" 2>&1
    end="$(now_ms)"
    record_metric "shell_image_build_duration" "$((end - start))" "ms" "cargo clean && TG_CH8_DEFAULT_APP=user_shell CHAPTER=0 cargo build --offline"
}

run_shell_metrics() {
    ensure_out_dir
    build_shell_image
    : > "$QEMU_LOG"

    python3 - "$CH8_DIR" "$QEMU_LOG" "$RUN_DIR/shell-metrics.env" <<'PY'
import os
import re
import selectors
import signal
import subprocess
import sys
import time

ch8_dir, log_path, env_path = sys.argv[1:4]

qemu_cmd = [
    "qemu-system-riscv64",
    "-machine", "virt",
    "-serial", "stdio",
    "-monitor", "none",
    "-display", "none",
    "-bios", "none",
    "-m", "128M",
    "-drive", "file=target/riscv64gc-unknown-none-elf/debug/fs.img,if=none,format=raw,id=x0",
    "-device", "virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0",
    "-device", "virtio-gpu-device,bus=virtio-mmio-bus.1",
    "-kernel", "target/riscv64gc-unknown-none-elf/debug/jiaxin2006-tg-rcore-tutorial-t1l5",
]

metrics = {
    "SHELL_STATUS": "fail",
    "DOOM_STATUS": "fail",
    "SHELL_BOOT_MS": "",
    "RACE_ADDER_REPORTED_MS": "",
    "RACE_ADDER_WALL_MS": "",
    "DOOM_LAUNCH_MS": "",
    "DOOM_FIRST_PRESENT_MS": "",
    "DOOM_MODE": "",
}

def now_ms():
    return time.time_ns() // 1_000_000

def wait_for_pattern(selector, proc, log_file, pattern, timeout_ms):
    deadline = now_ms() + timeout_ms
    buffer = ""
    while now_ms() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"qemu exited unexpectedly while waiting for {pattern!r}")
        events = selector.select(timeout=0.2)
        for key, _ in events:
            chunk = os.read(key.fd, 4096).decode(errors="replace")
            if not chunk:
                continue
            log_file.write(chunk)
            log_file.flush()
            buffer += chunk
            if pattern in buffer:
                return now_ms(), buffer
    raise TimeoutError(f"timeout while waiting for {pattern!r}")

def send_command(proc, log_file, command):
    log_file.write(f"[host] >>> {command}\n")
    log_file.flush()
    proc.stdin.write((command + "\n").encode())
    proc.stdin.flush()
    return now_ms()

proc = subprocess.Popen(
    qemu_cmd,
    cwd=ch8_dir,
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.STDOUT,
    bufsize=0,
)

selector = selectors.DefaultSelector()
selector.register(proc.stdout, selectors.EVENT_READ)

with open(log_path, "w", encoding="utf-8") as log_file:
    try:
        qemu_start = now_ms()
        wait_for_pattern(selector, proc, log_file, "Rust user shell", 30000)
        prompt_ms, _ = wait_for_pattern(selector, proc, log_file, ">> ", 5000)
        metrics["SHELL_STATUS"] = "pass"
        metrics["SHELL_BOOT_MS"] = str(prompt_ms - qemu_start)

        cmd_start = send_command(proc, log_file, "race_adder_mutex_blocking")
        _, race_buf = wait_for_pattern(selector, proc, log_file, "time cost is ", 60000)
        m = re.search(r"time cost is (\d+)ms", race_buf)
        if m:
            metrics["RACE_ADDER_REPORTED_MS"] = m.group(1)
        exit_ms, _ = wait_for_pattern(selector, proc, log_file, "Shell: Process", 30000)
        metrics["RACE_ADDER_WALL_MS"] = str(exit_ms - cmd_start)

        cmd_start = send_command(proc, log_file, "doomgeneric")
        launch_ms, launch_buf = wait_for_pattern(selector, proc, log_file, "[doomgeneric] framebuffer:", 30000)
        metrics["DOOM_LAUNCH_MS"] = str(launch_ms - cmd_start)
        mode_ms, mode_buf = wait_for_pattern(selector, proc, log_file, "[doomgeneric] mode=", 5000)
        m = re.search(r"\[doomgeneric\] mode=([^\r\n ]+)", mode_buf)
        if m:
            metrics["DOOM_MODE"] = m.group(1)
        first_present_ms, _ = wait_for_pattern(selector, proc, log_file, "virtio-gpu: first present", 30000)
        metrics["DOOM_FIRST_PRESENT_MS"] = str(first_present_ms - cmd_start)
        metrics["DOOM_STATUS"] = "pass"
    except Exception:
        with open(env_path, "w", encoding="utf-8") as f:
            for k, v in metrics.items():
                f.write(f"{k}={v}\n")
        raise
    finally:
        try:
            proc.send_signal(signal.SIGTERM)
            proc.wait(timeout=5)
        except Exception:
            proc.kill()
            proc.wait(timeout=5)

with open(env_path, "w", encoding="utf-8") as f:
    for k, v in metrics.items():
        f.write(f"{k}={v}\n")
PY

    # shellcheck disable=SC1090
    source "$RUN_DIR/shell-metrics.env"
    if [[ -z "${DOOM_MODE:-}" ]]; then
        DOOM_MODE="$(sed -n 's/.*\[doomgeneric\] mode=\([^ ]*\).*/\1/p' "$QEMU_LOG" | tail -n 1)"
    fi
    if [[ -z "${DOOM_MODE:-}" ]]; then
        DOOM_MODE="unknown"
    fi
    record_metric "shell_boot_duration" "${SHELL_BOOT_MS:-timeout}" "ms" "qemu start -> Rust user shell"
    record_metric "race_adder_reported" "${RACE_ADDER_REPORTED_MS:-timeout}" "ms" "guest reported time cost"
    record_metric "race_adder_wall" "${RACE_ADDER_WALL_MS:-timeout}" "ms" "shell command -> process exit"
    record_metric "doom_launch" "${DOOM_LAUNCH_MS:-timeout}" "ms" "shell command -> first doom framebuffer log"
    record_metric "doom_mode" "${DOOM_MODE:-unknown}" "" "doomgeneric startup mode"
    record_metric "doom_first_present" "${DOOM_FIRST_PRESENT_MS:-timeout}" "ms" "shell command -> first virtio-gpu present"
    write_summary
}

show_help() {
    cat <<EOF
Usage: bash scripts/t4-phase0-baseline.sh <command>

Commands:
  metadata       Capture environment and git baseline information
  check          Run cargo check for tg-rcore-tutorial-ch8
  base           Run ch8 base test
  exercise       Run ch8 exercise test
  shell-metrics  Build CHAPTER=0 image, run shell session, and record fb_demo / race_adder / doom metrics
  all            Run metadata + check + base + exercise + shell-metrics in one run directory

Optional:
  T4_PHASE0_RUN_DIR=/path/to/output  Reuse a specific output directory
EOF
}

case "${1:-help}" in
    metadata)
        capture_metadata
        write_summary
        ;;
    check)
        run_check
        ;;
    base)
        run_base_test
        ;;
    exercise)
        run_exercise_test
        ;;
    shell-metrics)
        run_shell_metrics
        ;;
    all)
        capture_metadata
        run_check
        run_base_test
        run_exercise_test
        run_shell_metrics
        write_summary
        ;;
    help|-h|--help)
        show_help
        ;;
    *)
        show_help
        exit 1
        ;;
esac
