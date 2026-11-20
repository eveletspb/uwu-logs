use crate::combat::{Event, Stamp};
use crate::reports::{Encounter, Report};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io;

#[derive(Serialize)]
struct Spell {
    id: String,
    name: String,
    color: &'static str,
    icon: Option<&'static str>,
}

type TimelineEvent = (i64, String, String, String, String, String);

#[derive(Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub struct Timeline {
    data: BTreeMap<String, Vec<TimelineEvent>>,
    spells: BTreeMap<String, Spell>,
    flags: BTreeSet<String>,
    rduration: f64,
    name: String,
    class: String,
}

pub fn build(report: &mut Report, encounter: &Encounter, name: &str) -> io::Result<Timeline> {
    let Some((guid, unit)) = report
        .metadata
        .units
        .iter()
        .find(|(guid, unit)| name == *guid || name == unit.name)
    else {
        return Err(io::ErrorKind::NotFound.into());
    };
    let guid = guid.clone();
    let year = 2000 + report.id[..2].parse::<u32>().unwrap_or(0);
    let mut result = Timeline {
        data: BTreeMap::new(),
        spells: BTreeMap::new(),
        flags: BTreeSet::new(),
        rduration: encounter.duration_seconds,
        name: unit.name.clone(),
        class: report
            .metadata
            .classes
            .get(&guid)
            .cloned()
            .unwrap_or_else(|| "npc".into()),
    };
    let Some(start) = stamp_at(report, encounter.start)? else {
        return Ok(result);
    };
    let origin = stamp_at(report, 0)?.unwrap_or(start);
    let seek = |second: u64, report: &Report| {
        let index = &report.metadata.time_index;
        let offset = index.partition_point(|(stamp, _)| *stamp <= second.saturating_sub(2));
        index
            .get(offset.saturating_sub(1))
            .map_or(0, |(_, row)| *row)
    };
    let elapsed = |stamp: Stamp| stamp.relative_to(origin, year);
    let tick = |elapsed: i64| (elapsed + i64::from(origin.millis)).div_euclid(1000);
    let mut previous_tick = None;
    let mut previous_anchor = 0;
    if encounter.start > 0 {
        let before = seek(elapsed(start).max(0) as u64 / 1000, report).min(encounter.start - 1);
        report.visit(Some(before..encounter.start), |_, line| {
            if let Some(stamp) = parse_stamp(line) {
                let elapsed = elapsed(stamp);
                let second = tick(elapsed);
                if previous_tick.replace(second) != Some(second) {
                    previous_anchor = previous_anchor.max(elapsed / 1000);
                }
            }
        })?;
    }
    // Pre-pull starts at the timestamp index entry 180 seconds before the combat row.
    let cutoff = (previous_anchor + 1).saturating_sub(180).max(0) as u64;
    let row = seek(cutoff, report).min(encounter.start);
    let mut in_window = cutoff == 0;
    let mut before_tick = None;
    report.visit(Some(row..encounter.end), |_, line| {
        let Some(event) = Event::parse(line) else {
            return;
        };
        if !in_window {
            let elapsed = elapsed(event.stamp);
            let second = tick(elapsed);
            in_window = before_tick.replace(second) != Some(second)
                && elapsed.max(0) as u64 / 1000 >= cutoff;
        }
        if !in_window {
            return;
        }
        if (event.source != guid && event.target != guid)
            || event.spell.is_empty()
            || event.payload.is_empty()
            || matches!(
                event.kind,
                "ENCHANT_APPLIED" | "ENCHANT_REMOVED" | "SPELL_DRAIN" | "ENVIRONMENTAL_DAMAGE"
            )
        {
            return;
        }
        let millis = event.stamp.relative_to(start, year);
        if !result.spells.contains_key(event.spell) {
            result.spells.insert(
                event.spell.into(),
                Spell {
                    id: event.spell.into(),
                    name: event.spell_name.into(),
                    color: crate::appearance::school(
                        event.payload.split(',').next().unwrap_or_default(),
                    ),
                    icon: Some(crate::appearance::spell_icon(event.spell)),
                },
            );
        }
        result.flags.insert(event.kind.into());
        result.data.entry(event.spell.into()).or_default().push((
            millis,
            event.kind.into(),
            event.source_name.into(),
            event.target_name.into(),
            event.target.into(),
            event.payload.into(),
        ));
    })?;
    for (id, main) in crate::compare::COMBINED_SPELLS.iter() {
        if result.spells.contains_key(main)
            && let Some(data) = result.data.remove(id)
        {
            let group = result.data.get_mut(main).unwrap();
            group.extend(data);
            group.sort();
            result.spells.remove(id);
        }
    }
    Ok(result)
}

fn parse_stamp(line: &str) -> Option<Stamp> {
    Stamp::parse(line.split_once(',')?.0)
}

fn stamp_at(report: &mut Report, row: u64) -> io::Result<Option<Stamp>> {
    let mut stamp = None;
    report.visit(Some(row..row + 1), |_, line| stamp = parse_stamp(line))?;
    Ok(stamp)
}
