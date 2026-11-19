use crate::combat::{CATALOG, Event, Stamp, guid_boss, normalize, player_guid, slug};
use crate::storage::{ReportFile, ReportWriter, read_event};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Serialize, Deserialize)]
pub struct Encounter {
    pub boss: String,
    pub boss_slug: String,
    pub start: u64,
    pub end: u64,
    pub attempt: usize,
    pub difficulty: String,
    pub outcome: String,
    pub duration_seconds: f64,
    pub start_second: u64,
    pub end_second: u64,
}

impl Encounter {
    pub fn query(&self) -> String {
        format!(
            "?boss={}&mode={}&attempt={}&s={}&f={}",
            self.boss_slug, self.difficulty, self.attempt, self.start_second, self.end_second
        )
    }
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Unit {
    pub name: String,
    pub master_guid: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Metadata {
    pub encounters: Vec<Encounter>,
    pub players: BTreeMap<String, String>,
    pub units: BTreeMap<String, Unit>,
    pub classes: BTreeMap<String, String>,
    #[serde(default)]
    pub time_index: Vec<(u64, u64)>,
}

impl Metadata {
    fn observe_units(&mut self, event: &Event<'_>) {
        for (guid, name) in [
            (event.source, event.source_name),
            (event.target, event.target_name),
        ] {
            if player_guid(guid) && !self.players.contains_key(guid) {
                self.players.insert(guid.into(), name.into());
            }
            if !self.units.contains_key(guid) {
                self.units.insert(
                    guid.into(),
                    Unit {
                        name: name.into(),
                        master_guid: None,
                    },
                );
            }
        }
        if player_guid(event.source)
            && !self.classes.contains_key(event.source)
            && let Some(class) = crate::absorbs::RULES.class_book.get(event.spell)
        {
            self.classes.insert(event.source.into(), class.clone());
        }
        if event.kind == "SPELL_SUMMON" && player_guid(event.source) {
            self.units.get_mut(event.target).unwrap().master_guid = Some(event.source.into());
        }
    }
}

pub struct Report {
    pub id: String,
    pub metadata: Metadata,
    source: Source,
    clock: Option<(Stamp, Stamp)>,
    legacy_rows: u64,
}

enum Source {
    Legacy(Vec<PathBuf>),
    Indexed(ReportFile<File>),
}

impl Report {
    pub fn open(root: &Path, id: &str) -> io::Result<Self> {
        if !valid_id(id) {
            return Err(io::ErrorKind::NotFound.into());
        }
        let directory = root.join("LogsDir");
        let native = directory.join(format!("{id}.uwu"));
        match File::open(native) {
            Ok(file) => {
                let report = ReportFile::open(file)?;
                let metadata =
                    serde_json::from_value(report.metadata().clone()).map_err(io::Error::other)?;
                Ok(Self {
                    id: id.into(),
                    metadata,
                    source: Source::Indexed(report),
                    clock: None,
                    legacy_rows: 0,
                })
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let legacy = directory.join(id);
                let mut report = Self {
                    id: id.into(),
                    source: Source::Legacy(vec![legacy.join("LOGS_CUT.zstd")]),
                    clock: None,
                    legacy_rows: 0,
                    metadata: Metadata {
                        players: read_cache(&legacy.join("PLAYERS_DATA.json"))?,
                        units: read_cache(&legacy.join("GUIDS_DATA.json"))?,
                        classes: read_cache(&legacy.join("CLASSES_DATA.json"))?,
                        ..Metadata::default()
                    },
                };
                let stored_encounters: BTreeMap<String, Vec<[u64; 2]>> =
                    read_cache(&legacy.join("ENCOUNTER_DATA.json"))?;
                let timestamps: Vec<u64> = read_cache(&legacy.join("TIMESTAMP_DATA.json"))?;
                report.load_legacy_encounters(stored_encounters, &timestamps)?;
                Ok(report)
            }
            Err(error) => Err(error),
        }
    }

    pub fn open_group(root: &Path, id: &str, groups: &LegacyGroups) -> io::Result<Self> {
        let id = groups.aliases.get(id).map_or(id, String::as_str);
        let mut report = Self::open(root, id)?;
        let Some(parts) = groups.parts.get(id) else {
            return Ok(report);
        };
        let Source::Legacy(paths) = &mut report.source else {
            return Ok(report);
        };
        let year = report.id[..2].parse::<u32>().unwrap_or(0) + 2000;
        let mut attempts = BTreeMap::<String, usize>::new();
        for segment in &report.metadata.encounters {
            *attempts.entry(segment.boss.clone()).or_default() += 1;
        }
        for part_id in parts.iter().skip(1) {
            let mut part = Self::open(root, part_id)?;
            if let Source::Legacy(fragment) = part.source {
                paths.extend(fragment);
                report.metadata.players.append(&mut part.metadata.players);
                for (guid, unit) in part.metadata.units {
                    let existing = report
                        .metadata
                        .units
                        .entry(guid)
                        .or_insert_with(|| unit.clone());
                    if existing.master_guid.is_none() {
                        existing.master_guid = unit.master_guid;
                    }
                }
                report.metadata.classes.append(&mut part.metadata.classes);
                let elapsed = part
                    .clock
                    .zip(report.clock)
                    .map(|((start, _), (first, _))| {
                        start.relative_to(first, year).max(0) as u64 / 1000
                    })
                    .unwrap_or(0);
                report.metadata.time_index.extend(
                    part.metadata
                        .time_index
                        .into_iter()
                        .map(|(second, line)| (second + elapsed, line + report.legacy_rows)),
                );
                for mut segment in part.metadata.encounters {
                    segment.start += report.legacy_rows;
                    segment.end += report.legacy_rows;
                    segment.start_second += elapsed;
                    segment.end_second += elapsed;
                    let attempt = attempts.entry(segment.boss.clone()).or_default();
                    segment.attempt = *attempt;
                    *attempt += 1;
                    report.metadata.encounters.push(segment);
                }
                report.legacy_rows += part.legacy_rows;
            }
        }
        Ok(report)
    }

    pub fn visit(
        &mut self,
        range: Option<Range<u64>>,
        mut visit: impl FnMut(u64, &str),
    ) -> io::Result<()> {
        struct Visitor<'a, F> {
            visit: &'a mut F,
            index: u64,
        }

        impl<F: FnMut(u64, &str)> Write for Visitor<'_, F> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                let text = std::str::from_utf8(bytes).map_err(io::Error::other)?;
                for line in text.lines() {
                    (self.visit)(self.index, line);
                    self.index += 1;
                }
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        match &mut self.source {
            Source::Indexed(report) => {
                let range = range.unwrap_or(0..report.event_count());
                let visitor = Visitor {
                    visit: &mut visit,
                    index: range.start,
                };
                report.read_events(range, visitor)
            }
            Source::Legacy(paths) => {
                let range = range.unwrap_or(0..u64::MAX);
                let mut raw = Vec::new();
                let mut index = 0;
                for path in paths {
                    let reader = zstd::stream::read::Decoder::new(File::open(path)?)?;
                    let mut reader = BufReader::new(reader);
                    while index < range.end && read_event(&mut reader, &mut raw)? != 0 {
                        if index >= range.start {
                            visit(
                                index,
                                std::str::from_utf8(&raw)
                                    .map_err(io::Error::other)?
                                    .trim_end_matches(['\r', '\n']),
                            );
                        }
                        index += 1;
                    }
                    if index >= range.end {
                        break;
                    }
                }
                Ok(())
            }
        }
    }

    pub fn copy_log(&mut self, mut output: impl Write) -> io::Result<()> {
        match &mut self.source {
            Source::Indexed(report) => report.read_events(0..report.event_count(), output),
            Source::Legacy(paths) => {
                for path in paths {
                    let mut reader = zstd::stream::read::Decoder::new(File::open(path)?)?;
                    io::copy(&mut reader, &mut output)?;
                }
                Ok(())
            }
        }
    }

    fn load_legacy_encounters(
        &mut self,
        stored: BTreeMap<String, Vec<[u64; 2]>>,
        timestamps: &[u64],
    ) -> io::Result<()> {
        let mut windows = stored
            .into_iter()
            .flat_map(|(boss, entries)| {
                entries
                    .into_iter()
                    .enumerate()
                    .filter(|(_, span)| span[0] < span[1])
                    .map(move |(attempt, [start, end])| {
                        Window::new(boss.clone(), attempt, start, end)
                    })
            })
            .collect::<Vec<_>>();
        windows.sort_by_key(|entry| entry.start);
        let mut next = 0;
        let mut active = Vec::new();
        let year = 2000 + self.id[..2].parse::<u32>().unwrap_or(0);
        let mut first = None;
        let mut last = None;
        let mut rows = 0;
        let mut rebuilt = Metadata::default();
        let mut pulls = windows.is_empty().then(crate::encounters::Pulls::default);
        let recover_units = self.metadata.units.is_empty() || self.metadata.players.is_empty();
        self.visit(None, |index, line| {
            rows = index + 1;
            active.retain(|entry: &usize| windows[*entry].end > index);
            while next < windows.len() && windows[next].start <= index {
                active.push(next);
                next += 1;
            }
            if let Some(event) = Event::parse(line) {
                let origin = *first.get_or_insert(event.stamp);
                last = Some(event.stamp);
                let time = event.stamp.relative_to(origin, year).max(0) as u64;
                for entry in &active {
                    windows[*entry].observe(index, time, &event);
                }
                if let Some(pulls) = &mut pulls {
                    pulls.observe(index, time, &event);
                }
                if recover_units {
                    rebuilt.observe_units(&event);
                }
                let second = time / 1000;
                if timestamps.is_empty()
                    && rebuilt
                        .time_index
                        .last()
                        .is_none_or(|(last, _)| second >= last + 5)
                {
                    rebuilt.time_index.push((second, index));
                }
            }
        })?;
        self.clock = first.zip(last);
        self.legacy_rows = rows;
        if recover_units {
            rebuilt.players.append(&mut self.metadata.players);
            rebuilt.classes.append(&mut self.metadata.classes);
            rebuilt.units.append(&mut self.metadata.units);
            self.metadata.players = rebuilt.players;
            self.metadata.classes = rebuilt.classes;
            self.metadata.units = rebuilt.units;
        }
        self.metadata.time_index = timestamps
            .iter()
            .copied()
            .enumerate()
            .step_by(5)
            .map(|(second, line)| (second as u64, line))
            .collect();
        if timestamps.is_empty() {
            self.metadata.time_index = rebuilt.time_index;
        }
        if let Some(pulls) = pulls {
            self.metadata.encounters = pulls.finish();
            return Ok(());
        }
        self.metadata.encounters = windows
            .into_iter()
            .filter_map(|window| {
                let mut encounter = window.finish()?;
                encounter.start_second = timestamps
                    .partition_point(|row| *row < encounter.start)
                    .saturating_sub(1) as u64;
                encounter.end_second =
                    timestamps.partition_point(|row| *row < encounter.end) as u64;
                Some(encounter)
            })
            .collect();
        Ok(())
    }
}

struct Window {
    boss: String,
    attempt: usize,
    start: u64,
    end: u64,
    first: Option<u64>,
    last: u64,
    killed: bool,
    roster: BTreeSet<String>,
}

impl Window {
    fn new(boss: String, attempt: usize, start: u64, end: u64) -> Self {
        Self {
            boss,
            attempt,
            start,
            end,
            first: None,
            last: 0,
            killed: false,
            roster: BTreeSet::new(),
        }
    }

    fn observe(&mut self, index: u64, time: u64, event: &Event) {
        self.first.get_or_insert(time);
        self.last = time;
        if index - self.start < 2000
            && self.roster.len() <= 10
            && player_guid(event.source)
            && !matches!(event.kind, "SPELL_AURA_REMOVED" | "SPELL_AURA_REFRESH")
        {
            self.roster.insert(event.source.into());
        }
        self.killed |= event.killing_blow() && guid_boss(event.target) == Some(&self.boss);
    }

    fn finish(self) -> Option<Encounter> {
        let first = self.first?;
        let duration = self.last.saturating_sub(first) as f64 / 1000.0;
        let minimum = CATALOG
            .minimum_seconds
            .get(&self.boss)
            .copied()
            .unwrap_or(15);
        let outcome = if self.killed && duration >= minimum as f64 {
            "kill"
        } else {
            "wipe"
        };
        Some(Encounter {
            boss_slug: slug(&self.boss),
            boss: self.boss,
            attempt: self.attempt,
            start: self.start,
            end: self.end,
            difficulty: if self.roster.len() > 10 { "25N" } else { "10N" }.into(),
            outcome: outcome.into(),
            duration_seconds: duration,
            start_second: first / 1000,
            end_second: self.last.div_ceil(1000),
        })
    }
}

pub fn list_ids(root: &Path) -> io::Result<Vec<String>> {
    let mut ids = BTreeSet::new();
    for entry in fs::read_dir(root.join("LogsDir"))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = entry.file_type()?;
        if kind.is_dir() && valid_id(&name) && entry.path().join("LOGS_CUT.zstd").is_file() {
            ids.insert(name);
        } else if kind.is_file()
            && let Some(id) = name.strip_suffix(".uwu")
            && valid_id(id)
        {
            ids.insert(id.to_owned());
        }
    }
    Ok(ids.into_iter().rev().collect())
}

pub fn valid_id(id: &str) -> bool {
    id.len() <= 240
        && id
            .as_bytes()
            .get(..2)
            .is_some_and(|prefix| prefix.iter().all(u8::is_ascii_digit))
        && !id
            .bytes()
            .any(|b| b.is_ascii_control() || matches!(b, b'/' | b'\\'))
        && id.split("--").count() >= 4
}

fn read_json<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    serde_json::from_reader(BufReader::new(File::open(path)?)).map_err(io::Error::other)
}

fn read_cache<T: DeserializeOwned + Default>(path: &Path) -> io::Result<T> {
    match File::open(path) {
        Ok(file) => match serde_json::from_reader(BufReader::new(file)) {
            Ok(value) => Ok(value),
            Err(error) if !error.is_io() => Ok(T::default()),
            Err(error) => Err(io::Error::from(error)),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(error),
    }
}

pub fn import_log(root: &Path, input: impl BufRead, year: u32, realm: &str) -> io::Result<String> {
    import_logs(root, [Ok(input)], year, realm)
}

pub fn import_logs(
    root: &Path,
    inputs: impl IntoIterator<Item = io::Result<impl BufRead>>,
    year: u32,
    realm: &str,
) -> io::Result<String> {
    if !(2000..2100).contains(&year) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "report year is out of range",
        ));
    }
    let directory = root.join("LogsDir");
    fs::create_dir_all(&directory)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let staged = directory.join(format!(".import-{}-{nonce}", std::process::id()));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)?;
    let result = (|| {
        let mut file = BufWriter::new(file);
        let mut writer = ReportWriter::new(&mut file)?;
        let mut raw = Vec::new();
        let mut normalized = String::new();
        let mut first = None;
        let mut author = None;
        let mut event_index = 0;
        let mut metadata = Metadata::default();
        let mut pulls = crate::encounters::Pulls::default();
        for input in inputs {
            let mut input = input?;
            while read_event(&mut input, &mut raw)? > 0 {
                let line = std::str::from_utf8(&raw).map_err(io::Error::other)?;
                if author.is_none()
                    && let Some((_, tail)) = line.split_once("  SPELL_CAST_FAILED,")
                {
                    let mut fields = tail.split(',');
                    if let (Some(guid), Some(name)) = (fields.next(), fields.next())
                        && player_guid(guid)
                    {
                        author = Some(name.trim_matches('"').to_owned());
                    }
                }
                normalize(line, &mut normalized);
                let Some(event) = Event::parse(&normalized) else {
                    continue;
                };
                let origin = *first.get_or_insert(event.stamp);
                let time = event.stamp.relative_to(origin, year).max(0) as u64;
                let second = time / 1000;
                if metadata
                    .time_index
                    .last()
                    .is_none_or(|(last, _)| second >= last + 5)
                {
                    metadata.time_index.push((second, event_index));
                }
                metadata.observe_units(&event);
                pulls.observe(event_index, time, &event);
                writer.push_event(normalized.as_bytes())?;
                event_index += 1;
            }
        }
        let first = first
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no valid combat events"))?;
        metadata.encounters = pulls.finish();
        writer.finish(serde_json::to_value(&metadata).map_err(io::Error::other)?)?;
        file.flush()?;
        file.get_ref().sync_all()?;
        let date = format!(
            "{:02}-{:02}-{:02}--{:02}-{:02}",
            year % 100,
            first.month,
            first.day,
            first.hour,
            first.minute
        );
        let author = id_component(author.as_deref().unwrap_or("Unknown"));
        let realm = id_component(realm);
        let id = format!("{date}--{author}--{realm}");
        let mut candidate = id.clone();
        let mut suffix = 1;
        loop {
            let target = directory.join(format!("{candidate}.uwu"));
            if !directory.join(&candidate).exists() {
                match fs::hard_link(&staged, target) {
                    Ok(()) => break,
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
            }
            suffix += 1;
            candidate = format!("{date}-{suffix}--{author}--{realm}");
        }
        Ok(candidate)
    })();
    let _ = fs::remove_file(staged);
    result
}

fn id_component(value: &str) -> String {
    let clean = value
        .trim()
        .chars()
        .take(25)
        .map(|ch| {
            if ch.is_alphanumeric() || ch == ' ' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if clean.is_empty() {
        "Unknown".into()
    } else {
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn one_import_with_multiple_days_publishes_one_report() {
        let root = std::env::temp_dir().join(format!(
            "uwu-logs-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let first = "11/17 21:01:14.020,SWING_DAMAGE,0x00000000000007DB,Bombas,0xF1300038E1000042,Boss,1,Melee,0x1,312,0,1,0,0,0,0,nil,nil\n";
        let second = "11/18 21:01:14.020,SWING_DAMAGE,0x00000000000007DB,Bombas,0xF1300038E1000042,Boss,1,Melee,0x1,418,0,1,0,0,0,0,nil,nil\n";
        let inputs = [Ok(Cursor::new(first)), Ok(Cursor::new(second))];
        let id = import_logs(&root, inputs, 2026, "test").unwrap();
        let ids = list_ids(&root).unwrap();
        let mut report = Report::open(&root, &id).unwrap();
        let mut output = Vec::new();
        report.copy_log(&mut output).unwrap();
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(ids, [id]);
        assert_eq!(
            String::from_utf8(output).unwrap(),
            format!("{first}{second}")
        );
    }
}

#[derive(Default)]
pub struct LegacyGroups {
    parts: BTreeMap<String, Vec<String>>,
    aliases: BTreeMap<String, String>,
}

impl LegacyGroups {
    pub fn load(root: &Path) -> io::Result<Self> {
        let mut folders = vec![root.join("uploads/0file_info")];
        let mut provenance = BTreeMap::<String, BTreeSet<PathBuf>>::new();
        while let Some(folder) = folders.pop() {
            let entries = match fs::read_dir(folder) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            for entry in entries {
                let entry = entry?;
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    folders.push(entry.path());
                } else if kind.is_file()
                    && entry
                        .path()
                        .extension()
                        .is_some_and(|suffix| suffix == "json")
                {
                    let document: serde_json::Value = match read_json(&entry.path()) {
                        Ok(document) => document,
                        Err(error) => {
                            eprintln!(
                                "Could not read provenance {}: {error}",
                                entry.path().display()
                            );
                            continue;
                        }
                    };
                    if let Some(slices) = document
                        .get("slices")
                        .and_then(serde_json::Value::as_object)
                    {
                        for id in slices.keys().filter(|id| valid_id(id)) {
                            provenance
                                .entry(id.clone())
                                .or_default()
                                .insert(entry.path());
                        }
                    }
                }
            }
        }
        let mut candidates = BTreeMap::<_, Vec<_>>::new();
        for (id, archives) in provenance {
            let Ok(report) = Report::open(root, &id) else {
                continue;
            };
            let Some((start, end)) = report.clock else {
                continue;
            };
            let mut raids = CATALOG.raids.iter().enumerate().filter(|(_, raid)| {
                report
                    .metadata
                    .encounters
                    .iter()
                    .any(|encounter| raid.bosses.contains(&encounter.boss))
            });
            let Some((raid, _)) = raids.next() else {
                continue;
            };
            if raids.next().is_some() || report.metadata.players.is_empty() {
                continue;
            }
            let fields = id.splitn(4, "--").collect::<Vec<_>>();
            let year = fields[0][..2].parse::<u32>().unwrap_or(0);
            let roster = report
                .metadata
                .players
                .into_values()
                .collect::<BTreeSet<_>>();
            let kills = report
                .metadata
                .encounters
                .into_iter()
                .filter(|segment| segment.outcome == "kill")
                .map(|segment| segment.boss)
                .collect::<BTreeSet<_>>();
            candidates
                .entry((
                    year,
                    fields[2].to_owned(),
                    fields[3].to_owned(),
                    raid,
                    roster,
                ))
                .or_default()
                .push((id, start, end, archives, kills));
        }
        let mut groups = BTreeMap::new();
        for ((year, _, _, _, _), parts) in candidates {
            let mut group = Vec::<String>::new();
            let mut last = None;
            let mut shared = BTreeSet::new();
            let mut kills = BTreeSet::new();
            for (id, start, end, archives, segment_kills) in parts {
                let common = shared
                    .intersection(&archives)
                    .cloned()
                    .collect::<BTreeSet<_>>();
                if common.is_empty()
                    || !kills.is_disjoint(&segment_kills)
                    || !last.is_some_and(|end| {
                        (1..=1_800_000).contains(&start.relative_to(end, year + 2000))
                    })
                {
                    if group.len() > 1 {
                        groups.insert(group[0].clone(), std::mem::take(&mut group));
                    }
                    group.clear();
                    shared = archives;
                    kills.clear();
                } else {
                    shared = common;
                }
                group.push(id);
                kills.extend(segment_kills);
                last = Some(end);
            }
            if group.len() > 1 {
                groups.insert(group[0].clone(), group);
            }
        }
        let aliases = match read_json(&root.join("_report_aliases.json")) {
            Ok(aliases) => aliases,
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error),
        };
        Ok(Self {
            parts: groups,
            aliases,
        })
    }

    pub fn is_fragment(&self, id: &str) -> bool {
        self.parts
            .values()
            .any(|parts| parts.iter().skip(1).any(|part| part == id))
    }
}
