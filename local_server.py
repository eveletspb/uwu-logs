"""Single loopback-only entry point for the upstream viewer and importer."""
import os
import zipfile
from logging.handlers import RotatingFileHandler
import logging
from pathlib import Path

os.chdir(Path(__file__).resolve().parent)
os.environ.setdefault("PYTHONUTF8", "1")

from starlette.middleware.wsgi import WSGIMiddleware
from fastapi.responses import JSONResponse
import uvicorn
from Z_SERVER import SERVER
SERVER.config["TEMPLATES_AUTO_RELOAD"] = True
import server_upload
from server_upload import app
from logs_upload import NewUpload, LogsArchive
from auto_raid import RaidWorker, JOB_LOCK
from local_library import recent_reports

@SERVER.context_processor
def local_library_context():
    return {'LOCAL_REPORTS': recent_reports()}

_archive_main = LogsArchive.main
def serialized_import(self):
    with JOB_LOCK:
        return _archive_main(self)
LogsArchive.main = serialized_import

worker = None

@app.on_event('startup')
def start_raid_maintenance():
    global worker
    log = logging.getLogger('auto_raid')
    if not log.handlers:
        handler = RotatingFileHandler('auto-import.log', maxBytes=1024*1024, backupCount=1, encoding='utf-8')
        handler.setFormatter(logging.Formatter('%(asctime)s %(levelname)s %(message)s'))
        log.addHandler(handler)
        log.setLevel(logging.INFO)
    worker = RaidWorker()
    worker.start()

@app.on_event('shutdown')
def stop_raid_maintenance():
    if worker is not None:
        worker.stop_event.set()
        worker.join(timeout=10)

@app.get('/local-status')
def local_status():
    return JSONResponse(worker.status if worker else {'state': 'not_started'})

server_upload.get_servers = lambda: ["stable", "develop"]

# Accept the game's original .txt without making the user archive it manually.
_save_uploaded_file = NewUpload.save_uploaded_file
def save_local_file(self, file_data):
    archive = _save_uploaded_file(self, file_data)
    if archive.archive_path.suffix.lower() == ".txt":
        zipped = archive.archive_path.with_suffix(".zip")
        with zipfile.ZipFile(zipped, "w", compression=zipfile.ZIP_DEFLATED) as output:
            output.write(archive.archive_path, "WoWCombatLog.txt")
        return LogsArchive(zipped, upload_data=archive.upload_data)
    return archive
NewUpload.save_uploaded_file = save_local_file

@app.get("/local-health")
def local_health():
    return JSONResponse({"app": "UwU Logs local", "local_only": True, "auto_import_version": 1})

app.mount("/", WSGIMiddleware(SERVER))

if __name__ == "__main__":
    uvicorn.run(app, host="127.0.0.1", port=5000, log_level="info")
