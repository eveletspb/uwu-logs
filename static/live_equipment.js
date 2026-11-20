import { QUALITY_COLOR } from "./gear_constants.js?v=240814-1";

const SLOTS = ["Голова", "Шея", "Плечи", "Рубашка", "Грудь", "Пояс", "Ноги", "Ступни",
  "Запястья", "Кисти рук", "Кольцо 1", "Кольцо 2", "Аксессуар 1", "Аксессуар 2",
  "Спина", "Правая рука", "Левая рука", "Дальний бой", "Гербовая накидка"];

const STATS = {
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

const RESISTANCES = {
  holy: "свету", fire: "огню", nature: "силам природы", frost: "льду",
  shadow: "тёмной магии", arcane: "тайной магии",
};
const DAMAGE_SCHOOLS = ["физического урона", "урона от светлой магии", "урона от огня",
  "урона от сил природы", "урона от магии льда", "урона от тёмной магии", "урона от тайной магии"];

const number = new Intl.NumberFormat("ru-RU", {maximumFractionDigits: 2});
const signed = new Intl.NumberFormat("ru-RU", {signDisplay: "always"});
const speed = new Intl.NumberFormat("ru-RU", {minimumFractionDigits: 2, maximumFractionDigits: 2});

function text(tag, value, className) {
  const element = document.createElement(tag);
  element.textContent = value;
  if (className) element.className = className;
  return element;
}

function names(effects = []) {
  return effects.map(effect => effect.nameRu || effect.name).filter(Boolean);
}

function tooltipContents(item, name, quality) {
  const lines = [text("strong", name, quality),
    text("div", `${SLOTS[item.slot]} · Уровень предмета ${item.itemLevel}`, "equipment-tooltip-meta")];
  for (const damage of item.damage ?? []) {
    if (damage.max) {
      lines.push(text("div", `${number.format(damage.min)}–${number.format(damage.max)} ${DAMAGE_SCHOOLS[damage.type] ?? "урона"}`));
    }
  }
  if (item.delayMs) lines.push(text("div", `Скорость ${speed.format(item.delayMs / 1000)} с`));
  if (item.armor) lines.push(text("div", `Броня: ${number.format(item.armor)}`));
  if (item.block) lines.push(text("div", `Блок: ${number.format(item.block)}`));
  for (const stat of item.stats ?? []) {
    if (stat.value) lines.push(text("div", `${signed.format(stat.value)} ${STATS[stat.type] ?? `(характеристика ${stat.type})`}`));
  }
  for (const [school, label] of Object.entries(RESISTANCES)) {
    const value = item.resistances?.[school];
    if (value) lines.push(text("div", `${signed.format(value)} к сопротивлению ${label}`));
  }
  for (const effect of names(item.spells)) lines.push(text("div", effect, "equipment-tooltip-effect"));
  const enchantments = [...names(item.enchantments), ...names(item.randomPropertyEnchantments)];
  if (enchantments.length) {
    lines.push(text("div", "Чары и камни", "equipment-tooltip-label"));
    for (const enchantment of enchantments) lines.push(text("div", enchantment, "equipment-tooltip-effect"));
  }
  return lines;
}

function itemIcon(icon) {
  const image = document.createElement("img");
  image.className = "equipment-icon";
  image.alt = "";
  image.width = image.height = 36;

  const filename = /^[a-z0-9_-]+$/i.test(icon ?? "") ? icon : "inv_misc_questionmark";
  image.addEventListener("error", () => {
    image.addEventListener("error", () => { image.hidden = true; }, {once: true});
    image.src = `/static/icons/${filename}.jpg`;
  }, {once: true});
  image.src = `/static/item-icons/assets/${filename}.webp`;
  return image;
}

export function showLiveEquipment(profile, signal) {
  const list = document.getElementById("live-equipment-items");
  const tooltip = document.getElementById("equipment-tooltip");
  document.getElementById("live-equipment-character").textContent =
    [`Уровень ${profile.level}`, profile.spec].filter(Boolean).join(" · ");

  let active;
  function hide() {
    active?.removeAttribute("aria-describedby");
    active = undefined;
    tooltip.hidden = true;
  }

  function position() {
    if (!active) return;

    const anchor = active.getBoundingClientRect();
    if (anchor.bottom <= 0 || anchor.top >= innerHeight) {
      hide();
      return;
    }
    const {offsetWidth: width, offsetHeight: height} = tooltip;
    const nextToItem = anchor.right + width + 12 < innerWidth;
    const x = nextToItem ? anchor.right + 8 : anchor.left;
    const y = nextToItem ? anchor.top : anchor.bottom + 8;
    tooltip.style.left = `${Math.max(8, Math.min(x, innerWidth - width - 8))}px`;
    const top = !nextToItem && y + height > innerHeight && anchor.top >= height + 8
      ? anchor.top - height - 8 : y;
    tooltip.style.top = `${Math.max(8, Math.min(top, innerHeight - height - 8))}px`;
  }

  function show(button, contents) {
    if (active === button) return;
    hide();
    active = button;
    tooltip.replaceChildren(...contents);
    tooltip.hidden = false;
    button.setAttribute("aria-describedby", tooltip.id);
    position();
  }

  list.replaceChildren(...profile.equipment.map(item => {
    const row = document.createElement("li");
    row.dataset.itemId = item.itemEntry;
    const button = document.createElement("button");
    button.className = "equipment-item";
    button.type = "button";
    const name = item.nameRu || item.name || `#${item.itemEntry}`;
    const quality = QUALITY_COLOR[item.quality] || "";
    const level = text("span", item.itemLevel, "equipment-level");
    level.title = `Уровень предмета ${item.itemLevel}`;
    button.append(itemIcon(item.icon), text("span", SLOTS[item.slot], "equipment-slot"),
      text("strong", name, quality), level);

    const contents = tooltipContents(item, name, quality);
    button.addEventListener("pointerenter", event => {
      if (event.pointerType !== "touch") show(button, contents);
    });
    button.addEventListener("pointerleave", event => {
      if (event.pointerType !== "touch" && active === button && button !== document.activeElement) hide();
    });
    button.addEventListener("focus", () => show(button, contents));
    button.addEventListener("blur", () => { if (active === button) hide(); });
    button.addEventListener("click", () => show(button, contents));
    row.append(button);
    return row;
  }));

  document.addEventListener("pointerdown", event => {
    if (active && !active.contains(event.target)) hide();
  }, {signal});
  document.addEventListener("keydown", event => {
    if (event.key === "Escape" && active) hide();
  }, {signal});
  window.addEventListener("scroll", position, {signal, passive: true});
  window.addEventListener("resize", position, {signal});
  signal.addEventListener("abort", hide, {once: true});
  document.getElementById("live-equipment").hidden = false;
}
