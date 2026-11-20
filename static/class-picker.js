import {SPECS} from './constants.js?v=240909-1';

const classes = SPECS.filter((_, index) => index % 4 === 0);

export function attachClassPicker(select, numeric = false) {
  const wrapper = document.createElement('div');
  wrapper.className = 'class-picker';
  const trigger = document.createElement('button');
  trigger.type = 'button';
  trigger.className = 'class-picker-trigger';
  trigger.setAttribute('aria-label', 'Class');
  trigger.setAttribute('aria-haspopup', 'listbox');
  trigger.setAttribute('aria-expanded', 'false');
  const list = document.createElement('div');
  list.className = 'class-picker-list';
  list.id = select.id + '-options';
  list.setAttribute('role', 'listbox');
  list.setAttribute('aria-label', 'Class');
  list.hidden = true;
  trigger.setAttribute('aria-controls', list.id);
  select.before(wrapper);
  wrapper.append(select, trigger, list);
  select.hidden = true;

  function fill(element, option) {
    const data = numeric ? classes[Number(option.value)] : classes.find(entry => option.value === entry[2]);
    const label = document.createElement('span');
    label.className = data?.[2] || '';
    label.textContent = option.textContent;
    element.replaceChildren();
    if (data) {
      const icon = document.createElement('img');
      icon.src = '/static/icons/' + data[1] + '.jpg';
      icon.alt = '';
      element.append(icon);
    }
    element.append(label);
  }

  const items = [...select.options].map((option, index) => {
    const item = document.createElement('button');
    item.type = 'button';
    item.setAttribute('role', 'option');
    item.tabIndex = -1;
    fill(item, option);
    item.addEventListener('click', () => {
      if (select.selectedIndex !== index) {
        select.selectedIndex = index;
        select.dispatchEvent(new Event('change', {bubbles: true}));
      }
      close(true);
      update();
    });
    list.append(item);
    return item;
  });

  function update() {
    const option = select.selectedOptions[0];
    if (option) {
      fill(trigger, option);
      trigger.setAttribute('aria-label', 'Class: ' + option.textContent);
    }
    trigger.disabled = select.disabled;
    items.forEach((item, index) => item.setAttribute('aria-selected', String(index === select.selectedIndex)));
  }

  function close(returnFocus = false) {
    list.hidden = true;
    trigger.setAttribute('aria-expanded', 'false');
    if (returnFocus) trigger.focus();
  }

  function open() {
    update();
    list.hidden = false;
    trigger.setAttribute('aria-expanded', 'true');
    (items[select.selectedIndex] ?? items[0]).focus();
  }

  trigger.addEventListener('click', () => list.hidden ? open() : close());
  trigger.addEventListener('keydown', event => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      open();
    }
  });
  list.addEventListener('keydown', event => {
    const active = items.indexOf(document.activeElement);
    const next = {ArrowDown: active + 1, ArrowUp: active - 1, Home: 0, End: items.length - 1}[event.key];
    if (next !== undefined) {
      event.preventDefault();
      items[Math.max(0, Math.min(items.length - 1, next))].focus();
    } else if (event.key === 'Escape') {
      event.preventDefault();
      close(true);
    }
  });
  wrapper.addEventListener('focusout', event => {
    if (!wrapper.contains(event.relatedTarget)) close();
  });
  select.addEventListener('change', update);
  new MutationObserver(update).observe(select, {attributes: true, attributeFilter: ['disabled']});
  update();
  return update;
}
