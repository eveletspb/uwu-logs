"""Local post-raid import, safe journal rotation, and 24-hour retention."""
import ctypes
import hashlib
import json
import logging
import os
import re
import shutil
import threading
import time
import zipfile
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo

import psutil
import zstandard

ROOT = Path(__file__).resolve().parent
STAMP = re.compile(rb'^(\d{1,2})/(\d{1,2}) (\d{2}):(\d{2}):(\d{2})\.(\d{3})')
LOGGER = logging.getLogger('auto_raid')
JOB_LOCK = threading.RLock()

def event_time(line, reference, zone):
    match = STAMP.match(line)
    if not match:
        return None
    month, day, hour, minute, second, ms = map(int, match.groups())
    year = datetime.fromtimestamp(reference, zone).year
    candidates = []
    for candidate in (year - 1, year, year + 1):
        try:
            candidates.append(datetime(candidate, month, day, hour, minute, second,
                                       ms * 1000, tzinfo=zone).timestamp())
        except ValueError:
            pass
    return min(candidates, key=lambda value: abs(value-reference)) if candidates else None

def remove_owned(path, root):
    path, root = Path(path), Path(root).resolve()
    resolved = path.resolve()
    if resolved == root or not resolved.is_relative_to(root) or path.is_symlink() or path.is_junction():
        raise ValueError(f'Unsafe cleanup path: {path}')
    if not path.exists():
        return
    if path.is_dir():
        for child in path.rglob('*'):
            if child.is_symlink() or child.is_junction():
                raise ValueError(f'Linked cleanup path: {child}')
        shutil.rmtree(path)
    else:
        path.unlink()

class WowStatus:
    """Read logging status only. No injection or writes to the game process."""
    def __init__(self, exe, digest, read_logging=True):
        self.exe = Path(exe).resolve()
        self.digest = digest
        self.verified = None
        self.read_logging = read_logging

    def read(self):
        processes = []
        unknown = False
        for process in psutil.process_iter(['pid', 'name']):
            if (process.info['name'] or '').lower() != 'wow.exe':
                continue
            try:
                if Path(process.exe()).resolve() == self.exe:
                    processes.append(process)
            except (psutil.AccessDenied, psutil.NoSuchProcess):
                unknown = True
        if not processes:
            realm = ''
            if not unknown:
                try:
                    config = (self.exe.parent/'WTF/Config.wtf').read_bytes()
                    value = re.search(rb'SET realmList\s+"([^"]+)"', config, re.I)
                    if value:
                        realm = value.group(1).decode('ascii')
                except (OSError, UnicodeError):
                    pass
            return {'running': unknown, 'logging': None if unknown else False, 'realm': realm}
        if not self.read_logging:
            return {'running': True, 'logging': None, 'realm': ''}
        signature = (self.exe.stat().st_size, self.exe.stat().st_mtime_ns)
        if self.verified != signature:
            if hashlib.sha256(self.exe.read_bytes()).hexdigest() != self.digest:
                return {'running': True, 'logging': None, 'realm': '', 'reason': 'Client version changed'}
            self.verified = signature
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.OpenProcess.restype = ctypes.c_void_p
        kernel.OpenProcess.argtypes = [ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
        kernel.ReadProcessMemory.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p,
                                            ctypes.c_size_t, ctypes.POINTER(ctypes.c_size_t)]
        kernel.CloseHandle.argtypes = [ctypes.c_void_p]
        active, realm = False, ''
        for process in processes:
            handle = kernel.OpenProcess(0x10 | 0x1000, False, process.pid)
            if not handle:
                return {'running': True, 'logging': None, 'realm': ''}
            def read(address, size):
                buffer = ctypes.create_string_buffer(size)
                count = ctypes.c_size_t()
                if not kernel.ReadProcessMemory(handle, address, buffer, size, ctypes.byref(count)):
                    raise OSError(ctypes.get_last_error(), 'Cannot read logging status')
                return buffer.raw[:count.value]
            try:
                # This exact 12340 executable has a fixed image base (no ASLR flag).
                enabled = int.from_bytes(read(0xB759D4, 4), 'little')
                if enabled not in (0, 1):
                    raise ValueError('Unexpected logging status')
                active = active or bool(enabled)
                try:
                    cvar = int.from_bytes(read(0xC79D00, 4), 'little')
                    string = int.from_bytes(read(cvar + 0x28, 4), 'little')
                    value = read(string, 128).split(b'\0', 1)[0].decode('ascii')
                    if re.fullmatch(r'[A-Za-z0-9.:-]{1,100}', value):
                        realm = value
                except (OSError, ValueError, UnicodeError):
                    pass
            except (OSError, ValueError):
                return {'running': True, 'logging': None, 'realm': realm}
            finally:
                kernel.CloseHandle(handle)
        return {'running': True, 'logging': active, 'realm': realm}

class RaidWorker(threading.Thread):
    def __init__(self, app_root=ROOT, config=None, status_reader=None):
        super().__init__(daemon=True, name='raid-auto-import')
        self.root = Path(app_root).resolve()
        config_file = self.root/'auto-config.json'
        if not config_file.exists():
            config_file = self.root/'auto-config.example.json'
        self.config = config or json.loads(config_file.read_text('utf-8'))
        self.source = (self.root/self.config['source']).resolve()
        self.raw_archive = (self.root/self.config['raw_archive']).resolve()
        self.raw_archive.mkdir(parents=True, exist_ok=True)
        self.zone = ZoneInfo(self.config['timezone'])
        self.hours = self.config['retention_hours']
        self.state_path = self.root/'auto-state.json'
        try:
            self.state = json.loads(self.state_path.read_text('utf-8'))
        except (FileNotFoundError, ValueError):
            self.state = {'offset': 0, 'session_start': 0}
        self.reader = status_reader or WowStatus(self.source.parent.parent/'wow.exe',
                                                self.config['client_sha256'],
                                                read_logging=not self.config.get('defer_until_client_exit', False))
        self.stop_event = threading.Event()
        self.status = {'state': 'starting', 'retention_hours': self.hours}
        self.last_signature = None
        self.stable_since = time.time()
        self.was_logging = False
        self.last_cleanup = 0
        self.next_retry = 0
        self.report_cache = {}

    def save(self):
        temporary = self.state_path.with_suffix('.tmp')
        temporary.write_text(json.dumps(self.state, indent=2), encoding='utf-8')
        temporary.replace(self.state_path)

    def run(self):
        while not self.stop_event.is_set():
            try:
                self.tick()
            except Exception as error:
                LOGGER.exception('Automatic raid maintenance failed')
                self.status.update(state='error', error=str(error))
                self.next_retry = time.time()+30
            self.stop_event.wait(self.config.get('poll_seconds', 3))

    def tick(self, now=None):
        now = time.time() if now is None else now
        game = self.reader.read()
        self.status.update(game_running=game['running'], recording=game['logging'])
        realm = game.get('realm', '').split(':', 1)[0]
        if realm in self.config['realms']:
            self.state['server'] = self.config['realms'][realm]
        if game['running'] and self.config.get('defer_until_client_exit', False):
            # No scanning, compression, parsing or retention during gameplay.
            self.status['state'] = 'deferred_until_client_exit'
            return
        if now-self.last_cleanup >= 300:
            self.cleanup(now)
            self.refresh_index()
            self.last_cleanup = now
        if not self.source.exists():
            self.status['state'] = 'waiting_for_log'
            self.was_logging = game['logging'] is True
            return
        stat = self.source.stat()
        identity = str(stat.st_ino)
        if self.state.get('identity') != identity or stat.st_size < self.state.get('offset', 0):
            self.state.update(identity=identity, offset=0, session_start=0)
        signature = (identity, stat.st_size, stat.st_mtime_ns)
        if signature != self.last_signature:
            self.last_signature, self.stable_since = signature, now
        if game['logging'] is True:
            if not self.was_logging:
                self.state['session_start'] = self.state.get('offset', 0)
                self.save()
            self.was_logging = True
            self.status['state'] = 'recording'
            if stat.st_size >= 3*1024**3:
                self.status['warning'] = 'Combat log exceeds 3 GiB; close WoW after this raid for rotation.'
            return
        self.was_logging = False
        if game['logging'] is None:
            self.status['state'] = 'waiting_for_logging_status'
            return
        if now < self.next_retry or now-self.stable_since < self.config.get('settle_seconds', 8):
            self.status['state'] = 'waiting_for_file_flush'
            return
        if stat.st_size > self.state.get('offset', 0):
            self.status['state'] = 'importing'
            self.import_log(stat, now)
        # The 3.3.5 file handle survives LoggingCombat(false). Never rename an
        # open game's file: a later raid could keep writing into the old handle.
        if not game['running'] and self.state.get('offset') == self.source.stat().st_size:
            self.rotate(now)
        else:
            self.status['state'] = 'imported_rotation_pending_client_exit'

    def import_log(self, stat, now):
        from c_path import PathExt, Directories
        from logs_upload import UploadData, LogsArchive
        from Z_SERVER import OPENED_LOGS
        cutoff = now-self.hours*3600
        upload = UploadData(ip='127.0.0.1', server=self.state.get('server', 'Unknown'),
                            timezone=self.config['timezone'])
        unique = str(time.time_ns())
        upload.directory = Directories.uploads.new_child('automatic').new_child(unique)
        archive_path = upload.directory/('automatic-raid-'+unique+'.zip')
        end = self.state.get('session_start', 0)
        kept = 0
        with zipfile.ZipFile(archive_path, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
            member = zipfile.ZipInfo('WoWCombatLog.txt',
                datetime.fromtimestamp(stat.st_mtime, self.zone).timetuple()[:6])
            member.compress_type = zipfile.ZIP_DEFLATED
            with archive.open(member, 'w') as output, self.source.open('rb') as source:
                source.seek(end)
                while source.tell() < stat.st_size:
                    line = source.readline()
                    if not line.endswith(b'\n') or source.tell() > stat.st_size:
                        break
                    end = source.tell()
                    timestamp = event_time(line, stat.st_mtime, self.zone)
                    if timestamp is not None and timestamp >= cutoff:
                        output.write(line)
                        kept += 1
        if self.reader.read()['logging'] is not False:
            remove_owned(upload.directory, self.root/'uploads')
            self.status['state'] = 'recording'
            return
        if kept:
            parsed = LogsArchive(PathExt(archive_path), upload_data=upload, forced=True)
            parsed.main()
            if parsed.has_error or any(not item.done for item in parsed.slices.values()):
                raise RuntimeError(f'Raid import failed: {parsed.status}')
            for report_id in parsed.slices:
                OPENED_LOGS.pop(report_id, None)
            self.index_reports(list(parsed.slices))
            self.status.update(last_reports=list(parsed.slices), last_import=now)
            LOGGER.info('Imported %s new/cumulative raid rows: %s', kept, list(parsed.slices))
        else:
            remove_owned(upload.directory, self.root/'uploads')
        self.state.update(offset=end, last_import=now)
        self.save()

    def index_reports(self, report_ids):
        if not report_ids:
            return
        import pandas
        import logs_calendar
        from c_path import CachePath
        with JOB_LOCK:
            # Upstream calendar indexing skips raw-only reports. Materialize the
            # encounter/player metadata first, then atomically update its index.
            rows = {report_id: logs_calendar.report_data(report_id) for report_id in report_ids}
            new = pandas.DataFrame.from_dict(rows, orient='index').astype(logs_calendar.COLUMN_TYPES)
            target = self.root/'_logs_list.pkl'
            old = pandas.read_pickle(target) if target.exists() else pandas.DataFrame()
            frame = pandas.concat([old.drop(report_ids, errors='ignore'), new])
            frame.sort_index(inplace=True)
            if target.exists():
                shutil.copy2(target, self.root/'_logs_list.bkp')
            temporary = self.root/'_logs_list.index-tmp'
            frame.to_pickle(temporary)
            temporary.replace(target)
            CachePath._cache.pop(target, None)
            LOGGER.info('Indexed reports in calendar: %s', report_ids)

    def refresh_index(self):
        import pandas
        target = self.root/'_logs_list.pkl'
        known = set(pandas.read_pickle(target).index) if target.exists() else set()
        logs_root = self.root/'LogsDir'
        missing = [folder.name for folder in logs_root.iterdir()
                   if folder.is_dir() and (folder/'LOGS_CUT.zstd').is_file() and folder.name not in known]
        self.index_reports(missing)

    def rotate(self, now):
        if self.reader.read()['running']:
            self.status['state'] = 'imported_rotation_pending_client_exit'
            return
        self.status['state'] = 'rotating'
        name = datetime.fromtimestamp(now, self.zone).strftime('WoWCombatLog-%Y%m%d-%H%M%S')
        destination = self.raw_archive/(name+'-'+str(time.time_ns())+'.txt')
        try:
            self.source.rename(destination)
        except PermissionError:
            self.status['state'] = 'rotation_waiting_for_file_unlock'
            return
        self.prune_raw(destination, now)
        self.state.update(offset=0, session_start=0, identity=None)
        self.save()
        self.status.update(state='ready', last_rotated=str(destination))
        LOGGER.info('Rotated game journal to %s', destination)

    def prune_raw(self, path, now):
        cutoff = now-self.hours*3600
        reference = path.stat().st_mtime
        with path.open('rb') as source:
            for _ in range(100):
                line = source.readline()
                if not line:
                    break
                stamp = event_time(line, reference, self.zone)
                if stamp is not None:
                    if stamp >= cutoff:
                        return
                    break
        temporary = path.with_suffix('.pruning')
        kept = removed = 0
        with path.open('rb') as source, temporary.open('wb') as target:
            for line in source:
                stamp = event_time(line, reference, self.zone)
                if stamp is not None and stamp < cutoff:
                    removed += 1
                else:
                    target.write(line)
                    kept += 1
        if removed:
            if kept:
                temporary.replace(path)
                os.utime(path, (reference, reference))
            else:
                remove_owned(path, self.raw_archive)
                remove_owned(temporary, self.raw_archive)
        else:
            remove_owned(temporary, self.raw_archive)

    def report_end(self, folder):
        compressed = folder/'LOGS_CUT.zstd'
        if not compressed.exists():
            return folder.stat().st_mtime
        signature = (compressed.stat().st_size, compressed.stat().st_mtime_ns)
        cached = self.report_cache.get(str(folder))
        if cached and cached[0] == signature:
            return cached[1]
        tail = b''
        with compressed.open('rb') as source, zstandard.ZstdDecompressor().stream_reader(source) as reader:
            while True:
                chunk = reader.read(1024*1024)
                if not chunk:
                    break
                tail = (tail+chunk)[-8192:]
        reference = compressed.stat().st_mtime
        for line in reversed(tail.splitlines()):
            stamp = event_time(line, reference, self.zone)
            if stamp is not None:
                self.report_cache[str(folder)] = (signature, stamp)
                return stamp
        return reference

    def cleanup(self, now):
        with JOB_LOCK:
            self._cleanup(now)

    def _cleanup(self, now):
        cutoff = now-self.hours*3600
        expired = []
        logs_root = self.root/'LogsDir'
        for folder in logs_root.iterdir() if logs_root.exists() else []:
            if not folder.is_dir() or folder.is_symlink() or folder.is_junction():
                continue
            if self.report_end(folder) < cutoff:
                expired.append(folder.name)
        from Z_SERVER import OPENED_LOGS
        for report in expired:
            OPENED_LOGS.pop(report, None)
            for base in (logs_root, self.root/'parsed'):
                remove_owned(base/report, base)
            for base in (self.root/'LogsRaw', self.root/'uploads/0archive_pending'):
                if base.exists():
                    for path in base.glob(report+'.*'):
                        remove_owned(path, base)
            remove_owned(logs_root/report, logs_root)
        if expired:
            import pandas
            for name in ('_logs_list.pkl', '_logs_list.bkp'):
                path = self.root/name
                if path.exists():
                    frame = pandas.read_pickle(path)
                    frame = frame.drop(expired, errors='ignore')
                    temporary = path.with_suffix('.retention-tmp')
                    frame.to_pickle(temporary)
                    temporary.replace(path)
        for base in (self.root/'uploads/uploaded', self.root/'uploads/0file_info',
                     self.root/'uploads/0todo', self.root/'uploads/0errors', self.root/'uploads/0no_text',
                     self.root/'uploads/automatic'):
            if base.exists():
                for path in base.rglob('*'):
                    if path.is_file() and path.stat().st_mtime < cutoff:
                        remove_owned(path, base)
        for path in self.raw_archive.glob('WoWCombatLog-*.txt'):
            if path.stat().st_mtime < cutoff:
                remove_owned(path, self.raw_archive)
            else:
                self.prune_raw(path, now)
        self.status.update(last_cleanup=now, deleted_reports=len(expired))
        if expired:
            LOGGER.info('Removed reports older than %s hours: %s', self.hours, expired)
