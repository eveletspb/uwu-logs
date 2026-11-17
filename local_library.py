"""Small view model for the existing locally indexed raid reports."""
from pathlib import Path
from datetime import datetime
import pandas
from raid_labels import raid_label

ROOT = Path(__file__).resolve().parent
_cache_signature = None
_cache_rows = []

def recent_reports():
    global _cache_signature, _cache_rows
    index = ROOT/'_logs_list.pkl'
    if not index.exists():
        return []
    signature = (index.stat().st_mtime_ns, index.stat().st_size)
    if signature == _cache_signature:
        return _cache_rows
    frame = pandas.read_pickle(index)
    rows = []
    for report_id, entry in frame.sort_index(ascending=False).head(100).iterrows():
        started = datetime.strptime('--'.join(str(report_id).split('--')[:2]), '%y-%m-%d--%H-%M')
        rows.append({'id': str(report_id), 'author': entry.get('author', 'Рейд'),
                     'server': entry.get('server', ''), 'date': started.strftime('%d.%m'),
                     'time': started.strftime('%H:%M'), 'players': len(entry.get('player', ())),
                     'raid_label': raid_label(entry.get('fight', ())),
                     'fights': ' · '.join(map(str, entry.get('fight', ())))})
    _cache_signature, _cache_rows = signature, rows
    return rows
