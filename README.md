# RaidLedger

RaidLedger is a self-hosted combat log analyzer for World of Warcraft 3.3.5a.
Import a log and browse the raid, boss attempts, and individual player
performance in your browser. The Rust server handles imports, report storage,
and exports. Reports are stored on the machine running RaidLedger.

[Quick start](#quick-start) · [Automatic import](#automatic-import-and-remote-viewing) · [Docker](#docker) · [Report storage](#report-storage-and-migration) · [Downloads](#downloads-and-json-metrics) · [Rankings](#rankings-and-pve-statistics)

## Explore a raid

- **Library and calendar.** Browse raids and find reports by boss, realm,
  author, or date. When the game server supplies a raid save snapshot, progress
  can include bosses defeated before you joined. A label such as `≥2/9` means
  the log confirms at least two of nine kills.
- **Raid and player reports.** Review kills and wipes, outgoing and incoming
  damage and healing, useful damage to priority targets, deaths, auras,
  consumables, and resources. Drill down by spell or target. Player rows show
  class colors and icons for recognized specializations when the icon pack is
  installed.
- **DPS graph.** Drag to select a time range, then choose **Apply period**
  (**Выделить период**) to filter the report tables. The selection follows the
  time axis and stays in place when you change the graph interval.
- **Player timeline.** Select a boss attempt and open a player's timeline for
  casts, buffs, debuffs, damage, and healing. Drag to zoom or click a track to
  inspect nearby events. The overview keeps its width during interaction.
  Dashed aura spans and `≈` mark estimates when the log lacks the application
  event. The cast count tooltip distinguishes attempts from recorded successes.
- **Search and comparison.** Find spells by name and compare players of the same
  class within a report.

## Quick start

Install stable Rust with Cargo and run these commands from the repository root.

```sh
cargo build --locked --release --jobs 8
./target/release/raidledger
```

Open [http://127.0.0.1:5000](http://127.0.0.1:5000). On the **Import** page
(**Импорт**), choose `WoWCombatLog.txt` from the game's `Logs/` directory or
upload a ZIP or 7z archive containing combat logs. Enter the realm and wait for
the report link. In the game, `/combatlog` toggles manual combat logging.

Archive imports require 7-Zip. Set `RAIDLEDGER_7Z` if the executable is installed
under a different name or outside your `PATH`. To display spell and
specialization icons, unpack the icon files into `static/icons/` before
starting the server or building the Docker image. [Upstream icon pack](https://drive.google.com/file/d/17DyiCJts01CkFIkd0-G1dVAypIlxd0pP)

### Windows

Build with `cargo build --locked --release --jobs 8` or place an existing
`raidledger.exe` in the repository root. Run the launcher in PowerShell.

```powershell
.\Start-RaidLedger.ps1
```

The launcher starts RaidLedger and opens the library. Use
`.\Start-RaidLedger.ps1 -NoBrowser` for a background start or set
`RAIDLEDGER_EXE` to use a different executable.

### Server configuration

| Variable | Purpose | Default |
| --- | --- | --- |
| `RAIDLEDGER_BIND` | HTTP listen address | `127.0.0.1:5000` |
| `RAIDLEDGER_ROOT` | Data directory for `LogsDir/`, `uploads/`, and `auto-config.json` | Working directory |
| `RAIDLEDGER_ASSETS` | Directory containing `templates/` and `static/` | Working directory |
| `RAIDLEDGER_7Z` | 7-Zip executable | `7z.exe` on Windows, `7zz` on macOS and Linux |

Use `GET /local-health` to check the server and `GET /local-status` to inspect
the automatic import and remote export worker.

## Automatic import and remote viewing

1. Install [RaidAutoLog](addon/RaidAutoLog) in the client's
   `Interface/AddOns/` directory alongside DBM-Core. The addon uses DBM boss
   events to start and stop logging, and records raid save details when
   available.
2. Copy [auto-config.example.json](auto-config.example.json) to
   `auto-config.json` in the RaidLedger data directory. Set `source`,
   `raw_archive`, `timezone`, and `server` for your installation. Relative
   paths resolve from the data directory. Use `realms` to map in-game realm
   names to the names used in the library. `poll_seconds` sets how often the
   worker checks the log, and `settle_seconds` is the quiet period before import.
3. Point `source` to the combat log under the client's `Logs/` directory, next
   to `WTF/`. RaidLedger reads the addon's saved variables from `WTF/`. After
   the game closes and the log finishes writing, RaidLedger imports it and
   preserves the original journal in `raw_archive`.

Set `remote_export_url` to upload completed captures to another RaidLedger
instance. Export uses 7-Zip to package the log and raid save data. RaidLedger
keeps the local report, verifies the report on the destination, and retries
pending exports after a restart.

Set `reports_url` to open a remote library by default. If it is empty, the
launcher uses `remote_export_url` when configured. Local report links preserve
their paths and encounter parameters when redirected.

## Docker

For a fresh installation, run the following from the repository root.

```sh
docker build -f deploy/rust.Dockerfile -t raidledger .
docker run -d --name raidledger --restart unless-stopped \
  -p 127.0.0.1:5000:5000 -v raidledger-data:/data raidledger
```

The image includes 7-Zip. The `raidledger-data` volume holds reports, settings,
and import data.

To migrate an existing UwU Logs deployment, use
[deploy/compose.raidledger.yaml](deploy/compose.raidledger.yaml). Back up the
existing `uwu-logs_*` volumes before the first start. If you use
`_report_aliases.json`, copy it to the root of the RaidLedger state volume.
Set `RAIDLEDGER_HOST` for LAN access and `RAIDLEDGER_PORT` to override the
default port. To load these settings from `.env`, add `--env-file .env`
after `docker compose`.

```sh
docker compose -f deploy/compose.raidledger.yaml up -d --build --wait
```

## Report storage and migration

- **One report per raid in an upload.** A large log or an archive with multiple
  log files still produces one report for each recognized raid. For example,
  AQ40 and BWL in the same upload get separate report links and downloads.
  [Encounter catalog](config/encounters.json)
- **Indexed storage.** Each new report is stored in a single
  `LogsDir/<id>.uwu` file. Independently compressed Zstandard blocks and an
  index let RaidLedger read just the blocks for an encounter. Full text
  downloads stream from storage.
- **Raid save continuity.** Captures tied to the same raid save through the
  `uwu-lockouts.json` manifest update the original report. RaidLedger
  deduplicates overlapping captures while preserving separate events recorded
  at the same timestamp.
- **Manual uploads.** Identical uploads for the same realm and year resolve to
  the existing reports. Each original upload is stored once by SHA-256 under
  `uploads/raidledger-originals/`, with a receipt linking it to each report.
- **Existing reports.** RaidLedger reads legacy report directories containing
  `LOGS_CUT.zstd` and their metadata in place. When moving an installation by
  hand, copy `LogsDir/`, `LogsRaw/`, `uploads/`, and
  `_report_aliases.json` where present. The upload metadata lets RaidLedger
  associate older fragments from the same archive.

### Maintenance commands

Run these subcommands with `./target/release/raidledger` from the repository
root. Set `RAIDLEDGER_ROOT` to use another data directory.

| Subcommand | What it does |
| --- | --- |
| `import WoWCombatLog.txt 2026 stable` | Imports a text log with the given year and realm and prints the resulting report IDs. |
| `summary REPORT_ID` | Prints the JSON summary for all recorded boss attempts. |
| `split-raids REPORT_ID` | Splits a mixed `.uwu` report by recognized raid. The first raid keeps the original URL, and the old file is backed up in `LogsDir/.raid-split-backups/`. |
| `merge REPORT_ID_1 REPORT_ID_2` | Joins reports from the same raid, author, and realm. Use it for parts of the same raid save. Old URLs resolve to the oldest report. Replaced data is backed up in `LogsDir/.raid-merge-backups/`. |
| `reindex-appearance REPORT_ID` | Updates class and specialization metadata in a `.uwu` report. Compressed events are preserved byte for byte. The previous file is backed up in `LogsDir/.raid-appearance-backups/`. |
| `reindex-difficulty [REPORT_ID ...]` | Recalculates encounter modes in native reports. With no IDs, it checks all reports. It backs up each changed file in `LogsDir/.raid-difficulty-backups/` and preserves the report ID and compressed event blocks. Legacy reports are analyzed when opened. |

Stop the server before running `merge`, `split-raids`, `reindex-appearance`,
or `reindex-difficulty`. Restart it when the command finishes.

## Downloads and JSON metrics

| Request | Result |
| --- | --- |
| `GET /reports/<id>/download?format=txt` | The full normalized UTF-8 combat log. The **Download log** (**Скачать лог**) button uses this format. |
| `GET /reports/<id>/download?format=json` | A versioned summary with encounter results, durations, raid totals, and players keyed by GUID. |
| `GET /reports/<id>/download` | The existing `.7z` archive for a standalone legacy report when available. Otherwise it streams text. |

The JSON summary covers recorded boss attempts, including wipes. Its total
duration is the sum of those attempts. Outgoing damage includes attributed
pets. Effective healing subtracts overhealing and includes attributed absorbs.
DPS, HPS, and DTPS are `null` when the duration is zero.

## Rankings and PvE statistics

Open `/top` to compare useful and total DPS across recorded kills. Filter by
realm, boss, difficulty, class, specialization, or external buffs. The **Best**
option keeps the highest-ranked result for each character. The **Points** view
uses the realm's configured progression phase. The **Speedrun** view uses reports
with a complete set of kills for the selected raid and difficulty. New captures
can supply the dominant raid guild and faction through the updated RaidAutoLog
add-on. Older logs without this metadata show an empty guild and faction.

Open `/pve_stats` to compare useful DPS percentiles by specialization. The
calculation uses the best useful DPS per character and day, with at least five
ranked results per specialization. Both pages are rebuilt from the stored combat
logs, including legacy reports, and refresh when report files change.

## Character profiles and realm providers

`/character` shows raid points by specialization and can display equipment
snapshots, talents, GearScore, and WoWSims exports from a configured gear source.
Copy [realms.example.json](realms.example.json) to `realms.json` under
`RAIDLEDGER_ROOT` (`/data` in Docker). You can also set `RAIDLEDGER_REALMS` to
the path of another config file. Restart RaidLedger after editing the file.
Use the server names that appear in your RaidLedger report IDs.

| Optional field under `servers.<realm>` | Behavior |
| --- | --- |
| `rank_api` | Full URL for a POST with `server`, `boss`, `mode`, and the `dps` and `specs` maps keyed by player name. Return a map keyed by player name with `rank`, `percentile`, `from_spec_top1`, and `total_raids_for_spec`. Omit it to compute ranks from the realm's local reports. |
| `gear_api` | Full GET URL for gear snapshots using `{server}` and `{name}`. Return a JSON object keyed by snapshot timestamp. Each snapshot contains `class`, `race`, `level`, `gear_data`, `specs`, and `profs`. |
| `gear_assets` | Base URL for gear metadata under `item/<id>.json`, `enchant/<id>.json`, and `icons/<name>.jpg`. |
| `gear_missing` | Full PUT URL for requesting missing assets. The app supplies the `type` and `id` query parameters. |
| `armory` | Character link template using `{server}` and `{name}`. The link appears when configured. |

Gear and Armory have no default external provider. Provider calls use the
platform's `curl`, included in the Docker image. The optional top-level
`ladder_websocket` specifies the live source for `/ladder`. Use a
`wss://` URL when serving the page over HTTPS.

## Private reports

Place one report ID per line in `RAIDLEDGER_ROOT/__private.txt` and restart the
server. Private reports and their aliases are excluded from public lists and
rankings. Enable password unlock over HTTPS with
`RAIDLEDGER_PRIVATE_PASSWORD_SHA256` (64 hex characters) and a separate random
`RAIDLEDGER_PRIVATE_SESSION_TOKEN` (64 hex characters). With either value
missing, the selected reports stay locked.

Set `RAIDLEDGER_REPORTS_PER_MINUTE` and `RAIDLEDGER_DOWNLOADS_PER_MINUTE` to
apply optional limits per connection address. A reverse proxy shares its
address across requests to RaidLedger.

## Development checks

```sh
cargo fmt --check
cargo test --locked --jobs 8
cargo clippy --locked --jobs 8 -- -D warnings
node test_timeline_model.cjs
```

RaidLedger is a fork of [UwU Logs](https://github.com/Ridepad/uwu-logs).
See the [deployment changelog](deploy/CHANGELOG.md) for earlier changes.
