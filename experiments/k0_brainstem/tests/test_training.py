import json,threading,time,unittest
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
import torch
from experiments.k0_brainstem.train.ppo import advantages
from experiments.k0_brainstem.eval.j72_bridge import RealJ72LanguageBackend

class Tests(unittest.TestCase):
    def test_terminal_return_no_bootstrap(self):
        reward=torch.tensor([[1.],[2.],[3.]])
        a,r=advantages(reward,torch.zeros_like(reward),1.,1.)
        torch.testing.assert_close(r,torch.tensor([[6.],[5.],[3.]]))
    def test_backend_failure_and_timeout(self):
        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                time.sleep(.2);self.send_response(503);self.end_headers()
            def log_message(self,*args):pass
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            b=RealJ72LanguageBackend(f'http://127.0.0.1:{server.server_port}',timeout=.03)
            r=b.invoke([0.]*16,{})
            self.assertEqual(r['status'],'LANGUAGE_BACKEND_UNAVAILABLE')
            self.assertLess(r['latency'],.18)
            b.timeout=1
            r=b.invoke([0.]*16,{})
            self.assertEqual(r['status'],'LANGUAGE_BACKEND_UNAVAILABLE')
            self.assertEqual(r['error_type'],'HTTPError')
        finally:server.shutdown();server.server_close()

if __name__=='__main__':unittest.main()
