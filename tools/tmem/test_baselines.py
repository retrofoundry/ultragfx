import gzip
import hashlib
import json
from pathlib import Path
import struct
import unittest

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent


def sha(data):
    return hashlib.sha256(data).hexdigest()


GOLDEN_OVERRIDES = {
    '2d-fill-texrect.bin': '54fdba72aed3c0b120de4075622ccad6b9662267f5dcc698ef7c4a47e9fe4f18',
    '2d-hud-over-3d.bin': 'ed7840adb54d7dd388201d9a7dcc091f5eaddf6fd196660f2c9a2d900ec74baa',
    '2d-texrectflip.bin': 'b4d7448776f5fa6df703d63bf01255c1b0493b6c2f863d84c8de7ef13ffd7c98',
    'alpha-threshold.bin': '67a6c3377ae31626a72b84ffe8998b9d964c8daf59c8f5c8fb73d2b97439d149',
    'ci4-grid.bin': '0af6d1a551dcd4b1ce41e8371dfd2ce5434f303af1d03d439f0b1f8963a30b6e',
    'ci8-ramp.bin': '7df745eaa1e2d09af2d9be7071f6c1571ddf4ea1be27075b613d2ee74133b488',
    'fogworld.bin': '5d170dfd6a0c099c053502a8e9c273a95c7fd974574a2eb2c7e47e6c09592550',
    'i4-ramp.bin': 'f4f2c0ba9cbf4eba8c748bf35886a8689d49a3c67d7ae3f1b56826082a8fbdcf',
    'i8-ramp.bin': '991af80709539bf2cc99e235617b7244578987e76cd6424eb9de59f6cda7a883',
    'ia16-ramp.bin': 'a63b4a7da084abca4d4fd46e51397c877579728cbbdc0e2b8d7288de670747ce',
    'ia4-ramp.bin': 'f4f2c0ba9cbf4eba8c748bf35886a8689d49a3c67d7ae3f1b56826082a8fbdcf',
    'ia8-ramp.bin': '991af80709539bf2cc99e235617b7244578987e76cd6424eb9de59f6cda7a883',
    'mirror-repeat.bin': '56f1bd9cab9ce22a922837f523f35d1260036da91e8fc9686f188685e140391c',
    'multi-material.bin': '446bb6b71869ef4831c21ceef629a0211ac8ca0df3b9fad7477a4c6993a80187',
    'rgba16-quad.bin': 'ce3913ddcab8a8d0dc896a6b66f6342adfd46a47697dabd70eeaa00ce6f48013',
    'wrap-repeat.bin': '92dfa57c4c7939135ad1e5b135c1c8956f0d9f02e7c8ddc98a1e67c51c6cf648',
}


class FrozenBaselines(unittest.TestCase):
    def test_all_27_goldens_match_c2_or_declared_overrides(self):
        ledger = json.loads((HERE / 'goldens-bb0fea3.json').read_text())
        rows = ledger['goldens']
        self.assertEqual(len(rows), 27)
        self.assertEqual({row['file'] for row in rows},
                         {str(path.relative_to(ROOT)) for path in (ROOT / 'ultragfx/goldens').glob('*.bin')})
        for row in rows:
            with self.subTest(file=row['file']):
                data = (ROOT / row['file']).read_bytes()
                self.assertEqual(len(data), row['width'] * row['height'] * 4)
                expected = GOLDEN_OVERRIDES.get(Path(row['file']).name, row['sha256'])
                self.assertEqual(sha(data), expected)
                self.assertEqual(set(row['c2_readbacks']), {'default', 'debug-ui', 'all-features'})
                self.assertEqual(set(row['c2_readbacks'].values()), {row['sha256']})

    def test_oracle_residual_values_and_exact_masks_are_intact(self):
        ledger = json.loads((HERE / 'oracle-bb0fea3.json').read_text())
        self.assertEqual(len(ledger['rows']), 60)
        self.assertEqual(len({row['scene'] for row in ledger['rows']}), 60)
        self.assertEqual(sum(row['scene'].startswith('coverage-') for row in ledger['rows']), 38)
        for row in ledger['rows']:
            with self.subTest(scene=row['scene']):
                data = gzip.decompress((HERE / row['residual_file']).read_bytes())
                self.assertEqual(sha(data), row['residual_sha256'])
                channels = len(row['channels'])
                self.assertEqual(len(data), row['rgba_bytes'] // 4 * channels * 2)
                values = [v[0] for v in struct.iter_unpack('<h', data)]
                self.assertTrue(all(-255 <= value <= 255 for value in values))
                mask = bytes(any(values[i:i + channels]) for i in range(0, len(values), channels))
                self.assertEqual(sha(mask), row['mask_sha256'])
                self.assertEqual(sum(mask), row['changed_pixels'])
                self.assertEqual(max(map(abs, values)), row['maximum'])
                if 'limit' in row:
                    self.assertLessEqual(row['maximum'], row['limit'])
        quad = next(row for row in ledger['rows'] if row['scene'] == 'f3dex2-quad-winding')
        self.assertEqual((quad['maximum'], quad['changed_pixels']), (0, 0))
        self.assertEqual(len(ledger['controls']), 1)
        tri2 = ledger['controls'][0]
        self.assertEqual(tri2['scene'], 'f3dex2-quad-winding-tri2')
        self.assertEqual(tri2['equivalent_to'], quad['scene'])
        self.assertEqual(tri2['channels'], 'RGBA')
        self.assertEqual((tri2['maximum'], tri2['changed_pixels']), (0, 0))
        self.assertEqual(tri2['rt64_sha256'], quad['rt64_sha256'])

    def test_role_count_expectations_describe_fixture_requests(self):
        contract = json.loads((HERE / 'dispatch-expectations.json').read_text())
        roles = contract['roles_fixture']
        distinct = set(role for draw in roles['draw_roles'] for role in draw)
        self.assertEqual(distinct, {'r0', 'r1', 'r2', 'r3'})
        self.assertEqual(roles['cold_dispatches'], len(distinct))
        self.assertTrue(set(roles['level_zero_aliases']) <= distinct)
        self.assertEqual(roles['warm_dispatches'], 0)
        for row in contract['changes']:
            self.assertEqual(row['dispatches'], len(row['new_requests']))


if __name__ == '__main__':
    unittest.main()
