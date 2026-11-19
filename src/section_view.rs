use crate::report_view::number;
use minijinja::{Value, context};
use raidledger::appearance::{self, UNKNOWN_ICON};
use raidledger::combat::{Event, Stamp, guid_boss, player_guid};
use raidledger::reports::{Encounter, Report};
use std::collections::{BTreeMap, BTreeSet};
use std::io;

pub fn entities(report: &mut Report, selected: &[Encounter]) -> io::Result<Value> {
    let mut guids = BTreeSet::new();
    for fight in selected {
        report.visit(Some(fight.start..fight.end), |_, line| {
            if let Some(event) = Event::parse(line).filter(Event::damage) {
                guids.extend([event.source.to_owned(), event.target.to_owned()]);
            }
        })?;
    }
    let labels = [
        "BOSSES",
        "PLAYERS' PERMANENT PETS",
        "PLAYERS' TEMPORARY PETS",
        "PLAYERS",
        "OTHER",
    ];
    let mut categories = labels.map(|label| (label, Vec::new()));
    for guid in guids {
        let Some(unit) = report.metadata.units.get(&guid) else {
            continue;
        };
        let owner = unit
            .master_guid
            .as_deref()
            .and_then(|guid| report.metadata.units.get(guid));
        let index = if player_guid(&guid) {
            3
        } else if guid.starts_with("0xF14") {
            1
        } else if guid_boss(&guid).is_some() {
            0
        } else if unit.master_guid.as_deref().is_some_and(player_guid) {
            2
        } else {
            4
        };
        let name = owner.map_or_else(
            || unit.name.clone(),
            |owner| format!("{} ({})", unit.name, owner.name),
        );
        categories[index].1.push((name, guid));
    }
    Ok(context! { entity_groups => categories })
}

type Table = BTreeMap<String, BTreeMap<String, i64>>;

pub fn damage(report: &mut Report, selected: &[Encounter]) -> io::Result<Value> {
    let roster = report.metadata.players.clone();
    let units = report.metadata.units.clone();
    let mut amounts = Table::new();
    let mut order = vec!["Useful".to_owned(), "Total".to_owned()];
    for fight in selected {
        report.visit(Some(fight.start..fight.end), |_, line| {
            let Some(event) = Event::parse(line).filter(Event::damage) else {
                return;
            };
            let source = units
                .get(event.source)
                .and_then(|unit| unit.master_guid.as_deref())
                .unwrap_or(event.source);
            if !roster.contains_key(source) {
                return;
            }
            let target = if player_guid(event.target) {
                "Friendly Fire"
            } else {
                event.target_name
            };
            if !amounts.contains_key(target) {
                order.push(target.to_owned());
            }
            let add = |table: &mut Table, target: &str, amount| {
                *table
                    .entry(target.to_owned())
                    .or_default()
                    .entry(source.to_owned())
                    .or_default() += amount;
            };
            add(&mut amounts, target, event.amount);
            if target != "Friendly Fire" {
                add(&mut amounts, "Total", event.amount);
            }
            if appearance::useful_target(event.target) {
                add(
                    &mut amounts,
                    "Useful",
                    event.amount.saturating_sub(event.excess),
                );
            }
        })?;
    }
    order.retain(|column| amounts.contains_key(column));
    let first_range = selected
        .first()
        .map_or(0..0, |fight| fight.start..fight.end);
    let specs = appearance::player_specs(report, first_range)?;
    let mut players = roster
        .into_iter()
        .filter(|(guid, _)| amounts.values().any(|values| values.contains_key(guid)))
        .collect::<Vec<_>>();
    players.sort_by_key(|(guid, _)| {
        std::cmp::Reverse(
            amounts
                .get("Useful")
                .and_then(|data| data.get(guid))
                .or_else(|| amounts.get("Total").and_then(|data| data.get(guid)))
                .copied()
                .unwrap_or_default(),
        )
    });
    let mut portraits = BTreeMap::from([(
        "Total".to_owned(),
        context! { spec => "Total", icon => "ability_hunter_readiness" },
    )]);
    for (guid, name) in &players {
        let (spec, icon) = specs
            .get(guid)
            .copied()
            .unwrap_or(("Unknown", UNKNOWN_ICON));
        portraits.insert(
            guid.clone(),
            context! { spec, icon, name, class => report.metadata.classes.get(guid) },
        );
    }
    let rows = std::iter::once("Total")
        .chain(players.iter().map(|(guid, _)| guid.as_str()))
        .collect::<Vec<_>>();
    let values = amounts
        .into_iter()
        .map(|(target, damage)| {
            let total = damage.values().sum::<i64>();
            let mut display = damage
                .into_iter()
                .map(|(guid, amount)| (guid, number(amount)))
                .collect::<BTreeMap<_, _>>();
            display.insert("Total".to_owned(), number(total));
            (target, display)
        })
        .collect::<BTreeMap<_, _>>();
    Ok(
        context! { TARGETS => values, target_columns => order, PLAYERS_SORTED => rows, SPECS => portraits },
    )
}

pub fn consumables(report: &mut Report, selected: &[Encounter]) -> io::Result<Value> {
    let roster = report.metadata.players.clone();
    let mut items = Table::new();
    let mut totals = roster
        .values()
        .map(|player| (player.clone(), 0))
        .collect::<BTreeMap<_, _>>();
    let info = appearance::item_columns()
        .iter()
        .filter_map(|id| appearance::item(id).map(|item| (id, item)))
        .collect::<BTreeMap<_, _>>();
    for fight in selected {
        report.visit(Some(fight.start..fight.end), |_, line| {
            let Some(event) = Event::parse(line) else {
                return;
            };
            let guid = match (event.kind, event.spell) {
                ("SPELL_CAST_SUCCESS", id) if id != "28714" => event.source,
                ("SPELL_AURA_APPLIED", "28714") => event.target,
                _ => return,
            };
            let id = appearance::item_alias(event.spell);
            if let Some(player) = roster.get(guid)
                && appearance::item(id).is_some()
            {
                *items
                    .entry(id.to_owned())
                    .or_default()
                    .entry(player.clone())
                    .or_default() += 1;
                if appearance::count_item(id) {
                    *totals.entry(player.clone()).or_default() += 1;
                }
            }
        })?;
    }
    Ok(
        context! { ITEM_INFO => info, ITEM_COLUMNS => appearance::item_columns(), ITEMS => items, ITEMS_TOTAL => totals },
    )
}

const POWERS: [(&str, &str); 7] = [
    ("0", "mana"),
    ("1", "rage"),
    ("3", "energy"),
    ("6", "runic"),
    ("2", "focus"),
    ("4", "happiness"),
    ("5", "runes"),
];

pub fn powers(report: &mut Report, selected: &[Encounter]) -> io::Result<Value> {
    let mut amounts = BTreeMap::<&str, Table>::new();
    let mut names = BTreeMap::new();
    for fight in selected {
        report.visit(Some(fight.start..fight.end), |_, line| {
            let Some(event) = Event::parse(line).filter(|event| event.kind.ends_with("_ENERGIZE"))
            else {
                return;
            };
            let kind = event.payload.split(',').nth(2).unwrap_or_default();
            if let Some((_, power)) = POWERS.iter().find(|(id, _)| *id == kind) {
                names
                    .entry(event.spell.to_owned())
                    .or_insert_with(|| event.spell_name.to_owned());
                *amounts
                    .entry(power)
                    .or_default()
                    .entry(event.target_name.to_owned())
                    .or_default()
                    .entry(event.spell.to_owned())
                    .or_default() += event.amount;
            }
        })?;
    }
    let mut totals = BTreeMap::new();
    let mut powers = BTreeMap::new();
    let mut info = BTreeMap::new();
    let mut columns = BTreeMap::new();
    for (power, players) in amounts {
        let mut spell_totals = BTreeMap::new();
        let mut display = BTreeMap::new();
        let mut total = BTreeMap::new();
        for (name, spells) in players {
            total.insert(name.clone(), number(spells.values().sum()));
            display.insert(
                name,
                spells
                    .into_iter()
                    .map(|(id, value)| {
                        *spell_totals.entry(id.clone()).or_default() += value;
                        (id, number(value))
                    })
                    .collect::<BTreeMap<_, _>>(),
            );
        }
        let mut order = spell_totals.iter().collect::<Vec<_>>();
        order.sort_by_key(|(_, value)| std::cmp::Reverse(**value));
        columns.insert(
            power,
            order
                .into_iter()
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>(),
        );
        let spell_info = spell_totals.into_iter().map(|(id, value)| {
            let data = context! { name => names.get(&id), icon => appearance::spell_icon(&id), value => number(value) };
            (id, data)
        }).collect::<BTreeMap<_, _>>();
        totals.insert(power, total);
        powers.insert(power, display);
        info.insert(power, spell_info);
    }
    let labels = POWERS
        .iter()
        .enumerate()
        .filter_map(|(index, (_, name))| powers.contains_key(name).then_some((index, *name)))
        .collect::<Vec<_>>();
    Ok(
        context! { LABELS => labels, SPELLS => info, POWER_COLUMNS => columns, TOTAL => totals, POWERS => powers },
    )
}

#[derive(Default)]
struct AuraState {
    start: Option<i64>,
    time: i64,
    count: u32,
    minimum: i64,
}

impl AuraState {
    fn close(&mut self, at: i64) {
        if let Some(start) = self.start.take().filter(|start| at - start >= self.minimum) {
            self.time += at - start;
        }
    }
}

pub fn auras(report: &mut Report, selected: &[Encounter]) -> io::Result<Value> {
    let roster = report.metadata.players.clone();
    let year = report.id[..2].parse::<u32>().unwrap_or_default() + 2000;
    let mut counts = Table::new();
    let mut uptimes = BTreeMap::<String, BTreeMap<&str, (f64, usize)>>::new();
    let mut info = BTreeMap::new();
    for fight in selected {
        let mut origin = None::<Stamp>;
        let mut states = BTreeMap::<(String, &str), AuraState>::new();
        let duration = (fight.duration_seconds * 1000.) as i64;
        report.visit(Some(fight.start..fight.end), |_, line| {
            let Some(event) = Event::parse(line) else {
                return;
            };
            let start = *origin.get_or_insert(event.stamp);
            if !event.kind.starts_with("SPELL_AURA_") {
                return;
            }
            let Some((id, icon, minimum)) = appearance::aura(event.spell) else {
                return;
            };
            let Some(player) = roster.get(event.target) else {
                return;
            };
            let offset = event.stamp.relative_to(start, year).clamp(0, duration);
            let state = states
                .entry((player.clone(), id))
                .or_insert_with(|| AuraState {
                    start: (event.kind != "SPELL_AURA_APPLIED").then_some(0),
                    count: u32::from(event.kind != "SPELL_AURA_APPLIED"),
                    minimum,
                    ..AuraState::default()
                });
            if event.kind == "SPELL_AURA_REMOVED" {
                state.close(offset);
            } else if state.start.is_some() || event.kind == "SPELL_AURA_APPLIED" {
                state.start.get_or_insert(offset);
                state.count += 1;
            }
            info.insert(id, icon);
        })?;
        for ((player, id), mut state) in states {
            state.close(duration);
            if duration > 0 && state.time >= state.minimum {
                *counts
                    .entry(player.clone())
                    .or_default()
                    .entry(id.to_owned())
                    .or_default() += i64::from(state.count);
                let (sum, fights) = uptimes.entry(player).or_default().entry(id).or_default();
                *sum += 100. * state.time as f64 / duration as f64;
                *fights += 1;
            }
        }
    }
    let used = uptimes
        .values()
        .flat_map(|values| values.keys().copied())
        .collect::<BTreeSet<_>>();
    info.retain(|id, _| used.contains(id));
    let uptimes = uptimes
        .into_iter()
        .map(|(player, auras)| {
            let display = auras
                .into_iter()
                .map(|(id, (sum, count))| (id, format!("{:.2}", sum / count as f64)))
                .collect::<BTreeMap<_, _>>();
            (player, display)
        })
        .collect::<BTreeMap<_, _>>();
    let columns = appearance::aura_columns()
        .iter()
        .filter(|id| info.contains_key(id.as_str()))
        .collect::<Vec<_>>();
    Ok(
        context! { AURA_COUNT => counts, AURA_INFO => info, AURA_COLUMNS => columns, AURA_UPTIME => uptimes },
    )
}

pub fn spells(report: &mut Report, selected: &[Encounter], id: &str) -> io::Result<Value> {
    let mut axes = Vec::<(String, Vec<(String, Vec<String>)>)>::new();
    let mut flags = BTreeMap::<String, Table>::new();
    let mut name = String::new();
    let mut color = "";
    for fight in selected {
        report.visit(Some(fight.start..fight.end), |_, line| {
            let Some(event) = Event::parse(line).filter(|event| event.spell == id) else {
                return;
            };
            if name.is_empty() {
                name = event.spell_name.to_owned();
                color = appearance::school(event.payload.split(',').next().unwrap_or_default());
            }

            let counts = flags
                .entry(event.kind.to_owned())
                .or_default()
                .entry(event.source_name.to_owned())
                .or_default();
            let value = counts.entry(event.target_name.to_owned()).or_default();
            if *value == 0 {
                let flag = match axes.iter().position(|(flag, _)| flag == event.kind) {
                    Some(index) => &mut axes[index].1,
                    None => {
                        axes.push((event.kind.to_owned(), Vec::new()));
                        &mut axes.last_mut().unwrap().1
                    }
                };
                match flag.iter().position(|(name, _)| name == event.source_name) {
                    Some(index) => flag[index].1.push(event.target_name.to_owned()),
                    None => flag.push((
                        event.source_name.to_owned(),
                        vec![event.target_name.to_owned()],
                    )),
                }
            }
            *value += 1;
            *counts.entry("Total".to_owned()).or_default() += 1;
        })?;
    }
    let mut targets = vec!["Total"];
    let mut seen = BTreeSet::from(["Total"]);
    for (_, sources) in &axes {
        for (_, names) in sources {
            targets.extend(
                names
                    .iter()
                    .map(String::as_str)
                    .filter(|name| seen.insert(*name)),
            );
        }
    }
    let rows = flags
        .iter()
        .map(|(flag, sources)| {
            let mut sorted = sources.iter().collect::<Vec<_>>();
            sorted.sort_by_key(|(_, counts)| std::cmp::Reverse(counts["Total"]));
            (flag, sorted)
        })
        .collect::<BTreeMap<_, _>>();
    Ok(context! {
        SPELLS => flags, TARGETS => targets, SORTED_SOURCES => rows,
        SPELL_ID => id, SPELL_ICON => appearance::spell_icon(id), SPELL_NAME => name, SPELL_COLOR => color,
    })
}

pub fn deaths(report: &mut Report, selected: &[Encounter]) -> io::Result<Value> {
    let data = raidledger::analysis::deaths(report, selected)?;
    Ok(context! { death_rows => data.deaths, SPELLS => data.spells,
    CLASSES => report.metadata.classes, GUIDS => report.metadata.units })
}

pub fn special(
    report: &mut Report,
    fights: &[Encounter],
    section: &str,
) -> io::Result<(&'static str, Value)> {
    let tables = raidledger::mechanics::analyze(report, fights, section)?;
    let content = match section {
        "lady_spirits" => {
            let pulls = tables.into_iter().map(|table| {
                table.rows.into_iter().filter_map(|row| {
                    let [_, by, targets, damage, prevented] = row.as_slice() else { return None };
                    Some(context! { by, targets_n => targets.split(", ").count(),
                        damage => number(damage.parse().unwrap_or_default()), prevented => number(prevented.parse().unwrap_or_default()) })
                }).collect::<Vec<_>>()
            }).collect::<Vec<_>>();
            ("lady_spirits.html", context! { PULLS => pulls })
        }
        "valks" => {
            let mut waves = Vec::<BTreeSet<String>>::new();
            let mut counts = BTreeMap::<String, usize>::new();
            for table in tables {
                let mut wave = BTreeMap::<usize, BTreeSet<String>>::new();
                for row in table.rows {
                    let [index, _, player, ..] = row.as_slice() else {
                        continue;
                    };
                    let Ok(index) = index.parse() else { continue };
                    if wave.entry(index).or_default().insert(player.clone()) {
                        *counts.entry(player.clone()).or_default() += 1;
                    }
                }
                waves.extend(wave.into_values());
            }
            (
                "valks.html",
                context! { WAVES => (1..=waves.len()).collect::<Vec<_>>(), ALL_GRABS => waves, GRABS_TOTAL => counts },
            )
        }
        "toc_valks" => {
            let portraits = portraits(report, fights)?;
            let mut totals = vec![0; tables.len()];
            let mut players = BTreeMap::<String, Vec<i64>>::new();
            let mut columns = Vec::new();
            for (index, table) in tables.into_iter().enumerate() {
                columns.push(table.caption);
                for row in table.rows {
                    let [player, amount] = row.as_slice() else {
                        continue;
                    };
                    let amount = amount.parse::<i64>().unwrap_or_default();
                    totals[index] += amount;
                    players
                        .entry(player.clone())
                        .or_insert_with(|| vec![0; totals.len()])[index] += amount;
                }
            }
            let rows = std::iter::once(("Total".to_owned(), totals))
                .chain(players)
                .map(|(player, damage)| {
                    (player, damage.into_iter().map(number).collect::<Vec<_>>())
                })
                .collect::<Vec<_>>();
            (
                "toc_valks.html",
                context! { TARGETS => columns, shield_rows => rows, SPECS => portraits },
            )
        }
        _ => {
            let portraits = portraits(report, fights)?;
            let mut rows = Vec::new();
            for table in tables {
                let parts = table.caption.rsplit(" · ").take(3).collect::<Vec<_>>();
                let [stacks, source, timestamp] = parts.as_slice() else {
                    continue;
                };
                let Some([_, actual, prevented, overkill]) = table.rows.first().map(Vec::as_slice)
                else {
                    continue;
                };
                let as_number = |value: &str| value.parse::<i64>().unwrap_or_default();
                let mut targets = BTreeMap::new();
                let mut overkills = BTreeMap::new();
                for row in table.rows.iter().skip(1) {
                    let [target, value, _, over] = row.as_slice() else {
                        continue;
                    };
                    targets.insert(target, number(as_number(value)));
                    if as_number(over) != 0 {
                        overkills.insert(target, number(as_number(over)));
                    }
                }
                let players_hit = targets
                    .keys()
                    .filter(|target| portraits.contains_key(target.as_str()))
                    .count();
                rows.push(context! { source, timestamp, stacks => stacks.trim_end_matches(" стаков"), players_hit,
                    actual_total => number(as_number(actual)), full_total => number(as_number(actual) + as_number(prevented) + as_number(overkill)),
                    prevented_total => number(as_number(prevented)), overkill_total => number(as_number(overkill)),
                    actual => targets, overkill => overkills,
                });
            }
            (
                "ucm.html",
                context! { UCM => vec![rows], SPECS => portraits },
            )
        }
    };
    Ok(content)
}

fn portraits(report: &mut Report, fights: &[Encounter]) -> io::Result<BTreeMap<String, Value>> {
    let specs = appearance::player_specs(
        report,
        fights.first().map_or(0..0, |fight| fight.start..fight.end),
    )?;
    let mut result = BTreeMap::from([(
        "Total".to_owned(),
        context! { spec => "Total", icon => "ability_hunter_readiness" },
    )]);
    for (guid, unit) in &report.metadata.units {
        let (spec, icon) = specs
            .get(guid)
            .copied()
            .unwrap_or(("Unknown", UNKNOWN_ICON));
        result.insert(
            unit.name.clone(),
            context! { spec, icon, name => &unit.name,
            class => report.metadata.classes.get(guid).map(String::as_str).unwrap_or_default() },
        );
    }
    Ok(result)
}
