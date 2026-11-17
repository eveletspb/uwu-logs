import tempfile
import unittest
from pathlib import Path
from raid_progress import progress_for, read_snapshots

class RaidProgressTests(unittest.TestCase):
    def row(self, **changes):
        return dict(time=1100, session=900, player='Bombas', realm='stable',
                    instance="Temple of Ahn'Qiraj", total=9, killed=5) | changes

    def test_existing_instance_progress_includes_pre_join_kills(self):
        value, _ = progress_for(0, 2, 'Bombas', 'stable', 1000, 1200, [self.row()])
        self.assertEqual(value, '5/9')

    def test_partial_log_is_explicit_lower_bound(self):
        value, note = progress_for(0, 2, 'Bombas', 'stable', 1000, 1200, [])
        self.assertEqual(value, '≥2/9')
        self.assertIn('неизвестен', note)

    def test_different_character_realm_session_and_later_kills_are_excluded(self):
        for row in [self.row(player='Other'), self.row(realm='develop'),
                    self.row(session=2000, time=2100), self.row(time=2000)]:
            self.assertEqual(progress_for(0, 2, 'Bombas', 'stable', 1000, 1200, [row])[0], '≥2/9')

    def test_versioned_saved_variable_records_and_invalid_counts(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder)/'WTF/Account/TEST/SavedVariables/RaidAutoLog.lua'
            path.parent.mkdir(parents=True)
            fields = ['1100', 'stable'.encode().hex(), 'Bombas'.encode().hex(),
                      "Temple of Ahn'Qiraj".encode().hex(), '9', '5', '900']
            path.write_text('"RAL1|'+'|'.join(fields)+'"', encoding='utf-8')
            self.assertEqual(read_snapshots(folder)[0]['killed'], 5)
            fields[5] = '10'
            path.write_text('"RAL1|'+'|'.join(fields)+'"', encoding='utf-8')
            self.assertEqual(read_snapshots(folder), [])

if __name__ == '__main__':
    unittest.main()
