"""Run with the viewer stopped. Originals are retained in .raid-merge-backups."""
import argparse
import json
import os
from pathlib import Path
from raid_lockouts import merge_reports

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reports',nargs='+')
    args=parser.parse_args()
    root=Path(__file__).resolve().parent
    os.chdir(root)
    aliases_path=root/'_report_aliases.json'
    aliases=json.loads(aliases_path.read_text('utf-8')) if aliases_path.exists() else {}
    reports=sorted({aliases.get(name,name) for name in args.reports})
    if len(reports)==1 and len(args.reports)>1:
        print('Already merged:',reports[0])
        raise SystemExit(0)
    canonical,backup=merge_reports(root,reports,
                                  {'source':'manual-same-lockout'})
    print('Merged report:',canonical)
    print('Originals retained:',backup)
