## 2026-11-18

- deploy/Dockerfile, deploy/compose.yaml — развёртывание uwu-logs 56d7251 в OrbStack на LAN-адресе Mac. Отчёты и данные импорта хранятся в Docker volumes.

- logs_upload.py — промежуточный Zstandard-файл создаётся на томе отчётов, чтобы атомарная запись работала при отдельных томах OrbStack.
