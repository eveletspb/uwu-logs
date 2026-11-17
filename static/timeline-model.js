/* Pure event normalization shared by the timeline and its tests. */
((root) => {
  const groupNames = {casts:'Касты', buffs:'Бафы', debuffs:'Дебафы', damage:'Урон', healing:'Лечение'};
  function build(data) {
    const duration = Number(data.RDURATION), player = data.NAME;
    const rows = [], events = [], damage = new Map();
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
        const event = {time, flag, source, target, guid, amount, extra, id, name:spell.name, group, critical, stacks};
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
        } else if (group === 'casts') {
          let start = null;
          for (const event of items) {
            if (event.flag === 'SPELL_CAST_START') start = event.time;
            else if (event.flag === 'SPELL_CAST_SUCCESS') {
              if (start !== null && event.time >= start) spans.push({start:Math.max(0,start),end:event.time});
              start = null;
            } else if (/FAILED|INTERRUPT/.test(event.flag)) start = null;
          }
        }
        const visible = items.filter(e => e.time >= 0), validSpans = spans.filter(s => s.end > s.start);
        if (!visible.length && !validSpans.length) continue;
        rows.push({id, name:spell.name, icon:spell.icon, group, events:visible,
          spans:validSpans, total:visible.reduce((n,e)=>n+e.amount,0),
          uptime:validSpans.reduce((n,s)=>n+s.end-s.start,0)});
      }
    }
    events.sort((a,b)=>a.time-b.time);
    rows.sort((a,b)=>Object.keys(groupNames).indexOf(a.group)-Object.keys(groupNames).indexOf(b.group) || b.total-a.total || b.events.length-a.events.length || a.name.localeCompare(b.name));
    return {duration,player,className:data.CLASS,rows,events,damage,groupNames};
  }
  const api = {build,groupNames};
  if (typeof module !== 'undefined') module.exports = api;
  else root.RaidTimelineModel = api;
})(typeof globalThis !== 'undefined' ? globalThis : this);
