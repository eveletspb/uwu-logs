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
    let mut start = None::<Stamp>;
    report.visit(Some(encounter.start..encounter.start + 1), |_, line| {
        start = line
            .split_once(',')
            .and_then(|(stamp, _)| Stamp::parse(stamp));
    })?;
    let Some(start) = start else {
        return Ok(result);
    };
    let index = &report.metadata.time_index;
    let seek =
        index.partition_point(|(second, _)| *second <= encounter.start_second.saturating_sub(180));
    let row = seek
        .checked_sub(1)
        .and_then(|position| index.get(position))
        .map(|(_, row)| *row)
        .unwrap_or(0)
        .min(encounter.start);
    report.visit(Some(row..encounter.end), |_, line| {
        let Some(event) = Event::parse(line) else {
            return;
        };
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
        if millis < -180_000 {
            return;
        }
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
