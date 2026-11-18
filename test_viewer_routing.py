import unittest
from viewer_routing import report_destination

class ViewerRoutingTests(unittest.TestCase):
    def test_report_and_query_move_to_preferred_server(self):
        self.assertEqual(report_destination('http://example.test:5000/', '127.0.0.1:5000',
            '/reports/raid/casts/Player/', 'boss=ouro&attempt=1'),
            'http://example.test:5000/reports/raid/casts/Player/?boss=ouro&attempt=1')

    def test_home_opens_remote_library(self):
        self.assertEqual(report_destination('http://example.test:5000', 'localhost:5000', '/'),
                         'http://example.test:5000/logs_list')

    def test_importer_health_and_destination_do_not_redirect(self):
        for path in ['/upload','/upload_progress','/local-status','/local-health','/static/style.css']:
            self.assertIsNone(report_destination('http://example.test:5000','localhost:5000',path))
        self.assertIsNone(report_destination('http://example.test:5000','example.test:5000','/logs_list'))
        self.assertIsNone(report_destination('', 'localhost:5000','/logs_list'))
