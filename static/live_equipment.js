import { equipmentLocale } from "./equipment_locale.js?v=1";
import { QUALITY_COLOR } from "./gear_constants.js?v=240814-1";

const locale = equipmentLocale(navigator.language);

function text(tag, value, className) {
  const element = document.createElement(tag);
  element.textContent = value;
  if (className) element.className = className;
  return element;
}

function tooltipContents(item, name, quality) {
  const lines = [text("strong", name, quality),
    text("div", `${locale.slot(item.slot)} · ${locale.itemLevel(item.itemLevel)}`, "equipment-tooltip-meta")];
  for (const damage of item.damage ?? []) {
    if (damage.max) {
      lines.push(text("div", locale.damage(damage)));
    }
  }
  if (item.delayMs) lines.push(text("div", locale.speed(item.delayMs)));
  if (item.armor) lines.push(text("div", locale.armor(item.armor)));
  if (item.block) lines.push(text("div", locale.block(item.block)));
  for (const stat of item.stats ?? []) {
    if (stat.value) lines.push(text("div", locale.stat(stat.type, stat.value)));
  }
  for (const [school, label] of Object.entries(locale.resistances)) {
    const value = item.resistances?.[school];
    if (value) lines.push(text("div", locale.resistance(value, label)));
  }
  for (const effect of (item.spells ?? []).map(locale.spell).filter(Boolean)) lines.push(text("div", effect, "equipment-tooltip-effect"));
  const enchantments = [...(item.enchantments ?? []), ...(item.randomPropertyEnchantments ?? [])]
    .map(locale.enchantment).filter(Boolean);
  if (enchantments.length) {
    lines.push(text("div", locale.enchantmentsLabel, "equipment-tooltip-label"));
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
    const name = locale.name(item) || `#${item.itemEntry}`;
    const quality = QUALITY_COLOR[item.quality] || "";
    const level = text("span", item.itemLevel, "equipment-level");
    level.title = locale.itemLevel(item.itemLevel);
    button.append(itemIcon(item.icon), text("span", locale.slot(item.slot), "equipment-slot"),
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
