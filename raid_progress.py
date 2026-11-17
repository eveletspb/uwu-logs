"""Read bounded, versioned progress records from RaidAutoLog (never execute Lua)."""
import re
from pathlib import Path

TOTALS = (9, 6, 12, 5, 14, 15, 1, 1, 1, 1, 4, 10, 8, 10)
ALIASES = {
    'Temple of Ahn\'Qiraj': 0, 'Ahn\'Qiraj': 0, 'Храм Ан\'Киража': 0,
    'Ruins of Ahn\'Qiraj': 1, 'Руины Ан\'Киража': 1,
    'Icecrown Citadel': 2, 'Цитадель Ледяной Короны': 2,
    'Trial of the Crusader': 3, 'Испытание крестоносца': 3,
    'Ulduar': 4, 'Ульдуар': 4, 'Naxxramas': 5, 'Наксрамас': 5,
    'The Ruby Sanctum': 6, 'Рубиновое святилище': 6,
    'The Obsidian Sanctum': 7, 'Обсидиановое святилище': 7,
    'The Eye of Eternity': 8, 'Око Вечности': 8,
    "Onyxia's Lair": 9, 'Логово Ониксии': 9,
    'Vault of Archavon': 10, 'Склеп Аркавона': 10,
    'Molten Core': 11, 'Огненные Недра': 11,
    'Blackwing Lair': 12, 'Логово Крыла Тьмы': 12,
    "Zul'Gurub": 13, 'Зул\'Гуруб': 13,
}
RECORD = re.compile(r'"RAL1\|([0-9|a-f]+)"')

def read_snapshots(client, realms=None):
    rows = []
    for path in Path(client).glob('WTF/Account/*/SavedVariables/RaidAutoLog.lua'):
        if path.stat().st_size > 2_000_000:
            continue
        for match in RECORD.finditer(path.read_text('utf-8', errors='replace')):
            try:
                stamp, realm, player, instance, total, killed, session = match[1].split('|')
                row = dict(time=int(stamp), realm=bytes.fromhex(realm).decode(),
                    player=bytes.fromhex(player).decode(), instance=bytes.fromhex(instance).decode(),
                    total=int(total), killed=int(killed), session=int(session))
                row['realm'] = (realms or {}).get(row['realm'].split(':', 1)[0], row['realm'])
                if 0 <= row['killed'] <= row['total'] <= 100:
                    rows.append(row)
            except (ValueError, UnicodeError):
                continue
    return rows

def progress_for(raid_index, kills, author, server, start, end, snapshots):
    total = TOTALS[raid_index]
    candidates = [r for r in snapshots if ALIASES.get(r['instance']) == raid_index
        and r['player'].casefold() == author.casefold()
        and r['realm'].casefold() == server.casefold()
        and start - 12*3600 <= r['session'] <= start + 60
        and r['session'] <= r['time'] <= end + 60]
    if candidates:
        session = max(r['session'] for r in candidates)
        latest = max((r for r in candidates if r['session'] == session), key=lambda r:r['time'])
        return f"{latest['killed']}/{latest['total']}", 'Прогресс сохранения, включая боссов до входа в рейд'
    # A partial combat log cannot prove which bosses were dead before joining.
    return f'≥{kills}/{total}', 'Минимум по логу. Прогресс до входа неизвестен'
