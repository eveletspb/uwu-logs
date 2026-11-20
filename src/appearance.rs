use crate::combat::Event;
use crate::reports::Report;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::ops::Range;
use std::sync::LazyLock;

pub const UNKNOWN_ICON: &str = "inv_misc_questionmark";

#[derive(serde::Deserialize)]
struct Appearance {
    specs: BTreeMap<String, Vec<(String, String)>>,
    spec_book: BTreeMap<String, BTreeMap<String, u8>>,
    schools: BTreeMap<String, String>,
    useful_targets: BTreeSet<String>,
    auras: BTreeMap<String, Icon>,
    aura_aliases: BTreeMap<String, String>,
    short_auras: BTreeSet<String>,
    items: BTreeMap<String, Icon>,
    item_order: Vec<String>,
    item_aliases: BTreeMap<String, String>,
    counted_items: BTreeSet<String>,
    aura_order: Vec<String>,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct Icon {
    pub name: String,
    pub icon: String,
}

static APPEARANCE: LazyLock<Appearance> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../config/report-appearance.json"))
        .expect("bundled report appearance")
});

static SPEC_CLASSES: LazyLock<HashMap<&str, Option<&str>>> = LazyLock::new(|| {
    let mut classes = HashMap::new();
    for (class, spells) in &APPEARANCE.spec_book {
        for spell in spells.keys() {
            classes
                .entry(spell.as_str())
                .and_modify(|owner| *owner = None)
                .or_insert(Some(class.as_str()));
        }
    }
    classes
});

pub(crate) fn class_for_specialization_spell(spell: &str) -> Option<&'static str> {
    SPEC_CLASSES.get(spell).copied().flatten()
}

pub(crate) fn specialization(class: &str, spell: &str) -> Option<u8> {
    APPEARANCE.spec_book.get(class)?.get(spell).copied()
}

static ICONS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let by_icon: BTreeMap<&str, BTreeMap<&str, serde::de::IgnoredAny>> =
        serde_json::from_str(include_str!("../static/spells_icons.json"))
            .expect("bundled spell icons");
    let mut icons = by_icon
        .into_iter()
        .flat_map(|(icon, spells)| spells.into_keys().map(move |spell| (spell, icon)))
        .collect::<HashMap<_, _>>();
    let auras: HashMap<&str, &str> =
        serde_json::from_str(include_str!("../static/aura_icons.json"))
            .expect("bundled aura icons");
    icons.extend(auras);
    icons
});

pub fn spell_icon(id: &str) -> &'static str {
    item(id)
        .or_else(|| aura(id).map(|(_, aura, _)| aura))
        .map(|data| data.icon.as_str())
        .or_else(|| ICONS.get(id).copied())
        .unwrap_or(UNKNOWN_ICON)
}

pub fn item(id: &str) -> Option<&'static Icon> {
    APPEARANCE.items.get(id)
}

pub fn item_columns() -> &'static [String] {
    &APPEARANCE.item_order
}

pub fn item_alias(id: &str) -> &str {
    APPEARANCE.item_aliases.get(id).map_or(id, String::as_str)
}

pub fn count_item(id: &str) -> bool {
    APPEARANCE.counted_items.contains(id)
}

pub fn aura_columns() -> &'static [String] {
    &APPEARANCE.aura_order
}

pub fn aura(id: &str) -> Option<(&'static str, &'static Icon, i64)> {
    let id = APPEARANCE.aura_aliases.get(id).map_or(id, String::as_str);
    let (key, icon) = APPEARANCE.auras.get_key_value(id)?;
    Some((
        key,
        icon,
        if APPEARANCE.short_auras.contains(key) {
            0
        } else {
            500
        },
    ))
}

pub fn school(hex: &str) -> &'static str {
    APPEARANCE.schools.get(hex).map_or("", String::as_str)
}

pub fn useful_target(guid: &str) -> bool {
    guid.get(6..12)
        .is_some_and(|id| APPEARANCE.useful_targets.contains(id))
}

pub fn player_specs(
    report: &mut Report,
    events: Range<u64>,
) -> io::Result<BTreeMap<String, (&'static str, &'static str)>> {
    let classes = report.metadata.classes.clone();
    let mut detected = BTreeMap::new();
    report.visit(
        Some(events.start..events.end.min(events.start + 100_000)),
        |_, line| {
            let Some(event) = Event::parse(line) else {
                return;
            };
            if event.kind != "SPELL_AURA_REMOVED"
                && !detected.contains_key(event.source)
                && let Some(spec) = classes
                    .get(event.source)
                    .and_then(|class| specialization(class, event.spell))
            {
                detected.insert(event.source.to_owned(), spec);
            }
        },
    )?;
    Ok(classes
        .into_iter()
        .filter_map(|(guid, class)| {
            let index = usize::from(
                detected
                    .get(&guid)
                    .or_else(|| report.metadata.specs.get(&guid))
                    .copied()
                    .unwrap_or(if class == "warlock" { 3 } else { 0 }),
            );
            let spec = APPEARANCE.specs.get(&class)?.get(index)?;
            Some((guid, (spec.0.as_str(), spec.1.as_str())))
        })
        .collect())
}
