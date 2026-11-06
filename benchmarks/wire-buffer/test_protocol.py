import importlib.util,json,pathlib,socket,struct,unittest
ROOT=pathlib.Path(__file__).resolve().parents[2]
s=importlib.util.spec_from_file_location('protocol',ROOT/'tests/service_protocol.py');base=importlib.util.module_from_spec(s);s.loader.exec_module(base)
class WireTests(base.ServiceTests):
 def test_ST_large_then_small_responses_preserve_values(self):
  c=self.client()
  for n in [4095,4096,4097,65536]:
   value='ab'*n;self.ok(c,op='put_commit',key='01',value=value)
   self.assertEqual(self.ok(c,op='get_latest',key='01')['value'],value)
   self.assertTrue(self.ok(c,op='ping')['ok'])
   self.ok(c,op='put_commit',key='01',value='ff')
   self.assertEqual(self.ok(c,op='get_latest',key='01')['value'],'ff')
  self.stop();self.start();self.assertEqual(self.ok(self.client(),op='get_latest',key='01')['value'],'ff')
 def test_EQ_fragmented_auth_and_coalesced_requests_stay_ordered(self):
  def frame(x):
   b=json.dumps(x).encode();return struct.pack('>I',len(b))+b
  def exact(sock,n):
   data=b''
   while len(data)<n:
    b=sock.recv(n-len(data));self.assertTrue(b);data+=b
   return data
  def receive(sock):return json.loads(exact(sock,struct.unpack('>I',exact(sock,4))[0]))
  with socket.create_connection(('127.0.0.1',self.port),timeout=3) as c:
   b=frame({'op':'auth','version':1,'token':self.token})
   for i in range(0,len(b),3):c.sendall(b[i:i+3])
   self.assertTrue(receive(c)['ok'])
   c.sendall(frame({'op':'ping'})+frame({'op':'get_latest','key':'01'})+frame({'op':'ping'}))
   first=receive(c);middle=receive(c);last=receive(c)
   self.assertTrue(first['ok']);self.assertIsNone(middle['value']);self.assertEqual(first,last)
if __name__=='__main__':unittest.main()
