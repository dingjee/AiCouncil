#!/usr/bin/env bash
# MM-F: native smoke — real Tauri webview + real IPC verification (debug only).
# - debug build with embedded frontend (custom protocol), never release
# - isolated AICOUNCIL_DATA_DIR + fake .env fixture (sk-fixture-*), strict
#   isolation (no exe/CWD fallback), real user data untouched
# - local OpenRouter-compatible mock via AICOUNCIL_BASE_URL (zero real spend)
# - the app injects src/native_smoke.rs on page load; the results JSON lands
#   in the isolated data dir; this script asserts it with Bun and fails loudly.
#
# Gate design (2026-10-28, after the "no report" incident):
#   0/6  parse the harness JS with a real JS parser BEFORE launching — a
#        syntax error in the harness makes the whole injected script
#        unparsable, so nothing runs (and the app still logs "injected").
#   1/6  frontend build (embedded assets)
#   2/6  debug build
#   3/6  isolated data dir + fake .env fixture + local mock
#   4/6  launch the app (pid is killed afterwards)
#   5/6  wait for the final report (bounded, ~50 s)
#   6/6  assert the report (all required steps, no failures)
#
# Bounded: the harness has its own 45 s hard timeout, the driver waits at
# most ~50 s for the report, so one run never exceeds ~60 s of wall clock
# after the app is launched.
#
# Usage: scripts/macos_native_smoke.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="/opt/homebrew/bin:$HOME/.cargo/bin:$PATH"

command -v bun >/dev/null || { echo "FAIL: bun not found" >&2; exit 1; }
command -v cargo >/dev/null || { echo "FAIL: cargo not found" >&2; exit 1; }

# 报告等待上限(秒)。harness 内部硬超时 45s,这里留 5s 余量。
REPORT_WAIT_SECS="${REPORT_WAIT_SECS:-50}"

echo "== 0/6 harness syntax gate (real JS parser) =="
# 注入脚本的语法错误会让整段脚本无法解析:必须在启动 app 之前拦住。
bun -e '
const fs = require("fs");
const rust = fs.readFileSync("src/native_smoke.rs", "utf8");
const m = rust.match(/pub fn page_load_script\(\) -> &'"'"'static str \{\s*r#"(.*)"#\s*\}/s);
if (!m) { console.error("FAIL: harness raw string not found in src/native_smoke.rs"); process.exit(1); }
fs.writeFileSync("/tmp/aicouncil-harness.js", m[1]);
' || { echo "FAIL: could not extract the harness JS" >&2; exit 1; }
bun build /tmp/aicouncil-harness.js --target=browser --outfile /tmp/aicouncil-harness.bundle.js \
  || { echo "FAIL: harness JS has a syntax error (see the parser output above)" >&2; exit 1; }
rm -f /tmp/aicouncil-harness.js /tmp/aicouncil-harness.bundle.js
echo "harness JS parses cleanly"

echo "== 1/6 frontend build (embedded assets) =="
( cd frontend && bun install --frozen-lockfile && bun run build )

echo "== 2/6 debug build =="
cargo build --locked

echo "== 3/6 isolated data dir + fixtures + local mock =="
WORK="$(mktemp -d "${TMPDIR:-/tmp}/aicouncil-native-smoke.XXXXXX")"
DATA="$WORK/data"
mkdir -p "$DATA"
printf 'OPENROUTER_API_KEY=sk-fixture-smoke-or\nDEEPSEEK_API_KEY=sk-fixture-smoke-ds\n' > "$DATA/.env"

MOCK_PORT=$((18901 + RANDOM % 400))
export MOCK_PORT
MOCK_LOG="$WORK/mock.log"
bun scripts/native_mock_server.ts >"$MOCK_LOG" 2>&1 &
MOCK_PID=$!
for _ in $(seq 1 50); do
    grep -q MOCK_READY "$MOCK_LOG" 2>/dev/null && break
    sleep 0.2
done
grep -q MOCK_READY "$MOCK_LOG" || { echo "FAIL: mock server did not start" >&2; cat "$MOCK_LOG" >&2; kill "$MOCK_PID" 2>/dev/null || true; exit 1; }

APP_LOG="$WORK/app.log"
export AICOUNCIL_DATA_DIR="$DATA"
export AICOUNCIL_NATIVE_SMOKE=1
export AICOUNCIL_BASE_URL="http://127.0.0.1:$MOCK_PORT/v1"
# 标题服务也指向本地 mock(假 DEEPSEEK_API_KEY 会启用标题服务;
# 绝不让测试流量到达真实 api.deepseek.com)
export AICOUNCIL_TITLE_BASE_URL="http://127.0.0.1:$MOCK_PORT"
unset AICOUNCIL_ENV_PATH 2>/dev/null || true

REPORT="$DATA/native_smoke_report.json"
APP_PID=""

cleanup() {
    [[ -n "$APP_PID" ]] && kill -9 "$APP_PID" 2>/dev/null || true
    kill "$MOCK_PID" 2>/dev/null || true
    # 不阻塞:短暂等待后强杀(慢速 mock 连接可能延迟 SIGTERM 生效)
    for _ in 1 2 3 4 5; do
        kill -0 "$MOCK_PID" 2>/dev/null || break
        sleep 0.2
    done
    kill -9 "$MOCK_PID" 2>/dev/null || true
    if [[ -f "$REPORT" ]] && grep -q '"finished_at"' "$REPORT" 2>/dev/null; then
        rm -rf "$WORK"   # 成功(最终报告):临时目录移除
    else
        echo "KEEP (failure logs until cause understood): $WORK (app.log mock.log report)" >&2
    fi
}
trap cleanup EXIT

echo "== 4/6 launch app (pid will be killed; isolated data: $DATA) =="
./target/debug/aicouncil >"$APP_LOG" 2>&1 &
APP_PID=$!

echo "== 5/6 wait for the final report (max ${REPORT_WAIT_SECS}s) =="
DEADLINE=$(( $(date +%s) + REPORT_WAIT_SECS ))
while :; do
    # 等待**完成**的报告:finished_at 只在 harness 全部步骤结束时写入
    if [[ -f "$REPORT" ]] && grep -q '"finished_at"' "$REPORT" 2>/dev/null; then
        break
    fi
    if ! kill -0 "$APP_PID" 2>/dev/null; then
        echo "FAIL: app exited before reporting" >&2
        tail -40 "$APP_LOG" >&2
        exit 1
    fi
    if (( $(date +%s) >= DEADLINE )); then
        break
    fi
    sleep 0.2
done
kill -9 "$APP_PID" 2>/dev/null || true
APP_PID=""

if [[ ! -f "$REPORT" ]]; then
    echo "FAIL: no native_smoke_report.json after ${REPORT_WAIT_SECS}s" >&2
    echo "--- app log (tail) ---" >&2
    tail -40 "$APP_LOG" >&2
    exit 1
fi
if ! grep -q '"finished_at"' "$REPORT" 2>/dev/null; then
    echo "FAIL: report is not final (no finished_at) — the harness did not finish" >&2
    echo "--- partial report ---" >&2
    cat "$REPORT" >&2
    echo "--- app log (tail) ---" >&2
    tail -40 "$APP_LOG" >&2
    exit 1
fi

echo "== 6/6 assert report =="
cat "$REPORT"
bun -e '
const r = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
const fails = r.steps.filter((s) => !s.ok);
const required = [
  "bootstrap", "create_group", "create_conversation", "rename_conversation",
  "set_conversation_group", "set_system_prompt", "draft_roundtrip",
  "update_settings", "list_conversations", "load_conversation",
  "error_not_found_code", "refresh_catalog", "set_model_preference",
  "list_preferences", "check_key_status", "send_message_single",
  "stream_persists_and_completes", "send_message_slow", "cancel_request",
  "export_markdown", "reload_credentials", "geometry_native_layout",
  "ime_composition_enter_gate", "dom_renders_conversation",
];
const missing = required.filter((n) => !r.steps.some((s) => s.name === n));
if (!r.enabled) { console.error("FAIL: harness not enabled in webview"); process.exit(1); }
if (!r.finished_at) { console.error("FAIL: report not final (no finished_at)"); process.exit(1); }
if (missing.length) { console.error("FAIL: missing steps:", missing.join(", ")); process.exit(1); }
if (fails.length) {
  console.error("FAIL:", fails.map((f) => `${f.name}: ${f.error}`).join(" | "));
  process.exit(1);
}
console.log("NATIVE_SMOKE_OK steps:", r.steps.length);
' "$REPORT"

echo "OK: macos_native_smoke (report printed above; temp dir removed at exit)"
