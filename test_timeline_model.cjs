const assert = require('node:assert/strict');
const {build} = require('./static/timeline-model.js');
const event = (ms,flag,source='Player',target='Boss',extra='1,100,0') => [ms,flag,source,target,'guid',extra];
const data = (events) => ({NAME:'Player',RDURATION:20,SPELLS:{1:{name:'Ability',icon:'icon'}},DATA:{1:events}});
// Start before the pull; refresh does not restart uptime, duplicate removals do not add uptime.
let result=build(data([event(-2000,'SPELL_AURA_APPLIED','Other','Player','1,BUFF'),event(3000,'SPELL_AURA_REFRESH','Other','Player','1,BUFF'),event(5000,'SPELL_AURA_REMOVED','Other','Player','1,BUFF'),event(6000,'SPELL_AURA_REMOVED','Other','Player','1,BUFF')]));
assert.equal(result.rows[0].uptime,5); assert.equal(result.rows[0].spans.length,1);
// An aura whose application is missing is explicitly marked as inferred.
result=build(data([event(4000,'SPELL_AURA_REMOVED','Other','Player','1,BUFF')]));
assert.equal(result.rows[0].spans[0].inferred,true);assert.equal(result.rows[0].uptime,4);
// A buff still active at the end is clipped to the encounter duration.
result=build(data([event(10000,'SPELL_AURA_APPLIED','Other','Player','1,BUFF')]));
assert.equal(result.rows[0].uptime,10);assert.equal(result.rows[0].spans[0].open,true);
// Incoming damage and auras on other players are excluded from this player's outgoing damage / self buffs.
result=build(data([event(1000,'SPELL_DAMAGE','Player','Boss','1,100,20'),event(2000,'SPELL_DAMAGE','Boss','Player','1,900,0'),event(3000,'SPELL_AURA_APPLIED','Player','Other','1,BUFF')]));
assert.equal(result.rows.length,1);assert.equal(result.rows[0].total,80);assert.equal(result.damage.get(1),80);
// Cast durations use start/success pairs; failed casts do not become successful spans.
result=build(data([event(1000,'SPELL_CAST_START'),event(3500,'SPELL_CAST_SUCCESS'),event(5000,'SPELL_CAST_START'),event(6000,'SPELL_CAST_FAILED')]));
assert.deepEqual(result.rows[0].spans,[{start:1,end:3.5}]);
// Debuffs and buffs remain separate; pre-pull events don't leak into damage totals.
result=build(data([event(-1000,'SPELL_DAMAGE'),event(1000,'SPELL_AURA_APPLIED','Other','Player','1,DEBUFF'),event(4000,'SPELL_AURA_REMOVED','Other','Player','1,DEBUFF')]));
assert.equal(result.rows.length,1);assert.equal(result.rows[0].group,'debuffs');assert.equal(result.damage.size,0);
// Critical damage and aura stacks remain available for detailed event tooltips.
result=build(data([event(1000,'SPELL_DAMAGE','Player','Boss','1,100,20,1,0,0,0,1,0,0'),event(2000,'SPELL_AURA_APPLIED_DOSE','Player','Player','1,BUFF,3')]));
assert.equal(result.events[0].critical,true);assert.equal(result.events[1].stacks,3);
// The log can record only cast starts or duplicate a cast under two spell IDs.
const alias = event(5000,'SPELL_CAST_SUCCESS','Player','nil');
const recorded = data([event(1000,'SPELL_CAST_START','Player','nil'),alias]);
recorded.DATA[2] = [alias];
recorded.SPELLS[2] = recorded.SPELLS[1];
result=build(recorded);
assert.equal(result.rows.length,1);assert.equal(result.rows[0].events.length,2);
assert.equal(result.rows[0].successes,1);assert.equal(result.events[0].target,'');
result=build(data([event(1000,'SPELL_CAST_START','Player','nil')]));
assert.equal(result.rows[0].total,1);assert.equal(result.rows[0].successes,0);
console.log('Timeline model: event, interval and cast alias scenarios passed.');
