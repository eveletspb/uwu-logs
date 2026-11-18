"""Small view model for the existing locally indexed raid reports."""
from pathlib import Path
from datetime import datetime
import pandas
from raid_labels import raid_label
from raid_labels import RAIDS
from raid_fragments import JoinedRaid, grouped_index, raid_groups
from raid_progress import read_snapshots, progress_for
from zoneinfo import ZoneInfo
import json

ROOT = Path(__file__).resolve().parent
_cache_signature = None
_cache_rows = []

def recent_reports():
    global _cache_signature, _cache_rows
    index = ROOT/'_logs_list.pkl'
    if not index.exists():
        return []
    saved = list(ROOT.parent.glob('WTF/Account/*/SavedVariables/RaidAutoLog.lua'))
    signature = (index.stat().st_mtime_ns, index.stat().st_size,
                 tuple((str(p), p.stat().st_mtime_ns) for p in saved))
    frame = pandas.read_pickle(index)
    groups = raid_groups(frame)
    signature += (tuple(groups.items()),)
    if signature == _cache_signature:
        return _cache_rows
    frame = grouped_index(frame)
    rows = []
    config = ROOT/'auto-config.json'
    settings = json.loads(config.read_text('utf-8')) if config.exists() else {}
    snapshots = read_snapshots(ROOT.parent, settings.get('realms', {}))
    zone = ZoneInfo(settings.get('timezone', 'UTC'))
    months = ('янв', 'фев', 'мар', 'апр', 'мая', 'июн', 'июл', 'авг', 'сен', 'окт', 'ноя', 'дек')
    for report_id, entry in frame.sort_index(ascending=False).head(100).iterrows():
        started = datetime.strptime('--'.join(str(report_id).split('--')[:2]), '%y-%m-%d--%H-%M')
        progress, progress_note = '—', 'Не удалось определить рейд'
        matches = [i for i, (_, bosses) in enumerate(RAIDS) if set(entry.get('fight', ())) & bosses]
        if len(matches) == 1:
            from logs_main import THE_LOGS
            parts = groups.get(report_id)
            report = JoinedRaid(parts) if parts else THE_LOGS(str(report_id))
            kills = len({segment.encounter_name for segment in report.SEGMENTS_KILLS})
            stamp = started.replace(tzinfo=zone).timestamp()
            progress, progress_note = progress_for(matches[0], kills, str(entry.get('author', '')),
                str(entry.get('server', '')), stamp, stamp + report.get_slice_duration(0, len(report.LOGS)), snapshots)
        rows.append({'id': str(report_id), 'author': entry.get('author', 'Рейд'),
                     'server': entry.get('server', ''), 'date': f'{started.day} {months[started.month-1]}',
                     'datetime': started.isoformat(), 'year': started.year,
                     'progress': progress, 'progress_note': progress_note,
                     'time': started.strftime('%H:%M'), 'players': len(entry.get('player', ())),
                     'raid_label': raid_label(entry.get('fight', ())),
                     'fights': ' · '.join(map(str, entry.get('fight', ())))})
    _cache_signature, _cache_rows = signature, rows
    return rows
