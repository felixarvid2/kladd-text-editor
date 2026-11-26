#!/usr/bin/env bash
# Lägger PR:ens prestandaprov (src/perfci.rs) över en annan trädversion (basen) så att båda mäts med exakt
# samma prov, även när basen inte har proven än. Idempotent.
#
#   perf/overlay.sh BASMAPP
set -euo pipefail
here=$(realpath "$(dirname "$0")/..")
base=$(realpath "${1:?ange basmapp}")
cp "$here/src/perfci.rs" "$base/src/perfci.rs"
cp "$here/perf/run.sh" "$base/perf/run.sh" 2>/dev/null || { mkdir -p "$base/perf"; cp "$here/perf/run.sh" "$base/perf/run.sh"; }
# Provmodulen deklareras bredvid ui_tests i app.rs (där finns åtkomst till gränssnittsharnessen).
if ! grep -q 'path = "perfci.rs"' "$base/src/app.rs"; then
  python3 - "$base/src/app.rs" <<'PY'
import sys
p = sys.argv[1]
s = open(p).read()
a = 'mod ui_tests;\n'
assert a in s, "hittar inte ui_tests i app.rs"
s = s.replace(a, a + '#[cfg(test)]\n#[path = "perfci.rs"]\nmod perfci;\n', 1)
open(p, "w").write(s)
PY
fi
# Hjälpfunktionerna provet använder ska vara synliga i hela kratet.
sed -i 's/^pub(super) fn frame(c: &mut layout::Cache/pub(crate) fn frame(c: \&mut layout::Cache/; s/^fn frame(c: &mut layout::Cache/pub(crate) fn frame(c: \&mut layout::Cache/; s/^pub(super) fn in_ui(ctx/pub(crate) fn in_ui(ctx/; s/^fn in_ui(ctx/pub(crate) fn in_ui(ctx/' "$base/src/word/bench.rs"
