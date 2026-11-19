use crate::report_view::number;
use minijinja::{Value, context};
use raidledger::appearance::{school, spell_icon};
use raidledger::combat::{Event, NIL_GUID, player_guid};
use raidledger::reports::{Encounter, Report};
use std::collections::BTreeMap;
use std::io;

#[derive(Default)]
struct Spell {
    name: String,
    color: &'static str,
    amount: i64,
    casts: i64,
    reduced: BTreeMap<&'static str, i64>,
    misses: BTreeMap<String, i64>,
    hits: [[Vec<i64>; 2]; 2],
}

fn rank(hits: &mut [i64]) -> (String, Vec<String>) {
    if hits.is_empty() {
        return (String::new(), Vec::new());
    }
    hits.sort_unstable();
    let avg = |slice: &[i64]| slice.iter().sum::<i64>() / slice.len() as i64;
    let len = hits.len();
    let tenth = (len / 10).max(1);
    let half = (len / 2).max(1);
    (
        number(avg(hits)),
        [
            hits[len - 1],
            avg(&hits[len - tenth..]),
            avg(&hits[len - half..]),
            avg(&hits[..half]),
            avg(&hits[..tenth]),
            hits[0],
        ]
        .into_iter()
        .map(number)
        .collect(),
    )
}

fn hit_group([hits, crits]: &mut [Vec<i64>; 2]) -> Value {
    let count = hits.len() + crits.len();
    let percent = if crits.is_empty() {
        String::new()
    } else {
        format!("{:.1}%", 100. * crits.len() as f64 / count as f64)
    };
    let (hit_avg, hits_avg) = rank(hits);
    let (crit_avg, crits_avg) = rank(crits);
    context! {
        total => number(count as i64), hits => number(hits.len() as i64), crits => number(crits.len() as i64),
        percent, hit_avg, hits_avg, crit_avg, crits_avg,
    }
}

fn target_key(guid: &str) -> &str {
    if player_guid(guid) {
        guid
    } else {
        guid.get(6..12).unwrap_or(guid)
    }
}

fn percent(value: i64, total: i64) -> String {
    if value == 0 {
        String::new()
    } else {
        format!(
            "{:.1}%",
            100. * value as f64 / if total == 0 { 1. } else { total as f64 }
        )
    }
}

pub fn breakdown(
    report: &mut Report,
    selected: &[Encounter],
    source: &str,
    view: &str,
    target: Option<&str>,
) -> io::Result<Value> {
    let is_heal = matches!(view, "heal" | "healed");
    let incoming = matches!(view, "taken" | "healed");
    let units = report.metadata.units.clone();
    let (guid, unit) = units
        .iter()
        .find(|(guid, unit)| source == guid.as_str() || source == unit.name)
        .ok_or(io::ErrorKind::NotFound)?;
    let owner_name = unit
        .master_guid
        .as_deref()
        .and_then(|owner| units.get(owner))
        .map(|owner| &owner.name);
    let pets = units
        .iter()
        .filter(|(_, unit)| unit.master_guid.as_ref() == Some(guid))
        .map(|(pet, unit)| (pet.as_str(), unit.name.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut spells = BTreeMap::<String, Spell>::new();
    let mut targets = BTreeMap::<&str, BTreeMap<String, String>>::new();
    for encounter in selected {
        report.visit(Some(encounter.start..encounter.end), |_, line| {
            let Some(event) = Event::parse(line) else {
                return;
            };
            let pet = pets.get(event.source);
            let relevant = if incoming {
                event.target == guid
            } else {
                event.source == guid || pet.is_some()
            };
            if !relevant || event.spell.is_empty() {
                return;
            }
            let hit = if is_heal {
                event.healing()
            } else {
                event.damage()
            };
            let cast = event.kind.ends_with("_CAST_SUCCESS");
            let missed = event.kind.ends_with("_MISSED");
            if !(hit || cast || missed) {
                return;
            }
            let (other, name) = if incoming {
                (event.source, event.source_name)
            } else {
                (event.target, event.target_name)
            };
            let key = target_key(other);
            if other != NIL_GUID {
                let category = if player_guid(other) {
                    "Players"
                } else {
                    "Targets"
                };
                targets
                    .entry(category)
                    .or_default()
                    .entry(key.to_owned())
                    .or_insert_with(|| name.to_owned());
            }
            if target.is_some_and(|wanted| wanted != key && wanted != other) {
                return;
            }
            let key = if !incoming && pet.is_some() {
                format!("{}--{}", event.spell, event.source)
            } else {
                event.spell.to_owned()
            };
            let fields = event.payload.split(',').collect::<Vec<_>>();
            let spell = spells.entry(key).or_insert_with(|| Spell {
                name: if let Some(pet) = pet.filter(|_| !incoming) {
                    format!("{} ({pet})", event.spell_name)
                } else {
                    event.spell_name.to_owned()
                },
                color: school(fields.first().copied().unwrap_or_default()),
                ..Spell::default()
            });
            if cast {
                spell.casts += 1;
            } else if missed {
                let Some(last) = fields.last() else { return };
                match last.parse::<i64>() {
                    Ok(amount) if fields.len() >= 2 => {
                        let category = fields[fields.len() - 2];
                        *spell.misses.entry(category.to_owned()).or_default() += 1;
                        if category == "ABSORB" {
                            *spell.reduced.entry("ABSORBED").or_default() += amount;
                        }
                    }
                    _ => *spell.misses.entry((*last).to_owned()).or_default() += 1,
                }
            } else {
                spell.amount += event.amount.saturating_sub(event.excess);
                let periodic = usize::from(event.kind.contains("_PERIODIC_"));
                let critical = usize::from(fields.get(if is_heal { 4 } else { 7 }) == Some(&"1"));
                spell.hits[periodic][critical].push(event.amount);
                if event.excess != 0 {
                    *spell
                        .reduced
                        .entry(if is_heal { "OVERHEAL" } else { "OVERKILL" })
                        .or_default() += event.excess;
                }
                if !is_heal {
                    for (index, category) in [(4, "RESISTED"), (5, "BLOCKED"), (6, "ABSORBED")] {
                        if let Some(amount) = fields
                            .get(index)
                            .and_then(|value| value.parse::<i64>().ok())
                            .filter(|amount| *amount != 0)
                        {
                            *spell.reduced.entry(category).or_default() += amount;
                        }
                    }
                    if fields.get(8) == Some(&"1") {
                        *spell.reduced.entry("GLANCED").or_default() += event.amount / 3;
                        *spell.misses.entry("GLANCING".to_owned()).or_default() += 1;
                    }
                }
            }
        })?;
    }
    let mut spells = spells.into_iter().collect::<Vec<_>>();
    spells.sort_by_key(|(_, spell)| std::cmp::Reverse(spell.amount));
    let total = spells.iter().map(|(_, spell)| spell.amount).sum::<i64>();
    let reduced_total = spells
        .iter()
        .flat_map(|(_, spell)| spell.reduced.values())
        .sum::<i64>();
    let rows = spells.into_iter().map(|(key, mut spell)| {
        let id = key.split_once("--").map_or(key.as_str(), |(id, _)| id);
        let reduced = spell.reduced.values().sum::<i64>();
        let misses = spell.misses.values().sum::<i64>();
        let details = spell.reduced.into_iter().map(|(kind, value)| (kind, number(value))).collect::<BTreeMap<_, _>>();
        let miss_details = spell.misses.into_iter().map(|(kind, value)| (kind, number(value))).collect::<BTreeMap<_, _>>();
        let hits = if spell.hits.iter().flatten().any(|hits| !hits.is_empty()) {
            spell.hits.iter_mut().map(hit_group).collect::<Vec<_>>()
        } else { Vec::new() };
        let row = context! { id, name => spell.name, icon => spell_icon(id), color => spell.color,
            actual => number(spell.amount), actual_percent => percent(spell.amount, total),
            reduced => number(reduced), reduced_percent => percent(reduced, reduced + spell.amount),
            reduced_details => details, casts => number(spell.casts), misses => number(misses), miss_details,
            hits,
        };
        (key, row)
    }).collect::<Vec<_>>();
    Ok(
        context! { SOURCE => source, SOURCE_NAME => &unit.name, OWNER_NAME => owner_name,
            IS_PLAYER => player_guid(guid), PETS => pets, TARGETS => targets,
            spell_rows => rows, actual_total => number(total), reduced_total => number(reduced_total),
        },
    )
}
