use crate::combat::{Event, player_guid};
use crate::reports::{Encounter, Report};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io;

#[derive(Default)]
struct Hits {
    hits: i64,
    hit_sum: i64,
    crits: i64,
    crit_sum: i64,
}

impl Hits {
    fn render(&self) -> Value {
        let total = self.hits + self.crits;
        json!({
            "total": total, "hits": self.hits, "hit_avg": self.hit_sum.checked_div(self.hits),
            "crits": self.crits, "crit_avg": self.crit_sum.checked_div(self.crits),
            "percent": (total > 0).then(|| format!("{:.1}%", self.crits as f64 / total as f64 * 100.0)),
        })
    }
}

#[derive(Default)]
struct Stats {
    damage: i64,
    reduced: i64,
    casts: u64,
    misses: u64,
    hits: Hits,
    periodic: Hits,
}

fn target_key(guid: &str) -> &str {
    if player_guid(guid) {
        guid
    } else {
        guid.get(6..12).unwrap_or(guid)
    }
}

pub fn compare(
    report: &mut Report,
    encounters: &[Encounter],
    class: &str,
    target: Option<&str>,
) -> io::Result<Value> {
    let classes = report.metadata.classes.clone();
    let units = report.metadata.units.clone();
    let mut players = BTreeMap::<_, BTreeMap<String, Stats>>::new();
    let mut targets = BTreeMap::new();
    let mut spells = BTreeMap::new();
    for encounter in encounters {
        report.visit(Some(encounter.start..encounter.end), |_, line| {
            let Some(event) = Event::parse(line) else {
                return;
            };
            let owner = units
                .get(event.source)
                .and_then(|unit| unit.master_guid.as_deref())
                .unwrap_or(event.source);
            if !classes
                .get(owner)
                .is_some_and(|value| value.eq_ignore_ascii_case(class))
                || event.spell.is_empty()
            {
                return;
            }
            if event.damage() {
                targets
                    .entry(target_key(event.target).to_owned())
                    .or_insert_with(|| event.target_name.to_owned());
            }
            if target
                .is_some_and(|target| target != target_key(event.target) && target != event.target)
            {
                return;
            }
            let stats = players
                .entry(owner.to_owned())
                .or_default()
                .entry(event.spell.to_owned())
                .or_default();
            spells
                .entry(event.spell.to_owned())
                .or_insert_with(|| json!({"name": event.spell_name}));
            if event.damage() {
                let fields = event.payload.split(',').collect::<Vec<_>>();
                let hit = if event.kind == "SPELL_PERIODIC_DAMAGE" {
                    &mut stats.periodic
                } else {
                    &mut stats.hits
                };
                let effective = event.amount - event.excess;
                if fields.get(7) == Some(&"1") {
                    hit.crits += 1;
                    hit.crit_sum += event.amount;
                } else {
                    hit.hits += 1;
                    hit.hit_sum += event.amount;
                }
                stats.damage += effective;
                stats.reduced += event.excess;
                for field in [4, 5, 6] {
                    stats.reduced += fields
                        .get(field)
                        .and_then(|value| value.parse::<i64>().ok())
                        .unwrap_or(0);
                }
            } else if event.kind.ends_with("_MISSED") {
                stats.misses += 1;
            } else if event.kind == "SPELL_CAST_SUCCESS" {
                stats.casts += 1;
            }
        })?;
    }
    let players = players.into_iter().map(|(guid, stats)| {
        let damage = stats.iter().map(|(id, stats)| (id, stats.damage)).collect::<BTreeMap<_, _>>();
        let reduced = stats.iter().map(|(id, stats)| (id, stats.reduced)).collect::<BTreeMap<_, _>>();
        let casts = stats.iter().map(|(id, stats)| (id, stats.casts)).collect::<BTreeMap<_, _>>();
        let misses = stats.iter().map(|(id, stats)| (id, stats.misses)).collect::<BTreeMap<_, _>>();
        let hits = stats.iter().map(|(id, stats)| (id, json!({"HIT": stats.hits.render(), "DOT": stats.periodic.render()}))).collect::<BTreeMap<_, _>>();
        json!({ "NAME": report.metadata.players.get(&guid).unwrap_or(&guid),
                "ACTUAL": damage, "REDUCED": reduced, "CASTS": casts, "MISSES": misses, "HITS": hits })
    }).collect::<Vec<_>>();
    Ok(json!({ "PLAYERS": players, "SPELLS": spells, "TARGETS": targets }))
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
