import unittest

from compare_anchors import compare


def image(pixels):
    return bytes(v for rgb in pixels for v in (*rgb, 255))


class AnchorBands(unittest.TestCase):
    def test_only_the_anchor_sample_decides_the_band(self):
        native = image([(100, 100, 100), (100, 100, 100)])
        # 2x: anchors are columns 0 and 2 of row 0; the other samples differ freely.
        present = image([
            (100, 100, 100), (0, 0, 0), (106, 100, 100), (255, 255, 255),
            (0, 0, 0), (0, 0, 0), (255, 255, 255), (255, 255, 255),
        ])
        report, beyond = compare(native, present, 2, 1, 2, step=8)
        self.assertEqual(report["anchors"], {"identical": 1, "within_step": 1, "beyond_step": 0})
        self.assertEqual(beyond, [False, False])
        self.assertIsNone(report["decision_mismatches"])
        self.assertIsNone(report["value_beyond_step"])
        self.assertEqual(report["unclassified"], 2)

    def test_decisions_are_independent_of_value_error(self):
        native = image([(10, 20, 30)] * 6)
        present = image([(10, 20, 30), (100, 20, 30), (18, 20, 30),
                         (10, 22, 30), (10, 23, 30), (100, 20, 30)])
        report, _ = compare(native, present, 3, 2, 1, step=8,
                            decisions=(bytes([0] * 6), bytes([1, 1, 0, 0, 0, 255])),
                            steps=bytes([8, 2, 1] * 6))
        self.assertEqual(report["decision_mismatches"], 2)
        self.assertEqual(report["value_beyond_step"], 1)
        self.assertEqual(report["value_tested"], 3)
        self.assertEqual(report["unclassified"], 1)

    def test_label_anchors_use_both_scaled_axes(self):
        native = image([(10, 20, 30)] * 4)
        present = image([(11, 20, 30)] * 36)
        labels = bytearray([99] * 36)
        labels[0], labels[3], labels[18], labels[21] = 0, 1, 2, 9
        report, _ = compare(native, present, 2, 2, 3,
                            decisions=(bytes([0, 1, 2, 3]), labels))
        self.assertEqual(report["decision_mismatches"], 1)
        self.assertEqual(report["value_beyond_step"], 0)
        self.assertEqual(report["anchors"]["identical"], 0)
        self.assertEqual(report["anchors"]["within_step"], 4)

    def test_tone_compares_the_block_average(self):
        native = image([(100, 100, 100)])
        present = image([(90, 100, 100), (110, 100, 100), (100, 100, 100), (100, 100, 100)])
        report, _ = compare(native, present, 1, 1, 2)
        self.assertEqual(report["tone_flat"]["mean"], 0.0)
        shifted = image([(104, 100, 100)] * 4)
        report, _ = compare(native, shifted, 1, 1, 2)
        self.assertEqual(report["tone_flat"]["mean"], 4.0)


if __name__ == "__main__":
    unittest.main()
