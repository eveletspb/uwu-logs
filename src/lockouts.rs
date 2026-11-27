use crate::combat::{CATALOG, Stamp};
use crate::reports::Metadata;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufRead, Read};
#[cfg(not(windows))]
use std::process::Command;

#[derive(Default, Deserialize)]
pub struct Manifest {
    version: u32,
    records: Vec<Record>,
}

#[derive(Deserialize)]
struct Record {
    time: u64,
    session: u64,
    reset: u64,
    server: String,
    player: String,
    instance: String,
    id: String,
    high: String,
    difficulty: u32,
    guild: Option<String>,
    faction: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    server: String,
    player: String,
    raid: usize,
    id: String,
    high: String,
    difficulty: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Lockout {
    key: Key,
    reset: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub faction: Option<u8>,
}

impl Lockout {
    pub fn matches(&self, other: &Self) -> bool {
        self.key == other.key && self.reset.abs_diff(other.reset) <= 600
    }
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        let manifest: Self = serde_json::from_slice(bytes).map_err(io::Error::other)?;
        if manifest.version != 1 || manifest.records.len() > 2000 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported raid lockout manifest",
            ));
        }
        Ok(manifest)
    }

    pub fn author(&self, metadata: &Metadata, realm: &str) -> Option<String> {
        let authors = self
            .records
            .iter()
            .filter(|record| {
                record.server.eq_ignore_ascii_case(realm)
                    && metadata
                        .players
                        .values()
                        .any(|player| player.eq_ignore_ascii_case(&record.player))
            })
            .map(|record| record.player.as_str())
            .collect::<BTreeSet<_>>();
        (authors.len() == 1).then(|| authors.into_iter().next().unwrap().to_owned())
    }

    pub fn identify(
        &self,
        metadata: &Metadata,
        player: &str,
        realm: &str,
        year: u32,
        stamps: (Stamp, Stamp),
        timezone: &str,
    ) -> Option<Lockout> {
        let mut raids = CATALOG.raids.iter().enumerate().filter(|(_, raid)| {
            metadata
                .encounters
                .iter()
                .any(|encounter| raid.bosses.contains(&encounter.boss))
        });
        let (raid, _) = raids.next()?;
        if raids.next().is_some() {
            return None;
        }
        let started = epoch(stamps.0, year, timezone)?;
        let end_year = year + u32::from(stamps.0.month > stamps.1.month + 6);
        let ended = epoch(stamps.1, end_year, timezone)?;
        let mut candidates = self
            .records
            .iter()
            .filter(|row| {
                row.server.eq_ignore_ascii_case(realm)
                    && row.player.eq_ignore_ascii_case(player)
                    && CATALOG.aliases.get(&row.instance) == Some(&raid)
                    && !row.id.is_empty()
                    && [&row.server, &row.player, &row.instance, &row.id, &row.high]
                        .iter()
                        .all(|field| field.len() <= 150)
                    && row.session <= started.saturating_add(120)
                    && (started.saturating_sub(43200)..=ended.saturating_add(300))
                        .contains(&row.time)
                    && row.reset > row.time
                    && row.reset > started
            })
            .collect::<Vec<_>>();
        let session = candidates.iter().map(|row| row.session).max()?;
        candidates.retain(|row| row.session == session);
        let keys = candidates
            .iter()
            .map(|row| Key {
                server: row.server.to_lowercase(),
                player: row.player.to_lowercase(),
                raid,
                id: row.id.clone(),
                high: row.high.clone(),
                difficulty: row.difficulty,
            })
            .collect::<BTreeSet<_>>();
        if keys.len() != 1 {
            return None;
        }
        let min_reset = candidates.iter().map(|row| row.reset).min()?;
        let max_reset = candidates.iter().map(|row| row.reset).max()?;
        if max_reset - min_reset > 600 {
            return None;
        }
        let latest = candidates.into_iter().max_by_key(|row| row.time)?;
        let guild = latest
            .guild
            .as_ref()
            .filter(|value| {
                !value.is_empty() && value.len() <= 120 && !value.chars().any(char::is_control)
            })
            .cloned();
        Some(Lockout {
            key: keys.into_iter().next()?,
            reset: latest.reset,
            guild,
            faction: match latest.faction.as_deref() {
                Some("Alliance") => Some(0),
                Some("Horde") => Some(1),
                _ => None,
            },
        })
    }

    pub fn identify_partition(
        &self,
        metadata: &Metadata,
        realm: &str,
        year: u32,
        stamps: (Stamp, Stamp),
        timezone: &str,
    ) -> Option<Lockout> {
        let players = self
            .records
            .iter()
            .filter(|row| row.server.eq_ignore_ascii_case(realm))
            .map(|row| row.player.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        let mut matches = players
            .iter()
            .filter(|name| {
                metadata
                    .players
                    .values()
                    .any(|player| player.eq_ignore_ascii_case(name))
            })
            .filter_map(|name| self.identify(metadata, name, realm, year, stamps, timezone));
        let lockout = matches.next()?;
        matches.next().is_none().then_some(lockout)
    }
}

pub fn epoch(stamp: Stamp, year: u32, timezone: &str) -> Option<u64> {
    if timezone == "UTC" || timezone.is_empty() {
        let leap = |year: u32| {
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
        };
        let days = (1970..year)
            .map(|year| 365 + u64::from(leap(year)))
            .sum::<u64>();
        return Some(days * 86_400 + stamp.millis_of_year(year) / 1000);
    }
    if timezone.len() > 100
        || timezone.starts_with('/')
        || timezone.split('/').any(|part| part == "..")
        || !timezone
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/_-+".contains(&byte))
    {
        return None;
    }
    named_epoch(stamp, year, timezone)
}

#[cfg(not(windows))]
fn named_epoch(stamp: Stamp, year: u32, timezone: &str) -> Option<u64> {
    if !std::path::Path::new("/usr/share/zoneinfo")
        .join(timezone)
        .is_file()
    {
        return None;
    }
    let local = format!(
        "{year}-{:02}-{:02} {:02}:{:02}:{:02}",
        stamp.month, stamp.day, stamp.hour, stamp.minute, stamp.second
    );
    let mut date = Command::new("date");
    date.env("TZ", timezone).env("LC_ALL", "C");
    if cfg!(target_os = "macos") {
        date.args(["-j", "-f", "%Y-%m-%d %H:%M:%S", &local, "+%s"]);
    } else {
        date.args(["-d", &local, "+%s"]);
    }
    let output = date.output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(windows)]
fn named_epoch(stamp: Stamp, year: u32, timezone: &str) -> Option<u64> {
    use std::ffi::{c_char, c_void};
    use std::ptr::{null, null_mut};

    #[link(name = "Ole32")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> i32;
        fn CoUninitialize();
    }

    #[link(name = "icu")]
    unsafe extern "C" {
        fn ucal_getCanonicalTimeZoneID(
            id: *const u16,
            len: i32,
            result: *mut u16,
            capacity: i32,
            system: *mut i8,
            error: *mut i32,
        ) -> i32;
        fn ucal_open(
            id: *const u16,
            len: i32,
            locale: *const c_char,
            calendar_type: i32,
            error: *mut i32,
        ) -> *mut c_void;
        fn ucal_setDateTime(
            calendar: *mut c_void,
            year: i32,
            month: i32,
            day: i32,
            hour: i32,
            minute: i32,
            second: i32,
            error: *mut i32,
        );
        fn ucal_getMillis(calendar: *const c_void, error: *mut i32) -> f64;
        fn ucal_close(calendar: *mut c_void);
    }

    struct Com(bool);
    impl Drop for Com {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }
    struct Calendar(*mut c_void);
    impl Drop for Calendar {
        fn drop(&mut self) {
            unsafe { ucal_close(self.0) };
        }
    }

    let initialized = unsafe { CoInitializeEx(null_mut(), 0) };
    if initialized < 0 && initialized as u32 != 0x80010106 {
        return None;
    }
    let _com = Com(initialized >= 0);
    let id = timezone.encode_utf16().collect::<Vec<_>>();
    let mut canonical = [0u16; 128];
    let mut system = 0i8;
    let mut error = 0;
    let len = unsafe {
        ucal_getCanonicalTimeZoneID(
            id.as_ptr(),
            id.len() as i32,
            canonical.as_mut_ptr(),
            canonical.len() as i32,
            &mut system,
            &mut error,
        )
    };
    if error > 0 || system == 0 || len <= 0 || len as usize >= canonical.len() {
        return None;
    }
    // ICU takes a zero-based month in a Gregorian calendar.
    let calendar = unsafe { ucal_open(canonical.as_ptr(), len, null(), 1, &mut error) };
    if calendar.is_null() {
        return None;
    }
    let calendar = Calendar(calendar);
    if error > 0 {
        return None;
    }
    unsafe {
        ucal_setDateTime(
            calendar.0,
            year as i32,
            stamp.month as i32 - 1,
            stamp.day as i32,
            stamp.hour as i32,
            stamp.minute as i32,
            stamp.second as i32,
            &mut error,
        );
    }
    let millis = unsafe { ucal_getMillis(calendar.0, &mut error) };
    (error <= 0 && millis.is_finite() && millis >= 0.0).then(|| (millis / 1000.0) as u64)
}

type TimeKey = (u32, u64);

struct Entry {
    time: TimeKey,
    bytes: Vec<u8>,
}

pub struct Merge<R> {
    inputs: [R; 2],
    years: [u32; 2],
    heads: [Option<Entry>; 2],
    first_month: [Option<u32>; 2],
    previous: Option<TimeKey>,
    occurrences: [BTreeMap<Vec<u8>, u32>; 2],
    buffered: std::io::Cursor<Vec<u8>>,
}

impl<R: BufRead> Merge<R> {
    pub fn new(inputs: [R; 2], years: [u32; 2]) -> io::Result<Self> {
        let mut merge = Self {
            inputs,
            years,
            heads: [None, None],
            first_month: [None, None],
            previous: None,
            occurrences: std::array::from_fn(|_| BTreeMap::new()),
            buffered: std::io::Cursor::new(Vec::new()),
        };
        merge.advance(0)?;
        merge.advance(1)?;
        Ok(merge)
    }

    fn advance(&mut self, side: usize) -> io::Result<()> {
        let mut bytes = Vec::new();
        self.heads[side] = if crate::storage::read_event(&mut self.inputs[side], &mut bytes)? == 0 {
            None
        } else {
            let text = std::str::from_utf8(&bytes).map_err(io::Error::other)?;
            let stamp = text
                .split_once(',')
                .and_then(|(stamp, _)| Stamp::parse(stamp))
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid event timestamp during merge",
                    )
                })?;
            let first = *self.first_month[side].get_or_insert(stamp.month);
            let year = self.years[side] + u32::from(first > stamp.month + 6);
            Some(Entry {
                time: (year, stamp.millis_of_year(year)),
                bytes,
            })
        };
        Ok(())
    }

    fn next_event(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            let side = match (&self.heads[0], &self.heads[1]) {
                (None, None) => return Ok(None),
                (Some(left), Some(right)) => usize::from(left.time > right.time),
                (None, Some(_)) => 1,
                (Some(_), None) => 0,
            };
            let event = self.heads[side].take().unwrap();
            if self.previous.is_some_and(|previous| event.time < previous) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "archive events are out of order",
                ));
            }
            if self.previous != Some(event.time) {
                self.occurrences.iter_mut().for_each(BTreeMap::clear);
                self.previous = Some(event.time);
            }
            self.advance(side)?;
            let left_count = self.occurrences[0].get(&event.bytes).copied().unwrap_or(0);
            let count = self.occurrences[side]
                .entry(event.bytes.clone())
                .or_default();
            *count += 1;
            if side == 0 || *count > left_count {
                return Ok(Some(event.bytes));
            }
        }
    }
}

impl<R: BufRead> Read for Merge<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.buffered.position() as usize == self.buffered.get_ref().len() {
            let Some(event) = self.next_event()? else {
                return Ok(0);
            };
            self.buffered = std::io::Cursor::new(event);
        }
        self.buffered.read(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn overlap_retains_highest_same_tick_multiplicity() {
        let same = "11/19 18:50:00.001,SPELL_DAMAGE,one\n";
        let second = "11/19 18:50:02.001,SPELL_DAMAGE,two\n";
        let last = "11/19 18:50:03.001,SPELL_DAMAGE,three\n";
        let left = format!("{same}{same}{last}");
        let right = format!("{same}{same}{same}{second}");
        let mut merge = Merge::new([Cursor::new(left), Cursor::new(right)], [2026, 2026]).unwrap();
        let mut result = String::new();
        merge.read_to_string(&mut result).unwrap();
        assert_eq!(result, format!("{same}{same}{same}{second}{last}"));
    }

    #[test]
    fn successive_archives_for_one_lockout_keep_the_original_report_id() {
        use crate::reports::{Report, import_with_manifest, list_ids};
        use serde_json::json;
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};

        let root = std::env::temp_dir().join(format!(
            "uwu-lockout-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let stamp = Stamp::parse("11/19 18:30:00.000").unwrap();
        let start = epoch(stamp, 2026, "UTC").unwrap();
        let manifest = Manifest::parse(
            &serde_json::to_vec(&json!({
               "version": 1,
               "records": [{
                   "time": start, "session": start, "reset": start + 3600,
                   "server": "stable", "player": "Bombas", "instance": "Temple of Ahn'Qiraj",
                   "id": "77", "high": "0", "difficulty": 25,
               }]
            }))
            .unwrap(),
        )
        .unwrap();
        let logs = |rows: std::ops::Range<u32>| {
            rows.map(|second| {
            format!("11/19 18:{:02}:{:02}.000,SWING_DAMAGE,0x00000000000007DB,Bombas,0xF130003C9C000042,Sartura,1,Melee,0x1,312,0,1,0,0,0,0,nil,nil\n",
                30 + second / 60, second % 60)
        }).collect::<String>()
        };
        let import = |log| {
            import_with_manifest(
                &root,
                [Ok(Cursor::new(log))],
                2026,
                "stable",
                Some((&manifest, "UTC")),
            )
            .unwrap()
        };
        let first = import(logs(0..120));
        let second = import(logs(60..180));
        let mut output = Vec::new();
        Report::open(&root, &first)
            .unwrap()
            .copy_log(&mut output)
            .unwrap();
        let found = list_ids(&root).unwrap();
        let backups = fs::read_dir(root.join("LogsDir/.raid-merge-backups"))
            .unwrap()
            .count();
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(first, second);
        assert_eq!(found, [first]);
        assert_eq!(output, logs(0..180).as_bytes());
        assert_eq!(backups, 1);
    }
}
