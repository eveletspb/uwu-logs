"""Explicit instance-lock identities and reversible report coalescing."""
import hashlib
import json
import re
import shutil
import tempfile
import time
from collections import Counter
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo

import zstd
from raid_labels import RAIDS
from raid_progress import ALIASES

MANIFEST = 'uwu-lockouts.json'
LOCKOUT_FILE = 'LOCKOUT.json'
RECORD = re.compile(r'"RAL2\|([0-9a-f|]+)"')

def validate_manifest(payload):
    try:
        manifest=json.loads(payload)
        if manifest.get('version')!=1: return []
        rows=manifest.get('records',[])
        if not isinstance(rows,list) or len(rows)>2000: return []
        valid=[]
        for row in rows:
            if not isinstance(row,dict): continue
            try:
                record={key:str(row[key]) for key in ['server','player','instance','id','high']}
                record.update({key:int(row[key]) for key in ['time','session','reset','difficulty']})
                if record['instance'] in ALIASES and record['id'] and record['reset']>record['time'] and all(len(record[k])<=150 for k in ['server','player','instance','id','high']):
                    valid.append(record)
            except (KeyError,ValueError,TypeError): continue
        return valid
    except (ValueError,TypeError,AttributeError): return []

def read_lockouts(client, realms):
    records=[]
    for path in Path(client).glob('WTF/Account/*/SavedVariables/RaidAutoLog.lua'):
        if path.stat().st_size>2_000_000: continue
        for match in RECORD.finditer(path.read_text('utf-8',errors='replace')):
            try:
                stamp,realm,player,instance,identifier,high,difficulty,reset,session=match[1].split('|')
                decode=lambda value: bytes.fromhex(value).decode('utf-8')
                realm=decode(realm)
                row=dict(time=int(stamp),server=realms.get(realm.split(':')[0],realm),
                    player=decode(player),instance=decode(instance),id=decode(identifier),high=decode(high),
                    difficulty=int(difficulty),reset=int(reset),session=int(session))
                if row['id'] and row['reset']>row['time'] and row['instance'] in ALIASES:
                    records.append(row)
            except (ValueError,UnicodeError): continue
    return records

def identity_for(report_id, bosses, started, ended, records):
    player,server=report_id.split('--')[-2:]
    raid_indices=[i for i,(_,names) in enumerate(RAIDS) if set(bosses)&names]
    if len(raid_indices)!=1: return None
    raid=raid_indices[0]
    matches=[r for r in records if r.get('player')==player and r.get('server')==server
        and ALIASES.get(r.get('instance'))==raid and r.get('session',0)<=started+120
        and started-12*3600<=r.get('time',0)<=ended+300 and r.get('reset',0)>started]
    if not matches: return None
    session=max(row['session'] for row in matches)
    matches=[row for row in matches if row['session']==session]
    # Never choose between conflicting instance IDs, even if the roster matches.
    def key(row):
        return (server,player,raid,row['id'],row['high'],row['difficulty'])
    keys={key(row) for row in matches}
    if len(keys)!=1 or max(r['reset'] for r in matches)-min(r['reset'] for r in matches)>600: return None
    row=max(matches,key=lambda r:r['time'])
    digest=hashlib.sha256(json.dumps(key(row)).encode()).hexdigest()
    return dict(key=digest,server=server,raid=raid,instance=row['instance'],
        lockout_id=row['id'],difficulty=row['difficulty'],reset=row['reset'],source='client-lockout')

def same_lockout(first,second):
    return bool(first and second and first.get('key')==second.get('key')
                and abs(first.get('reset',0)-second.get('reset',0))<=600)

def merge_payloads(payloads):
    """Union overlapping captures while retaining legitimate duplicate same-tick hits."""
    maximum=Counter(); ordered=[]
    for report_id,payload in sorted(payloads):
        counts=Counter(payload.splitlines())
        seen=Counter()
        for line in payload.splitlines():
            seen[line]+=1
            if seen[line]>maximum[line]: ordered.append((report_id,line))
        maximum |= counts
    def timestamp(item):
        report,line=item
        stamp=datetime.strptime(line.split(b',',1)[0].decode('ascii'),'%m/%d %H:%M:%S.%f')
        year=2000+int(report[:2]); month=int(report[3:5])
        if stamp.month<month-6: year+=1
        elif stamp.month>month+6: year-=1
        return stamp.replace(year=year)
    ordered.sort(key=timestamp)
    return b'\n'.join(line for _,line in ordered)

def _folder(root,name):
    path=root/name
    if path.resolve().parent!=root.resolve() or path.is_symlink() or path.is_junction():
        raise ValueError('Unsafe report path: '+name)
    if any(p.is_symlink() or p.is_junction() for p in path.rglob('*')):
        raise ValueError('Linked report contents: '+name)
    return path

def merge_reports(root, report_ids, identity=None):
    root=Path(root).resolve(); logs=root/'LogsDir'
    ids=sorted(set(report_ids)); canonical=ids[0]
    if len({tuple(name.split('--')[-2:]) for name in ids})!=1:
        raise ValueError('Reports must belong to the same author and server')
    paths=[_folder(logs,name) for name in ids]
    payload=merge_payloads([(name,zstd.decompress((path/'LOGS_CUT.zstd').read_bytes())) for name,path in zip(ids,paths)])
    if not payload: raise ValueError('Cannot merge empty reports')
    backup=root/'.raid-merge-backups'/str(time.time_ns())
    if not backup.resolve().is_relative_to(root): raise ValueError('Unsafe backup path')
    backup.mkdir(parents=True)
    for name,path in zip(ids,paths): shutil.copytree(path,backup/name)
    state_files=['_logs_list.pkl','_logs_list.bkp','_report_aliases.json']
    for name in state_files:
        if (root/name).exists(): shutil.copy2(root/name,backup/name)
    metadata=dict(identity or {},merged_from=ids,canonical=canonical)
    try:
        with tempfile.TemporaryDirectory(prefix='.raid-merge-',dir=root) as staging:
            staged=Path(staging)/canonical;staged.mkdir()
            (staged/'LOGS_CUT.zstd').write_bytes(zstd.compress(payload))
            (staged/LOCKOUT_FILE).write_text(json.dumps(metadata,indent=2),encoding='utf-8')
            for path in paths: shutil.rmtree(path)
            staged.rename(logs/canonical)
        from Z_SERVER import OPENED_LOGS
        for name in ids: OPENED_LOGS.pop(name,None)
        import pandas
        import logs_calendar
        from c_path import CachePath
        entry=logs_calendar.report_data(canonical)
        index=root/'_logs_list.pkl'
        frame=pandas.read_pickle(index) if index.exists() else pandas.DataFrame()
        frame=frame.drop(ids,errors='ignore')
        frame=pandas.concat([frame,pandas.DataFrame.from_dict({canonical:entry},orient='index')])
        temp=index.with_suffix('.merge-tmp');frame.to_pickle(temp);temp.replace(index)
        CachePath._cache.pop(index,None)
        aliases_path=root/'_report_aliases.json'
        aliases=json.loads(aliases_path.read_text('utf-8')) if aliases_path.exists() else {}
        aliases={name:(canonical if value in ids else value) for name,value in aliases.items()}
        aliases.update({name:canonical for name in ids if name!=canonical})
        aliases_path.write_text(json.dumps(aliases,indent=2),encoding='utf-8')
        return canonical,backup
    except Exception:
        for name in ids:
            path=_folder(logs,name)
            if path.exists(): shutil.rmtree(path)
            shutil.copytree(backup/name,path)
        for name in state_files:
            if (backup/name).exists(): shutil.copy2(backup/name,root/name)
            elif (root/name).exists(): (root/name).unlink()
        raise

def coalesce_import(archive,root):
    records=getattr(archive,'lockout_records',[])
    if not records or archive.has_error: return
    from logs_main import THE_LOGS
    root=Path(root)
    from Z_SERVER import OPENED_LOGS
    from auto_raid import remove_owned
    for name in archive.slices:
        OPENED_LOGS.pop(name,None)
        remove_owned(root/'parsed'/name,root/'parsed')
    groups=[]
    zone=ZoneInfo(archive.timezone)
    for name in list(archive.slices):
        report=THE_LOGS(name)
        first=datetime.strptime(report.LOGS[0].split(',')[0],'%m/%d %H:%M:%S.%f').replace(year=report.year,tzinfo=zone)
        start=first.timestamp();end=start+report.get_slice_duration(0,len(report.LOGS))
        identity=identity_for(name,report.get_enc_data(),start,end,records)
        if identity:
            (report.path/LOCKOUT_FILE).write_text(json.dumps(identity),encoding='utf-8')
            if not any(same_lockout(identity,group) for group in groups): groups.append(identity)
    for identity in groups:
        ids=[]
        for path in (root/'LogsDir').glob('*/'+LOCKOUT_FILE):
            if same_lockout(json.loads(path.read_text('utf-8')),identity): ids.append(path.parent.name)
        if len(ids)<2: continue
        canonical,_=merge_reports(root,ids,identity)
        from logs_upload import LogsSliceInfo
        report=THE_LOGS(canonical)
        info=LogsSliceInfo(players=set(report.get_players_guids().values()),bosses=list(report.get_enc_data()),
                           duration=report.get_slice_duration(0,len(report.LOGS)),id=canonical,status='Done',done=1)
        for name in ids: archive.slices.pop(name,None)
        archive.slices[canonical]=info;archive.changed=True
    # The exporter reads these sidecars, so they must name the canonical report.
    for path in (root/'uploads/0file_info').rglob('*.json'):
        data=json.loads(path.read_text('utf-8'))
        if any(Path(name.replace('\\','/')).name==archive.archive_path.name for name in data.get('archives',[])):
            slices={}
            for name,info in archive.slices.items():
                entry=dict(info.__dict__);entry['players']=sorted(entry['players']);slices[name]=entry
            data['slices']=slices
            path.write_text(json.dumps(data,indent=2),encoding='utf-8')
