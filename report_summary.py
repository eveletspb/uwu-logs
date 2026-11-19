"""Numeric export of the same encounter windows used by the report viewer."""
from collections import Counter, defaultdict

from logs_dmg_heals import add_pets_guids

METRICS = ("damage", "healing", "damage_taken", "deaths")


def with_rates(totals, duration):
    return dict(totals, **{
        rate: round(totals[key] / duration, 2) if duration > 0 else None
        for rate, key in (("dps", "damage"), ("hps", "healing"), ("dtps", "damage_taken"))
    })


def report_summary(report):
    names = report.PLAYERS_GUIDS
    raid_players = defaultdict(Counter)
    encounters = []
    duration = 0
    for segment in sorted((s for group in report.SEGMENTS.values() for s in group),
                          key=lambda segment: segment.start):
        start, end = segment.start, segment.end
        seconds = segment.duration
        damage_heal = report.get_slice_damage_heal_absorbs(start, end)
        outgoing = []
        for key in ("damage", "heal"):
            units = add_pets_guids(damage_heal[key], report.ALL_GUIDS)
            outgoing.append(Counter(units["players"]) + Counter(units["custom"]))
        damage, healing = outgoing

        deaths = Counter()
        participants = set(damage) | set(healing)
        for line in report.LOGS[start:end]:
            _, event, source, _, target, _ = line.split(",", 5)
            participants.update((source, target))
            if event == "UNIT_DIED":
                deaths[target] += 1

        players = {}
        totals = dict.fromkeys(METRICS, 0)
        for guid in sorted(participants & names.keys()):
            values = dict(damage=damage[guid], healing=healing[guid],
                          damage_taken=damage_heal["taken"].get(guid, 0), deaths=deaths[guid])
            players[guid] = dict(name=names[guid], **with_rates(values, seconds))
            raid_players[guid].update(values)
            for key in METRICS:
                totals[key] += values[key]

        duration += seconds
        encounters.append(dict(boss=segment.encounter_name, boss_slug=segment.encounter_name_html,
                               difficulty=segment.difficulty, attempt=segment.attempt,
                               outcome=segment.attempt_type, duration_seconds=seconds,
                               report_path=f"/reports/{report.NAME}/{segment.href}",
                               totals=with_rates(totals, seconds), players=players))

    totals = {key: sum(player[key] for player in raid_players.values()) for key in METRICS}
    return dict(schema_version=1, report_id=report.NAME, duration_seconds=round(duration, 3),
                totals=with_rates(totals, duration), encounters=encounters,
                players={guid: dict(name=names[guid], **with_rates(values, duration))
                         for guid, values in raid_players.items()})
