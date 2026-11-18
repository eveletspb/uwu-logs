"""Join fragments of a recorded raid while preserving the original reports."""
from collections import defaultdict
from functools import lru_cache
from tempfile import TemporaryDirectory
import json

from c_path import Directories, FileNames, PathExt
from logs_main import THE_LOGS
from raid_labels import RAIDS


@lru_cache(maxsize=1)
def _groups(report_rows, archives):
    provenance = defaultdict(set)
    for path, _ in archives:
        try:
            data = json.loads(path.read_text())
        except (FileNotFoundError, json.JSONDecodeError):
            continue
        for report_id in data['slices'] if isinstance(data.get('slices'), dict) else ():
            provenance[report_id].add(path)

    candidates = defaultdict(list)
    for report_id, author, server, players, fights, _ in report_rows:
        raids = [i for i, (_, bosses) in enumerate(RAIDS) if set(fights) & bosses]
        if len(raids) == 1 and players and provenance[report_id]:
            candidates[(author, server, report_id[:2], raids[0], frozenset(players))].append(report_id)

    groups = {}
    for ids in candidates.values():
        if len(ids) < 2:
            continue
        group, seen_archives, kills, end = [], set(), set(), None
        for report_id in ids:
            report = THE_LOGS(report_id)
            first = report.LOGS[0]
            fragment_kills = {segment.encounter_name for segment in report.SEGMENTS_KILLS}
            shared = seen_archives & provenance[report_id]
            # shortcut: legacy logs lack a lockout ID, use archive and session continuity until IDs are recorded.
            if not (shared and not kills & fragment_kills and
                    0 < report.get_timedelta_seconds(end, first) <= 30 * 60):
                if len(group) > 1:
                    groups[group[0]] = tuple(group)
                group, kills, shared = [], set(), provenance[report_id]
            group.append(report_id)
            seen_archives = shared
            kills.update(fragment_kills)
            end = report.LOGS[-1]
        if len(group) > 1:
            groups[group[0]] = tuple(group)
    return groups


def raid_groups(frame):
    if frame.empty:
        return {}
    archives = tuple(sorted((p, p.stat().st_mtime_ns)
                            for p in Directories.info_uploads.rglob('*.json') if p.is_file()))
    rows = []
    for report_id, row in frame.sort_index().iterrows():
        log = Directories.logs / report_id / FileNames.logs_cut
        try:
            revision = log.stat().st_mtime_ns
        except FileNotFoundError:
            continue
        rows.append((report_id, row.author, row.server, tuple(row.player), tuple(row.fight), revision))
    return _groups(tuple(rows), archives)


def grouped_index(frame):
    groups = raid_groups(frame)
    if not groups:
        return frame
    result = frame.drop(report_id for parts in groups.values() for report_id in parts[1:]).copy()
    for anchor, parts in groups.items():
        for column in ('fight', 'player'):
            result.at[anchor, column] = tuple(dict.fromkeys(
                value for fragment in parts for value in frame.at[fragment, column]))
    return result


class JoinedRaid(THE_LOGS):
    @staticmethod
    def revisions(parts):
        return tuple((Directories.logs / part / FileNames.logs_cut).stat().st_mtime_ns
                     for part in parts)

    def __init__(self, parts):
        super().__init__(parts[0])
        self.parts = parts
        self.source_revisions = self.revisions(parts)
        self._metadata = TemporaryDirectory(prefix='uwu-raid-')
        self._fragments = [THE_LOGS(part) for part in parts]

    @property
    def path(self):
        return PathExt(self._metadata.name)

    def _open_logs(self):
        return [line for fragment in self._fragments for line in fragment.LOGS]

    def _get_enc_data(self):
        encounters = defaultdict(list)
        offset = 0
        for fragment in self._fragments:
            for boss, attempts in fragment.ENCOUNTER_DATA.items():
                encounters[boss].extend((s + offset, f + offset) for s, f in attempts)
            offset += len(fragment.LOGS)
        return dict(encounters)

    def parse_request_custom_slice(self, query):
        if not query.start and not query.end:
            return self.parse_request_all_bosses()
        return super().parse_request_custom_slice(query)
