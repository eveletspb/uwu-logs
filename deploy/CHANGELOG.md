## 2026-11-18

- deploy/Dockerfile, deploy/compose.yaml — развёртывание uwu-logs 56d7251 в OrbStack на LAN-адресе Mac. Отчёты и данные импорта хранятся в Docker volumes.

- logs_upload.py — промежуточный Zstandard-файл создаётся на томе отчётов, чтобы атомарная запись работала при отдельных томах OrbStack.

- 2026-11-18 — logs_check_difficulty.py: смерть Сартуры и Висцидуса распознаётся внутри сегмента, даже если после неё записано снятие аур.

- 2026-11-19 — raid_fragments.py, local_library.py, logs_calendar.py, Z_SERVER.py: фрагменты рейда из одного архива показываются одним отчётом с общим прогрессом и боями.
