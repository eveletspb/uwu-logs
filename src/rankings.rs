use crate::appearance::{class_for_specialization_spell, class_specs, specialization};
use crate::combat::{CATALOG, Event, NIL_GUID, Stamp, guid_boss};
use crate::reports::{Encounter, LegacyGroups, Metadata, Report, list_ids};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;
use std::sync::LazyLock;
use std::time::SystemTime;

const CLASSES: [&str; 10] = [
    "death-knight",
    "druid",
    "hunter",
    "mage",
    "paladin",
    "priest",
    "rogue",
    "shaman",
    "warlock",
    "warrior",
];
const EXTERNALS: [u32; 4] = [10060, 19753, 49016, 57933];

#[derive(Deserialize)]
struct Rules {
    useful: HashSet<String>,
    auras: BTreeMap<String, (u8, i64, Option<String>)>,
    points: BTreeMap<String, Vec<(String, String)>>,
}
static RULES: LazyLock<Rules> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../config/rankings.json")).expect("bundled ranking rules")
});
static PHASES: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../config/server_phases.json"))
        .expect("bundled server phases")
});

#[derive(Serialize)]
pub struct Spec {
    name: &'static str,
    key: String,
    icon: &'static str,
    class: &'static str,
}

fn spec(index: u8) -> Option<Spec> {
    let class = *CLASSES.get(usize::from(index / 4))?;
    let (name, icon) = class_specs(class)?.get(usize::from(index % 4))?;
    Some(Spec {
        name,
        key: name.to_lowercase().replace(' ', "-"),
        icon,
        class,
    })
}

pub fn specs() -> Vec<Spec> {
    (0..40)
        .filter(|i| eligible_spec(*i))
        .filter_map(spec)
        .collect()
}

fn eligible_spec(index: u8) -> bool {
    index < 40 && !index.is_multiple_of(4) && !matches!(index, 7 | 17 | 18 | 21 | 22 | 31 | 39)
}

fn row_spec(class: &str, specialization: Option<u8>) -> u8 {
    CLASSES
        .iter()
        .position(|value| *value == class)
        .map_or(40, |index| {
            index as u8 * 4 + specialization.unwrap_or(if class == "warlock" { 3 } else { 0 })
        })
}

#[derive(Clone)]
struct Row {
    report: String,
    guid: String,
    name: String,
    duration: f64,
    useful: i64,
    total: i64,
    spec: u8,
    auras: Vec<Aura>,
}

type Aura = (u32, u32, f64, u8);
pub type TopRow<'a> = (&'a str, f64, &'a str, &'a str, i64, i64, u8, &'a [Aura]);

impl Row {
    fn dps(&self, amount: i64) -> f64 {
        round(amount as f64 / self.duration.min(9999.0), 100.0)
    }

    fn daily_key(&self) -> (&str, &str) {
        (&self.report[..8], &self.guid)
    }

    fn top_row(&self) -> TopRow<'_> {
        (
            &self.report,
            self.duration,
            &self.guid,
            &self.name,
            self.useful,
            self.total,
            self.spec,
            &self.auras,
        )
    }
}

fn round(value: f64, scale: f64) -> f64 {
    (value * scale).round_ties_even() / scale
}

#[derive(Default)]
struct AuraSpan {
    start: Option<i64>,
    count: u32,
    millis: i64,
}

impl AuraSpan {
    fn observe(&mut self, time: i64, removed: bool, id: &str, max_seconds: i64) {
        if let Some(start) = self.start {
            let elapsed = (time - start).min(max_seconds * 1000);
            if elapsed < 500 && id != "19753" {
                return;
            }
            if elapsed > 0 {
                self.count += 1;
                self.millis += elapsed;
            }
        }
        self.start = (!removed).then_some(time);
    }
}

#[derive(Default)]
struct Fight {
    origin: Option<Stamp>,
    classes: HashMap<String, String>,
    specs: HashMap<String, u8>,
    damage: HashMap<String, (i64, i64)>,
    auras: HashMap<String, BTreeMap<&'static str, AuraSpan>>,
    trinkets: BTreeMap<&'static str, usize>,
    room: BTreeSet<i64>,
    special: SpecialDamage,
}

#[derive(Default)]
struct SpecialDamage {
    overrides: HashMap<String, HashMap<String, i64>>,
    valks: HashMap<String, i64>,
    dead: HashSet<String>,
    stacks: HashMap<String, i64>,
    festergut: HashMap<String, (i64, i64)>,
}

fn damage_event(kind: &str) -> bool {
    kind.contains("DAMAGE") && !matches!(kind, "DAMAGE_SHIELD_MISSED" | "DAMAGE_SPLIT")
}

fn npc(guid: &str) -> &str {
    guid.get(6..12).unwrap_or_default()
}

impl SpecialDamage {
    fn add(&mut self, target: &str, source: &str, amount: i64) {
        *self
            .overrides
            .entry(target.into())
            .or_default()
            .entry(source.into())
            .or_default() += amount;
    }

    // These target buckets replace the raw useful damage buckets used by the old leaderboard.
    fn observe(&mut self, boss: &str, event: &Event<'_>) {
        let target = npc(event.target);
        let damage = damage_event(event.kind);
        let absorbed = || {
            if event.kind.contains("ABSORB")
                || event.payload.split(',').any(|field| field == "ABSORB")
            {
                event
                    .payload
                    .splitn(3, ',')
                    .nth(2)
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0)
            } else {
                0
            }
        };
        match boss {
            "The Lich King" if target == "008F01" && damage => {
                let hp = self
                    .valks
                    .entry(event.target.into())
                    .or_insert(2_992_500 / 2);
                let amount = (*hp).min(event.amount);
                *hp -= amount;
                self.add("008F01", event.source, amount);
            }
            "The Lich King" if npc(event.source) == "008F01" && damage => {
                self.valks.insert(event.source.into(), 0);
            }
            "Freya" if target == "00808A" => {
                if event.kind == "SPELL_PERIODIC_HEAL" && event.excess != 0 {
                    self.overrides.remove("00808A");
                } else if damage {
                    self.add(target, event.source, event.amount);
                }
            }
            "Assembly of Iron" if matches!(target, "008059" | "00809F" | "008063") => {
                if damage {
                    self.add(target, event.source, event.amount);
                }
                if event.kind == "UNIT_DIED" || damage && event.excess != 0 {
                    self.dead.insert(target.into());
                    self.overrides.retain(|id, _| self.dead.contains(id));
                }
            }
            "Festergut" if event.spell == "72553" => {
                let stacks = self.stacks.entry(event.target.into()).or_default();
                match event.kind {
                    "SPELL_AURA_APPLIED" => *stacks = 1,
                    "SPELL_AURA_APPLIED_DOSE" => *stacks += 1,
                    "SPELL_AURA_REMOVED" => *stacks = 0,
                    _ => {}
                }
            }
            "Festergut" if target == "008F12" && damage => {
                let stacks = self.stacks.get(event.source).copied().unwrap_or(0);
                let pair = self.festergut.entry(event.source.into()).or_default();
                pair.0 += event.amount;
                pair.1 +=
                    (event.amount as f64 / (1.0 + stacks as f64 / 10.0)) as i64 - event.excess;
            }
            "Twin Val'kyr" if matches!(target, "0086C0" | "0086C1") => {
                let amount = absorbed();
                if amount > 0 {
                    self.add("0086C2", event.source, amount);
                }
            }
            "Kologarn" if target == "0080A6" => {
                let amount = absorbed();
                if amount > 0 {
                    self.add("0080A6", event.source, amount);
                }
            }
            _ => {}
        }
    }
}

fn owner<'a>(metadata: &'a Metadata, guid: &'a str) -> &'a str {
    metadata
        .units
        .get(guid)
        .and_then(|unit| unit.master_guid.as_deref())
        .unwrap_or(guid)
}

fn adjusted_target(boss: &str, npc: &str) -> bool {
    matches!(
        (boss, npc),
        ("The Lich King", "008F01")
            | ("Festergut", "008F12")
            | ("Freya", "00808A")
            | ("Kologarn", "0080A6")
            | ("Assembly of Iron", "008059" | "00809F" | "008063")
    )
}

fn allowed_aura(id: &str, spec: u8) -> bool {
    match id {
        "63848" | "58427" => spec != 25,
        "16870" => spec == 6,
        "57933" => !(24..28).contains(&spec),
        "54646" => !(12..16).contains(&spec),
        "23060" => {
            !(12..16).contains(&spec)
                && !(20..24).contains(&spec)
                && !matches!(spec, 29 | 31 | 33 | 35)
        }
        _ => true,
    }
}

impl Fight {
    fn observe(
        &mut self,
        index: u64,
        metadata: &Metadata,
        friendly: &HashSet<&str>,
        encounter: &Encounter,
        event: &Event<'_>,
        year: u32,
    ) {
        let origin = *self.origin.get_or_insert(event.stamp);
        let time = event.stamp.relative_to(origin, year).max(0);
        let duration = encounter.duration_millis() as i64;
        if index - encounter.start < 100_000
            && event.kind != "SPELL_AURA_REMOVED"
            && metadata.players.contains_key(event.source)
            && !self.specs.contains_key(event.source)
            && let Some(class) = metadata
                .classes
                .get(event.source)
                .map(String::as_str)
                .or_else(|| class_for_specialization_spell(event.spell))
        {
            self.classes
                .entry(event.source.into())
                .or_insert_with(|| class.into());
            if let Some(spec) = specialization(class, event.spell) {
                self.specs.insert(event.source.into(), spec);
            }
        }
        self.special.observe(&encounter.boss, event);
        if event.kind.starts_with("SPELL_A")
            && let Some((_, length, alias)) = RULES.auras.get(event.spell)
        {
            let id = alias
                .as_deref()
                .unwrap_or_else(|| RULES.auras.get_key_value(event.spell).unwrap().0);
            if id == "74297" {
                self.room.insert(time);
            }
            if metadata.players.contains_key(event.target) {
                let player = self.auras.entry(event.target.into()).or_default();
                let a = player.entry(id).or_insert_with(|| {
                    if matches!(id, "73822" | "73828") {
                        *self.trinkets.entry(id).or_default() += 1;
                    }
                    AuraSpan {
                        start: (event.kind != "SPELL_AURA_APPLIED").then_some(0),
                        ..AuraSpan::default()
                    }
                });
                a.observe(
                    time.min(duration),
                    event.kind == "SPELL_AURA_REMOVED",
                    id,
                    *length,
                );
            }
        }
        let player = owner(metadata, event.source);
        if !metadata.players.contains_key(player) {
            return;
        }
        let target = npc(event.target);
        if encounter.boss == "Valithria Dreamwalker" {
            if event.healing() {
                let amount = event.amount - event.excess;
                let pair = self.damage.entry(player.into()).or_default();
                pair.1 += amount;
                if target == "008FB5" && event.source == player {
                    pair.0 += amount;
                }
            }
        } else if damage_event(event.kind) {
            let pair = self.damage.entry(player.into()).or_default();
            if event.target != NIL_GUID && !friendly.contains(target) {
                pair.1 += event.amount;
            }
            if npc(event.source) != "00958D"
                && !adjusted_target(&encounter.boss, target)
                && (RULES.useful.contains(target) || guid_boss(event.target).is_some())
            {
                pair.0 += event.amount - event.excess;
            }
        }
    }

    fn finish(mut self, id: &str, metadata: &Metadata, encounter: &Encounter) -> Vec<Row> {
        let duration = encounter.duration_seconds;
        let duration_ms = encounter.duration_millis() as i64;
        let spec_of = |guid: &str| {
            let class = metadata
                .classes
                .get(guid)
                .or_else(|| self.classes.get(guid));
            let spec = self.specs.get(guid).or_else(|| metadata.specs.get(guid));
            row_spec(class.map_or("", String::as_str), spec.copied())
        };
        if encounter.boss == "Festergut" {
            let amounts = self.special.overrides.entry("008F12".into()).or_default();
            for (guid, (total, useful)) in self.special.festergut {
                let index = spec_of(&guid);
                let adjusted = matches!(index, 0..=11 | 15..=17 | 19 | 22 | 31 | 36..=38)
                    && (index != 1 || useful >= 2_000_000);
                amounts.insert(guid, if adjusted { useful } else { total });
            }
        }
        for (guid, amount) in self.special.overrides.into_values().flatten() {
            let player = owner(metadata, &guid);
            if npc(&guid) != "00958D" && metadata.players.contains_key(player) {
                self.damage.entry(player.into()).or_default().0 += amount;
            }
        }
        let buff = self
            .trinkets
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(id, _)| *id);
        let room = self.room.last().map(|last| {
            let gaps = self
                .room
                .iter()
                .zip(self.room.iter().skip(1))
                .filter(|(a, b)| *b - *a > 60_000)
                .count();
            let end = (duration_ms - last).max(0);
            (
                (gaps + usize::from(end >= 10_000)) as u32,
                gaps as i64 * 50_000 + end.min(50_000),
            )
        });
        self.damage
            .into_iter()
            .filter(|(_, (useful, total))| *useful != 0 || *total != 0)
            .filter(|(_, (useful, _))| encounter.boss != "Valithria Dreamwalker" || *useful != 0)
            .map(|(guid, (useful, total))| {
                let spec = spec_of(&guid);
                let mut auras = Vec::new();
                if let Some(player) = self.auras.get_mut(&guid) {
                    for (id, a) in player {
                        if Some(*id) == buff || *id == "74297" || !allowed_aura(id, spec) {
                            continue;
                        }
                        let (group, limit, _) = &RULES.auras[*id];
                        a.observe(duration_ms, true, id, *limit);
                        if a.count > 0 {
                            auras.push((
                                id.parse().expect("numeric aura ID"),
                                a.count,
                                round(a.millis as f64 / duration_ms as f64 * 100.0, 10.0),
                                *group,
                            ));
                        }
                    }
                    if let Some(id) = buff {
                        auras.push((
                            id.parse().expect("numeric aura ID"),
                            1,
                            100.0,
                            RULES.auras[id].0,
                        ));
                    }
                    if let Some((count, time)) = room {
                        auras.push((
                            74297,
                            count,
                            round(time as f64 / duration_ms as f64 * 100.0, 10.0),
                            RULES.auras["74297"].0,
                        ));
                    }
                }
                Row {
                    report: id.into(),
                    guid: guid[guid.len().saturating_sub(7)..].into(),
                    name: metadata.players[&guid].clone(),
                    duration,
                    useful,
                    total,
                    spec,
                    auras,
                }
            })
            .collect()
    }
}

type FightKey = (String, String);
type Run = (String, f64, f64, String, Option<u8>);

struct CachedReport {
    fingerprint: (u64, SystemTime),
    fights: BTreeMap<FightKey, Vec<Row>>,
    runs: BTreeMap<FightKey, Run>,
    characters: BTreeMap<String, (String, u8)>,
}

fn cached_report(
    root: &Path,
    id: &str,
    groups: &LegacyGroups,
    fingerprint: (u64, SystemTime),
) -> io::Result<CachedReport> {
    let mut report = Report::open_group(root, id, groups)?;
    let year = report.id[..2].parse::<u32>().unwrap_or_default() + 2000;
    let metadata = std::mem::take(&mut report.metadata);
    let mut friendly = HashSet::from(["000000"]);
    friendly.extend(
        metadata
            .units
            .iter()
            .filter(|(_, unit)| {
                unit.master_guid
                    .as_deref()
                    .is_some_and(crate::combat::player_guid)
            })
            .map(|(guid, _)| npc(guid)),
    );
    let mut kills = metadata
        .encounters
        .iter()
        .filter(|fight| {
            fight.outcome == "kill" && fight.duration_millis() > 0 && fight.end > fight.start
        })
        .collect::<Vec<_>>();
    kills.sort_by_key(|fight| fight.start);
    let mut builders = (0..kills.len())
        .map(|_| Fight::default())
        .collect::<Vec<_>>();
    let mut starts = vec![None; kills.len()];
    let mut ends = vec![None; kills.len()];
    if let Some(first) = kills.first() {
        let mut next = 0;
        let mut active = Vec::<usize>::new();
        let end = kills.iter().map(|fight| fight.end).max().unwrap();
        report.visit(Some(first.start..end), |row, line| {
            active.retain(|index| kills[*index].end > row);
            while next < kills.len() && kills[next].start <= row {
                active.push(next);
                next += 1;
            }
            if !active.is_empty()
                && let Some(event) = Event::parse(line)
            {
                for &index in &active {
                    starts[index].get_or_insert(event.stamp);
                    ends[index] = Some(event.stamp);
                    builders[index].observe(row, &metadata, &friendly, kills[index], &event, year);
                }
            }
        })?;
    }
    let prefix = id.splitn(4, "--").take(3).collect::<Vec<_>>().join("--");
    let mut fights = BTreeMap::<FightKey, Vec<Row>>::new();
    for (kill, builder) in kills.iter().zip(builders) {
        fights
            .entry((kill.boss.clone(), kill.difficulty.clone()))
            .or_default()
            .extend(builder.finish(&prefix, &metadata, kill));
    }
    let mut runs = BTreeMap::new();
    for raid in &CATALOG.raids {
        let mut modes = BTreeMap::<&str, BTreeMap<&str, usize>>::new();
        for (index, kill) in kills
            .iter()
            .enumerate()
            .filter(|(_, kill)| raid.bosses.contains(&kill.boss))
        {
            modes
                .entry(&kill.difficulty)
                .or_default()
                .entry(&kill.boss)
                .or_insert(index);
        }
        for (mode, bosses) in modes {
            if bosses.len() < raid.total {
                continue;
            }
            let first = *bosses.values().min().unwrap();
            let last = *bosses.values().max().unwrap();
            if let (Some(start), Some(end)) = (starts[first], ends[last]) {
                let total = end.relative_to(start, year) as f64 / 1000.0;
                let segments = bosses.values().map(|i| kills[*i].duration_seconds).sum();
                let guild = metadata
                    .lockout
                    .as_ref()
                    .and_then(|lockout| lockout.guild.as_deref())
                    .unwrap_or("—")
                    .into();
                let faction = metadata
                    .lockout
                    .as_ref()
                    .and_then(|lockout| lockout.faction);
                runs.insert(
                    (raid.name.clone(), mode.into()),
                    (prefix.clone(), total, segments, guild, faction),
                );
            }
        }
    }
    Ok(CachedReport {
        fingerprint,
        fights,
        runs,
        characters: metadata
            .players
            .iter()
            .filter_map(|(guid, name)| {
                let class = metadata.classes.get(guid)?;
                let spec = row_spec(class, metadata.specs.get(guid).copied());
                (spec < 40).then(|| (name.to_lowercase(), (name.clone(), spec)))
            })
            .collect(),
    })
}

fn realm(id: &str) -> &str {
    id.splitn(4, "--").nth(3).unwrap_or_default()
}

pub fn servers(root: &Path, groups: &LegacyGroups) -> io::Result<BTreeSet<String>> {
    Ok(list_ids(root)?
        .into_iter()
        .filter(|id| !groups.is_fragment(id))
        .map(|id| realm(&id).into())
        .collect())
}

#[derive(Default)]
pub struct Index {
    reports: BTreeMap<String, CachedReport>,
}

fn raid_name(name: &str) -> Option<&str> {
    CATALOG
        .aliases
        .get(name)
        .and_then(|index| CATALOG.raids.get(*index))
        .or_else(|| CATALOG.raids.iter().find(|raid| raid.name == name))
        .map(|raid| raid.name.as_str())
}

pub fn raid_options() -> BTreeMap<&'static str, Vec<&'static str>> {
    CATALOG
        .raids
        .iter()
        .enumerate()
        .map(|(index, raid)| {
            let name = CATALOG
                .aliases
                .iter()
                .find(|(name, slot)| **slot == index && name.is_ascii())
                .map_or(raid.name.as_str(), |(name, _)| name.as_str());
            let bosses = raid.bosses.iter().map(String::as_str).collect();
            (name, bosses)
        })
        .collect()
}

impl Index {
    pub fn default_fight(&self, server: &str) -> Option<&FightKey> {
        self.reports
            .iter()
            .rev()
            .filter(|(id, _)| realm(id) == server)
            .find_map(|(_, report)| {
                report
                    .fights
                    .iter()
                    .filter(|(key, rows)| !rows.is_empty() && valid_mode(&key.1))
                    .max_by_key(|(_, rows)| rows.len())
                    .map(|(key, _)| key)
            })
    }
    pub fn refresh(&mut self, root: &Path, groups: &LegacyGroups, server: &str) -> io::Result<()> {
        self.refresh_public(root, groups, server, |_| true)
    }

    pub fn refresh_public(
        &mut self,
        root: &Path,
        groups: &LegacyGroups,
        server: &str,
        public: impl Fn(&str) -> bool,
    ) -> io::Result<()> {
        let ids = list_ids(root)?
            .into_iter()
            .filter(|id| !groups.is_fragment(id))
            .filter(|id| public(id))
            .collect::<BTreeSet<_>>();
        self.reports.retain(|id, _| ids.contains(id));
        for id in ids.into_iter().filter(|id| realm(id) == server) {
            let dir = root.join("LogsDir");
            let file = dir.join(format!("{id}.uwu"));
            let meta = if file.is_file() {
                fs::metadata(file)?
            } else {
                fs::metadata(dir.join(&id).join("LOGS_CUT.zstd"))?
            };
            let fingerprint = (meta.len(), meta.modified()?);
            if self
                .reports
                .get(&id)
                .is_none_or(|previous| previous.fingerprint != fingerprint)
            {
                self.reports
                    .insert(id.clone(), cached_report(root, &id, groups, fingerprint)?);
            }
        }
        Ok(())
    }

    fn rows(&self, server: &str, key: &FightKey) -> Vec<&Row> {
        let mut daily = BTreeMap::<_, &Row>::new();
        for row in self
            .reports
            .iter()
            .filter(|(id, _)| realm(id) == server)
            .filter_map(|(_, report)| report.fights.get(key))
            .flatten()
        {
            let entry = daily.entry(row.daily_key()).or_insert(row);
            if row.dps(row.useful) > entry.dps(entry.useful) {
                *entry = row;
            }
        }
        daily.into_values().collect()
    }

    pub fn top(&self, query: &TopQuery) -> Vec<TopRow<'_>> {
        let mut rows = self.rows(&query.fight.server, &query.fight.key());
        rows.retain(|row| {
            (query.class_i.0 == -1
                || i32::from(row.spec / 4) == query.class_i.0
                    && (query.spec_i.0 == -1 || i32::from(row.spec % 4) == query.spec_i.0))
                && (query.externals
                    || !row.auras.iter().any(|(id, _, _, _)| EXTERNALS.contains(id)))
        });
        rows.sort_by(|a, b| match query.sort_by.as_str() {
            "head-useful-amount" => b.useful.cmp(&a.useful),
            "head-total-amount" => b.total.cmp(&a.total),
            "head-total-dps" => b.dps(b.total).total_cmp(&a.dps(a.total)),
            "head-duration" => a.duration.total_cmp(&b.duration),
            "head-date" => b.daily_key().cmp(&a.daily_key()),
            _ => b.dps(b.useful).total_cmp(&a.dps(a.useful)),
        });
        let mut seen = HashSet::new();
        rows.into_iter()
            .filter(|row| !query.best_only || seen.insert(&row.guid))
            .take(query.limit.0 as usize)
            .map(Row::top_row)
            .collect()
    }

    pub fn stats(
        &self,
        query: &FightQuery,
    ) -> BTreeMap<String, BTreeMap<&'static str, Percentile>> {
        let mut samples = BTreeMap::<u8, Vec<f64>>::new();
        for row in self
            .rows(&query.server, &query.key())
            .into_iter()
            .filter(|row| eligible_spec(row.spec))
        {
            samples
                .entry(row.spec)
                .or_default()
                .push(row.dps(row.useful));
        }
        samples
            .into_iter()
            .filter(|(_, values)| values.len() >= 5)
            .filter_map(|(index, values)| Some((spec(index)?.key, percentiles(values))))
            .collect()
    }

    pub fn raid_rank<'a>(&self, query: &'a RankQuery) -> BTreeMap<&'a str, Rank> {
        let mut samples = BTreeMap::<u8, Vec<f64>>::new();
        for row in self.rows(&query.fight.server, &query.fight.key()) {
            samples
                .entry(row.spec)
                .or_default()
                .push(row.dps(row.useful));
        }
        for values in samples.values_mut() {
            values.sort_by(f64::total_cmp);
        }
        query
            .dps
            .iter()
            .filter_map(|(name, dps)| {
                let dps = dps.as_f64().or_else(|| dps.as_str()?.parse().ok())?;
                let id = spec_id(query.specs.get(name)?)?;
                let scores = samples.get(&id).map(Vec::as_slice).unwrap_or_default();
                Some((name.as_str(), Rank::of(dps, scores)))
            })
            .collect()
    }

    fn point_fights(server: &str) -> &'static [FightKey] {
        let phase =
            PHASES.get(server).map_or(
                "Tier_10",
                |phase| {
                    if phase == "Tier_7" { "Tier_3" } else { phase }
                },
            );
        RULES.points.get(phase).unwrap_or(&RULES.points["Tier_10"])
    }

    fn point_scores(&self, server: &str, key: &FightKey, spec: i32) -> Vec<&Row> {
        let mut scores = self.rows(server, key);
        scores.retain(|row| i32::from(row.spec) == spec && row.useful > 0);
        scores.sort_by(|a, b| b.dps(b.useful).total_cmp(&a.dps(a.useful)));
        scores
    }

    pub fn points(&self, query: &PointsQuery) -> Vec<(String, f64, i64)> {
        let spec = query.class_i.0 * 4 + query.spec_i.0;
        let mut points = BTreeMap::<&str, (&str, f64)>::new();
        for key in Self::point_fights(&query.server) {
            for score in fight_scores(&self.point_scores(&query.server, key, spec)) {
                let row = score.row;
                points.entry(&row.guid).or_insert((&row.name, 0.0)).1 += score.points();
            }
        }
        let mut rows = points.into_values().collect::<Vec<_>>();
        rows.sort_by(|a, b| b.1.total_cmp(&a.1));
        let max = rows.first().map_or(1.0, |row| row.1);
        rows.into_iter()
            .map(|(name, score)| {
                (
                    name.into(),
                    round(score / max * 100.0, 1000.0),
                    (score * 10_000.0) as i64,
                )
            })
            .collect()
    }

    pub fn character(
        &self,
        server: &str,
        name: &str,
        selected_spec: Option<u8>,
    ) -> Option<serde_json::Value> {
        use serde_json::json;
        let name = name.to_lowercase();
        let (display_name, detected) = self
            .reports
            .iter()
            .rev()
            .filter(|(id, _)| realm(id) == server)
            .find_map(|(_, report)| report.characters.get(&name))?;
        let class = detected / 4;
        let default = [3, 1, 2, 2, 3, 3, 2, 1, 2, 2][class as usize];
        let selected = selected_spec
            .filter(|value| (1..=3).contains(value))
            .unwrap_or(default);
        let spec = i32::from(class * 4 + selected);
        let rank = self.points(&PointsQuery {
            server: server.into(),
            class_i: Numeric(class.into()),
            spec_i: Numeric(selected.into()),
        });
        let overall = rank
            .iter()
            .enumerate()
            .find(|(_, row)| row.0.to_lowercase() == name);
        let mut fights = Self::point_fights(server)
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        // A realm may run earlier raids alongside its configured ranking tier.
        fights.extend(
            self.reports
                .iter()
                .filter(|(id, _)| realm(id) == server)
                .flat_map(|(_, report)| report.fights.keys().cloned()),
        );
        let mut bosses = serde_json::Map::new();
        for key in fights {
            let rows = self.point_scores(server, &key, spec);
            let scores = fight_scores(&rows);
            let label = format!("{} ({})", key.0, key.1);
            let Some(score) = scores
                .iter()
                .find(|score| score.row.name.to_lowercase() == name)
            else {
                continue;
            };
            let row = score.row;
            let auras = row
                .auras
                .iter()
                .map(|(id, count, uptime, kind)| format!("{id}/{count}/{uptime}/{kind}"))
                .collect::<Vec<_>>()
                .join("#");
            bosses.insert(label, json!({
                "rank_players": score.player_rank, "rank_raids": score.raid_rank,
                "points": score.points() * 10000.0, "dps_max": row.dps(row.useful),
                "fastest_kill_duration": row.duration, "report_id": format!("{}--{server}",row.report),
                "raids": rows.iter().filter(|entry| entry.guid == row.guid).count(), "auras": auras,
                "spec_total_players": scores.len(), "spec_total_raids": rows.len(),
                "spec_r1_dps": rows[0].dps(rows[0].useful), "points_rank_players": score.player_points * 10000.0,
                "points_rank_raids": score.raid_points * 10000.0, "points_dps": score.dps_points * 10000.0,
            }));
        }
        Some(json!({
            "name": display_name, "server": server, "class_i": class, "spec_i": selected,
            "overall_rank": overall.map(|(index, _)| index + 1),
            "overall_points": overall.map_or(0, |(_, row)| row.2), "bosses": bosses,
        }))
    }

    pub fn speedruns(&self, query: &SpeedrunQuery) -> Vec<&Run> {
        let key = (
            raid_name(&query.raid).unwrap_or(&query.raid).into(),
            query.mode.to_ascii_uppercase(),
        );
        let mut runs = self
            .reports
            .iter()
            .filter(|(id, _)| realm(id) == query.server)
            .filter_map(|(_, report)| report.runs.get(&key))
            .collect::<Vec<_>>();
        if query.sort_by == "head-speedrun-segments-sum" {
            runs.sort_by(|a, b| a.2.total_cmp(&b.2));
        } else {
            runs.sort_by(|a, b| a.1.total_cmp(&b.1));
        }
        runs
    }
}

struct FightScore<'a> {
    row: &'a Row,
    player_rank: usize,
    raid_rank: usize,
    player_points: f64,
    raid_points: f64,
    dps_points: f64,
}

impl FightScore<'_> {
    fn points(&self) -> f64 {
        self.player_points
            .max(self.raid_points)
            .max(self.dps_points)
    }
}

fn fight_scores<'a>(rows: &[&'a Row]) -> Vec<FightScore<'a>> {
    let Some(leader) = rows.first() else {
        return Vec::new();
    };
    let players = rows
        .iter()
        .map(|row| &row.guid)
        .collect::<HashSet<_>>()
        .len()
        .min(10_000);
    let total = rows.len();
    let mut seen = HashSet::new();
    let mut scores = Vec::new();
    for (offset, row) in rows.iter().enumerate() {
        if seen.insert(&row.guid) {
            let player_rank = seen.len();
            scores.push(FightScore {
                row,
                player_rank,
                raid_rank: offset + 1,
                player_points: (players.saturating_sub(player_rank) as f64)
                    / players.saturating_sub(1).max(1) as f64,
                raid_points: if total < 10_000 {
                    (total - offset - 1) as f64 / total.saturating_sub(1).max(1) as f64
                } else {
                    rank_score(offset + 1)
                },
                dps_points: row.dps(row.useful) / leader.dps(leader.useful),
            });
        }
    }
    scores
}

#[derive(Deserialize, Serialize)]
pub struct RankQuery {
    #[serde(flatten)]
    pub fight: FightQuery,
    pub dps: BTreeMap<String, serde_json::Value>,
    pub specs: BTreeMap<String, serde_json::Value>,
}

fn spec_id(value: &serde_json::Value) -> Option<u8> {
    let named = || {
        let name = value.as_str()?;
        (0..40).find(|&index| spec(index).is_some_and(|s| s.name.eq_ignore_ascii_case(name)))
    };
    value
        .as_u64()
        .and_then(|value| u8::try_from(value).ok())
        .or_else(|| value.as_str()?.parse().ok())
        .or_else(named)
        .filter(|&id| eligible_spec(id))
}

#[derive(Deserialize, Serialize)]
pub struct Rank {
    rank: usize,
    percentile: f64,
    from_spec_top1: f64,
    total_raids_for_spec: usize,
}

impl Rank {
    fn of(dps: f64, scores: &[f64]) -> Self {
        let Some(top) = scores.last().filter(|top| **top > 0.0) else {
            return Self {
                rank: 1,
                percentile: 100.0,
                from_spec_top1: 100.0,
                total_raids_for_spec: 1,
            };
        };
        let dps = dps + 0.01;
        let below = scores.partition_point(|score| *score <= dps);
        Self {
            rank: scores.len() - below + 1,
            percentile: (below as f64 / scores.len() as f64 * 10000.0).floor() / 100.0,
            from_spec_top1: round(dps / top * 100.0, 10.0),
            total_raids_for_spec: scores.len(),
        }
    }
}

#[derive(Serialize)]
pub struct Percentile {
    dps: f64,
    raids: usize,
}

fn percentiles(mut values: Vec<f64>) -> BTreeMap<&'static str, Percentile> {
    values.sort_by(f64::total_cmp);
    let max = values.len() - 1;
    let mut result = BTreeMap::from([
        (
            "top100",
            Percentile {
                dps: values[max],
                raids: 1,
            },
        ),
        (
            "all",
            Percentile {
                dps: 0.0,
                raids: values.len(),
            },
        ),
    ]);
    for (label, p) in [
        ("top99", 99),
        ("top95", 95),
        ("top90", 90),
        ("top75", 75),
        ("top50", 50),
        ("top10", 10),
    ] {
        let index = max as f64 * (p as f64 / 100.0);
        let (lo, hi) = (index.floor() as usize, index.ceil() as usize);
        let fraction = index.fract();
        let delta = values[hi] - values[lo];
        let value = if fraction < 0.5 {
            values[lo] + delta * fraction
        } else {
            values[hi] - delta * (1.0 - fraction)
        };
        let dps = round(value, 100.0);
        let raids = values.len() - values.partition_point(|value| *value <= dps);
        result.insert(label, Percentile { dps, raids });
    }
    result
}

fn rank_score(rank: usize) -> f64 {
    let mut previous = 1;
    let mut points = 1.0;
    for (threshold, penalty) in [
        (100, 0.0001),
        (200, 0.00009),
        (300, 0.00008),
        (400, 0.00007),
        (500, 0.00006),
        (1000, 0.00005),
        (2000, 0.00004),
        (10000, 0.00003),
        (75000, 0.00001),
    ] {
        points -= (rank.min(threshold) - previous) as f64 * penalty;
        if rank <= threshold {
            break;
        }
        previous = threshold;
    }
    points
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Number {
    Integer(i32),
    String(String),
}

#[derive(Deserialize)]
#[serde(try_from = "Number")]
pub struct Numeric(i32);

impl TryFrom<Number> for Numeric {
    type Error = std::num::ParseIntError;
    fn try_from(value: Number) -> Result<Self, Self::Error> {
        Ok(Self(match value {
            Number::Integer(n) => n,
            Number::String(n) => n.parse()?,
        }))
    }
}

#[derive(Deserialize, Serialize)]
pub struct FightQuery {
    pub server: String,
    pub boss: String,
    pub mode: String,
}

pub fn valid_mode(mode: &str) -> bool {
    matches!(
        mode.to_ascii_uppercase().as_str(),
        "10N" | "10H" | "25N" | "25H"
    )
}

impl FightQuery {
    fn key(&self) -> FightKey {
        (self.boss_name().to_owned(), self.mode.to_ascii_uppercase())
    }

    fn boss_name(&self) -> &str {
        if self.boss == "Sulfuron Harbinger" {
            "Sulfuron Harbringer"
        } else {
            &self.boss
        }
    }

    pub fn valid(&self) -> bool {
        valid_mode(&self.mode)
            && (CATALOG.bosses.values().any(|boss| boss == self.boss_name())
                || CATALOG
                    .raids
                    .iter()
                    .any(|raid| raid.bosses.contains(self.boss_name()))
                || self.boss == "Chess Event")
    }
}

#[derive(Deserialize)]
pub struct TopQuery {
    #[serde(flatten)]
    pub fight: FightQuery,
    class_i: Numeric,
    spec_i: Numeric,
    #[serde(default = "default_sort")]
    sort_by: String,
    #[serde(default = "default_limit")]
    limit: Numeric,
    #[serde(default = "enabled")]
    best_only: bool,
    #[serde(default = "enabled")]
    externals: bool,
}

fn default_sort() -> String {
    "head-useful-dps".into()
}
fn default_limit() -> Numeric {
    Numeric(100)
}
fn enabled() -> bool {
    true
}

impl TopQuery {
    pub fn valid(&self) -> bool {
        self.fight.valid()
            && (-1..10).contains(&self.class_i.0)
            && matches!(self.spec_i.0, -1 | 1..=3)
            && matches!(self.limit.0, 10 | 100 | 1000 | 10000 | 50000)
            && matches!(
                self.sort_by.as_str(),
                "head-useful-dps"
                    | "head-useful-amount"
                    | "head-total-dps"
                    | "head-total-amount"
                    | "head-duration"
                    | "head-date"
            )
    }
}

#[derive(Deserialize)]
pub struct PointsQuery {
    pub server: String,
    class_i: Numeric,
    spec_i: Numeric,
}

impl PointsQuery {
    pub fn valid(&self) -> bool {
        (0..10).contains(&self.class_i.0) && (1..=3).contains(&self.spec_i.0)
    }
}

#[derive(Deserialize)]
pub struct SpeedrunQuery {
    pub server: String,
    pub raid: String,
    #[serde(default = "default_speedrun_mode")]
    mode: String,
    #[serde(default = "default_speedrun_sort")]
    sort_by: String,
}

fn default_speedrun_mode() -> String {
    "25H".into()
}
fn default_speedrun_sort() -> String {
    "head-speedrun-total-length".into()
}

impl SpeedrunQuery {
    pub fn valid(&self) -> bool {
        valid_mode(&self.mode)
            && raid_name(&self.raid).is_some()
            && matches!(
                self.sort_by.as_str(),
                "head-speedrun-total-length" | "head-speedrun-segments-sum"
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(json: serde_json::Value) -> TopQuery {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn rankings_keep_daily_records_and_use_python_percentile_thresholds() {
        let mut index = Index::default();
        let key = ("Nefarian".to_owned(), "25N".to_owned());
        for day in 1..=5 {
            let prefix = format!("26-11-{day:02}--20-00--Uploader");
            let row = Row {
                report: prefix.clone(),
                guid: "0000123".into(),
                name: "Hunter".into(),
                duration: 10.0,
                useful: day * 100,
                total: day * 125,
                spec: 11,
                auras: Vec::new(),
            };
            let mut rows = vec![row.clone()];
            if day == 5 {
                rows.push(Row {
                    useful: 50,
                    ..row.clone()
                });
                rows.push(Row {
                    guid: "0000456".into(),
                    name: "Buffed".into(),
                    useful: 900,
                    auras: vec![(10060, 1, 80.0, 0)],
                    ..row
                });
            }
            index.reports.insert(
                format!("{prefix}--stable"),
                CachedReport {
                    fingerprint: (0, SystemTime::UNIX_EPOCH),
                    fights: BTreeMap::from([(key.clone(), rows)]),
                    runs: BTreeMap::new(),
                    characters: BTreeMap::new(),
                },
            );
        }

        let mut args = serde_json::json!({
            "server":"stable", "boss":"Nefarian", "mode":"25n",
            "class_i":"2", "spec_i":3, "best_only":true, "externals":false, "limit":50000
        });
        let filtered = query(args.clone());
        assert!(filtered.valid());
        assert_eq!(
            index
                .top(&filtered)
                .iter()
                .map(|row| row.4)
                .collect::<Vec<_>>(),
            [500]
        );
        args["best_only"] = false.into();
        let all = query(args);
        assert_eq!(
            index.top(&all).iter().map(|row| row.4).collect::<Vec<_>>(),
            [500, 400, 300, 200, 100]
        );
        let stats = index.stats(&all.fight);
        let hunter = &stats["survival-hunter"];
        assert_eq!(hunter["all"].raids, 6);
        assert_eq!(hunter["top50"].dps, 35.0);
        assert_eq!(hunter["top95"].dps, 80.0);
        assert_eq!(hunter["top95"].raids, 1);
    }
}
