use crate::combat::{Event, Stamp, player_guid};
use crate::reports::{Encounter, Report};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io;

#[derive(Serialize)]
pub struct Table {
    pub caption: String,
    pub columns: &'static [&'static str],
    pub rows: Vec<Vec<String>>,
}

pub fn analyze(
    report: &mut Report,
    encounters: &[Encounter],
    section: &str,
) -> io::Result<Vec<Table>> {
    let names = report.metadata.players.clone();
    let units = report.metadata.units.clone();
    let name = |guid: &str| {
        names
            .get(guid)
            .or_else(|| units.get(guid).map(|unit| &unit.name))
            .cloned()
            .unwrap_or_else(|| guid.to_owned())
    };
    let year = report.id[..2].parse::<u32>().unwrap_or(0) + 2000;
    let mut tables = Vec::new();
    for encounter in encounters {
        let mut first = None::<Stamp>;
        let mut last = 0;
        let mut shades = BTreeMap::<String, Shade>::new();
        let mut shield = None::<Shield>;
        let mut shields = Vec::new();
        let mut backlash = Backlash::default();
        let mut grabs = Grabs::default();
        report.visit(Some(encounter.start..encounter.end), |row, line| {
            let Some(event) = Event::parse(line) else {
                return;
            };
            let start = *first.get_or_insert(event.stamp);
            let time = event.stamp.relative_to(start, year);
            last = last.max(time);
            match section {
                "lady_spirits" => {
                    if npc(event.source) == "00954E" {
                        shades
                            .entry(event.source.into())
                            .or_default()
                            .observe(&event, time);
                    }
                }
                "toc_valks" => {
                    if shield.as_ref().is_some_and(|shield| {
                        time - shield.start >= 16_000 || event.spell == shield.pact
                    }) {
                        let mut closed = shield.take().unwrap();
                        closed.end = time.min(closed.start + 16_000);
                        shields.push(closed);
                        return;
                    }
                    if let Some(active) = &mut shield {
                        let mut payload = event.payload.rsplit(',');
                        let amount = payload.next().and_then(|value| value.parse::<i64>().ok());
                        if event.kind.ends_with("_MISSED")
                            && payload.next() == Some("ABSORB")
                            && event.target == active.target
                            && let Some(amount) = amount
                        {
                            let owner = units
                                .get(event.source)
                                .and_then(|unit| unit.master_guid.as_deref())
                                .unwrap_or(event.source);
                            if player_guid(owner) {
                                *active.damage.entry(owner.into()).or_default() += amount;
                            }
                        }
                    } else {
                        let pact = if event.kind == "SPELL_CAST_START" {
                            match event.spell {
                                "67303" | "67304" | "67305" | "65875" => Some("Eydis Darkbane"),
                                "67306" | "67307" | "67308" | "65876" => Some("Fjola Lightbane"),
                                _ => None,
                            }
                        } else {
                            None
                        };
                        if let Some(boss) = pact {
                            shield = Some(Shield {
                                pact: event.spell.into(),
                                boss,
                                target: event.source.into(),
                                start: time,
                                end: time,
                                damage: BTreeMap::new(),
                            });
                        }
                    }
                }
                "ucm" => backlash.observe(&event, time),
                "valks" => grabs.observe(&event, row, time),
                _ => {}
            }
        })?;
        let mut result = match section {
            "lady_spirits" => vec![Table {
                caption: "Vengeful Blast".into(),
                columns: &["Время", "Кто взорвал", "Цели", "Урон", "Предотвращено"],
                rows: shades
                    .into_values()
                    .filter(|shade| !shade.targets.is_empty())
                    .map(|shade| {
                        let source = shade.activator.as_deref().or_else(|| {
                            (shade.targets.len() == 1)
                                .then(|| shade.targets.first().unwrap().as_str())
                        });
                        vec![
                            clock(shade.time.unwrap_or(0)),
                            source.map(&name).unwrap_or("—".into()),
                            shade
                                .targets
                                .iter()
                                .map(|guid| name(guid))
                                .collect::<Vec<_>>()
                                .join(", "),
                            shade.damage.to_string(),
                            shade.prevented.to_string(),
                        ]
                    })
                    .collect(),
            }],
            "toc_valks" => {
                if let Some(mut active) = shield {
                    active.end = last;
                    shields.push(active);
                }
                shields
                    .into_iter()
                    .map(|shield| {
                        let mut damage = shield.damage.into_iter().collect::<Vec<_>>();
                        damage.sort_by_key(|(guid, amount)| {
                            (std::cmp::Reverse(*amount), guid.clone())
                        });
                        Table {
                            caption: format!(
                                "{} · {}–{}",
                                shield.boss,
                                clock(shield.start),
                                clock(shield.end)
                            ),
                            columns: &["Игрок", "Урон по щиту"],
                            rows: damage
                                .into_iter()
                                .map(|(guid, amount)| vec![name(&guid), amount.to_string()])
                                .collect(),
                        }
                    })
                    .collect()
            }
            "ucm" => backlash.tables(&name),
            "valks" => grabs.tables(&names),
            _ => Vec::new(),
        };
        for table in &mut result {
            table.caption = format!(
                "{} · Попытка {} · {}",
                encounter.boss,
                encounter.attempt + 1,
                table.caption
            );
        }
        tables.extend(result);
    }
    Ok(tables)
}

fn npc(guid: &str) -> &str {
    guid.get(6..12).unwrap_or_default()
}

fn clock(millis: i64) -> String {
    let ms = millis.max(0);
    format!("{}:{:02}.{}", ms / 60_000, ms / 1000 % 60, ms / 100 % 10)
}

#[derive(Default)]
struct Shade {
    time: Option<i64>,
    activator: Option<String>,
    targets: BTreeSet<String>,
    damage: i64,
    prevented: i64,
}

impl Shade {
    fn observe(&mut self, event: &Event<'_>, time: i64) {
        if !matches!(event.spell, "71544" | "72010" | "72011" | "72012") {
            if player_guid(event.target) {
                self.activator = Some(event.target.into());
            }
            return;
        }
        if !player_guid(event.target) {
            return;
        }
        self.time.get_or_insert(time);
        self.targets.insert(event.target.into());
        let payload = event.payload.split(',').collect::<Vec<_>>();
        if event.damage() {
            self.damage += event.amount;
            self.prevented += number(&payload, 4) + number(&payload, 6);
        } else {
            self.prevented += number(&payload, 2);
        }
    }
}

fn number(fields: &[&str], index: usize) -> i64 {
    fields
        .get(index)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

struct Shield {
    pact: String,
    boss: &'static str,
    target: String,
    start: i64,
    end: i64,
    damage: BTreeMap<String, i64>,
}

#[derive(Default)]
struct Backlash {
    stacks: BTreeMap<String, i64>,
    removals: BTreeMap<String, Vec<(i64, i64)>>,
    hits: BTreeMap<(String, bool), Vec<Hit>>,
}

struct Hit {
    time: i64,
    target: String,
    actual: i64,
    prevented: i64,
    overkill: i64,
}

impl Backlash {
    fn observe(&mut self, event: &Event<'_>, time: i64) {
        if event.spell == "69766" {
            match event.kind {
                "SPELL_AURA_APPLIED"
                | "SPELL_AURA_APPLIED_DOSE"
                | "SPELL_AURA_REMOVED_DOSE"
                | "SPELL_AURA_REFRESH" => {
                    let count = event
                        .payload
                        .rsplit(',')
                        .next()
                        .and_then(|count| count.parse().ok())
                        .unwrap_or(1);
                    self.stacks.insert(event.target.to_owned(), count);
                }
                "SPELL_AURA_REMOVED" => {
                    if let Some(count) = self.stacks.remove(event.target) {
                        self.removals
                            .entry(event.target.to_owned())
                            .or_default()
                            .push((time, count));
                    }
                }
                _ => {}
            }
        }
        if matches!(event.spell, "71046" | "71045")
            && (event.damage() || event.kind.ends_with("_MISSED"))
        {
            let payload = event.payload.split(',').collect::<Vec<_>>();
            let (actual, prevented, overkill) = if event.damage() {
                (
                    event.amount - event.excess,
                    number(&payload, 4) + number(&payload, 6),
                    event.excess,
                )
            } else {
                (0, number(&payload, 2), 0)
            };
            self.hits
                .entry((event.source.to_owned(), event.spell == "71046"))
                .or_default()
                .push(Hit {
                    time,
                    target: event.target.to_owned(),
                    actual,
                    prevented,
                    overkill,
                });
        }
    }

    fn tables(self, name: &impl Fn(&str) -> String) -> Vec<Table> {
        let normal_id = self.hits.keys().any(|(_, id)| *id);
        let mut tables = Vec::new();
        for ((source, id), hits) in self.hits {
            if id != normal_id {
                continue;
            }
            let from_boss = npc(&source) == "008FF5";
            let bucket = if from_boss { 50 } else { 1000 };
            let mut start = 0;
            while start < hits.len() {
                let mut end = start + 1;
                while end < hits.len() && hits[end].time - hits[end - 1].time <= bucket {
                    end += 1;
                }
                let time = hits[start].time;
                let owner = if from_boss {
                    self.removals
                        .iter()
                        .flat_map(|(guid, removals)| {
                            removals.iter().filter_map(move |&(removed, count)| {
                                (0..200).contains(&(removed - time)).then_some((
                                    guid,
                                    count,
                                    (removed - time).abs(),
                                ))
                            })
                        })
                        .min_by_key(|(_, _, delta)| *delta)
                        .map(|(guid, count, _)| (guid.as_str(), count))
                } else {
                    self.removals
                        .get(&source)
                        .and_then(|removals| {
                            removals
                                .iter()
                                .find(|(removed, _)| (*removed - time).abs() < 4000)
                        })
                        .map(|(_, count)| (source.as_str(), *count))
                };
                if let Some((owner, stacks)) = owner.or_else(|| {
                    (!from_boss && self.removals.contains_key(&source))
                        .then_some((source.as_str(), 0))
                }) {
                    let mut targets = BTreeMap::<&str, [i64; 3]>::new();
                    for hit in &hits[start..end] {
                        let total = targets.entry(&hit.target).or_default();
                        total[0] += hit.actual;
                        total[1] += hit.prevented;
                        total[2] += hit.overkill;
                    }
                    let sum = |column: usize| {
                        targets
                            .values()
                            .map(|value| value[column])
                            .sum::<i64>()
                            .to_string()
                    };
                    let mut rows = vec![vec!["Всего".into(), sum(0), sum(1), sum(2)]];
                    rows.extend(targets.into_iter().map(|(target, counts)| {
                        vec![
                            name(target),
                            counts[0].to_string(),
                            counts[1].to_string(),
                            counts[2].to_string(),
                        ]
                    }));
                    tables.push((
                        time,
                        Table {
                            caption: format!(
                                "{} · {} · {} стаков",
                                clock(time),
                                name(owner),
                                stacks
                            ),
                            columns: &["Цель", "Урон", "Предотвращено", "Избыточно"],
                            rows,
                        },
                    ));
                }
                start = end;
            }
        }
        tables.sort_by_key(|(time, _)| *time);
        tables.into_iter().map(|(_, table)| table).collect()
    }
}

#[derive(Default)]
struct Grabs {
    start: Option<u64>,
    end: Option<u64>,
    summons: Vec<(String, u64, i64)>,
    first_cast: BTreeMap<String, u64>,
    actions: Vec<(u64, String)>,
    drops: Vec<(u64, String)>,
    tank_hits: BTreeMap<String, usize>,
}

impl Grabs {
    fn observe(&mut self, event: &Event<'_>, row: u64, time: i64) {
        if self.end.is_some() {
            return;
        }
        if npc(event.source) == "008EF5" || npc(event.target) == "008EF5" {
            match (event.kind, event.spell) {
                ("SPELL_AURA_REMOVED", "68981" | "74271" | "74270" | "74272") => {
                    self.start = Some(row)
                }
                ("SPELL_AURA_APPLIED", "72259" | "74274" | "74273" | "74275") => {
                    self.end = Some(row)
                }
                _ => {}
            }
        }
        if self.start.is_none() {
            if event.kind.starts_with("SWING_")
                && matches!(npc(event.source), "008EF5" | "009342" | "008F5D")
                && player_guid(event.target)
            {
                *self.tank_hits.entry(event.target.into()).or_default() += 1;
            }
            return;
        }
        if event.kind == "SPELL_SUMMON" && npc(event.target) == "008F01" {
            self.summons.push((event.target.into(), row, time));
        }
        if event.kind == "SPELL_CAST_START" && npc(event.source) == "008F01" {
            self.first_cast.entry(event.source.into()).or_insert(row);
        }
        if matches!(
            event.kind,
            "SPELL_CAST_START"
                | "SPELL_CAST_SUCCESS"
                | "SWING_DAMAGE"
                | "RANGE_DAMAGE"
                | "ENVIRONMENTAL_DAMAGE"
        ) && player_guid(event.source)
        {
            self.actions.push((row, event.source.into()));
        }
        if event.kind == "ENVIRONMENTAL_DAMAGE"
            && event.spell_name == "FALLING"
            && player_guid(event.target)
        {
            self.drops.push((row, event.target.into()));
        }
    }

    fn tables(self, names: &BTreeMap<String, String>) -> Vec<Table> {
        let mut tanks = self.tank_hits.into_iter().collect::<Vec<_>>();
        tanks.sort_by_key(|(guid, count)| (std::cmp::Reverse(*count), guid.clone()));
        let tanks = tanks
            .into_iter()
            .take(2)
            .map(|(guid, _)| guid)
            .collect::<BTreeSet<_>>();
        let Some((_, first, _)) = self.summons.first() else {
            return Vec::new();
        };
        let candidates = self
            .actions
            .iter()
            .filter(|(row, guid)| row < first && !tanks.contains(guid) && names.contains_key(guid))
            .map(|(_, guid)| guid.as_str())
            .collect::<BTreeSet<_>>();
        let size = if candidates.len() < 11 { 1 } else { 3 };
        let waves = self
            .summons
            .chunks(size)
            .filter_map(|wave| {
                let (_, start, time) = wave.last()?;
                let end = wave
                    .iter()
                    .filter_map(|(guid, _, _)| self.first_cast.get(guid))
                    .copied()
                    .min()?;
                (end > *start).then_some((*start, end, *time))
            })
            .collect::<Vec<_>>();
        let mut rows = Vec::new();
        for (index, &(start, end, time)) in waves.iter().enumerate() {
            let next = waves
                .get(index + 1)
                .map(|wave| wave.0)
                .or(self.end)
                .unwrap_or(u64::MAX);
            let next = if next == u64::MAX {
                self.actions
                    .last()
                    .map_or(end, |event| event.0.saturating_add(1))
            } else {
                next
            };
            let after = end + next.saturating_sub(end) / 4;
            let fall_end = end + next.saturating_sub(end) / 2;
            let counts = |from: u64, to: u64| {
                let first = self.actions.partition_point(|(row, _)| *row < from);
                let mut count = BTreeMap::<&str, usize>::new();
                for (_, guid) in self.actions[first..]
                    .iter()
                    .take_while(|(row, _)| *row < to)
                {
                    *count.entry(guid.as_str()).or_default() += 1;
                }
                count
            };
            let activity = counts(start, end);
            let afterwards = counts(end, after.max(end.saturating_add(1)));
            let drops = self
                .drops
                .iter()
                .filter(|(row, _)| (end..fall_end).contains(row))
                .map(|(_, guid)| guid.as_str())
                .collect::<BTreeSet<_>>();
            let mut ranked = candidates
                .iter()
                .filter_map(|&guid| {
                    let actions = activity.get(guid).copied().unwrap_or_default();
                    (drops.contains(guid) || actions > 0 || afterwards.contains_key(guid))
                        .then_some((guid, actions, drops.contains(guid)))
                })
                .collect::<Vec<_>>();
            ranked.sort_by_key(|&(guid, actions, dropped)| (!dropped, actions, guid));
            for (guid, actions, dropped) in ranked.into_iter().take(size) {
                rows.push(vec![
                    (index + 1).to_string(),
                    clock(time),
                    names[guid].clone(),
                    actions.to_string(),
                    if dropped {
                        "Падение".into()
                    } else {
                        "По активности".into()
                    },
                ]);
            }
        }
        vec![Table {
            caption: "Волны валь’кир".into(),
            columns: &["Волна", "Время", "Игрок", "Действия", "Признак"],
            rows,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shade_and_backlash_account_for_mitigation() {
        let shade_event = Event::parse("1/19 19:45:07.755,SPELL_DAMAGE,0xF13000954E00020A,Vengeful Shade,0x06000000005D571D,Sana,72012,Vengeful Blast,0x30,14156,11970,48,6066,0,700,0,0,0").unwrap();
        let mut shade = Shade::default();
        shade.observe(&shade_event, 50);
        assert_eq!(
            (shade.damage, shade.prevented, shade.targets.len()),
            (14156, 6766, 1)
        );
        let guid = "0x0600000000526D6E";
        let mut backlash = Backlash::default();
        for (time, kind, spell, payload) in [
            (1, "SPELL_AURA_APPLIED_DOSE", "69766", "0x40,DEBUFF,8"),
            (15, "SPELL_AURA_REMOVED", "69766", "0x40,DEBUFF"),
            (18, "SPELL_DAMAGE", "71046", "0x40,22349,0,64,5587,0,0"),
        ] {
            let line = format!(
                "1/19 21:49:03.234,{kind},{guid},Player,{guid},Player,{spell},Instability,{payload}"
            );
            backlash.observe(&Event::parse(&line).unwrap(), time);
        }
        let output = backlash.tables(&str::to_owned);
        assert!(output[0].caption.contains("8 стаков"));
        assert_eq!(output[0].rows[0], ["Всего", "22349", "5587", "0"]);
    }
}
