"""Regression tests of the comparison harness, requiring neither SDK nor Java."""
import unittest
import hashlib
import json
from pathlib import Path
from specs import SPECS, Spec, belt_surfaces_equal
from run import compare_lines


class CompareTests(unittest.TestCase):
    def setUp(self):
        self.spec = Spec("fixture", "unused", (), None, "harness test", "F", 1e-6)

    def test_exact_state_comparison_ignores_only_whitespace(self):
        r = compare_lines(self.spec, ["S 1"], ["0:149:0 0:148:0 "], ["0:149:0  0:148:0"])
        self.assertEqual(r["passed"], 1)

    def test_one_timer_tick_is_not_accepted_as_float_tolerance(self):
        r = compare_lines(self.spec, ["S 1"], ["0:149:0"], ["0:148:0"])
        self.assertEqual(r["different"], 1)

    def test_float_tolerance_is_explicit_per_operation(self):
        r = compare_lines(self.spec, ["F 1"], ["1.0000001"], ["1.0"])
        self.assertEqual(r["passed"], 1)

    def test_integral_float_spelling_is_not_a_mechanical_difference(self):
        r = compare_lines(self.spec, ["F 1"], ["1.0"], ["1"])
        self.assertEqual(r["passed"], 1)

    def test_tolerance_boundary_is_not_accepted(self):
        r = compare_lines(self.spec, ["F 1"], ["0.0"], ["0.000001"])
        self.assertEqual(r["different"], 1)

    def test_nan_is_never_a_pass(self):
        r = compare_lines(self.spec, ["F 1"], ["NaN"], ["NaN"])
        self.assertEqual(r["different"], 1)

    def test_numeric_vectors_compare_all_coordinates(self):
        r = compare_lines(self.spec, ["F 1"], ["1 2 3"], ["1 2 4"])
        self.assertEqual(r["different"], 1)
        r = compare_lines(self.spec, ["F 1"], ["1 2 3"], ["1 2"])
        self.assertEqual(r["different"], 1)

    def test_adapter_errors_are_not_reported_as_engine_panics(self):
        with self.assertRaisesRegex(ValueError, "adapter error"):
            compare_lines(self.spec, ["S 1"], ["1"], ["!harness fixture integer"])

    def test_belt_surface_identity_and_shift_flags_are_exact(self):
        self.assertTrue(belt_surfaces_equal("middle 1 0.2500001", "middle 1 0.25"))
        self.assertFalse(belt_surfaces_equal("middle 1 0.25", "middle 0 0.25"))
        self.assertFalse(belt_surfaces_equal("middle 1 0.25", "end 1 0.25"))
        self.assertFalse(belt_surfaces_equal("middle 1 NaN", "middle 1 NaN"))
        with self.assertRaises(ValueError):
            belt_surfaces_equal("middle 1", "middle 1")

    def test_each_nonpassing_category_retains_examples(self):
        r = compare_lines(self.spec, ["S 1"] * 4, ["1"] * 4,
                          ["!unsupported first"] * 2 + ["2", "!panic overflow"], example_limit=1)
        self.assertEqual({x["outcome"] for x in r["examples"]}, {"unsupported", "different", "panicked"})

    def test_unavailable_and_panic_are_not_passing_cases(self):
        r = compare_lines(self.spec, ["S 1", "S 2"], ["1", "2"], ["!unsupported no accessor", "!panic overflow"])
        self.assertEqual((r["passed"], r["unsupported"], r["panicked"]), (0, 1, 1))

    def test_truncation_extra_rows_and_empty_suite_are_harness_errors(self):
        for cases, java, rust in [(["S 1", "S 2"], ["1", "2"], ["1"]),
                                  (["S 1"], ["1"], ["1", "2"]), ([], [], []),
                                  (["S 1"], [""], [""]), ([""], ["1"], ["1"])]:
            with self.subTest(cases=cases, java=java, rust=rust):
                with self.assertRaises(ValueError):
                    compare_lines(self.spec, cases, java, rust)

    def test_reports_every_count_but_bounds_failure_examples(self):
        r = compare_lines(self.spec, ["S 1"] * 30, ["1"] * 30, ["2"] * 30, example_limit=3)
        self.assertEqual(r["different"], 30)
        self.assertEqual(len(r["examples"]), 3)


class MigrationTests(unittest.TestCase):
    def test_migrated_input_streams_match_the_original_generators(self):
        # Recorded by running only the pure generator AST from the exact origin
        # commit, independently of the newly written specs.py functions.
        provenance = json.loads(Path(__file__).with_name("matrix-provenance.json").read_text())
        for name, origin in provenance["suites"].items():
            with self.subTest(suite=name):
                cases = next(s for s in SPECS if s.name == name).cases()
                self.assertEqual(len(cases), origin["cases"])
                self.assertEqual(hashlib.sha256(("\n".join(cases) + "\n").encode()).hexdigest(),
                                 origin["inputs_sha256"])


if __name__ == "__main__":
    unittest.main()
