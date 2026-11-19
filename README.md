# RaidLedger

## Updated design

A local raid analytics interface for WoW 3.3.5a with encounter navigation, metric tabs, player search and a visible DPS graph: a raid library, damage and
healing summaries, readable player tables, and a simple combat-log import flow.

### Raid library

Recent reports show the raid name, author, server, start time and player count.

![Raid library with raid names and recent reports](showcase/redesign/raid-library.jpg)

### Raid report

Damage, effective healing and participants at a glance, with controls for
switching damage and healing views and opening individual player details.

![Raid report with damage and healing summaries and player rankings](showcase/redesign/raid-report.jpg)

### Player timeline

The timeline separates casts, buffs, debuffs, outgoing damage and healing into
switchable tracks. Select a range in the DPS overview or drag across a track
to zoom in; use the horizontal scrollbar or Shift + wheel to move through time.
Spell labels and the time ruler stay visible while scrolling.

![Player timeline with separate cast tracks and a damage overview](showcase/redesign/timeline-overview.jpg)

Search for an ability, hover events for details, or click to pin a shared time
cursor. The event table shows nearby casts/hits and buffs active at that time.
Arrow keys move the cursor; Escape or **Весь бой** restores the full encounter.
Buffs whose application is absent from the log have dashed spans and an
approximate uptime marker. Auras with no recorded events cannot be recovered.

![Zoomed buff timeline with active aura and nearby events](showcase/redesign/timeline-buffs.jpg)

### Combat-log import

Automatic recording uses DBM boss encounters. Import and 30-day cleanup run
after WoW closes; manual import is also available.

![Local combat-log import page](showcase/redesign/log-import.jpg)

---

<https://uwu-logs.xyz/>

RaidLedger is a Rust combat-log parser and raid viewer for World of Warcraft 3.3.5a, originally forked from UwU Logs.

Supports any Wrath of the Lich King (3.3.5) server.

❤️ Inspired by World of Logs, LegacyPlayers and Warcraft Logs.

## Run RaidLedger

Build with `cargo build --locked --release --jobs 8` and launch from the
repository directory with `target/release/raidledger`. The server listens on
`127.0.0.1:5000` by default. `RAIDLEDGER_ROOT` selects the data directory,
`RAIDLEDGER_BIND` sets the listen address, and `RAIDLEDGER_7Z` selects the
7-Zip executable for archive imports.

A single raid produces one `LogsDir/<report-id>.uwu` file regardless of
upload size or the number of log files in the archive. If an upload spans
different recognized raids, each raid gets its own report and download.
Reports use independently compressed Zstandard blocks and an index for
reading encounter ranges and streaming text downloads.

To split an existing mixed report, run `raidledger split-raids <report-id>`.
The first raid keeps the existing link and the original indexed file is
preserved in `LogsDir/.raid-split-backups`.

When separate uploads identify the same raid save in their
`uwu-lockouts.json` manifests, the server updates the first report in
place. Overlapping combat events are deduplicated with their same-tick
counts preserved. The previous version is saved in
`LogsDir/.raid-merge-backups` before the replacement.

The server also opens existing `LogsDir/<report-id>/LOGS_CUT.zstd` reports
and their metadata in place. Keep `uploads/0file_info` and
`_report_aliases.json` alongside the report directory when copying a
legacy installation. The archive metadata identifies older report
fragments that belong together.

### Local Windows installation

The native launcher targets Windows 10 version 1903 and later.

Place the project next to the game's `Logs` and `WTF` directories. Copy
`auto-config.example.json` to `auto-config.json`. Set the log location,
timezone and realm mapping as needed. Build for Windows with
`cargo build --locked --release --jobs 8` or put an existing
`raidledger.exe` next to `Start-RaidLedger.ps1`, then run the script from
PowerShell. The previous `Start-UwU-Logs.ps1` script forwards to the new
launcher. Use `-NoBrowser` for a background start.

Install `7z.exe` or set `RAIDLEDGER_7Z` for archive imports and remote
export. Copy `addon/RaidAutoLog` into WoW's `Interface/AddOns` directory.
It uses DBM-Core callbacks to record raid encounters. RaidLedger waits
for WoW to exit and the journal to stop changing before importing.
The original raw journal is retained in the configured `raw_archive`
folder, and interrupted imports resume from the pending capture.

### Docker

`deploy/rust.Dockerfile` builds the standalone server. The migration
configuration `deploy/compose.raidledger.yaml` reuses the existing UwU Logs
volumes for reports, raw archives, uploads, auxiliary databases and game logs.
For LAN access, put `RAIDLEDGER_HOST` in `.env` and pass that file explicitly
when starting Compose from the repository root:

```sh
docker compose --env-file .env -f deploy/compose.raidledger.yaml up -d --build --wait
```

Back up the original volumes and separately capture `_report_aliases.json`
if present. Copy that file into the RaidLedger state volume before startup.

The optional icon pack is available from [the original project](https://drive.google.com/file/d/17DyiCJts01CkFIkd0-G1dVAypIlxd0pP).

## Showcase

### Top

<https://uwu-logs.xyz/top>

![Showcase top](https://raw.githubusercontent.com/Ridepad/uwu-logs/main/static/thumb.png)

### PvE Statistics

<https://uwu-logs.xyz/top_stats>

![Showcase PvE statistics](https://raw.githubusercontent.com/Ridepad/uwu-logs/main/showcase/pve_stats.png)

### Player total and per target spell info

![Showcase player spell info](https://raw.githubusercontent.com/Ridepad/uwu-logs/main/showcase/spell_info.png)

### Damage to targets + useful

![Showcase useful](https://raw.githubusercontent.com/Ridepad/uwu-logs/main/showcase/useful.png)

### Player comparison

![Showcase comparison](https://raw.githubusercontent.com/Ridepad/uwu-logs/main/showcase/compare.png)

### Spell search and overall info

![Showcase spell search](https://raw.githubusercontent.com/Ridepad/uwu-logs/main/showcase/spells.png)

### Consumables

![Showcase consumables](https://raw.githubusercontent.com/Ridepad/uwu-logs/main/showcase/consume.png)

## TODO

- friendly fire: Bloodbolt Splash, ucm, vortex
- self harm: ucm, Chilled to the Bone
- site side logs parser - filter forms - guid spell etc

- portal stacks
- valk grabs + necrotic + defile targets

- fix unlogical buff duration max 30 sec? last combat log entry? filter out long spells - ff hmark
- if buff not in long_buffs check top50% avg of this buff

- add summary stats like max hit done max hit taken max absorb max grabs
- 1 tick total - all targets dmg from 1 hurricane tick or typhoon

## Raid progress

The raid library shows boss progress and a readable start date. RaidAutoLog
records instance progress from the server lock prompt (and extended saved-raid
API when the server supports it), including bosses killed before you joined.
Its saved variables are read locally after logout. Reload the client after
installing or updating the addon.

A partial combat log cannot reveal earlier boss kills. Reports without an
instance snapshot show a lower bound such as `≥2/9` with a “from log” hint;
this is not presented as the complete lockout progress. Servers that do not
provide encounter progress retain this lower-bound display.

## Download a report log

Use **Скачать лог** in the report menu, or request
`GET /reports/<report_id>/download?format=txt`. The response is a UTF-8
attachment containing the report's full normalized combat log, including
all fragments of a combined report. The download covers the full report
regardless of the encounter selected in the viewer.

Omit `format` to receive the existing `.7z` archive for a standalone report
when available. Otherwise the endpoint returns the text log.
Request `GET /reports/<report_id>/download?format=json` for the versioned
JSON summary. It covers all recorded boss attempts, including wipes, and
excludes trash and time between attempts. Each encounter has its boss,
difficulty, zero-based attempt, outcome, duration, viewer path, raid totals
and players keyed by GUID. The top-level totals and players aggregate those
attempts. `damage` is outgoing damage including attributed pets, `healing`
is effective healing including attributed absorbs, `damage_taken` counts
damage received by the character, and `deaths` counts player `UNIT_DIED`
events. DPS, HPS and DTPS use the full attempt duration or the sum of all
attempt durations for the report. Rates are null for zero duration. The response is a single JSON document.

An unknown report returns HTTP 404. An unsupported format returns HTTP 400.

## Automatic remote export

Set `remote_export_url` in the private `auto-config.json` to a RaidLedger server
URL. Completed captures are uploaded after WoW closes. The original server label and timezone are preserved. Reports are
checked on the destination before an archive is marked as exported.

Failed exports remain queued and retry after a restart. Original reports,
raw archives and export checkpoints are retained on disk.

Set `reports_url` to open reports on that server by default. The launcher opens
its raid library, and local home/library/report URLs redirect there while
preserving the report path and encounter query. Upload and maintenance endpoints
continue running locally. When `reports_url` is empty, `remote_export_url` is
used as the preferred viewer if configured.

## One report per raid lockout

Update `addon/RaidAutoLog/Progress.lua` in the game client and reload the UI.
The addon records the actual instance save ID and difficulty using the classic
saved-instance API. Automatic archives carry a small `uwu-lockouts.json`
manifest. The client and destination server both need this version of the code.

When the save ID matches, pauses, roster changes and client restarts append
to the same report. Different save IDs, difficulties or reset periods remain
separate. Without a reliable save ID, the importer keeps its legacy separation
instead of guessing that two runs share a lockout. Overlapping captures are
deduplicated without removing legitimate identical hits in the same timestamp.

To combine existing reports known to share a lockout, stop the viewer and run
the following **inside the project folder**, then restart the viewer:

```sh
./target/release/raidledger merge REPORT_ID_1 REPORT_ID_2 REPORT_ID_3
```

The oldest report ID becomes canonical. The original fragment files are
retained and the other IDs continue opening the combined report. When a
native report is replaced, its previous version is linked in
`LogsDir/.raid-merge-backups`. The report defaults to boss segments so
overnight pauses do not dilute DPS/HPS.
