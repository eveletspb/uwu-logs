use crate::appearance::{school, spell_icon};
use crate::combat::{Event, player_guid};
use crate::metrics::formatted_amount as number;
use crate::reports::{Encounter, Report};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io;
use std::sync::LazyLock;

pub(crate) static COMBINED_SPELLS: LazyLock<BTreeMap<String, String>> =
    LazyLock::new(|| serde_json::from_str(include_str!("../config/combined-spells.json")).unwrap());

#[derive(Default)]
struct Stats {
    damage: Option<i64>,
    casts: i64,
    reduced: BTreeMap<String, i64>,
    misses: BTreeMap<String, i64>,
    hits: [[Vec<i64>; 2]; 2],
}

fn avgs(values: &mut [i64]) -> (String, Vec<String>) {
    if values.is_empty() {
        return (String::new(), Vec::new());
    }
    values.sort_unstable();
    let avg = |slice: &[i64]| slice.iter().sum::<i64>().div_euclid(slice.len() as i64);
    let n = values.len();
    let tenth = (n / 10).max(1);
    let half = (n / 2).max(1);
    (
        number(avg(values)),
        [
            values[n - 1],
            avg(&values[n - tenth..]),
            avg(&values[n - half..]),
            avg(&values[..half]),
            avg(&values[..tenth]),
            values[0],
        ]
        .map(number)
        .to_vec(),
    )
}

fn hit_group([hits, crits]: &mut [Vec<i64>; 2]) -> Value {
    let count = hits.len() + crits.len();
    let percent = if crits.is_empty() {
        String::new()
    } else {
        percentage(crits.len() as i64, count as i64)
    };
    let (hit_avg, hits_avg) = avgs(hits);
    let (crit_avg, crits_avg) = avgs(crits);
    json!({ "total": number(count as i64), "hits": number(hits.len() as i64), "crits": number(crits.len() as i64),
        "percent": percent, "hit_avg": hit_avg, "hits_avg": hits_avg, "crit_avg": crit_avg, "crits_avg": crits_avg })
}

fn percentage(value: i64, total: i64) -> String {
    format!(
        "{:.1}%",
        100.0 * value as f64 / if total == 0 { 1.0 } else { total as f64 }
    )
}

fn packed(data: &BTreeMap<String, i64>, total: bool) -> BTreeMap<String, String> {
    let mut result: BTreeMap<_, _> = data
        .iter()
        .map(|(id, value)| (id.to_owned(), number(*value)))
        .collect();
    if total {
        result.insert("Total".into(), number(data.values().sum()));
    }
    result
}

fn target_key(guid: &str) -> &str {
    if guid.starts_with("0x0") {
        guid
    } else {
        guid.get(6..12).unwrap_or(guid)
    }
}

fn damage(kind: &str) -> bool {
    matches!(
        kind,
        "SPELL_DAMAGE"
            | "SWING_DAMAGE"
            | "RANGE_DAMAGE"
            | "DAMAGE_SHIELD"
            | "SPELL_PERIODIC_DAMAGE"
    )
}

pub fn compare(
    report: &mut Report,
    encounters: &[Encounter],
    class: &str,
    target: Option<&str>,
) -> io::Result<Value> {
    let units = report.metadata.units.clone();
    let class = class.to_ascii_lowercase();
    let mut players = report
        .metadata
        .classes
        .iter()
        .filter(|(_, value)| *value == &class)
        .map(|(guid, _)| (guid.to_owned(), BTreeMap::<String, Stats>::new()))
        .collect::<BTreeMap<_, _>>();
    let mut targets: BTreeMap<&str, BTreeMap<String, String>> = ["Players", "Pets", "NPCS"]
        .into_iter()
        .map(|key| (key, BTreeMap::new()))
        .collect();
    let mut spell_metadata = BTreeMap::new();
    let mut spell_names = BTreeMap::new();
    let mut pet_names = BTreeMap::new();
    let target = target.filter(|target| !target.is_empty() && *target != "all");
    for encounter in encounters {
        report.visit(Some(encounter.start..encounter.end), |_, line| {
            let Some(event) = Event::parse(line) else {
                return;
            };
            let owner = units
                .get(event.source)
                .and_then(|unit| unit.master_guid.as_deref())
                .unwrap_or(event.source);
            let Some(player) = players.get_mut(owner) else {
                return;
            };
            let hit = damage(event.kind);
            let cast = event.kind.contains("_CAST");
            let missed = event.kind.ends_with("_MISSED");
            if !(hit || cast || missed) || event.spell.is_empty() {
                return;
            }
            let fields = event.payload.split(',').collect::<Vec<_>>();
            spell_metadata
                .entry(event.spell.to_owned())
                .or_insert_with(|| {
                    (
                        event.spell_name.to_owned(),
                        school(fields.first().copied().unwrap_or_default()),
                    )
                });
            let key = target_key(event.target);
            let category = if key.starts_with("0x0") {
                "Players"
            } else if event.target.starts_with("0xF14") {
                "Pets"
            } else {
                "NPCS"
            };
            targets
                .get_mut(category)
                .unwrap()
                .entry(key.to_owned())
                .or_insert_with(|| event.target_name.to_owned());
            if target.is_some_and(|target| target != key && target != event.target) {
                return;
            }
            let id = if let Some(pet) = event
                .source
                .get(6..12)
                .filter(|pet| *pet != "000000" && !player_guid(event.source))
            {
                pet_names
                    .entry(pet.to_owned())
                    .or_insert_with(|| event.source_name.to_owned());
                format!("{}--{pet}", event.spell)
            } else {
                event.spell.to_owned()
            };
            let stats = player.entry(id).or_default();
            if hit {
                let effective = event.amount - event.excess;
                *stats.damage.get_or_insert(0) += effective;
                let group = usize::from(event.kind == "SPELL_PERIODIC_DAMAGE");
                let crit = usize::from(fields.get(7) == Some(&"1"));
                stats.hits[group][crit].push(event.amount);
                if event.excess != 0 {
                    *stats.reduced.entry("OVERKILL".into()).or_default() += event.excess;
                }
                for (index, category) in [(4, "RESISTED"), (6, "ABSORBED")] {
                    if let Some(value) = fields
                        .get(index)
                        .and_then(|field| field.parse::<i64>().ok())
                        .filter(|&value| value != 0)
                    {
                        *stats.reduced.entry(category.into()).or_default() += value;
                    }
                }
                if fields.get(8) == Some(&"1") {
                    *stats.reduced.entry("GLANCED".into()).or_default() += effective / 3;
                    *stats.misses.entry("GLANCING".into()).or_default() += 1;
                }
            } else if cast {
                stats.casts += 1;
            } else if let Some(last) = fields.last() {
                let category =
                    if let Some(amount) = last.parse::<i64>().ok().filter(|_| fields.len() >= 2) {
                        let category = fields[fields.len() - 2];
                        *stats.reduced.entry(format!("{category}ED")).or_default() += amount;
                        category
                    } else {
                        last
                    };
                *stats.misses.entry(category.to_owned()).or_default() += 1;
            }
        })?;
    }
    let players = players.into_iter().map(|(guid, spells)| {
        let mut combined = BTreeMap::<String, Stats>::new();
        for (id, mut stats) in spells {
            let (spell, pet) = id.split_once("--").map_or((id.as_str(), None), |(spell, pet)| (spell, Some(pet)));
            let primary = COMBINED_SPELLS.get(spell).filter(|main| spell_metadata.contains_key(*main)).map_or(spell, String::as_str);
            let (name, color) = &spell_metadata[primary];
            let (key, name) = pet.map_or_else(|| (primary.to_owned(), name.to_owned()), |pet| {
                (format!("{primary}--{pet}"), format!("{name} ({})", pet_names[pet]))
            });
            spell_names.entry(key.clone()).or_insert_with(|| json!({
                "id": primary, "name": name, "color": color, "icon": spell_icon(primary)
            }));
            let merged = combined.entry(key).or_default();
            if let Some(amount) = stats.damage { *merged.damage.get_or_insert(0) += amount; }
            merged.casts += stats.casts;
            for (kind, amount) in stats.reduced { *merged.reduced.entry(kind).or_default() += amount; }
            for (kind, amount) in stats.misses { *merged.misses.entry(kind).or_default() += amount; }
            for (groups, other) in merged.hits.iter_mut().zip(&mut stats.hits) {
                for (values, other) in groups.iter_mut().zip(other) { values.append(other); }
            }
        }
        let mut actual = BTreeMap::new();
        let mut hits = BTreeMap::new();
        let mut reduced = BTreeMap::new();
        let mut missed = BTreeMap::new();
        let mut casts = BTreeMap::new();
        let mut reduced_details = BTreeMap::new();
        let mut missed_details = BTreeMap::new();
        for (id, mut stats) in combined {
            if let Some(amount) = stats.damage {
                actual.insert(id.clone(), amount);
                hits.insert(id.clone(), json!({"HIT": hit_group(&mut stats.hits[0]), "DOT": hit_group(&mut stats.hits[1])}));
            }
            if stats.casts != 0 { casts.insert(id.clone(), number(stats.casts)); }
            if !stats.reduced.is_empty() {
                reduced.insert(id.clone(), stats.reduced.values().sum());
                reduced_details.insert(id.clone(), packed(&stats.reduced, false));
            }
            if !stats.misses.is_empty() {
                missed.insert(id.clone(), stats.misses.values().sum());
                missed_details.insert(id, packed(&stats.misses, false));
            }
        }
        let total = actual.values().sum();
        let mut actual_percent = actual.iter().map(|(id, amount)| (id.clone(), percentage(*amount, total))).collect::<BTreeMap<_, _>>();
        actual_percent.insert("Total".into(), percentage(total, total));
        let reduced_percent = reduced.iter().map(|(id, amount)| (id.to_owned(), percentage(*amount, amount + actual.get(id).copied().unwrap_or_default()))).collect::<BTreeMap<_, _>>();
        json!({
            "NAME": report.metadata.players.get(&guid).unwrap_or(&guid), "ACTUAL": packed(&actual, true),
            "ACTUAL_PERCENT": actual_percent, "HITS": hits, "HITS_DATA": hits, "CASTS": casts,
            "REDUCED": packed(&reduced, true), "REDUCED_PERCENT": reduced_percent, "REDUCED_DETAILED": reduced_details,
            "MISSES": packed(&missed, true), "MISS_DETAILED": missed_details,
        })
    }).collect::<Vec<_>>();
    Ok(json!({ "PLAYERS": players, "SPELLS": spell_names, "TARGETS": targets }))
}

pub fn spell_search(report: &mut Report, filter: &str) -> io::Result<BTreeMap<String, String>> {
    let filter = filter.to_lowercase();
    let mut found = BTreeMap::new();
    if !filter.is_empty() {
        let by_id = filter.bytes().all(|byte| byte.is_ascii_digit());
        report.visit(None, |_, line| {
            if let Some(event) = Event::parse(line)
                && !event.spell.is_empty()
                && if by_id {
                    event.spell.contains(&filter)
                } else {
                    event.spell_name.to_lowercase().contains(&filter)
                }
            {
                found
                    .entry(event.spell.into())
                    .or_insert_with(|| event.spell_name.into());
            }
        })?;
    }
    Ok(found)
}
