use crate::combat::{Event, Stamp};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::LazyLock;

const AEGIS: &str = "47753";
const VALANYR: &str = "64413";
const ANTI_MAGIC_SHELL: &str = "48707";

#[derive(Deserialize)]
pub struct Rules {
    pub class_book: HashMap<String, String>,
    pub consumables: BTreeSet<String>,
    pub pet_filter_spells: BTreeSet<String>,
    pub boss_pets: BTreeSet<String>,
    pub flag_ignore_player_class: BTreeSet<String>,
    spec_book: HashMap<String, HashMap<String, u8>>,
    absorb_spells: HashMap<String, Limit>,
    abs_order: Vec<String>,
    abs_flags: BTreeSet<String>,
    aura_flags: BTreeSet<String>,
    dmg_split: BTreeSet<String>,
    idkshields: BTreeSet<String>,
    missed: BTreeSet<String>,
    ignored_max_values: BTreeSet<String>,
}

#[derive(Clone, Copy, Default, Deserialize)]
struct Limit {
    #[serde(default)]
    avg: i64,
    #[serde(default)]
    cap: i64,
}

pub static RULES: LazyLock<Rules> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../config/combat-rules.json")).expect("bundled combat rules")
});

#[derive(Default)]
pub struct Absorbs {
    specs: HashMap<String, u8>,
    valanyr_active: BTreeSet<String>,
    events: Vec<AbsorbEvent>,
}

struct AbsorbEvent {
    time: Stamp,
    origin: String,
    target: String,
    spell: String,
    kind: Kind,
}

enum Kind {
    Heal {
        amount: i64,
        crit: bool,
        valanyr: bool,
    },
    Aura {
        removed: bool,
    },
    Split(i64),
    Damage {
        absorbed: i64,
        damage: i64,
        physical: bool,
        swing: bool,
    },
}

impl Absorbs {
    pub fn observe(&mut self, row: u64, event: &Event<'_>, classes: &BTreeMap<String, String>) {
        if row < 100_000
            && event.kind != "SPELL_AURA_REMOVED"
            && let Some(class) = classes.get(event.source)
            && let Some(class_spells) = RULES.spec_book.get(class)
            && let Some(spec) = class_spells.get(event.spell)
            && !self.specs.contains_key(event.source)
        {
            self.specs.insert(event.source.into(), *spec);
        }
        if event.spell == "64411" {
            if event.kind == "SPELL_AURA_REMOVED" {
                self.valanyr_active.remove(event.target);
            } else {
                self.valanyr_active.insert(event.target.into());
            }
            return;
        }
        let mut source = event.source;
        let mut target = event.target;
        let kind = if event.kind == "DAMAGE_SPLIT" && RULES.dmg_split.contains(event.spell) {
            target = event.source;
            if event.spell != "25228" {
                source = event.target;
            }
            Kind::Split(event.amount)
        } else if event.healing() {
            let valanyr = self.valanyr_active.contains(event.source) && event.spell != "52000";
            let crit = event.payload.rsplit(',').next() == Some("1");
            if !valanyr
                && !(crit
                    && classes
                        .get(event.source)
                        .is_some_and(|class| class == "priest"))
            {
                return;
            }
            Kind::Heal {
                amount: event.amount,
                crit,
                valanyr,
            }
        } else if RULES.absorb_spells.contains_key(event.spell) {
            if !RULES.aura_flags.contains(event.kind) {
                return;
            }
            Kind::Aura {
                removed: event.kind == "SPELL_AURA_REMOVED",
            }
        } else if RULES.abs_flags.contains(event.kind) {
            let (absorbed, damage) = if RULES.missed.contains(event.kind) {
                let mut tail = event.payload.rsplit(',');
                let absorbed = tail
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
                if tail.next() != Some("ABSORB") {
                    return;
                }
                (absorbed, 0)
            } else {
                let absorbed = event
                    .payload
                    .split(',')
                    .nth(6)
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
                (absorbed, event.excess)
            };
            if absorbed == 0 {
                return;
            }
            Kind::Damage {
                absorbed,
                damage,
                physical: event.payload.split(',').next() == Some("0x1"),
                swing: event.kind == "SWING_DAMAGE",
            }
        } else {
            return;
        };
        self.events.push(AbsorbEvent {
            time: event.stamp,
            origin: source.into(),
            target: target.into(),
            spell: event.spell.into(),
            kind,
        });
    }

    pub fn finish(self, classes: &BTreeMap<String, String>, year: u32) -> HashMap<String, i64> {
        let mut targets = HashMap::<String, ShieldSet>::new();
        let mut totals = HashMap::<String, i64>::new();
        for event in self.events {
            let table = targets.entry(event.target.clone()).or_default();
            let is_disco = self.specs.get(&event.origin) == Some(&1)
                && classes
                    .get(&event.origin)
                    .is_some_and(|class| class == "priest");
            match event.kind {
                Kind::Heal {
                    amount,
                    crit,
                    valanyr,
                } if valanyr || is_disco && crit => {
                    let (spell, portion, cap) = if is_disco {
                        (AEGIS, 0.3, 10_000)
                    } else {
                        (VALANYR, 0.15, 20_000)
                    };
                    let shield = table.get(spell, &event.origin);
                    shield.applied = Some(event.time);
                    shield.remaining =
                        (shield.remaining + (amount as f64 * portion) as i64).min(cap);
                }
                Kind::Heal { .. } => {}
                Kind::Aura { removed } => {
                    let shield = table.get(&event.spell, &event.origin);
                    if removed {
                        shield.transient = true;
                        shield.removed = Some(event.time);
                    } else {
                        shield.applied = Some(event.time);
                    }
                }
                Kind::Split(amount) => {
                    let shield = table.get(&event.spell, &event.origin);
                    shield.applied = Some(event.time);
                    shield.remaining = amount;
                    shield.transient = true;
                }
                Kind::Damage {
                    absorbed,
                    damage,
                    physical,
                    swing,
                } => {
                    let blood = self.specs.get(&event.target) == Some(&1)
                        && classes
                            .get(&event.target)
                            .is_some_and(|class| class == "deathknight");
                    if blood {
                        let extra = blood_absorbed(table, absorbed, damage, swing);
                        *totals.entry(event.target.clone()).or_default() += extra;
                        table.consume(
                            &event,
                            absorbed - extra,
                            damage,
                            physical,
                            year,
                            &mut totals,
                        );
                    } else {
                        table.consume(&event, absorbed, damage, physical, year, &mut totals);
                    }
                }
            }
        }
        totals
    }
}

struct Shield {
    spell: String,
    owner: String,
    limit: Limit,
    remaining: i64,
    applied: Option<Stamp>,
    removed: Option<Stamp>,
    transient: bool,
}

#[derive(Default)]
struct ShieldSet {
    shields: Vec<Shield>,
    learned: HashMap<String, i64>,
}

impl ShieldSet {
    fn get(&mut self, spell: &str, owner: &str) -> &mut Shield {
        let position = self.shields.iter().position(|shield| shield.spell == spell);
        if let Some(index) = position
            && !self.shields[index].transient
        {
            return &mut self.shields[index];
        }
        let mut limit = RULES.absorb_spells.get(spell).copied().unwrap_or_default();
        if let Some(learned) = self.learned.get(spell) {
            limit.cap = *learned;
        }
        let shield = Shield {
            spell: spell.into(),
            owner: owner.into(),
            limit,
            remaining: 0,
            applied: None,
            removed: None,
            transient: false,
        };
        match position {
            Some(index) => {
                self.shields[index] = shield;
                &mut self.shields[index]
            }
            None => {
                self.shields.push(shield);
                self.shields.last_mut().unwrap()
            }
        }
    }

    fn consume(
        &mut self,
        event: &AbsorbEvent,
        mut absorbed: i64,
        damage: i64,
        physical: bool,
        year: u32,
        totals: &mut HashMap<String, i64>,
    ) {
        let mut order = (0..self.shields.len()).collect::<Vec<_>>();
        order.sort_by_key(|index| {
            let shield = &self.shields[*index];
            if shield.spell == VALANYR && shield.transient {
                0
            } else {
                1 + RULES
                    .abs_order
                    .iter()
                    .position(|spell| spell == &shield.spell)
                    .unwrap_or(1000 + index)
            }
        });
        let mut removed = Vec::new();
        let mut last = None;
        let mut active = self.shields.len();
        for (rank, index) in order.iter().copied().enumerate() {
            let shield = &mut self.shields[index];
            if shield.transient {
                active -= 1;
                removed.push(index);
                if shield
                    .removed
                    .is_some_and(|stamp| event.time.relative_to(stamp, year) > 500)
                {
                    continue;
                }
            }
            if !RULES.ignored_max_values.contains(&shield.spell) {
                last = Some(index);
            }
            let since_applied = shield
                .applied
                .map_or(-1, |stamp| event.time.relative_to(stamp, year));
            let used = match shield.spell.as_str() {
                "58597" if since_applied == 0 => 0,
                ANTI_MAGIC_SHELL if !physical => {
                    if active < 2 {
                        absorbed
                    } else {
                        damage * 3
                    }
                }
                _ if RULES.dmg_split.contains(&shield.spell) => shield.remaining,
                AEGIS if (1..100).contains(&since_applied) && active > 0 => 0,
                _ if RULES.idkshields.contains(&shield.spell) => {
                    let available = if shield.remaining != 0 {
                        shield.remaining
                    } else if let Some(learned) = self
                        .learned
                        .get(&shield.spell)
                        .copied()
                        .filter(|value| *value != 0)
                    {
                        learned
                    } else if shield.limit.cap != 0
                        && (active == 1 || rank + 1 == order.len() || shield.limit.avg == 0)
                    {
                        shield.limit.cap
                    } else {
                        shield.limit.avg
                    };
                    let used = available.min(absorbed);
                    shield.remaining = available - used;
                    used
                }
                _ => 0,
            };
            if used != 0 {
                *totals.entry(shield.owner.clone()).or_default() += used;
                absorbed -= used;
            }
        }
        if absorbed > 0
            && let Some(index) = last
        {
            let shield = &self.shields[index];
            if shield.spell != ANTI_MAGIC_SHELL {
                *totals.entry(shield.owner.clone()).or_default() += absorbed;
                *self
                    .learned
                    .entry(shield.spell.clone())
                    .or_insert(shield.limit.avg) += absorbed;
            }
        }
        removed.sort_unstable();
        for index in removed.into_iter().rev() {
            self.shields.remove(index);
        }
    }
}

fn blood_absorbed(table: &ShieldSet, absorbed: i64, damage: i64, swing: bool) -> i64 {
    let max_cap = table
        .shields
        .iter()
        .map(|shield| {
            if shield.remaining != 0 {
                shield.remaining
            } else {
                table
                    .learned
                    .get(&shield.spell)
                    .copied()
                    .unwrap_or(shield.limit.cap)
            }
        })
        .sum::<i64>();
    let total = absorbed + damage;
    if max_cap >= absorbed || total == 0 {
        return 0;
    }
    let ratio = absorbed as f64 / total as f64 * 100.0;
    if (45.0 - ratio) as i64 == 0 {
        return if swing { 0 } else { absorbed };
    }
    if (15.0 - ratio) as i64 == 0 {
        return absorbed;
    }
    let mut portion = 0.0;
    if ratio > 45.0 {
        let remaining = absorbed as f64 - total as f64 * 0.45 - (damage * 3) as f64;
        if table
            .shields
            .iter()
            .any(|shield| shield.spell == ANTI_MAGIC_SHELL)
        {
            let max_avg = table
                .shields
                .iter()
                .map(|shield| {
                    if shield.remaining != 0 {
                        shield.remaining
                    } else {
                        shield.limit.avg
                    }
                })
                .sum::<i64>();
            if max_cap != 0 && remaining > (max_cap - 15) as f64 {
                portion = 0.15;
            } else if remaining > (max_avg - 15) as f64 {
                portion = 0.45;
            } else if (damage * 3 - absorbed).abs() > 10 {
                portion = 0.15;
            }
        } else if !swing && absorbed > max_cap + 500 {
            portion = 0.45;
        }
    } else if ratio > 15.0 && absorbed > max_cap + 500 {
        portion = 0.15;
    }
    (total as f64 * portion) as i64
}
