use crate::ReportQuery;
use axum::http::Uri;
use minijinja::{Value, context};
use raidledger::appearance::{UNKNOWN_ICON, player_specs};
use raidledger::metrics::{Summary, Totals};
use raidledger::reports::{Encounter, Report};
use std::collections::BTreeMap;
use std::io;

pub fn class_names(report: &Report) -> BTreeMap<&str, &str> {
    report
        .metadata
        .players
        .iter()
        .filter_map(|(guid, name)| {
            Some((name.as_str(), report.metadata.classes.get(guid)?.as_str()))
        })
        .collect()
}

fn clock(seconds: f64) -> String {
    let millis = (seconds * 1000.).round() as u64;
    format!(
        "{:02}:{:02}.{:03}",
        millis / 60000,
        millis / 1000 % 60,
        millis % 1000
    )
}

fn attempt_type(fight: &Encounter) -> String {
    if fight.outcome == "kill" {
        "Kill".into()
    } else {
        format!("Wipe {}", fight.attempt + 1)
    }
}

fn attempt(fight: &Encounter) -> Value {
    let duration = clock(fight.duration_seconds);
    context! {
        href => fight.query(),
        css_class => format!("{}-link", fight.outcome),
        segment_str => format!("{duration} | {}", attempt_type(fight)),
        segment_label => format!("{duration} | {}", fight.difficulty), boss_name => &fight.boss
    }
}

fn formatted_name(id: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let fields: Vec<_> = id.splitn(4, "--").collect();
    let date: Vec<_> = fields[0].split('-').collect();
    if let ([year, month, day], [_, time, author, _]) = (date.as_slice(), fields.as_slice())
        && let Some(month) = month
            .parse::<usize>()
            .ok()
            .and_then(|month| month.checked_sub(1))
            .and_then(|month| MONTHS.get(month))
    {
        return format!(
            "{day} {month} {year}, {} - {author}",
            time.replace('-', ":")
        );
    }
    id.to_owned()
}

pub fn context(
    report: &Report,
    id: &str,
    selected: &[Encounter],
    query: &ReportQuery,
    uri: &Uri,
) -> Value {
    let mut groups = Vec::<(&Encounter, Vec<&Encounter>)>::new();
    for fight in &report.metadata.encounters {
        if let Some((_, attempts)) = groups
            .iter_mut()
            .find(|(first, _)| first.boss_slug == fight.boss_slug)
        {
            attempts.push(fight);
        } else {
            groups.push((fight, vec![fight]));
        }
    }
    let mut links = vec![
        context! { boss_name => "all", href => "?boss=all", text => "All boss segments",
        css_class => "boss-link", by_difficulty => BTreeMap::<String, Value>::new() },
    ];
    for (first, attempts) in groups {
        let mut modes = BTreeMap::<&str, Vec<Value>>::new();
        for fight in attempts {
            modes
                .entry(&fight.difficulty)
                .or_default()
                .push(attempt(fight));
        }
        let by_difficulty = modes
            .into_iter()
            .map(|(mode, segments)| {
                (
                    mode,
                    context! { href => format!("?boss={}&mode={mode}", first.boss_slug),
                    mode, css_class => "boss-link", segments },
                )
            })
            .collect::<BTreeMap<_, _>>();
        links.push(
            context! { boss_name => &first.boss, href => format!("?boss={}", first.boss_slug),
            css_class => "boss-link", by_difficulty },
        );
    }
    let kills = report
        .metadata
        .encounters
        .iter()
        .filter(|fight| fight.outcome == "kill")
        .map(attempt)
        .collect::<Vec<_>>();
    let details = selected
        .first()
        .filter(|fight| query.boss.as_deref() == Some(fight.boss_slug.as_str()));
    let (slice_name, slice_tries) = if let Some(fight) = details {
        let label = if selected.len() == 1 && query.attempt.is_some() {
            attempt_type(fight)
        } else {
            "All".into()
        };
        (
            fight.boss.as_str(),
            format!(
                "{} {label}",
                query.mode.as_deref().unwrap_or(&fight.difficulty)
            ),
        )
    } else if query.boss.as_deref() == Some("all")
        || (query.boss.is_none()
            && report.has_lockout()
            && selected
                .first()
                .is_none_or(|fight| fight.boss_slug != "custom"))
    {
        ("Bosses", "All".to_owned())
    } else {
        ("Custom Slice", String::new())
    };
    let query_string = uri.query().unwrap_or_default();
    let query_no_custom = query_string
        .split('&')
        .filter(|pair| !pair.starts_with("sc=") && !pair.starts_with("fc="))
        .collect::<Vec<_>>()
        .join("&");
    let encode_query = |value: &str| {
        if value.is_empty() {
            String::new()
        } else {
            format!("?{value}")
        }
    };
    let duration = selected
        .iter()
        .map(|fight| fight.duration_seconds)
        .sum::<f64>();
    let millis = (duration * 1000.).round() as u64;
    context! {
        request => context! { path => uri.path() },
        REPORT_ID => id, REPORT_NAME => formatted_name(id),
        QUERY => encode_query(query_string), QUERY_NO_CUSTOM => encode_query(&query_no_custom),
        SLICE_NAME => slice_name, SLICE_TRIES => slice_tries,
        SEGMENTS_LINKS => links, SEGMENTS_KILLS => kills, PLAYER_CLASSES => class_names(report),
        DURATION => duration, DURATION_STR => format!("{}:{}", millis / 3_600_000, clock(duration % 3600.)),
        SERVER => id.rsplit_once("--").map(|(_, server)| server).unwrap_or_default(),
    }
}

pub use raidledger::metrics::formatted_amount as number;

fn metric(value: i64, duration: f64, maximum: i64) -> Value {
    let tenths = (10. * value as f64 / if duration > 0. { duration } else { 1. }) as i64;
    let per_second = match tenths {
        0 => String::new(),
        -9..=-1 => format!("-0.{}", tenths.unsigned_abs()),
        1..=9 => format!("0.{tenths}"),
        _ => format!("{}.{}", number(tenths / 10), tenths.unsigned_abs() % 10),
    };
    context! {
        value => number(value), per_second,
        percent => value.saturating_mul(100).checked_div(maximum).unwrap_or_default(),
    }
}

pub fn overview(
    report: &mut Report,
    selected: &[Encounter],
    use_useful: bool,
    summary: Summary,
) -> io::Result<Value> {
    type Column = (&'static str, fn(&Totals) -> i64);
    let columns: [Column; 5] = [
        ("useful", |total| total.useful_damage),
        ("damage", |total| total.damage),
        ("heal", |total| total.healing),
        ("heal_total", |total| total.total_healing),
        ("taken", |total| total.damage_taken),
    ];
    let events = if use_useful {
        selected
            .first()
            .map_or(0..0, |fight| fight.start..fight.end)
    } else {
        0..selected.last().map_or(0, |fight| fight.end)
    };
    let mut specs = player_specs(report, events)?;
    let duration = summary.duration_seconds;
    let mut players = summary.players.into_iter().collect::<Vec<_>>();
    players.sort_by_key(|(_, player)| std::cmp::Reverse(player.rates.totals.damage));
    let mut names = vec![("Total".to_owned(), ("Total", "ability_hunter_readiness"))];
    names.extend(players.iter().map(|(guid, player)| {
        (
            player.name.clone(),
            specs.remove(guid).unwrap_or(("Unknown", UNKNOWN_ICON)),
        )
    }));
    let mut data = BTreeMap::new();
    for (key, amount) in columns {
        let mut values = BTreeMap::new();
        if key != "useful" || use_useful {
            let maximum = players
                .iter()
                .map(|(_, player)| amount(&player.rates.totals))
                .max()
                .unwrap_or_default();
            values.insert(
                "Total".to_owned(),
                metric(amount(&summary.totals.totals), duration, 0),
            );
            for (_, player) in &players {
                values.insert(
                    player.name.clone(),
                    metric(amount(&player.rates.totals), duration, maximum),
                );
            }
        }
        data.insert(key, values);
    }
    Ok(context! { SPECS => names, DATA => data })
}

pub fn configure_templates(templates: &mut minijinja::Environment<'_>) {
    use minijinja::value::ValueKind;
    templates.set_unknown_method_callback(|state, value, method, args| {
        if value.kind() == ValueKind::Map {
            match (method, args) {
                ("items", []) => return state.apply_filter("items", std::slice::from_ref(value)),
                ("values", []) => {
                    return value.try_iter()?.map(|key| value.get_item(&key)).collect();
                }
                ("get", [key] | [key, _]) => {
                    let result = value.get_item(key)?;
                    return Ok(if result.is_undefined() {
                        args.get(1).cloned().unwrap_or(Value::UNDEFINED)
                    } else {
                        result
                    });
                }
                _ => {}
            }
        }
        Err(minijinja::Error::from(minijinja::ErrorKind::UnknownMethod))
    });
}
