use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

pub const NIL_GUID: &str = "0x0000000000000000";

#[derive(Deserialize)]
pub struct Catalog {
    pub bosses: BTreeMap<String, String>,
    pub minimum_seconds: BTreeMap<String, u64>,
    pub raw_bosses: BTreeSet<String>,
    pub group_leaders: BTreeMap<String, String>,
    pub cowards: BTreeSet<String>,
    pub idle_seconds: BTreeMap<String, u64>,
    pub max_tail: BTreeMap<String, usize>,
    pub flags: BTreeSet<String>,
    pub ignored_spell_ids: BTreeSet<String>,
    pub some_boss_spells: BTreeSet<String>,
    pub can_be_short: BTreeSet<String>,
    pub raids: Vec<Raid>,
}

#[derive(Deserialize)]
pub struct Raid {
    pub name: String,
    pub total: usize,
    pub bosses: BTreeSet<String>,
}

pub static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../config/encounters.json")).expect("bundled encounters")
});

pub fn player_guid(guid: &str) -> bool {
    guid.len() == 18 && guid.starts_with("0x0") && guid != NIL_GUID
}

pub fn guid_boss(guid: &str) -> Option<&'static str> {
    CATALOG.bosses.get(guid.get(6..12)?).map(String::as_str)
}

pub fn slug(name: &str) -> String {
    name.to_lowercase().replace(' ', "-").replace('\'', "")
}

#[derive(Clone, Copy)]
pub struct Stamp {
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millis: u32,
}

pub fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl Stamp {
    pub fn parse(value: &str) -> Option<Self> {
        let (date, time) = value.trim().split_once(' ')?;
        let (month, day) = date.split_once('/')?;
        let mut clock = time.trim().split(':');
        let (second, fraction) = clock.nth(2)?.split_once('.')?;
        let mut fraction_value = 0;
        for (index, byte) in fraction.bytes().take(3).enumerate() {
            if !byte.is_ascii_digit() {
                return None;
            }
            fraction_value += (u32::from(byte - b'0')) * [100, 10, 1][index];
        }
        let (hour, remainder) = time.trim().split_once(':')?;
        let (minute, _) = remainder.split_once(':')?;
        let stamp = Self {
            month: month.parse().ok()?,
            day: day.parse().ok()?,
            hour: hour.parse().ok()?,
            minute: minute.parse().ok()?,
            second: second.parse().ok()?,
            millis: fraction_value,
        };
        (stamp.month >= 1
            && stamp.month <= 12
            && stamp.day >= 1
            && stamp.day <= days_in_month(2000, stamp.month)
            && stamp.hour < 24
            && stamp.minute < 60
            && stamp.second < 60)
            .then_some(stamp)
    }

    pub fn millis_of_year(self, year: u32) -> u64 {
        let days = (1..self.month)
            .map(|month| days_in_month(year, month))
            .sum::<u32>()
            + self.day
            - 1;
        u64::from((((days * 24 + self.hour) * 60 + self.minute) * 60) + self.second) * 1000
            + u64::from(self.millis)
    }

    pub fn relative_to(self, first: Self, year: u32) -> i64 {
        let origin = first.millis_of_year(year) as i64;
        if first.month > self.month + 6 || self.month > first.month + 6 {
            let end = Self {
                month: 12,
                day: 31,
                hour: 23,
                minute: 59,
                second: 59,
                millis: 999,
            };
            if first.month > self.month + 6 {
                return (end.millis_of_year(year) + 1 + self.millis_of_year(year + 1)) as i64
                    - origin;
            }
            let prior = year.saturating_sub(1);
            return self.millis_of_year(prior) as i64
                - (end.millis_of_year(prior) + 1) as i64
                - origin;
        }
        self.millis_of_year(year) as i64 - origin
    }
}

pub struct Event<'a> {
    pub stamp: Stamp,
    pub kind: &'a str,
    pub source: &'a str,
    pub source_name: &'a str,
    pub target: &'a str,
    pub target_name: &'a str,
    pub spell: &'a str,
    pub spell_name: &'a str,
    pub payload: &'a str,
    pub amount: i64,
    pub excess: i64,
}

impl<'a> Event<'a> {
    pub fn parse(line: &'a str) -> Option<Self> {
        let mut fields = line.trim_end().splitn(9, ',');
        let stamp = Stamp::parse(fields.next()?)?;
        let kind = fields.next()?;
        let source = fields.next()?;
        let source_name = fields.next()?;
        let target = fields.next()?;
        let target_name = fields.next()?;
        if kind.is_empty()
            || !kind
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
            || !guid(source)
            || !guid(target)
        {
            return None;
        }
        let spell = fields.next().unwrap_or_default();
        let spell_name = fields.next().unwrap_or_default();
        let payload = fields.next().unwrap_or_default();
        let mut rest = payload.split(',');
        rest.next();
        let amount = rest
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let excess = rest
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        Some(Self {
            stamp,
            kind,
            source,
            source_name,
            target,
            target_name,
            spell,
            spell_name,
            payload,
            amount,
            excess,
        })
    }

    pub fn damage(&self) -> bool {
        self.kind.ends_with("_DAMAGE")
    }

    pub fn healing(&self) -> bool {
        self.kind.ends_with("_HEAL")
    }

    pub fn killing_blow(&self) -> bool {
        matches!(self.kind, "UNIT_DIED" | "PARTY_KILL")
            || self.damage() && self.excess > 0 && self.amount.saturating_sub(self.excess) != 1
    }
}

fn guid(value: &str) -> bool {
    value.strip_prefix("0x").is_some_and(|digits| {
        digits.len() == 16 && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

pub fn normalize(line: &str, output: &mut String) {
    output.clear();
    let line = line.trim_matches(['\n', '\r', '\0', '\u{feff}']);
    let Some(first_column_end) = line.find(',') else {
        return;
    };
    let first_column = &line[..first_column_end];
    let Some((stamp, kind)) = first_column.rsplit_once("  ") else {
        if Event::parse(line).is_some() && !line.contains(",SPELL_CAST_FAILED,") {
            output.push_str(line);
            output.push('\n');
        }
        return;
    };
    if matches!(kind, "SPELL_CAST_FAILED" | "SPELL_DURABILITY_DAMAGE")
        || Stamp::parse(stamp).is_none()
    {
        return;
    }
    let rest = &line[first_column_end + 1..];
    let replaced;
    let rest = if rest.contains(", ") {
        replaced = rest.replace(", ", " ");
        replaced.as_str()
    } else {
        rest
    };
    let fields = rest.split(',').collect::<Vec<_>>();
    let [source, source_name, _, target, target_name, _, rest @ ..] = fields.as_slice() else {
        return;
    };
    if !guid(source) || !guid(target) {
        return;
    }
    use std::fmt::Write as _;
    let _ = write!(
        output,
        "{},{},{},{},{},{}",
        stamp.trim(),
        kind,
        source,
        source_name.trim_matches('"'),
        target,
        target_name.trim_matches('"')
    );
    let mut rest = rest.iter().map(|field| field.trim_matches('"'));
    match kind {
        "SWING_DAMAGE" | "SWING_MISSED" => output.push_str(",1,Melee,0x1"),
        "ENCHANT_APPLIED" | "ENCHANT_REMOVED" => {
            if let (Some(name), Some(item)) = (rest.next(), rest.next()) {
                let _ = write!(output, ",{item},{name}");
            }
        }
        "ENVIRONMENTAL_DAMAGE" => {
            if let (Some(kind), Some(amount), Some(excess), Some(school)) =
                (rest.next(), rest.next(), rest.next(), rest.next())
            {
                let id = match kind {
                    "FALLING" => 90001,
                    "LAVA" => 90002,
                    "DROWNING" => 90003,
                    "FIRE" => 90004,
                    "FATIGUE" => 90005,
                    "SLIME" => 90006,
                    _ => 90007,
                };
                let school_value = school.parse::<u32>().unwrap_or(1);
                let _ = write!(
                    output,
                    ",{id},{kind},{school_value:#x},{amount},{excess},{school}"
                );
            }
        }
        _ => {}
    }
    for field in rest {
        output.push(',');
        output.push_str(field);
    }
    output.push('\n');
}
