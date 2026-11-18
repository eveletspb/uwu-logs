import json
import unittest
from collections import Counter
from datetime import datetime, timedelta
from zoneinfo import ZoneInfo
from raid_lockouts import merge_payloads, identity_for, validate_manifest, same_lockout

class LockoutTests(unittest.TestCase):
    def row(self, **changes):
        return dict(time=1000, session=900, server='stable',player='Bombas',
            instance="Zul'Gurub",id='123',high='0',difficulty=1,reset=90000)|changes

    def test_repeated_hits_are_not_lost_and_overlap_not_double_counted(self):
        a=b'11/18 22:53:00.000,DAMAGE,100'
        b=b'11/18 23:41:00.000,DAMAGE,200'
        result=merge_payloads([('26-11-18--22-53--Bombas--stable',b'\n'.join([a,a,b])),
                               ('26-11-18--23-41--Bombas--stable',b'\n'.join([a,a,b,b]))])
        self.assertEqual(Counter(result.splitlines()),Counter({a:2,b:2}))

    def test_logical_raid_identity_uses_lockout_not_roster_or_pause(self):
        args=('26-11-18--22-53--Bombas--stable',['Hakkar'],1000,1100)
        first=identity_for(*args,[self.row()])
        later=identity_for(args[0],args[1],5000,5100,[self.row(time=5000,session=4900)])
        self.assertEqual(first['key'],later['key'])
        for changed in [dict(id='124'),dict(difficulty=2)]:
            self.assertNotEqual(first['key'],identity_for(*args,[self.row(**changed)])['key'])
        self.assertFalse(same_lockout(first,identity_for(*args,[self.row(reset=190000)])))
        self.assertTrue(same_lockout(first,identity_for(*args,[self.row(reset=90001)])))

    def test_unknown_or_wrong_character_server_never_gets_a_lockout(self):
        args=('26-11-18--22-53--Bombas--stable',['Hakkar'],1000,1100)
        for rows in [[],[self.row(player='Other')],[self.row(server='develop')]]:
            self.assertIsNone(identity_for(*args,rows))

    def test_latest_entry_session_resolves_reused_instance_context(self):
        records=[self.row(id='old',session=400,time=500),self.row(id='new')]
        result=identity_for('26-11-18--22-53--Bombas--stable',['Hakkar'],1000,1100,records)
        self.assertEqual(result['lockout_id'],'new')

    def test_malformed_manifest_is_ignored(self):
        for value in [b'not json',b'[]',b'{}',b'{"version":1,"records":[null,{}]}']:
            self.assertEqual(validate_manifest(value),[])
        self.assertEqual(len(validate_manifest(json.dumps({'version':1,'records':[self.row()]}))),1)

    def test_hour_and_day_gaps_are_kept_in_timestamp_index(self):
        from logs_get_time import Timestamps
        class Report(Timestamps):
            LOGS=['11/18 23:59:59.000,EVENT','11/19 00:00:02.000,EVENT','11/19 02:00:02.000,EVENT']
            def get_timedelta_seconds(self,a,b):
                parse=lambda v:datetime.strptime(v.split(',')[0],'%m/%d %H:%M:%S.%f')
                return (parse(b)-parse(a)).total_seconds()
        times=Report('26-11-18--23-59--Test--stable')._new_timestamps()
        self.assertEqual(times[0],0);self.assertEqual(times[3],1);self.assertEqual(times[7203],2)

    def test_separator_uses_same_lockout_when_roster_changes(self):
        from logs_upload import LogsSeparator
        from test_auto_raid import log_rows
        zone=ZoneInfo('Asia/Bangkok')
        start=datetime.now(zone)-timedelta(hours=2)
        def record(at,identifier):
            return dict(time=int(at.timestamp()),session=int(at.timestamp())-30,server='stable',player='Unknown',
                instance='Icecrown Citadel',id=identifier,high='0',difficulty=1,reset=int(start.timestamp())+3*86400)
        second=start+timedelta(minutes=20)
        first=log_rows(start).splitlines(keepends=True)
        later=log_rows(second).replace(b'0x0000000000000001',b'0x0000000000000002').splitlines(keepends=True)
        records=[record(start,'123'),record(second,'123')]
        def split(rows):
            separator=LogsSeparator(server='stable',timestamp=second.timestamp(),lockout_records=rows,timezone_name='Asia/Bangkok')
            return list(separator.generate_segments(first+later))
        self.assertEqual(len(split(records)),1)
        self.assertEqual(len(split([record(start,'123'),record(second,'124')])),2)

if __name__=='__main__': unittest.main()
