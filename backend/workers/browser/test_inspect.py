import importlib.util,unittest
from pathlib import Path
from urllib.parse import urlsplit
spec=importlib.util.spec_from_file_location('maestro_browser',Path(__file__).with_name('inspect.py'))
worker=importlib.util.module_from_spec(spec);spec.loader.exec_module(worker)
class DevChannelTests(unittest.TestCase):
 def test_only_same_origin_next_hmr_connects(self):
  origin=urlsplit('http://127.0.0.1:4000')
  self.assertTrue(worker.dev_socket('ws://127.0.0.1:4000/_next/hmr?id=1',origin))
  for url in ['ws://127.0.0.1:4001/_next/hmr','ws://example.com:4000/_next/hmr','ws://127.0.0.1:4000/chat','wss://127.0.0.1:4000/_next/hmr','ws://user:pass@127.0.0.1:4000/_next/hmr']:
   self.assertFalse(worker.dev_socket(url,origin),url)
 def test_only_chunk_subscriptions_are_forwarded(self):
  self.assertTrue(worker.hmr_subscription('{"type":"turbopack-subscribe","path":"static/chunks/page.js","headers":null}'))
  for message in ['{"event":"browser-logs","entries":["secret"]}','{"type":"delete","path":"static/chunks/a"}','{"type":"turbopack-subscribe","path":"static/chunks/../../secret"}','{"type":"turbopack-subscribe","path":"static/chunks/a","headers":{"Authorization":"secret"}}','[]','bad',b'binary']:
   self.assertFalse(worker.hmr_subscription(message),message)
if __name__=='__main__':unittest.main()
