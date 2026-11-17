(() => {
  'use strict';
  const app = document.getElementById('timeline-app');
  const $ = id => document.getElementById(id);
  const board = $('tl-board'), tracks = $('tl-tracks'), overview = $('tl-overview');
  const tooltip = $('tl-tooltip'), zoom = $('tl-zoom');
  const colors = {casts:'#67d2c6',buffs:'#72ba8b',debuffs:'#ba8798',damage:'#5cb9d4',healing:'#8bcbb7'};
  const active = new Set(['casts','buffs','damage']);
  try {const stored=JSON.parse(localStorage.getItem('raid-timeline-layers'));if(Array.isArray(stored)){active.clear();stored.filter(key=>key in colors).forEach(key=>active.add(key));}} catch (_) {}
  const names = {SPELL_CAST_START:'Начало каста',SPELL_CAST_SUCCESS:'Каст',SPELL_CAST_FAILED:'Каст не выполнен',SPELL_AURA_APPLIED:'Аура наложена',SPELL_AURA_REMOVED:'Аура снята',SPELL_AURA_REFRESH:'Аура обновлена',SPELL_AURA_APPLIED_DOSE:'Добавлен эффект',SPELL_AURA_REMOVED_DOSE:'Снят эффект',SPELL_DAMAGE:'Урон',SWING_DAMAGE:'Ближний бой',RANGE_DAMAGE:'Дальний бой',SPELL_PERIODIC_DAMAGE:'Периодический урон',SPELL_HEAL:'Лечение',SPELL_PERIODIC_HEAL:'Периодическое лечение'};
  let model, visibleRows = [], canvases = [], scale = 1, cursor = null, hoverTime = null, drag = null, raf = 0, lastRange='';
  const number = value => Math.round(value).toLocaleString('ru-RU');
  const clamp = (n,min,max) => Math.max(min,Math.min(max,n));
  const time = (n, precise=false) => { const value=Math.round(Math.max(0,n)*(precise?1000:1))/(precise?1000:1);return `${Math.floor(value/60)}:${(value%60).toFixed(precise?3:0).padStart(precise?6:2,'0')}`; };
  function labelWidth() { return innerWidth < 650 ? 165 : 205; }
  function plotWidth() { return Math.max(120,board.clientWidth-labelWidth()); }
  function pps() { return plotWidth()/Math.max(1,model.duration)*scale; }
  function windowRange() { return [board.scrollLeft/pps(), Math.min(model.duration,(board.scrollLeft+plotWidth())/pps())]; }
  function canvasContext(canvas, width, height) {
    const dpr = Math.min(devicePixelRatio || 1,2);
    const w = Math.round(width*dpr), h = Math.round(height*dpr);
    if (canvas.width !== w || canvas.height !== h) { canvas.width=w; canvas.height=h; }
    canvas.style.width=width+'px'; canvas.style.height=height+'px';
    const ctx=canvas.getContext('2d'); ctx.setTransform(dpr,0,0,dpr,0,0); ctx.clearRect(0,0,width,height);
    return ctx;
  }
  function ticks() {
    const seconds=65/pps();
    return [1,2,5,10,15,30,60,120,300,600].find(v=>v>=seconds) || 1200;
  }
  function grid(ctx,w,h) {
    const [start,end]=windowRange(), step=ticks();
    ctx.strokeStyle='#283747';ctx.lineWidth=1;
    for(let t=Math.ceil(start/step)*step;t<=end;t+=step){const x=(t-start)*pps();ctx.beginPath();ctx.moveTo(x,0);ctx.lineTo(x,h);ctx.stroke();}
    const crosshair=cursor??hoverTime;
    if(crosshair!==null){const x=(crosshair-start)*pps();ctx.strokeStyle='#d6ece9';ctx.beginPath();ctx.moveTo(x,0);ctx.lineTo(x,h);ctx.stroke();}
    if(drag?.kind==='track'){const a=(Math.min(drag.start,drag.end)-start)*pps(),b=(Math.max(drag.start,drag.end)-start)*pps();ctx.fillStyle='#67d2c630';ctx.fillRect(a,0,b-a,h);}
  }
  function render() {
    raf=0;if(!model)return;
    const w=plotWidth(), [start,end]=windowRange();
    for(const {canvas,row} of canvases){
      if(row){const top=canvas.parentElement.offsetTop;if(top+30<board.scrollTop||top>board.scrollTop+board.clientHeight)continue;}
      const h=row?30:30,ctx=canvasContext(canvas,w,h);grid(ctx,w,h);
      if(!row){ctx.font='10px Ubuntu, sans-serif';ctx.fillStyle='#a4b7cc';const step=ticks();for(let t=Math.ceil(start/step)*step;t<end;t+=step)ctx.fillText(time(t),(t-start)*pps()+3,19);continue;}
      const color=colors[row.group];ctx.fillStyle=color;
      for(const span of row.spans){if(span.end<start||span.start>end)continue;const x=(Math.max(start,span.start)-start)*pps(),right=(Math.min(end,span.end)-start)*pps();ctx.globalAlpha=row.group==='casts'?.55:.42;ctx.fillRect(x,6,Math.max(2,right-x),18);ctx.globalAlpha=1;ctx.strokeStyle=color;ctx.setLineDash(span.inferred?[3,3]:[]);ctx.strokeRect(x,6,Math.max(2,right-x),18);ctx.setLineDash([]);}
      const max=Math.max(1,...row.events.map(e=>e.amount));
      for(const event of row.events){if(event.time<start||event.time>end)continue;const x=(event.time-start)*pps();let h=16;if(row.group==='damage'||row.group==='healing')h=event.amount?5+19*Math.sqrt(event.amount/max):4;ctx.globalAlpha=/MISSED|FAILED/.test(event.flag)?.35:.95;ctx.fillRect(x,27-h,Math.max(2,Math.min(5,pps()*.12)),h);}
      ctx.globalAlpha=1;
    }
    $('tl-window').textContent=`${time(start)} — ${time(end)} · ${scale.toFixed(1)}×`;
    drawOverview();
    const rangeKey=`${Math.floor(start*2)}:${Math.floor(end*2)}`;
    if(cursor===null&&lastRange!==rangeKey){lastRange=rangeKey;showEvents();}
  }
  function schedule(){if(!raf)raf=requestAnimationFrame(render);}
  function drawOverview(){
    const w=overview.clientWidth,h=86,ctx=canvasContext(overview,w,h),d=model.duration;
    const max=Math.max(1,...model.damage.values());ctx.beginPath();ctx.moveTo(0,h-7);
    for(let sec=0;sec<=Math.ceil(d);sec++)ctx.lineTo(sec/d*w,h-7-(model.damage.get(sec)||0)/max*(h-20));
    ctx.lineTo(w,h-7);ctx.closePath();ctx.fillStyle='#67d2c61f';ctx.fill();ctx.strokeStyle='#67d2c6';ctx.lineWidth=1.2;ctx.stroke();
    const [start,end]=windowRange();ctx.fillStyle='#060b1399';ctx.fillRect(0,0,start/d*w,h);ctx.fillRect(end/d*w,0,w-end/d*w,h);ctx.strokeStyle='#72c9bf';ctx.strokeRect(start/d*w,1,(end-start)/d*w,h-2);
    ctx.fillStyle='#9bb1c9';ctx.font='10px Ubuntu,sans-serif';ctx.fillText('0:00',5,13);ctx.fillText(time(d),w-40,13);
    if(drag?.kind==='overview'){ctx.fillStyle='#67d2c64a';ctx.fillRect(Math.min(drag.start,drag.end)/d*w,0,Math.abs(drag.end-drag.start)/d*w,h);}
  }
  function buildRows(){
    tracks.replaceChildren();canvases=[];
    const search=$('tl-search').value.toLocaleLowerCase().trim();
    visibleRows=model.rows.filter(r=>active.has(r.group)&&r.name.toLocaleLowerCase().includes(search));
    const rowElement=(row)=>{
      const line=document.createElement('div');line.className='tl-track'+(!row?' tl-ruler':'');
      const label=document.createElement('div');label.className='tl-track-name';
      if(row){
        if(row.icon){const img=document.createElement('img');img.src='/static/icons/'+encodeURIComponent(row.icon)+'.jpg';img.alt='';label.append(img);}
        const text=document.createElement('span');text.textContent=row.name;label.append(text);label.title=row.name;
        const estimated=row.spans.some(span=>span.inferred);
        const count=document.createElement('small');count.textContent=row.group==='buffs'||row.group==='debuffs'?(estimated?'≈':'')+Math.round(row.uptime/model.duration*100)+'%':number(row.group==='damage'||row.group==='healing'?row.total:row.events.filter(e=>e.flag==='SPELL_CAST_SUCCESS').length);
        count.title=row.group==='buffs'||row.group==='debuffs'?(estimated?'Оценка времени действия: начало ауры отсутствует в записи':'Время действия за бой'):row.group==='casts'?'Успешные касты':'Суммарное значение';label.append(count);
      }else label.textContent='Способность / время';
      const canvas=document.createElement('canvas');canvas.className='tl-track-plot';canvas.setAttribute('aria-label',row?row.name+' · '+model.groupNames[row.group]:'Шкала времени');
      canvas.style.width=plotWidth()+'px';
      line.append(label,canvas);tracks.append(line);canvases.push({canvas,row});
      canvas.addEventListener('pointermove',e=>hover(e,row,canvas));canvas.addEventListener('pointerleave',()=>{tooltip.hidden=true;hoverTime=null;schedule();});
      canvas.addEventListener('pointerdown',e=>{if(e.button!==0)return;canvas.setPointerCapture(e.pointerId);const t=at(e,canvas);drag={kind:'track',start:t,end:t};tooltip.hidden=true;});
      canvas.addEventListener('pointerup',e=>endDrag(e,canvas));
      canvas.addEventListener('pointercancel',()=>{drag=null;schedule();});
    };
    rowElement(null);
    for(const group of Object.keys(model.groupNames)){
      const rows=visibleRows.filter(r=>r.group===group);if(!rows.length)continue;
      const header=document.createElement('div');header.className='tl-group';header.style.setProperty('--track-color',colors[group]);const caption=document.createElement('span');caption.textContent=model.groupNames[group]+' · '+rows.length;header.append(caption);tracks.append(header);
      rows.forEach(rowElement);
    }
    $('tl-empty').hidden=visibleRows.length>0;
    resize();showEvents();
  }
  function resize(){if(!model)return;board.style.setProperty('--tl-label',labelWidth()+'px');tracks.style.width=(labelWidth()+plotWidth()*scale)+'px';schedule();}
  function setZoom(value,center){
    if(!model)return;const oldCenter=center??(board.scrollLeft+plotWidth()/2)/pps();
    scale=clamp(Number(value),1,Number(zoom.max));zoom.value=scale;tracks.style.width=(labelWidth()+plotWidth()*scale)+'px';board.scrollLeft=clamp(oldCenter*pps()-plotWidth()/2,0,plotWidth()*(scale-1));schedule();
  }
  function focusRange(a,b){const start=clamp(Math.min(a,b),0,model.duration),end=clamp(Math.max(a,b),0,model.duration);if(end-start<.5)return;setZoom(model.duration/(end-start),(start+end)/2);}
  function at(event,canvas){return clamp((event.clientX-canvas.getBoundingClientRect().left+board.scrollLeft)/pps(),0,model.duration);}
  function hover(event,row,canvas){
    const t=at(event,canvas);if(drag?.kind==='track'){drag.end=t;schedule();return;}
    hoverTime=t;schedule();
    if(!row)return;
    const nearest=row.events.reduce((best,e)=>!best||Math.abs(e.time-t)<Math.abs(best.time-t)?e:best,null);
    const span=row.spans.find(s=>s.start<=t&&s.end>=t);
    if(!span&&(!nearest||Math.abs(nearest.time-t)*pps()>9)){tooltip.hidden=true;return;}
    const lines=[row.name,span?`${time(span.start,true)} — ${time(span.end,true)} · ${(span.end-span.start).toFixed(2)} сек.`:time(nearest.time,true)];
    if(span?.inferred)lines.push('Начало ауры до доступных событий; длительность приблизительная.');
    if(nearest&&Math.abs(nearest.time-t)*pps()<=9){lines.push(names[nearest.flag]||nearest.flag,nearest.source+' → '+nearest.target);if(nearest.amount)lines.push(number(nearest.amount)+(nearest.critical?' · критический эффект':''));if(nearest.stacks)lines.push('Стаков: '+nearest.stacks);}
    tooltip.replaceChildren();lines.forEach((line,i)=>{const el=document.createElement(i===0?'strong':'div');el.textContent=line;tooltip.append(el);});
    tooltip.hidden=false;tooltip.style.left=clamp(event.clientX+14,8,innerWidth-300)+'px';tooltip.style.top=clamp(event.clientY+14,8,innerHeight-tooltip.offsetHeight-8)+'px';
  }
  function endDrag(event,canvas){
    if(!drag||drag.kind!=='track')return;const a=drag.start,b=at(event,canvas);drag=null;
    if(Math.abs(a-b)*pps()>6)focusRange(a,b);else{cursor=b;$('tl-clear-cursor').hidden=false;showEvents();schedule();}
  }
  function showEvents(){
    if(!model)return;const search=$('tl-search').value.toLocaleLowerCase().trim();
    const [viewStart,viewEnd]=windowRange();
    const list=model.events.filter(e=>active.has(e.group)&&e.name.toLocaleLowerCase().includes(search)&&(cursor===null?(e.time>=viewStart&&e.time<=viewEnd):Math.abs(e.time-cursor)<=1));
    const body=$('tl-events-body');body.replaceChildren();
    $('tl-event-time').textContent=(cursor===null?'· видимый интервал':`· ${time(cursor,true)} ± 1 сек.`)+(list.length>80?' · первые 80 из '+number(list.length):'');
    const activeAuras=cursor===null?[]:visibleRows.filter(row=>(row.group==='buffs'||row.group==='debuffs')&&row.spans.some(s=>s.start<=cursor&&s.end>cursor));
    for(const row of activeAuras){
      const span=row.spans.find(s=>s.start<=cursor&&s.end>cursor),tr=document.createElement('tr');tr.className='tl-active-aura';
      [time(cursor,true),row.group==='buffs'?'Баф активен':'Дебаф активен',row.name,model.player,`${(span.end-cursor).toFixed(1)} сек. ${span.open?'до конца записи':'до снятия'}${span.inferred?' · начало неизвестно':''}`].forEach(value=>{const td=document.createElement('td');td.textContent=value;td.title=value;tr.append(td);});body.append(tr);
    }
    for(const event of list.slice(0,80)){
      const tr=document.createElement('tr');
      [time(event.time,true),names[event.flag]||event.flag,event.name,event.source+' → '+event.target,event.amount?number(event.amount):'—'].forEach(value=>{const td=document.createElement('td');td.textContent=value;tr.append(td);});
      tr.addEventListener('click',()=>{cursor=event.time;setZoom(scale,cursor);$('tl-clear-cursor').hidden=false;showEvents();});body.append(tr);
    }
    if(!list.length&&!activeAuras.length){const tr=document.createElement('tr'),td=document.createElement('td');td.colSpan=5;td.textContent='Нет событий в этом интервале.';tr.append(td);body.append(tr);}
  }
  async function load(){
    $('tl-retry').hidden=true;$('tl-workspace').hidden=true;$('tl-status').hidden=false;
    const params=new URLSearchParams(location.search),boss=params.get('boss'),attempt=params.get('attempt');
    if(!boss||boss==='all'||!/^\d+$/.test(attempt||'')){$('tl-status').textContent='Выбери конкретную попытку босса в списке слева, чтобы посмотреть её таймлайн.';return;}
    $('tl-status').textContent='Загрузка событий боя…';
    try{
      const response=await fetch(`/reports/${encodeURIComponent(app.dataset.report)}/casts/`,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:app.dataset.player,boss,attempt:/^\d+$/.test(attempt||'')?Number(attempt):-1})});
      if(!response.ok)throw Error('HTTP '+response.status);
      model=RaidTimelineModel.build(await response.json());if(!Number.isFinite(model.duration)||model.duration<=0)throw Error('Invalid duration');
      zoom.max=Math.max(20,model.duration/5);
      document.querySelector('.tl-heading h2').className=model.className||'';
      $('tl-status').hidden=true;$('tl-workspace').hidden=false;
      $('tl-summary').textContent=`${model.player} · ${time(model.duration)} · ${number(model.events.length)} событий`;
      buildRows();
    }catch(error){$('tl-status').textContent='Не удалось загрузить таймлайн. Проверь выбранную попытку и попробуй снова.';$('tl-retry').hidden=false;}
  }
  document.querySelectorAll('[data-layer]').forEach(button=>{button.setAttribute('aria-pressed',String(active.has(button.dataset.layer)));button.addEventListener('click',()=>{const key=button.dataset.layer;active.has(key)?active.delete(key):active.add(key);button.setAttribute('aria-pressed',String(active.has(key)));localStorage.setItem('raid-timeline-layers',JSON.stringify([...active]));if(model)buildRows();});});
  $('tl-search').addEventListener('input',()=>{if(model)buildRows();});
  zoom.addEventListener('input',()=>setZoom(zoom.value));
  $('tl-zoom-in').addEventListener('click',()=>setZoom(scale*1.5));$('tl-zoom-out').addEventListener('click',()=>setZoom(scale/1.5));
  $('tl-fit').addEventListener('click',()=>{cursor=null;$('tl-clear-cursor').hidden=true;setZoom(1,model.duration/2);showEvents();});
  $('tl-clear-cursor').addEventListener('click',()=>{cursor=null;$('tl-clear-cursor').hidden=true;showEvents();schedule();});
  board.addEventListener('scroll',schedule,{passive:true});
  board.addEventListener('wheel',event=>{if(event.shiftKey){event.preventDefault();board.scrollLeft+=event.deltaY||event.deltaX;}else if(event.ctrlKey){event.preventDefault();setZoom(scale*(event.deltaY<0?1.15:1/1.15));}},{passive:false});
  board.addEventListener('keydown',event=>{if(!model)return;if(event.key==='Escape'){$('tl-fit').click();event.preventDefault();}else if(event.key==='ArrowLeft'||event.key==='ArrowRight'){event.preventDefault();cursor=clamp((cursor??windowRange()[0])+(event.key==='ArrowLeft'?-1:1),0,model.duration);$('tl-clear-cursor').hidden=false;showEvents();setZoom(scale,cursor);}});
  overview.addEventListener('pointerdown',event=>{if(!model||event.button!==0)return;overview.setPointerCapture(event.pointerId);const t=clamp((event.clientX-overview.getBoundingClientRect().left)/overview.clientWidth*model.duration,0,model.duration);drag={kind:'overview',start:t,end:t};});
  overview.addEventListener('pointermove',event=>{if(drag?.kind!=='overview')return;drag.end=clamp((event.clientX-overview.getBoundingClientRect().left)/overview.clientWidth*model.duration,0,model.duration);schedule();});
  overview.addEventListener('pointerup',()=>{if(drag?.kind!=='overview')return;const {start,end}=drag;drag=null;focusRange(start,end);schedule();});
  overview.addEventListener('pointercancel',()=>{drag=null;schedule();});
  new ResizeObserver(resize).observe(board);
  $('tl-retry').addEventListener('click',load);
  fetch(`/reports/${encodeURIComponent(app.dataset.report)}/players_classes/`,{method:'POST'}).then(r=>r.ok?r.json():{}).then(players=>{
    const select=$('tl-player');select.replaceChildren();for(const name of Object.keys(players).sort()){const option=new Option(name,name);option.selected=name===app.dataset.player;select.add(option);}if(!Object.hasOwn(players,app.dataset.player))select.add(new Option(app.dataset.player,app.dataset.player,true,true));
  }).catch(()=>{});
  $('tl-player').addEventListener('change',event=>{location.href=`/reports/${encodeURIComponent(app.dataset.report)}/casts/${encodeURIComponent(event.target.value)}/${location.search}`;});
  load();
})();
