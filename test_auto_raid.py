import json
import os
import tempfile
import time
import unittest
from datetime import datetime, timedelta
from pathlib import Path
from unittest.mock import patch
from zoneinfo import ZoneInfo

import zstandard
import local_server
from auto_raid import ROOT, RaidWorker, event_time, remove_owned
from c_path import Directories, PathExt

ZONE = ZoneInfo('Asia/Bangkok')

class Game:
    def __init__(self):
        self.value = {'running': True, 'logging': True, 'realm': '192.168.1.195'}
    def read(self):
        return dict(self.value)

def log_rows(at, count=450):
    player = '0x0000000000000001,"AutoRaidTest",0x511'
    boss = '0xF130008F04000001,"Lord Marrowgar",0xa48'
    rows = []
    for tick in range(count):
        stamp = (at+timedelta(milliseconds=tick*200)).strftime('%m/%d %H:%M:%S.%f')[:-3]
        if (tick//10) % 2 == 0:
            row = f'RANGE_DAMAGE,{player},{boss},75,"Auto Shot",0x1,1000,0,1,0,0,0,nil,nil,nil'
        else:
            row = f'SWING_DAMAGE,{boss},{player},500,0,1,0,0,0,nil,nil,nil'
        rows.append(f'{stamp}  {row}\n'.encode())
    return b''.join(rows)

class AutomaticRaidTests(unittest.TestCase):
    def setUp(self):
        area = ROOT/'.auto-tests'
        area.mkdir(exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(dir=area)
        self.base = Path(self.temporary.name).resolve()
        assert self.base.is_relative_to(ROOT)
        self.app = self.base/'app'
        self.app.mkdir()
        self.saved_dirs = {}
        for key, value in list(vars(Directories).items()):
            if isinstance(value, Path):
                self.saved_dirs[key] = value
                new = PathExt(self.app/key)
                new.mkdir(parents=True, exist_ok=True)
                setattr(Directories, key, new)
        self.saved_dirs['main'] = self.saved_dirs.get('main', ROOT)
        Directories.main = PathExt(self.app)
        Directories.logs = PathExt(self.app/'LogsDir')
        Directories.logs.mkdir()
        Directories.uploads = PathExt(self.app/'uploads')
        Directories.uploads.mkdir(exist_ok=True)
        for key, suffix in [('uploaded','uploaded'),('pending_archive','0archive_pending'),
                            ('info_uploads','0file_info'),('todo','0todo'),
                            ('errors','0errors'),('no_text','0no_text')]:
            new = PathExt(Directories.uploads/suffix)
            new.mkdir(exist_ok=True)
            setattr(Directories,key,new)
        self.game = Game()
        config = {'source': '../game/Logs/WoWCombatLog.txt', 'raw_archive': '../game/Logs/RaidArchive',
                  'retention_hours':24, 'timezone':'Asia/Bangkok', 'settle_seconds':8,
                  'client_sha256':'test', 'realms':{'192.168.1.195':'develop'}}
        self.worker = RaidWorker(self.app, config, self.game)
        self.source = self.worker.source
        self.source.parent.mkdir(parents=True, exist_ok=True)
        self.now = time.time()

    def tearDown(self):
        local_server.SERVER.config['TESTING'] = False
        from Z_SERVER import OPENED_LOGS
        OPENED_LOGS.clear()
        for key,value in self.saved_dirs.items():
            setattr(Directories,key,value)
        self.temporary.cleanup()

    def finish_raid(self):
        self.source.write_bytes(log_rows(datetime.now(ZONE)-timedelta(minutes=3)))
        self.worker.tick(self.now)
        self.assertEqual(self.worker.status['state'],'recording')
        self.assertFalse(list(Directories.logs.iterdir()))
        self.game.value['logging'] = False
        self.worker.tick(self.now+10)

    def test_post_raid_import_checkpoint_reimport_and_safe_rotation(self):
        self.finish_raid()
        self.assertTrue(self.source.exists())
        self.assertEqual(self.worker.state['offset'],self.source.stat().st_size)
        self.assertEqual(self.worker.state['server'],'develop')
        self.assertEqual(len(list(Directories.logs.iterdir())),1)
        self.assertTrue(next(Directories.logs.iterdir()).name.startswith(datetime.now(ZONE).strftime('%y-')))
        prior_import = self.worker.state['last_import']
        self.worker.tick(self.now+20)
        self.assertEqual(self.worker.state['last_import'],prior_import)
        # A final delayed flush updates the same report, without losing earlier rows.
        with self.source.open('ab') as output:
            output.write(log_rows(datetime.now(ZONE)-timedelta(seconds=60),20))
        self.worker.tick(self.now+21)
        self.worker.tick(self.now+30)
        report = next(Directories.logs.iterdir())
        decoded = zstandard.ZstdDecompressor().decompress((report/'LOGS_CUT.zstd').read_bytes())
        self.assertGreaterEqual(len(decoded.splitlines()),470)
        self.game.value['running'] = False
        self.worker.tick(self.now+40)
        self.assertFalse(self.source.exists())
        self.assertEqual(len(list(self.worker.raw_archive.glob('*.txt'))),1)
        self.assertEqual(self.worker.state['offset'],0)

    def test_retention_uses_event_time_not_import_time(self):
        old = Directories.logs/'old-report'
        recent = Directories.logs/'recent-report'
        old.mkdir(); recent.mkdir()
        old_data = log_rows(datetime.now(ZONE)-timedelta(hours=25),20)
        new_data = log_rows(datetime.now(ZONE)-timedelta(minutes=5),20)
        for folder,data in [(old,old_data),(recent,new_data)]:
            (folder/'LOGS_CUT.zstd').write_bytes(zstandard.ZstdCompressor().compress(data))
        raw = self.worker.raw_archive/'WoWCombatLog-test.txt'
        raw.write_bytes(old_data+new_data)
        self.worker.cleanup(self.now)
        self.assertFalse(old.exists())
        self.assertTrue(recent.exists())
        self.assertEqual(raw.read_bytes(),new_data)

    def test_failure_does_not_commit_or_rotate(self):
        self.source.write_bytes(log_rows(datetime.now(ZONE)-timedelta(minutes=3)))
        self.worker.tick(self.now)
        self.game.value.update(logging=False,running=False)
        with patch('logs_upload.LogsArchive',side_effect=RuntimeError('test failure')):
            with self.assertRaises(RuntimeError):
                self.worker.tick(self.now+10)
        self.assertEqual(self.worker.state['offset'],0)
        self.assertTrue(self.source.exists())

    def test_unknown_logging_status_does_not_import(self):
        self.source.write_bytes(b'not complete\n')
        self.game.value['logging'] = None
        self.worker.tick(self.now)
        self.worker.tick(self.now+20)
        self.assertEqual(self.worker.state['offset'],0)

    def test_performance_mode_skips_import_and_cleanup_while_game_is_open(self):
        self.worker.config['defer_until_client_exit'] = True
        self.source.write_bytes(log_rows(datetime.now(ZONE)-timedelta(minutes=3)))
        self.game.value['logging'] = False
        with patch.object(self.worker,'cleanup') as cleanup, patch.object(self.worker,'import_log') as importer:
            self.worker.tick(self.now)
            self.worker.tick(self.now+600)
            cleanup.assert_not_called()
            importer.assert_not_called()
        self.assertEqual(self.worker.status['state'],'deferred_until_client_exit')

    def test_cleanup_rejects_outside_paths_and_year_rollover(self):
        outside = self.base/'outside.txt'
        outside.write_text('keep')
        with self.assertRaises(ValueError):
            remove_owned(outside,self.app)
        self.assertTrue(outside.exists())
        reference = datetime(2027,1,1,tzinfo=ZONE).timestamp()
        stamp = event_time(b'12/31 23:59:59.000  TEST',reference,ZONE)
        self.assertEqual(datetime.fromtimestamp(stamp,ZONE).year,2026)

if __name__ == '__main__':
    unittest.main(verbosity=2)
