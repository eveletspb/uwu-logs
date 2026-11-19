import tempfile
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch
from remote_export import RemoteExporter

class RemoteExportTests(unittest.TestCase):
    def test_old_archives_are_drained_and_not_sent_twice(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory); folder=root/'uploads/uploaded'; folder.mkdir(parents=True)
            for i in range(3): (folder/f'{i}.zip').write_bytes(b'archive')
            exporter=RemoteExporter(root,{'remote_export_url':'http://example.test:5000'})
            with patch.object(exporter,'metadata',return_value={}), patch.object(exporter,'send',return_value=['report']) as send:
                exporter.tick(drain=True)
                self.assertEqual(send.call_count,3)
            restarted=RemoteExporter(root,{'remote_export_url':'http://example.test:5000'})
            with patch.object(restarted,'send') as send:
                restarted.tick(drain=True); send.assert_not_called()

    def test_disk_error_keeps_archive_and_persists_retry(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory); folder=root/'uploads/uploaded'; folder.mkdir(parents=True)
            archive=folder/'raid.zip'; archive.write_bytes(b'archive')
            exporter=RemoteExporter(root,{'remote_export_url':'http://example.test:5000'})
            with patch.object(exporter,'metadata',return_value={}), patch.object(exporter,'send',side_effect=RuntimeError('No disk space')):
                status={}; exporter.tick(status)
            self.assertEqual(status['export_state'],'retry_pending')
            self.assertNotIn('raid.zip',exporter.state[exporter.url])
            self.assertTrue(archive.exists())
            restarted=RemoteExporter(root,{'remote_export_url':exporter.url})
            with patch.object(restarted,'send') as send:
                restarted.tick(); send.assert_not_called()

    def test_error_page_with_http_200_is_not_treated_as_a_report(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory); archive=root/'raid.zip'; archive.write_bytes(b'archive')
            exporter=RemoteExporter(root,{'remote_export_url':'http://example.test:5000'})
            session=MagicMock(); session.__enter__.return_value=session
            uploaded=[False]
            def get(url,**kwargs):
                response=MagicMock()
                if '/reports/' in url:
                    response.status_code=200
                    response.text='<main id="report-main">/reports/raid</main>' if uploaded[0] else '<main id="error-page">Missing</main>'
                else:
                    response.status_code=200 if uploaded[0] else 204
                    response.json.return_value={'done':1}
                return response
            def post(url,**kwargs):
                if 'json' in kwargs: uploaded[0]=True
                return MagicMock()
            session.get.side_effect=get; session.post.side_effect=post
            with patch('remote_export.requests.Session',return_value=session):
                self.assertEqual(exporter.send(archive,{'slices':{'raid':{}},'server':'stable','timezone':'UTC'}),['raid'])
            self.assertEqual(session.post.call_count,2)

    def test_destination_can_return_one_report_for_an_archive_with_several_old_slices(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / 'raid.zip'
            archive.write_bytes(b'archive')
            exporter = RemoteExporter(root, {'remote_export_url': 'http://example.test:5000'})
            session = MagicMock()
            session.__enter__.return_value = session
            before = MagicMock(status_code=204)
            complete = MagicMock(status_code=200)
            complete.json.return_value = {'done': 1, 'slices': {'combined': {'done': 1}}}
            saved = MagicMock(status_code=200, text='<main id="report-main">/reports/combined</main>')
            session.get.side_effect = [before, complete, saved]
            with patch('remote_export.requests.Session', return_value=session):
                reports = exporter.send(archive, {
                    'slices': {'old-a': {}, 'old-b': {}},
                    'server': 'stable',
                    'timezone': 'UTC',
                })
            self.assertEqual(reports, ['combined'])
