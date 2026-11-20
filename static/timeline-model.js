/* Pure event normalization shared by the timeline and its tests. */
((root) => {
  const groupNames = {casts:'Касты', buffs:'Бафы', debuffs:'Дебафы', damage:'Урон', healing:'Лечение'};
  function build(data) {
    const duration = Number(data.RDURATION), player = data.NAME;
    const rows = [], events = [], damage = new Map();
    const castGroups = new Map();
    for (const [id, raw] of Object.entries(data.DATA || {})) {
      const spell = data.SPELLS[id] || {name:id};
      const groups = {};
      for (const rawEvent of [...raw].sort((a,b) => a[0]-b[0])) {
        const [ms, flag, source, target, guid, extra] = rawEvent;
        const fields = String(extra || '').split(',');
        const time = Number(ms)/1000;
        let group;
        if (flag.startsWith('SPELL_AURA_') && target === player) group = fields.includes('DEBUFF') ? 'debuffs' : 'buffs';
        else if (source === player && flag.startsWith('SPELL_CAST_')) group = 'casts';
        else if (source === player && /DAMAGE|MISSED/.test(flag)) group = 'damage';
        else if (source === player && /HEAL/.test(flag)) group = 'healing';
        if (!group || time > duration) continue;
        const amount = /DAMAGE|HEAL/.test(flag) ? Math.max(0, (Number(fields[1]) || 0) - (Number(fields[2]) || 0)) : 0;
        const critical = /DAMAGE/.test(flag) ? fields[7] === '1' : /HEAL/.test(flag) && fields[4] === '1';
        const stacks = /DOSE/.test(flag) ? Number(fields.at(-1)) : null;
        const event = {time, flag, source, target:target === 'nil' ? '' : target, guid, amount, extra, id, name:spell.name, group, critical, stacks};
        if (group === 'casts') {
          const key = JSON.stringify([spell.name, spell.icon]);
          if (!castGroups.has(key)) castGroups.set(key, {id, spell, items:new Map()});
          const items = castGroups.get(key).items;
          const eventKey = JSON.stringify([ms, flag, guid]);
          if (!items.has(eventKey)) items.set(eventKey, event);
          continue;
        }
        (groups[group] ||= []).push(event);
        if (time >= 0) {
          events.push(event);
          if (group === 'damage') damage.set(Math.floor(time), (damage.get(Math.floor(time)) || 0) + amount);
        }
      }
      for (const [group, items] of Object.entries(groups)) {
        const spans = [];
        if (group === 'buffs' || group === 'debuffs') {
          let active = null, inferred = false, seen = false;
          for (const event of items) {
            if (event.flag === 'SPELL_AURA_APPLIED') {
              if (active === null) { active = event.time; inferred = false; }
            } else if (event.flag === 'SPELL_AURA_REMOVED') {
              if (active !== null || (!seen && event.time >= 0)) spans.push({start:Math.max(0, active ?? 0), end:Math.max(0, event.time), inferred:active === null || inferred});
              active = null;
            } else if (active === null && /REFRESH|DOSE/.test(event.flag)) {
              active = event.time < 0 ? event.time : 0; inferred = true;
            }
            seen = true;
          }
          if (active !== null) spans.push({start:Math.max(0,active),end:duration,inferred,open:true});
        }
        const visible = items.filter(e => e.time >= 0), validSpans = spans.filter(s => s.end > s.start);
        if (!visible.length && !validSpans.length) continue;
        rows.push({id, name:spell.name, icon:spell.icon, group, events:visible,
          spans:validSpans, total:visible.reduce((n,e)=>n+e.amount,0),
          uptime:validSpans.reduce((n,s)=>n+s.end-s.start,0)});
      }
    }
    for (const {id, spell, items} of castGroups.values()) {
      const ordered = [...items.values()].sort((a,b) => a.time-b.time);
      const visible = ordered.filter(event => event.time >= 0), spans = [];
      let start = null, attempts = 0, successes = 0;
      for (const {time, flag} of ordered) {
        if (flag === 'SPELL_CAST_START') {
          start = time;
          if (time >= 0) attempts++;
        } else if (flag === 'SPELL_CAST_SUCCESS' || flag === 'SPELL_CAST_FAILED') {
          if (time >= 0) {
            if (start === null || start < 0) attempts++;
            if (flag === 'SPELL_CAST_SUCCESS') {
              successes++;
              if (start !== null && time > Math.max(0,start)) spans.push({start:Math.max(0,start),end:time});
            }
          }
          start = null;
        }
      }
      if (!visible.length) continue;
      for (const event of visible) events.push(event);
      rows.push({id, name:spell.name, icon:spell.icon, group:'casts', events:visible, spans, total:attempts, successes});
    }
    events.sort((a,b)=>a.time-b.time);
    rows.sort((a,b)=>Object.keys(groupNames).indexOf(a.group)-Object.keys(groupNames).indexOf(b.group) || b.total-a.total || b.events.length-a.events.length || a.name.localeCompare(b.name));
    return {duration,player,className:data.CLASS,rows,events,damage,groupNames};
  }
  const api = {build,groupNames};
  if (typeof module !== 'undefined') module.exports = api;
  else root.RaidTimelineModel = api;
})(typeof globalThis !== 'undefined' ? globalThis : this);
