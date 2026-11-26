#!/usr/bin/env bash
# Mäter instruktioner (valgrind --tool=callgrind) för prestandaproven `perfci_*` i src/perfci.rs.
#
#   perf/run.sh UT_MAPP [KÄLLMAPP]
#
# Bygger provbinären i KÄLLMAPP (standard: repots rot), skapar indata, kör varje prov under callgrind
# (flera samtidigt, antalet kan sättas med PERF_JOBS) och skriver UT_MAPP/ir.json: {"prov": instruktioner}.
# Siffrorna är instruktionsräknare och därför oberoende av hur belastad maskinen är; jämför bara siffror från
# samma maskin, samma kompilator och samma valgrind (perf/compare.py används på två körningar i samma jobb).
set -euo pipefail

out=$(realpath -m "${1:?ange utkatalog}")
src=$(realpath "${2:-$(dirname "$0")/..}")
mkdir -p "$out"
cd "$src"

# Optimerat provbygge (opt-level 2, utan debuginfo): nära release men med snabbare byggtid.
export CARGO_TARGET_DIR="${PERF_TARGET_DIR:-$src/target-perf}"
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_DEV_OPT_LEVEL=2
export KLADD_PERFCI_DIR="$out/fixtures"

echo "::group::Bygg provbinären ($src)" >&2
bin=$(cargo test --locked --no-run --message-format=json 2>"$out/build.log" |
  python3 -c '
import json, sys
exe = None
for line in sys.stdin:
    try:
        m = json.loads(line)
    except ValueError:
        continue
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == "kladd" and m.get("profile", {}).get("test"):
        exe = m["executable"]
print(exe or "")
') || { cat "$out/build.log" >&2; exit 1; }
tail -n 3 "$out/build.log" >&2
echo "::endgroup::" >&2
[ -n "$bin" ] || { echo "Hittade ingen provbinär" >&2; exit 1; }

# Proven körs med egna användarkataloger så att inga inställningar läcker in.
export HOME="$out/home" XDG_RUNTIME_DIR="$out/run" XDG_DATA_HOME="$out/home/.local/share" XDG_CONFIG_HOME="$out/home/.config"
mkdir -p "$HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

all=$("$bin" --ignored --list 2>/dev/null | sed -n 's/^\(.*perfci_[a-z0-9_]*\): test$/\1/p')
fx=$(grep 'perfci_fixtures$' <<<"$all" || true)
[ -n "$fx" ] || { echo "Hittade inte perfci_fixtures" >&2; exit 1; }
"$bin" --ignored --exact "$fx" --test-threads=1 >"$out/fixtures.log" 2>&1 || { cat "$out/fixtures.log" >&2; exit 1; }
[ -f "$KLADD_PERFCI_DIR/stor.docx" ] || { echo "Indata skapades inte" >&2; exit 1; }

mapfile -t tests < <(grep -v 'perfci_fixtures$' <<<"$all" | sort)
[ "${#tests[@]}" -gt 0 ] || { echo "Inga perfci_-prov hittades" >&2; exit 1; }

run_one() {
  local t=$1 name=${1##*::}
  # Proven slår själva på räkningen kring mätningen (callgrind-förfrågan, se src/perfci.rs).
  local flags=(--collect-atstart=no)
  # Prov som arbetar i trådar (_mt) räknas för hela processen: förfrågan gäller bara den tråd som anropar.
  [[ $name == *_mt ]] && flags=(--collect-atstart=yes)
  valgrind --tool=callgrind --callgrind-out-file="$out/$name.callgrind" --log-file="$out/$name.valgrind.log" "${flags[@]}" \
    "$bin" --ignored --exact "$t" --test-threads=1 >"$out/$name.log" 2>&1 || { echo "$name misslyckades:" >&2; tail -n 20 "$out/$name.log" >&2; return 1; }
  local ir
  ir=$(sed -n 's/^totals: *//p' "$out/$name.callgrind" | head -n 1)
  [ -n "$ir" ] && [ "$ir" != 0 ] || { echo "$name: inga instruktioner räknades (se $out/$name.valgrind.log)" >&2; return 1; }
  echo "$name $ir"
}
export -f run_one
export bin out

printf '%s\n' "${tests[@]}" | xargs -P "${PERF_JOBS:-4}" -I{} bash -c 'run_one {}' | sort >"$out/ir.txt"
python3 - "$out/ir.txt" "$out/ir.json" <<'PY'
import json, sys
d = {}
for line in open(sys.argv[1]):
    k, v = line.split()
    d[k] = int(v)
json.dump(d, open(sys.argv[2], "w"), indent=1, sort_keys=True)
PY
cat "$out/ir.txt" >&2
