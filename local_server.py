"""Single loopback-only entry point for the upstream viewer and importer."""
import os
import zipfile
from logging.handlers import RotatingFileHandler
import logging
import json
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
from viewer_routing import report_destination
from flask import request as flask_request, redirect

_settings_path = Path('auto-config.json')
_settings = json.loads(_settings_path.read_text('utf-8')) if _settings_path.exists() else {}
SERVER.config['REPORTS_URL'] = _settings.get('reports_url') or _settings.get('remote_export_url', '')

@SERVER.before_request
def preferred_report_viewer():
    if flask_request.method not in ('GET', 'HEAD'):
        return None
    destination = report_destination(SERVER.config.get('REPORTS_URL'), flask_request.host,
                                     flask_request.path, flask_request.query_string.decode('utf-8'))
    if destination:
        return redirect(destination, code=302)
    if flask_request.path.startswith('/reports/'):
        aliases_path = Path('_report_aliases.json')
        aliases = json.loads(aliases_path.read_text('utf-8')) if aliases_path.exists() else {}
        parts = flask_request.path.split('/')
        if parts[2] in aliases:
            parts[2] = aliases[parts[2]]
            target = '/'.join(parts)
            if flask_request.query_string:
                target += '?' + flask_request.query_string.decode('utf-8')
            return redirect(target, code=302)

# Redirect old IDs before the upstream missing-report guard returns 404.
SERVER.before_request_funcs[None].remove(preferred_report_viewer)
SERVER.before_request_funcs[None].insert(0, preferred_report_viewer)

@SERVER.context_processor
def local_library_context():
    return {'LOCAL_REPORTS': recent_reports()}

_archive_main = LogsArchive.main
def serialized_import(self):
    with JOB_LOCK:
        result = _archive_main(self)
        from raid_lockouts import coalesce_import
        from c_path import Directories
        try:
            coalesce_import(self, Directories.main)
        except Exception:
            logging.getLogger('auto_raid').exception('Lockout report merge failed')
            self.has_error = 'Error merging lockout reports; original data retained'
            self.change_main_status(self.has_error, all_done=0)
            return result
        if worker is not None and not self.has_error:
            worker.index_reports(list(self.slices))
        return result
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
