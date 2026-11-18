"""Export completed archives through the normal UwU Logs upload protocol."""
import json
import logging
import time
from pathlib import Path
from urllib.parse import urlparse

import requests

class RemoteExporter:
    def __init__(self, root, config):
        self.root = Path(root)
        self.url = config.get('remote_export_url', '').rstrip('/')
        self.path = self.root/'auto-export-state.json'
        try:
            self.state = json.loads(self.path.read_text('utf-8'))
        except (FileNotFoundError, ValueError):
            self.state = {}
        self.next_try = self.state.get('_retry_after', {}).get(self.url, 0)

    def save(self):
        temporary = self.path.with_suffix('.tmp')
        temporary.write_text(json.dumps(self.state, indent=2), encoding='utf-8')
        temporary.replace(self.path)

    def defer(self, error):
        self.next_try = time.time()+(3600 if 'No disk space' in str(error) else 60)
        self.state.setdefault('_retry_after', {})[self.url] = self.next_try
        self.state.setdefault('_last_error', {})[self.url] = str(error)
        self.save()

    def metadata(self, archive):
        for path in (self.root/'uploads/0file_info').rglob('*.json'):
            data = json.loads(path.read_text('utf-8'))
            if any(archive.name.endswith(Path(name.replace('\\', '/')).name) for name in data.get('archives', [])):
                return data
        raise ValueError('No import metadata for '+archive.name)

    def send(self, archive, info):
        with requests.Session() as session:
            def report_exists(name):
                response = session.get(self.url+'/reports/'+name+'/', timeout=30, allow_redirects=False)
                return response.status_code == 200 and 'id="report-main"' in response.text and '/reports/'+name in response.text
            # Only skip an archive when every report can actually be opened.
            reports = list(info.get('slices', {}))
            # A later archive may add bosses to an existing lockout report.
            # Filename checkpoints prevent repeats; report existence alone cannot.
            progress = session.get(self.url+'/upload_progress', timeout=15)
            if progress.status_code == 200 and not progress.json().get('done'):
                raise RuntimeError('Destination is processing another upload')
            upload_id = str(time.time_ns())
            count = 0
            with archive.open('rb') as source:
                while chunk := source.read(256*1024):
                    response = session.post(self.url+'/upload', data=chunk,
                        headers={'X-Upload-ID': upload_id, 'X-Chunk': str(count), 'Content-Type':'application/octet-stream'}, timeout=60)
                    response.raise_for_status()
                    count += 1
            response = session.post(self.url+'/upload', json={'filename':archive.name,
                'server':info['server'], 'timezone':info['timezone'], 'chunks':count}, timeout=60)
            response.raise_for_status()
            deadline = time.monotonic()+600
            result = None
            while time.monotonic() < deadline:
                response = session.get(self.url+'/upload_progress', timeout=30)
                if response.status_code == 200:
                    result = response.json()
                    if result.get('done'):
                        break
                elif response.status_code == 204:
                    break
                else:
                    response.raise_for_status()
                time.sleep(1)
            if result and (result.get('done') == -1 or 'Error' in str(result.get('status', ''))):
                raise RuntimeError('Destination import failed: '+str(result.get('status')))
            if not reports or not all(report_exists(name) for name in reports):
                raise RuntimeError('Destination reports are not ready')
            return reports

    def tick(self, status=None, drain=False):
        status = status if status is not None else {}
        if not self.url:
            return
        if time.time() < self.next_try:
            status.update(export_state='retry_pending', export_target=self.url,
                          export_error=self.state.get('_last_error', {}).get(self.url, ''))
            return
        if urlparse(self.url).scheme not in ('http','https'):
            raise ValueError('Export URL must use http or https')
        records = self.state.setdefault(self.url, {})
        for archive in sorted((self.root/'uploads/uploaded').glob('*')):
            if not archive.is_file() or archive.name in records:
                continue
            try:
                status.update(export_state='sending', export_target=self.url)
                info = self.metadata(archive)
                reports = self.send(archive, info)
                records[archive.name] = {'reports':reports, 'completed':time.time()}
                self.state.get('_last_error', {}).pop(self.url, None)
                self.state.get('_retry_after', {}).pop(self.url, None)
                self.save()
                status.update(export_state='complete', export_reports=reports)
                logging.getLogger('auto_raid').info('Exported %s to %s', archive.name, self.url)
                if drain:
                    print('Verified: '+', '.join(reports), flush=True)
                else:
                    return
            except Exception as error:
                self.defer(error)
                status.update(export_state='retry_pending', export_error=str(error))
                logging.getLogger('auto_raid').warning('Remote export pending: %s', error)
                if drain:
                    raise
                return
        status.update(export_state='up_to_date', export_target=self.url)

if __name__ == '__main__':
    root = Path(__file__).resolve().parent
    RemoteExporter(root, json.loads((root/'auto-config.json').read_text('utf-8'))).tick(drain=True)
