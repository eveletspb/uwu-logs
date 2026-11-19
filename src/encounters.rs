use crate::combat::{CATALOG, Event, player_guid, slug};
use crate::reports::Encounter;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Copy)]
struct Marker {
    row: u64,
    time: u64,
    buff: bool,
    main_target: bool,
    heal: bool,
    aura_removal: bool,
    aura_application: bool,
    death: bool,
    damage: bool,
    overkill: i64,
    amount: i64,
    fury: bool,
}

#[derive(Default)]
struct Pull {
    count: usize,
    tail: VecDeque<Marker>,
    first: Option<Marker>,
    first_non_buff: Option<Marker>,
    last: Option<Marker>,
    first_fury: Option<Marker>,
    last_fury: Option<Marker>,
    casters: BTreeSet<String>,
    killed: bool,
}

impl Pull {
    fn append(&mut self, marker: Marker, tail_limit: usize) {
        self.first.get_or_insert(marker);
        if !marker.buff {
            self.first_non_buff.get_or_insert(marker);
        }
        if marker.fury {
            self.first_fury.get_or_insert(marker);
            self.last_fury = Some(marker);
        }
        self.count += 1;
        self.last = Some(marker);
        if self.tail.len() == tail_limit {
            self.tail.pop_front();
        }
        self.tail.push_back(marker);
    }

    fn finish(
        self,
        guid: &str,
        attempts: &mut BTreeMap<String, usize>,
        output: &mut Vec<Encounter>,
    ) {
        if self.count <= 100 && !CATALOG.can_be_short.contains(guid) {
            return;
        }
        let Some(boss) = CATALOG.bosses.get(guid) else {
            return;
        };
        let Some(first) = self.first_non_buff.or(self.last) else {
            return;
        };
        let Some(last) = self.refined_end(guid) else {
            return;
        };
        let minimum = CATALOG.minimum_seconds.get(boss).copied().unwrap_or(15);
        let mut add = |first: Marker, last: Marker, defeated: bool| {
            if first.row > last.row {
                return;
            }
            let duration = last.time.saturating_sub(first.time) as f64 / 1000.0;
            let attempt = attempts.entry(boss.clone()).or_default();
            output.push(Encounter {
                boss: boss.clone(),
                boss_slug: slug(boss),
                start: first.row,
                end: last.row + 1,
                attempt: *attempt,
                difficulty: if self.casters.len() > 10 {
                    "25N"
                } else {
                    "10N"
                }
                .into(),
                outcome: if defeated && duration >= minimum as f64 {
                    "kill"
                } else {
                    "wipe"
                }
                .into(),
                duration_seconds: duration,
                start_second: first.time / 1000,
                end_second: last.time.div_ceil(1000),
            });
            *attempt += 1;
        };
        if guid == "008EF5"
            && let Some((first_fury, last_fury)) = self.first_fury.zip(self.last_fury)
        {
            add(first, first_fury, true);
            add(last_fury, last, false);
        } else {
            add(
                first,
                last,
                self.killed || last.death || last.overkill > 0 && last.amount - last.overkill != 1,
            );
        }
    }

    fn refined_end(&self, guid: &str) -> Option<Marker> {
        let mut end = self.last?;
        let mut detected_death = false;
        let mut detected_overkill = false;
        let mut next_hits = -20;
        let mut removals = 0;
        let mut first_removal = end;
        for (index, marker) in self.tail.iter().rev().enumerate() {
            if !marker.main_target || marker.heal {
                continue;
            }
            if marker.aura_removal || marker.aura_application {
                if !detected_death && CATALOG.cowards.contains(guid) && marker.aura_removal {
                    if removals == 0 {
                        first_removal = *marker;
                    }
                    removals += 1;
                    if removals >= 15 {
                        return Some(first_removal);
                    }
                }
                continue;
            }
            removals = 0;
            if marker.death {
                if !detected_death && index <= 30 {
                    end = *marker;
                    next_hits = 0;
                    detected_death = true;
                }
            } else if marker.damage {
                if marker.overkill == 0 {
                    next_hits += 1;
                    if next_hits > 5 {
                        break;
                    }
                } else if !detected_overkill {
                    next_hits = 0;
                    if marker.amount - marker.overkill != 1 {
                        end = *marker;
                        detected_overkill = true;
                    }
                }
            }
        }
        Some(end)
    }
}

#[derive(Default)]
pub struct Pulls {
    current: BTreeMap<String, Pull>,
    attempts: BTreeMap<String, usize>,
    output: Vec<Encounter>,
}

impl Pulls {
    pub fn observe(&mut self, row: u64, time: u64, event: &Event<'_>) {
        for pull in self.current.values_mut() {
            if pull.casters.len() <= 10
                && pull.first.is_some_and(|first| row - first.row < 2000)
                && player_guid(event.source)
                && !matches!(event.kind, "SPELL_AURA_REMOVED" | "SPELL_AURA_REFRESH")
            {
                pull.casters.insert(event.source.into());
            }
        }
        let target = event.target.get(6..12).unwrap_or_default();
        if event.kind == "PARTY_KILL" {
            let leader = CATALOG
                .group_leaders
                .get(target)
                .map_or(target, String::as_str);
            if let Some(pull) = self.current.get_mut(leader) {
                pull.killed = true;
            }
            return;
        }
        if !CATALOG.flags.contains(event.kind) || CATALOG.ignored_spell_ids.contains(event.spell) {
            return;
        }
        let guid = if CATALOG.bosses.contains_key(target) {
            target
        } else if CATALOG.some_boss_spells.contains(event.spell) {
            event.source.get(6..12).unwrap_or_default()
        } else {
            return;
        };
        let guid = CATALOG.group_leaders.get(guid).map_or(guid, String::as_str);
        if !CATALOG.bosses.contains_key(guid) {
            return;
        }
        let idle = CATALOG.idle_seconds.get(guid).copied().unwrap_or(30) * 1000;
        if self
            .current
            .get(guid)
            .and_then(|pull| pull.last)
            .is_some_and(|last| time.saturating_sub(last.time) > idle)
            && let Some(old) = self.current.remove(guid)
        {
            old.finish(guid, &mut self.attempts, &mut self.output);
        }
        let pull = self.current.entry(guid.into()).or_default();
        let marker = Marker {
            row,
            time,
            buff: event.payload.ends_with(",BUFF"),
            main_target: CATALOG.raw_bosses.contains(target),
            heal: event.healing() && target != "008FB5",
            aura_removal: event.kind == "SPELL_AURA_REMOVED",
            aura_application: event.kind == "SPELL_AURA_APPLIED",
            death: event.kind == "UNIT_DIED",
            damage: event.damage() || event.healing(),
            overkill: event.excess,
            amount: event.amount,
            fury: event.spell == "72350",
        };
        pull.append(marker, CATALOG.max_tail.get(guid).copied().unwrap_or(1000));
    }

    pub fn finish(mut self) -> Vec<Encounter> {
        for (guid, pull) in self.current {
            pull.finish(&guid, &mut self.attempts, &mut self.output);
        }
        self.output.sort_by_key(|encounter| encounter.start);
        self.output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boss_pull_ends_at_kill_before_aura_cleanup() {
        let mut pulls = Pulls::default();
        let guid = "0xF130003C9C000042";
        let start = format!(
            "11/18 21:01:14.020,SPELL_AURA_APPLIED,0x00000000000007DB,Bombas,{guid},Sartura,48461,Wrath,0x08,BUFF"
        );
        pulls.observe(0, 0, &Event::parse(&start).unwrap());
        let hit = format!(
            "11/18 21:01:14.020,SWING_DAMAGE,0x00000000000007DB,Bombas,{guid},Sartura,1,Melee,0x1,312,0,1,0,0,0,0,nil,nil"
        );
        let hit = Event::parse(&hit).unwrap();
        for row in 1..110 {
            pulls.observe(row, row * 200, &hit);
        }
        let death = format!("11/18 21:01:36.020,UNIT_DIED,0x0000000000000000,nil,{guid},Sartura");
        pulls.observe(110, 22_000, &Event::parse(&death).unwrap());
        let cleanup = start.replace("AURA_APPLIED", "AURA_REMOVED");
        pulls.observe(111, 22_100, &Event::parse(&cleanup).unwrap());
        let reports = pulls.finish();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].outcome, "kill");
        assert_eq!((reports[0].start, reports[0].end), (1, 111));
    }
}
