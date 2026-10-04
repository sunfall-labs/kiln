import unittest

from bench.checksum_validation import validate_repeated_checksums


class RepeatedChecksumTests(unittest.TestCase):
    def test_all_repetitions_must_match_the_node_reference(self):
        excluded, diagnostics = validate_repeated_checksums(
            {"node": ["ok", "ok", "ok"], "bun": ["ok", "wrong", "ok"]}, 3
        )

        self.assertEqual(excluded, {"bun"})
        self.assertEqual(
            diagnostics,
            ["bun pass 2: checksum 'wrong' != node's 'ok'"],
        )

    def test_node_reference_must_be_stable_across_repetitions(self):
        excluded, diagnostics = validate_repeated_checksums(
            {"node": ["first", "second"], "bun": ["first", "first"]}, 2
        )

        self.assertEqual(excluded, {"node", "bun"})
        self.assertEqual(
            diagnostics,
            [
                "node pass 2: checksum 'second' != node pass 1 'first'",
                "bun: no stable node checksum reference; excluding runtime",
            ],
        )

    def test_missing_repetition_cannot_be_used_as_a_reference(self):
        excluded, diagnostics = validate_repeated_checksums(
            {"node": ["ok"], "bun": ["ok", "ok"]}, 2
        )

        self.assertEqual(excluded, {"node", "bun"})
        self.assertEqual(
            diagnostics,
            [
                "node: got 1 checksum samples, expected 2; excluding all runtimes",
            ],
        )

    def test_consistent_samples_are_retained(self):
        excluded, diagnostics = validate_repeated_checksums(
            {"node": ["ok"] * 3, "bun": ["ok"] * 3, "kiln": ["ok"] * 3}, 3
        )

        self.assertEqual(excluded, set())
        self.assertEqual(diagnostics, [])

    def test_binary_checksums_are_reported_without_dumping_the_payload(self):
        excluded, diagnostics = validate_repeated_checksums(
            {"node": [b"reference"], "kiln": [b"different"]}, 1
        )

        self.assertEqual(excluded, {"kiln"})
        self.assertIn("kiln pass 1: checksum <9 bytes sha256=", diagnostics[0])
        self.assertNotIn("different", diagnostics[0])


if __name__ == "__main__":
    unittest.main()
