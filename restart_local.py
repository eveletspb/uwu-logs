"""Restart only this installation's verified loopback listener."""
import subprocess
import time
from pathlib import Path
import psutil

root = Path(__file__).resolve().parent
entry = str(root/'local_server.py').lower()
stopped = set()
for connection in psutil.net_connections(kind='tcp'):
    if not connection.laddr or connection.laddr.port != 5000 or connection.status != psutil.CONN_LISTEN:
        continue
    process = psutil.Process(connection.pid)
    if entry not in [arg.lower() for arg in process.cmdline()]:
        raise RuntimeError('Port 5000 is owned by another program; refusing to stop it')
    parents = []
    parent = process.parent()
    if parent and entry in [arg.lower() for arg in parent.cmdline()]:
        parents.append(parent)
    process.terminate()
    process.wait(timeout=10)
    stopped.add(process.pid)
    for parent in parents:
        try:
            parent.wait(timeout=3)
        except psutil.TimeoutExpired:
            parent.terminate()
    print('Stopped previous UwU Logs listener:', connection.pid)

with (root/'local-server.out.log').open('ab') as out, (root/'local-server.err.log').open('ab') as err:
    process = subprocess.Popen([str(root/'.venv/Scripts/pythonw.exe'), '-X', 'utf8',
                                str(root/'local_server.py')], cwd=root, stdout=out, stderr=err,
                                creationflags=subprocess.CREATE_NO_WINDOW)
(root/'local-server.pid').write_text(str(process.pid))
print('Started UwU Logs:', process.pid)
