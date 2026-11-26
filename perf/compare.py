#!/usr/bin/env python3
"""Jämför två instruktionsmätningar från perf/run.sh och skriver en tabell.

    perf/compare.py BAS.json PR.json [--fail 10] [--warn 3] [--summary FIL]

Fel (utgångskod 1) om något prov ökar mer än --fail procent, varning över --warn procent. Tabellen skrivs
som Markdown till --summary (i CI: $GITHUB_STEP_SUMMARY) och till stdout; fel och varningar skrivs också som
GitHub-annotationer. Prov som bara finns i ena filen redovisas men räknas inte som fel.
"""
import argparse
import json
import sys


def fmt(n):
    return f"{n:,}".replace(",", " ")


def compare(base, head, fail, warn):
    rows, worst, level = [], 0.0, 0  # level: 0 ok, 1 varning, 2 fel
    notes = []
    for name in sorted(set(base) | set(head)):
        b, h = base.get(name), head.get(name)
        if b is None:
            rows.append((name, "-", fmt(h), "ny", "ny"))
            continue
        if h is None:
            rows.append((name, fmt(b), "-", "saknas", "saknas"))
            notes.append(("warning", f"{name}: provet finns inte längre i PR:en"))
            continue
        pct = (h - b) / b * 100 if b else 0.0
        state = "ok"
        if pct > fail:
            state, level = "FEL", 2
            notes.append(("error", f"{name}: {pct:+.1f} % instruktioner (gräns {fail:g} %)"))
        elif pct > warn:
            state, level = "varning", max(level, 1)
            notes.append(("warning", f"{name}: {pct:+.1f} % instruktioner (varning över {warn:g} %)"))
        elif pct < -warn:
            state = "snabbare"
        worst = max(worst, pct)
        rows.append((name, fmt(b), fmt(h), f"{pct:+.2f} %", state))
    return rows, notes, level, worst


def table(rows, fail, warn):
    out = ["### Prestandaprov (instruktioner, callgrind)", "", "| Prov | Bas | PR | Ändring | Status |", "|---|--:|--:|--:|---|"]
    out += [f"| `{n}` | {b} | {h} | {c} | {s} |" for n, b, h, c, s in rows]
    out += ["", f"Fel över {fail:g} % fler instruktioner, varning över {warn:g} %. Instruktioner är deterministiska: samma kod ger samma tal oavsett maskinens belastning."]
    return "\n".join(out) + "\n"


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("base")
    ap.add_argument("head")
    ap.add_argument("--fail", type=float, default=10.0)
    ap.add_argument("--warn", type=float, default=3.0)
    ap.add_argument("--summary")
    a = ap.parse_args(argv)
    base, head = json.load(open(a.base)), json.load(open(a.head))
    rows, notes, level, _ = compare(base, head, a.fail, a.warn)
    text = table(rows, a.fail, a.warn)
    if level == 2:
        text += "\n**Minst ett prov blev långsammare än gränsen.** Förklara eller åtgärda ökningen; om den är avsiktlig, motivera den i PR:en.\n"
    print(text)
    if a.summary:
        with open(a.summary, "a") as f:
            f.write(text)
    for kind, msg in notes:
        print(f"::{kind}::{msg}")
    return 1 if level == 2 else 0


if __name__ == "__main__":
    sys.exit(main())
