#!/usr/bin/env python3
"""Prov för perf/compare.py: python3 perf/test_compare.py"""
import json, os, subprocess, sys, tempfile, unittest

HERE = os.path.dirname(os.path.abspath(__file__))


def run(base, head, *extra):
    with tempfile.TemporaryDirectory() as d:
        for n, v in (("b.json", base), ("h.json", head)):
            json.dump(v, open(os.path.join(d, n), "w"))
        sm = os.path.join(d, "s.md")
        p = subprocess.run([sys.executable, os.path.join(HERE, "compare.py"), os.path.join(d, "b.json"), os.path.join(d, "h.json"), "--summary", sm, *extra], capture_output=True, text=True)
        return p.returncode, p.stdout, open(sm).read()


class Compare(unittest.TestCase):
    def test_unchanged_passes(self):
        rc, out, md = run({"a": 1000}, {"a": 1000})
        self.assertEqual(rc, 0)
        self.assertIn("| `a` |", md)

    def test_small_increase_warns_but_passes(self):
        rc, out, _ = run({"a": 1000}, {"a": 1050})
        self.assertEqual(rc, 0)
        self.assertIn("::warning::a: +5.0 %", out)

    def test_large_increase_fails(self):
        rc, out, md = run({"a": 1000, "b": 1000}, {"a": 1111, "b": 1000})
        self.assertEqual(rc, 1)
        self.assertIn("::error::a: +11.1 %", out)
        self.assertIn("FEL", md)

    def test_threshold_is_adjustable(self):
        self.assertEqual(run({"a": 1000}, {"a": 1111}, "--fail", "15")[0], 0)

    def test_improvement_and_new_or_removed_tests_do_not_fail(self):
        rc, out, md = run({"a": 1000, "gone": 5}, {"a": 500, "new": 7})
        self.assertEqual(rc, 0)
        self.assertIn("snabbare", md)
        self.assertIn("ny", md)


if __name__ == "__main__":
    unittest.main()
