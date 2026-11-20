const RU_SLOTS = ["Голова", "Шея", "Плечи", "Рубашка", "Грудь", "Пояс", "Ноги", "Ступни",
  "Запястья", "Кисти рук", "Кольцо 1", "Кольцо 2", "Аксессуар 1", "Аксессуар 2",
  "Спина", "Правая рука", "Левая рука", "Дальний бой", "Гербовая накидка"];

const RU_STATS = {
  0: "к мане", 1: "к здоровью", 3: "к ловкости", 4: "к силе", 5: "к интеллекту",
  6: "к духу", 7: "к выносливости", 12: "к рейтингу защиты", 13: "к рейтингу уклонения",
  14: "к рейтингу парирования", 15: "к рейтингу блока", 16: "к меткости в ближнем бою",
  17: "к меткости в дальнем бою", 18: "к меткости заклинаний",
  19: "к критическому удару в ближнем бою", 20: "к критическому удару в дальнем бою",
  21: "к критическому удару заклинаний", 22: "к защите от ударов в ближнем бою",
  23: "к защите от ударов в дальнем бою", 24: "к защите от попаданий заклинаниями",
  25: "к защите от критических ударов в ближнем бою", 26: "к защите от критических ударов в дальнем бою",
  27: "к защите от критических ударов заклинаниями", 28: "к скорости ближнего боя",
  29: "к скорости дальнего боя", 30: "к скорости произнесения заклинаний",
  31: "к рейтингу меткости", 32: "к рейтингу критического удара", 33: "к защите от попаданий",
  34: "к защите от критических ударов", 35: "к рейтингу устойчивости", 36: "к рейтингу скорости",
  37: "к рейтингу мастерства", 38: "к силе атаки", 39: "к силе атаки дальнего боя",
  41: "к силе исцеления", 42: "к урону от заклинаний", 43: "к восстановлению маны",
  44: "к рейтингу пробивания брони", 45: "к силе заклинаний", 46: "к восстановлению здоровья",
  47: "к проникающей способности заклинаний", 48: "к величине блока",
};

const RU_RESISTANCES = {
  holy: "свету", fire: "огню", nature: "силам природы", frost: "льду",
  shadow: "тёмной магии", arcane: "тайной магии",
};
const RU_DAMAGE_SCHOOLS = ["физического урона", "урона от светлой магии", "урона от огня",
  "урона от сил природы", "урона от магии льда", "урона от тёмной магии", "урона от тайной магии"];

const EN_SLOTS = ["Head", "Neck", "Shoulders", "Shirt", "Chest", "Waist", "Legs", "Feet",
  "Wrists", "Hands", "Ring 1", "Ring 2", "Trinket 1", "Trinket 2", "Back",
  "Main Hand", "Off Hand", "Ranged", "Tabard"];

const EN_STATS = {
  0: "Mana", 1: "Health", 3: "Agility", 4: "Strength", 5: "Intellect", 6: "Spirit", 7: "Stamina",
  12: "Defense Rating", 13: "Dodge Rating", 14: "Parry Rating", 15: "Block Rating",
  16: "Melee Hit Rating", 17: "Ranged Hit Rating", 18: "Spell Hit Rating",
  19: "Melee Critical Strike Rating", 20: "Ranged Critical Strike Rating", 21: "Spell Critical Strike Rating",
  22: "Melee Hit Avoidance Rating", 23: "Ranged Hit Avoidance Rating", 24: "Spell Hit Avoidance Rating",
  25: "Melee Critical Avoidance Rating", 26: "Ranged Critical Avoidance Rating", 27: "Spell Critical Avoidance Rating",
  28: "Melee Haste Rating", 29: "Ranged Haste Rating", 30: "Spell Haste Rating",
  31: "Hit Rating", 32: "Critical Strike Rating", 33: "Hit Avoidance Rating", 34: "Critical Avoidance Rating",
  35: "Resilience Rating", 36: "Haste Rating", 37: "Expertise Rating", 38: "Attack Power",
  39: "Ranged Attack Power", 41: "Healing Power", 42: "Spell Damage", 43: "Mana Regeneration",
  44: "Armor Penetration Rating", 45: "Spell Power", 46: "Health Regeneration",
  47: "Spell Penetration", 48: "Block Value",
};

const EN_RESISTANCES = {
  holy: "Holy", fire: "Fire", nature: "Nature", frost: "Frost", shadow: "Shadow", arcane: "Arcane",
};
const EN_DAMAGE_SCHOOLS = ["physical damage", "Holy damage", "Fire damage", "Nature damage",
  "Frost damage", "Shadow damage", "Arcane damage"];

const BONUS_NAMES = new Map(Object.entries(EN_STATS).map(([id, name]) => [name.toLowerCase(), RU_STATS[id]]));
for (const [name, label] of Object.entries({
  "critical": RU_STATS[32],
  "crit rating": RU_STATS[32],
  "dodge": RU_STATS[13],
  "attack power ranged": RU_STATS[39],
  "all stats": "ко всем характеристикам",
  "damage": "к урону оружия",
})) BONUS_NAMES.set(name, label);

const EFFECT_NAMES = new Map([
  ["arcane infused", "Насыщение тайной магией"],
]);

export function equipmentLocale(language) {
  const russian = /^ru(?:[-_]|$)/i.test(language);
  const tag = russian ? "ru-RU" : "en-US";
  const number = new Intl.NumberFormat(tag, {maximumFractionDigits: 2});
  const signed = new Intl.NumberFormat(tag, {signDisplay: "always"});
  const speed = new Intl.NumberFormat(tag, {minimumFractionDigits: 2, maximumFractionDigits: 2});
  const localized = ({name, nameRu}) => russian ? nameRu?.trim() : name?.trim();
  const name = item => localized(item) || item.name?.trim() || item.nameRu?.trim();
  const inRussian = value => /\p{Script=Cyrillic}/u.test(value);
  const bonus = (value, label) => label ? `${signed.format(value)} ${label}` : "";
  const stat = (type, value) => bonus(value,
    (russian ? RU_STATS : EN_STATS)[type] ?? (russian ? `(характеристика ${type})` : `(stat ${type})`));

  function spell(effect) {
    const original = name(effect);
    if (!russian || !original || inRussian(original)) return original;
    const english = effect.name?.trim() || original;
    const known = EFFECT_NAMES.get(english.toLowerCase());
    if (known) return known;
    const match = /^(?:Increased\s+)?(.+?)\s+([+-]?\d+)$/i.exec(english);
    // shortcut: preserve the original name for unique effects without a source translation.
    return (match && bonus(Number(match[2]), BONUS_NAMES.get(match[1].toLowerCase()))) || original;
  }

  function enchantment(effect) {
    const original = name(effect);
    if (!russian || !original || inRussian(original)) return original;
    const english = effect.name?.trim() || original;

    const scope = /^Scope\s*\(([+-]\d+) Damage\)$/i.exec(english);
    if (scope) return `Прицел (${bonus(Number(scope[1]), BONUS_NAMES.get("damage"))})`;

    const effects = (effect.effects ?? []).map(entry => {
      if (entry.type === 5) return bonus(entry.amount, RU_STATS[entry.spellId]);
      if (entry.type === 2) return bonus(entry.amount, BONUS_NAMES.get("damage"));
      return "";
    });
    if (effects.length && effects.every(Boolean)) return effects.join(" · ");

    const matches = [...english.matchAll(/([+-]\d+)\s+([^+-]+)/g)];
    const translated = matches.map(([, value, label]) => bonus(Number(value), BONUS_NAMES.get(label.trim().toLowerCase())));
    if (matches.length && matches[0].index === 0 && translated.every(Boolean)) return translated.join(" · ");
    return original;
  }

  return {
    name, stat, spell, enchantment,
    slot: index => (russian ? RU_SLOTS : EN_SLOTS)[index],
    itemLevel: value => russian ? `Уровень предмета ${value}` : `Item Level ${value}`,
    damage: ({min, max, type}) => {
      const school = (russian ? RU_DAMAGE_SCHOOLS : EN_DAMAGE_SCHOOLS)[type] ?? (russian ? "урона" : "damage");
      return `${number.format(min)}–${number.format(max)} ${school}`;
    },
    speed: ms => russian ? `Скорость ${speed.format(ms / 1000)} с` : `Speed ${speed.format(ms / 1000)} s`,
    armor: value => `${russian ? "Броня" : "Armor"}: ${number.format(value)}`,
    block: value => `${russian ? "Блок" : "Block"}: ${number.format(value)}`,
    resistances: russian ? RU_RESISTANCES : EN_RESISTANCES,
    resistance: (value, school) => russian
      ? `${signed.format(value)} к сопротивлению ${school}` : `${signed.format(value)} ${school} Resistance`,
    enchantmentsLabel: russian ? "Чары и камни" : "Enchantments and gems",
  };
}
