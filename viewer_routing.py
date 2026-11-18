"""Choose the configured report viewer without redirecting importer endpoints."""
from urllib.parse import urlsplit

def report_destination(base, host, path, query=''):
    base = str(base or '').rstrip('/')
    parsed = urlsplit(base)
    if parsed.scheme not in ('http', 'https') or not parsed.netloc:
        return None
    if parsed.netloc.casefold() == host.casefold():
        return None
    if path not in ('/', '/logs_list') and not path.startswith('/reports/'):
        return None
    target = '/logs_list' if path == '/' else path
    return base + target + ('?'+query if query else '')
