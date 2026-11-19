use crate::appearance::school;
use crate::combat::{Event, Stamp};
use crate::reports::{Encounter, Report};
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::io;

type DeathRow = (
    String,
    &'static str,
    &'static str,
    String,
    String,
    String,
    String,
    String,
);

#[derive(Serialize)]
pub struct Death {
    pub player: String,
    pub from_start: String,
    pub death: Vec<DeathRow>,
}

struct Entry {
    time: i64,
    flag: &'static str,
    detail: &'static str,
    source: String,
    spell: String,
    value: String,
    extra: String,
}

pub struct Deaths {
    pub deaths: Vec<(usize, Death)>,
    pub spells: BTreeMap<String, Spell>,
}

#[derive(Serialize)]
pub struct Spell {
    name: String,
    color: &'static str,
}

fn time(millis: i64) -> String {
    format!(
        "{:02}:{:02}.{:03}",
        millis / 60_000,
        millis / 1000 % 60,
        millis % 1000
    )
}

fn flags(event: &Event<'_>) -> Option<(&'static str, &'static str)> {
    Some(match event.kind {
        "SWING_DAMAGE" => ("DAMAGE", "SWING"),
        "RANGE_DAMAGE" => ("DAMAGE", "RANGE"),
        "SPELL_DAMAGE" => ("DAMAGE", "SPELL"),
        "SPELL_PERIODIC_DAMAGE" => ("DAMAGE", "DOT"),
        "ENVIRONMENTAL_DAMAGE" => ("DAMAGE", "ENVIRONMENT"),
        "DAMAGE_SHIELD" => ("DAMAGE", "SHIELD"),
        "SPELL_HEAL" => ("HEAL", "HEAL"),
        "SPELL_PERIODIC_HEAL" => ("HEAL", "HOT"),
        "SWING_MISSED" => ("MISS", "SWING"),
        "RANGE_MISSED" => ("MISS", "RANGE"),
        "SPELL_MISSED" => ("MISS", "SPELL"),
        "SPELL_PERIODIC_MISSED" => ("MISS", "DOT"),
        "SPELL_AURA_APPLIED" => (
            if event.payload.contains("DEBUFF") {
                "DEBUFF"
            } else {
                "BUFF"
            },
            "APPLIED",
        ),
        "SPELL_AURA_REFRESH" => (
            if event.payload.contains("DEBUFF") {
                "DEBUFF"
            } else {
                "BUFF"
            },
            "REFRESH",
        ),
        "SPELL_AURA_REMOVED" => (
            if event.payload.contains("DEBUFF") {
                "DEBUFF"
            } else {
                "BUFF"
            },
            "REMOVED",
        ),
        "UNIT_DIED" => ("DIED", ""),
        "SPELL_INSTAKILL" => ("DIED", "INSTAKILL"),
        _ => return None,
    })
}

pub fn deaths(report: &mut Report, encounters: &[Encounter]) -> io::Result<Deaths> {
    let year = report.id[..2].parse::<u32>().unwrap_or(0) + 2000;
    let roster = report.metadata.players.clone();
    let mut result = Deaths {
        deaths: Vec::new(),
        spells: BTreeMap::new(),
    };
    for encounter in encounters {
        let mut start = None::<Stamp>;
        let mut history = BTreeMap::<String, VecDeque<Entry>>::new();
        report.visit(Some(encounter.start..encounter.end), |_, line| {
            let Some(event) = Event::parse(line) else {
                return;
            };
            let origin = *start.get_or_insert(event.stamp);
            let Some(player) = roster.get(event.target) else {
                return;
            };
            let Some((flag, detail)) = flags(&event) else {
                return;
            };
            let millis = event.stamp.relative_to(origin, year);
            if !event.spell.is_empty() {
                result
                    .spells
                    .entry(event.spell.to_owned())
                    .or_insert_with(|| Spell {
                        name: event.spell_name.to_owned(),
                        color: school(event.payload.split(',').next().unwrap_or_default()),
                    });
            }
            let hits = history.entry(event.target.to_owned()).or_default();
            while hits
                .front()
                .is_some_and(|entry| millis - entry.time > 60_000)
            {
                hits.pop_front();
            }
            hits.push_back(Entry {
                time: millis,
                flag,
                detail,
                source: event.source.to_owned(),
                spell: event.spell.to_owned(),
                value: if event.amount == 0 {
                    String::new()
                } else {
                    event.amount.to_string()
                },
                extra: if event.excess == 0 {
                    String::new()
                } else {
                    event.excess.to_string()
                },
            });
            if flag == "DIED" {
                let death = hits
                    .drain(..)
                    .rev()
                    .map(|entry| {
                        let offset = if entry.time == millis {
                            "0:00.000".into()
                        } else {
                            format!("-{}", time(millis - entry.time))
                        };
                        (
                            offset,
                            entry.flag,
                            entry.detail,
                            entry.source,
                            event.target.to_owned(),
                            entry.spell,
                            entry.value,
                            entry.extra,
                        )
                    })
                    .collect();
                result.deaths.push((
                    result.deaths.len(),
                    Death {
                        player: player.clone(),
                        from_start: time(millis),
                        death,
                    },
                ));
            }
        })?;
    }
    Ok(result)
}
