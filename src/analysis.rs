use crate::absorbs::RULES;
use crate::combat::{Event, Stamp};
use crate::reports::{Encounter, Report};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;

#[derive(Serialize)]
pub struct Row {
    pub unit: String,
    pub detail: String,
    pub value: i64,
}

#[derive(Serialize)]
pub struct Death {
    pub player: String,
    pub seconds: f64,
    pub boss: String,
    pub hits: Vec<Hit>,
}

#[derive(Clone, Serialize)]
pub struct Hit {
    pub source: String,
    pub spell: String,
    pub amount: i64,
    pub offset_millis: i64,
}

pub fn deaths(report: &mut Report, encounters: &[Encounter]) -> io::Result<Vec<Death>> {
    let year = report.id[..2].parse::<u32>().unwrap_or(0) + 2000;
    let known = report
        .metadata
        .players
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut deaths = Vec::new();
    for encounter in encounters {
        let mut start = None::<Stamp>;
        let mut history = BTreeMap::<String, VecDeque<Hit>>::new();
        report.visit(Some(encounter.start..encounter.end), |_, line| {
            let Some(event) = Event::parse(line) else {
                return;
            };
            let origin = *start.get_or_insert(event.stamp);
            if !known.contains(event.target) {
                return;
            }
            let millis = event.stamp.relative_to(origin, year);
            if event.damage() {
                let hits = history.entry(event.target.into()).or_default();
                while hits
                    .front()
                    .is_some_and(|hit| millis - hit.offset_millis > 10_000)
                {
                    hits.pop_front();
                }
                hits.push_back(Hit {
                    source: event.source_name.into(),
                    spell: event.spell_name.into(),
                    amount: event.amount,
                    offset_millis: millis,
                });
            } else if event.kind == "UNIT_DIED" {
                let hits = history
                    .remove(event.target)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|hit| millis - hit.offset_millis <= 10_000)
                    .map(|mut hit| {
                        hit.offset_millis -= millis;
                        hit
                    })
                    .collect();
                deaths.push(Death {
                    player: event.target_name.into(),
                    seconds: millis as f64 / 1000.0,
                    boss: encounter.boss.clone(),
                    hits,
                });
            }
        })?;
    }
    Ok(deaths)
}

#[derive(Clone, Copy)]
pub enum View<'a> {
    Targets,
    Auras,
    Consumables,
    Resources,
    Units,
    Spell(&'a str),
}

impl View<'_> {
    pub fn labels(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Targets => ("Урон по целям", "Цель", "Урон"),
            Self::Auras => ("Ауры", "Игрок", "Применения"),
            Self::Consumables => ("Расходники", "Игрок", "Использования"),
            Self::Resources => ("Ресурсы", "Игрок", "Получено"),
            Self::Units => ("Участники", "Персонаж", "События"),
            Self::Spell(_) => ("Заклинание", "Источник", "События"),
        }
    }
}

fn resource(value: &str) -> &str {
    match value {
        "0" => "Mana",
        "1" => "Rage",
        "2" => "Focus",
        "3" => "Energy",
        "4" => "Happiness",
        "5" => "Runes",
        "6" => "Runic Power",
        _ => value,
    }
}

pub fn inspect(
    report: &mut Report,
    encounters: &[Encounter],
    view: View<'_>,
) -> io::Result<Vec<Row>> {
    let mut totals = BTreeMap::<(String, String), i64>::new();
    let roster = report.metadata.players.clone();
    let units = report.metadata.units.clone();
    for encounter in encounters {
        report.visit(Some(encounter.start..encounter.end), |_, line| {
            let Some(event) = Event::parse(line) else {
                return;
            };
            let owner = units
                .get(event.source)
                .and_then(|unit| unit.master_guid.as_deref())
                .unwrap_or(event.source);
            let outgoing_player = roster.contains_key(owner);
            let on_player = roster.contains_key(event.target);
            let spell = event.spell_name;
            let (unit, detail, value) = match view {
                View::Targets if event.damage() && outgoing_player && !on_player => (
                    event.target_name,
                    roster
                        .get(owner)
                        .cloned()
                        .unwrap_or_else(|| event.source_name.into()),
                    event.amount - event.excess,
                ),
                View::Auras
                    if on_player
                        && matches!(event.kind, "SPELL_AURA_APPLIED" | "SPELL_AURA_REFRESH") =>
                {
                    (
                        event.target_name,
                        format!("{spell} · {}", event.source_name),
                        1,
                    )
                }
                View::Consumables
                    if outgoing_player
                        && event.kind == "SPELL_CAST_SUCCESS"
                        && RULES.consumables.contains(event.spell) =>
                {
                    (event.source_name, spell.to_owned(), 1)
                }
                View::Consumables
                    if on_player
                        && event.kind == "SPELL_AURA_APPLIED"
                        && event.spell == "28714" =>
                {
                    (event.target_name, spell.to_owned(), 1)
                }
                View::Resources if on_player && event.kind.ends_with("_ENERGIZE") => (
                    event.target_name,
                    format!(
                        "{} · {spell}",
                        resource(event.payload.split(',').nth(2).unwrap_or_default())
                    ),
                    event.amount,
                ),
                View::Units => (
                    event.source_name,
                    roster
                        .get(owner)
                        .filter(|name| *name != event.source_name)
                        .cloned()
                        .unwrap_or_default(),
                    1,
                ),
                View::Spell(id) if id == event.spell => {
                    (event.source_name, event.kind.to_owned(), 1)
                }
                _ => return,
            };
            *totals.entry((unit.to_owned(), detail)).or_default() += value;
        })?;
    }
    let mut result = totals
        .into_iter()
        .map(|((unit, detail), value)| Row {
            unit,
            detail,
            value,
        })
        .collect::<Vec<_>>();
    result.sort_by_key(|row| std::cmp::Reverse(row.value));
    Ok(result)
}
