use crate::absorbs::Absorbs;
use crate::combat::{Event, NIL_GUID};
use crate::reports::{Encounter, Report, Unit};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io;
use std::ops::{AddAssign, Range};

#[derive(Serialize, Clone, Default)]
pub struct Totals {
    pub damage: i64,
    pub healing: i64,
    pub damage_taken: i64,
    pub deaths: i64,
}

impl AddAssign<&Self> for Totals {
    fn add_assign(&mut self, other: &Self) {
        self.damage += other.damage;
        self.healing += other.healing;
        self.damage_taken += other.damage_taken;
        self.deaths += other.deaths;
    }
}

#[derive(Serialize)]
pub struct Rates {
    #[serde(flatten)]
    pub totals: Totals,
    pub dps: Option<f64>,
    pub hps: Option<f64>,
    pub dtps: Option<f64>,
}

impl Totals {
    fn rates(self, seconds: f64) -> Rates {
        let value = |amount| (seconds > 0.0).then(|| round(amount as f64 / seconds, 100.0));
        Rates {
            dps: value(self.damage),
            hps: value(self.healing),
            dtps: value(self.damage_taken),
            totals: self,
        }
    }
}

fn round(value: f64, precision: f64) -> f64 {
    (value * precision).round() / precision
}

#[derive(Serialize)]
pub struct Player {
    pub name: String,
    pub class: String,
    #[serde(flatten)]
    pub rates: Rates,
}

#[derive(Serialize)]
pub struct EncounterSummary {
    pub boss: String,
    pub boss_slug: String,
    pub difficulty: String,
    pub attempt: usize,
    pub outcome: String,
    pub duration_seconds: f64,
    pub report_path: String,
    pub totals: Rates,
    pub players: BTreeMap<String, Player>,
}

#[derive(Serialize)]
pub struct Summary {
    pub schema_version: u8,
    pub report_id: String,
    pub encounters: Vec<EncounterSummary>,
    pub duration_seconds: f64,
    pub totals: Rates,
    pub players: BTreeMap<String, Player>,
}

fn add_event(
    totals: &mut BTreeMap<String, Totals>,
    roster: &BTreeMap<String, String>,
    units: &BTreeMap<String, Unit>,
    event: &Event<'_>,
) {
    for guid in [event.source, event.target] {
        if roster.contains_key(guid) && !totals.contains_key(guid) {
            totals.insert(guid.into(), Totals::default());
        }
    }
    let source = units
        .get(event.source)
        .and_then(|unit| unit.master_guid.as_deref())
        .unwrap_or(event.source);
    let friendly = |guid| {
        roster.contains_key(guid)
            || units
                .get(guid)
                .and_then(|unit| unit.master_guid.as_deref())
                .is_some_and(|owner| roster.contains_key(owner))
    };
    if event.damage() {
        if let Some(record) = totals.get_mut(event.target) {
            record.damage_taken += event.amount;
        }
        if event.target != NIL_GUID && !friendly(event.target) && roster.contains_key(source) {
            totals.entry(source.into()).or_default().damage += event.amount;
        }
    } else if event.healing() && roster.contains_key(source) {
        totals.entry(source.into()).or_default().healing +=
            event.amount.saturating_sub(event.excess);
    } else if event.kind == "UNIT_DIED"
        && let Some(record) = totals.get_mut(event.target)
    {
        record.deaths += 1;
    }
}

pub fn aggregate(report: &mut Report, encounters: &[Encounter]) -> io::Result<Summary> {
    let roster = report.metadata.players.clone();
    let units = report.metadata.units.clone();
    let classes = report.metadata.classes.clone();
    let year = report.id[..2].parse::<u32>().unwrap_or(0) + 2000;
    let mut windows = encounters
        .iter()
        .map(|encounter| {
            (
                encounter,
                BTreeMap::<String, Totals>::new(),
                Some(Absorbs::default()),
            )
        })
        .collect::<Vec<_>>();
    let finalize = |players: &mut BTreeMap<String, Totals>, absorbs: Absorbs| {
        for (source, value) in absorbs.finish(&classes, year) {
            let source = units
                .get(&source)
                .and_then(|unit| unit.master_guid.as_deref())
                .unwrap_or(&source);
            if roster.contains_key(source) {
                players.entry(source.into()).or_default().healing += value;
            }
        }
    };
    if let Some(first) = encounters.first() {
        let end = encounters
            .iter()
            .map(|encounter| encounter.end)
            .max()
            .unwrap_or(first.end);
        let mut next = 0;
        let mut current = Vec::<usize>::new();
        report.visit(Some(first.start..end), |row, line| {
            current.retain(|index| {
                let (window, totals, absorbs) = &mut windows[*index];
                if row < window.end {
                    return true;
                }
                if let Some(absorbs) = absorbs.take() {
                    finalize(totals, absorbs);
                }
                false
            });
            while next < encounters.len() && encounters[next].start <= row {
                current.push(next);
                next += 1;
            }
            if let Some(event) = Event::parse(line) {
                for index in &current {
                    let (window, totals, absorbs) = &mut windows[*index];
                    add_event(totals, &roster, &units, &event);
                    if let Some(absorbs) = absorbs {
                        absorbs.observe(row - window.start, &event, &classes);
                    }
                }
            }
        })?;
    }
    let class = |guid: &str| {
        report
            .metadata
            .classes
            .get(guid)
            .cloned()
            .unwrap_or_default()
    };
    let mut duration = 0.0;
    let mut players = BTreeMap::<String, Totals>::new();
    let mut report_totals = Totals::default();
    let mut rows = Vec::new();
    for (window, mut window_players, absorbs) in windows {
        if let Some(absorbs) = absorbs {
            finalize(&mut window_players, absorbs);
        }
        let mut result = BTreeMap::new();
        let mut totals = Totals::default();
        for (guid, total) in window_players {
            totals += &total;
            *players.entry(guid.clone()).or_default() += &total;
            result.insert(
                guid.clone(),
                Player {
                    name: roster[&guid].clone(),
                    class: class(&guid),
                    rates: total.rates(window.duration_seconds),
                },
            );
        }
        duration += window.duration_seconds;
        report_totals += &totals;
        rows.push(EncounterSummary {
            boss: window.boss.clone(),
            boss_slug: window.boss_slug.clone(),
            attempt: window.attempt,
            difficulty: window.difficulty.clone(),
            outcome: window.outcome.clone(),
            duration_seconds: window.duration_seconds,
            report_path: format!("/reports/{}/{}", report.id, window.query()),
            totals: totals.rates(window.duration_seconds),
            players: result,
        });
    }
    let players = players
        .into_iter()
        .map(|(guid, totals)| {
            let player = Player {
                name: roster[&guid].clone(),
                class: class(&guid),
                rates: totals.rates(duration),
            };
            (guid, player)
        })
        .collect();
    Ok(Summary {
        schema_version: 1,
        report_id: report.id.clone(),
        duration_seconds: round(duration, 1000.0),
        totals: report_totals.rates(duration),
        encounters: rows,
        players,
    })
}

#[derive(Default, Serialize)]
pub struct Spell {
    pub name: String,
    pub damage: i64,
    pub healing: i64,
    pub taken: i64,
    pub casts: u64,
}

pub fn player_spells(
    report: &mut Report,
    windows: &[Range<u64>],
    name: &str,
) -> io::Result<BTreeMap<String, Spell>> {
    let Some((guid, _)) = report
        .metadata
        .players
        .iter()
        .find(|(guid, player)| name == *guid || name == *player)
    else {
        return Err(io::ErrorKind::NotFound.into());
    };
    let guid = guid.clone();
    let units = report.metadata.units.clone();
    let mut spells = BTreeMap::<String, Spell>::new();
    for window in windows {
        report.visit(Some(window.clone()), |_, line| {
            let Some(event) = Event::parse(line) else {
                return;
            };
            let outgoing = event.source == guid
                || units
                    .get(event.source)
                    .and_then(|unit| unit.master_guid.as_deref())
                    == Some(guid.as_str());
            if event.spell.is_empty() || !(outgoing || event.target == guid && event.damage()) {
                return;
            }
            let data = spells.entry(event.spell.into()).or_insert_with(|| Spell {
                name: event.spell_name.into(),
                ..Spell::default()
            });
            if outgoing {
                if event.damage() {
                    data.damage += event.amount.saturating_sub(event.excess);
                } else if event.healing() {
                    data.healing += event.amount.saturating_sub(event.excess);
                } else if event.kind == "SPELL_CAST_SUCCESS" {
                    data.casts += 1;
                }
            }
            if event.target == guid && event.damage() {
                data.taken += event.amount;
            }
        })?;
    }
    Ok(spells)
}

pub fn dps(
    report: &mut Report,
    encounters: &[Encounter],
    player: Option<&str>,
    interval: u64,
) -> io::Result<BTreeMap<String, f64>> {
    use crate::combat::Stamp;
    let roster = report.metadata.players.clone();
    let units = report.metadata.units.clone();
    let selected = player
        .filter(|name| !name.is_empty())
        .and_then(|name| {
            roster
                .iter()
                .find(|(guid, player)| name == *guid || name == *player)
        })
        .map(|(guid, _)| guid);
    let year = report.id[..2].parse::<u32>().unwrap_or(0) + 2000;
    let mut points = BTreeMap::<u64, i64>::new();
    let mut total_duration = 0u64;
    for encounter in encounters {
        let mut origin = None::<Stamp>;
        report.visit(Some(encounter.start..encounter.end), |_, line| {
            let Some(event) = Event::parse(line) else {
                return;
            };
            let start = *origin.get_or_insert(event.stamp);
            if !matches!(
                event.kind,
                "SWING_DAMAGE"
                    | "RANGE_DAMAGE"
                    | "SPELL_DAMAGE"
                    | "SPELL_PERIODIC_DAMAGE"
                    | "DAMAGE_SHIELD"
            ) {
                return;
            }
            let owner = |guid| {
                units
                    .get(guid)
                    .and_then(|unit| unit.master_guid.as_deref())
                    .unwrap_or(guid)
            };
            let attacker = owner(event.source);
            if !(selected.is_none_or(|guid| guid == attacker) && roster.contains_key(attacker))
                || roster.contains_key(owner(event.target))
            {
                return;
            }
            let offset = event.stamp.relative_to(start, year).max(0) as u64;
            *points.entry((total_duration + offset) / 1000).or_default() += event.amount;
        })?;
        total_duration += (encounter.duration_seconds * 10.0).round() as u64 * 100 + 100;
    }
    let Some((&last, _)) = points.last_key_value() else {
        return Ok(BTreeMap::new());
    };
    let format = |seconds: u64| format!("{:02}:{:02}", seconds / 60, seconds % 60);
    let mut output = BTreeMap::new();
    let mut current = 0;
    if interval > 0 {
        output.insert(format(0), 0.0);
    }
    for second in 0..=last {
        current += points.get(&second).copied().unwrap_or(0);
        if interval == 0 {
            output.insert(
                format(second),
                round(current as f64 / (second + 1) as f64, 10.0),
            );
        } else if (second + 1).is_multiple_of(interval) {
            output.insert(
                format(second + 1),
                round(current as f64 / interval as f64, 10.0),
            );
            current = 0;
        }
    }
    Ok(output)
}
