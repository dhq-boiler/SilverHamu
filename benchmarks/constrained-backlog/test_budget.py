"""A staged-budget rejection must leave prior transactions and accepted data intact."""
import importlib.util,pathlib,unittest
ROOT=pathlib.Path(__file__).resolve().parents[2]
s=importlib.util.spec_from_file_location('batch_protocol',ROOT/'benchmarks/backlog/test_protocol.py');base=importlib.util.module_from_spec(s);s.loader.exec_module(base)
class BudgetTests(base.BacklogTests):
    def test_ER_global_staged_limit_rejects_whole_unsubmitted_batch_and_recovers(self):
        self.stop();self.start('--max-staged-bytes','4096')
        held=[]
        for i in range(10):
            c=self.client();self.ok(c,op='begin');self.ok(c,op='put',key=(100+i).to_bytes(8,'big').hex(),value=('ab'*256));held.append(c)
        c=self.client();self.ok(c,op='begin')
        self.assertEqual(c.request(op='put',key=(200).to_bytes(8,'big').hex(),value='cd'*256),{'ok':False,'error':'resource_exhausted'})
        self.ok(c,op='rollback')
        for c in held[3:]:self.ok(c,op='rollback')
        entries=[dict(key=i.to_bytes(8,'big').hex(),value='ef'*256) for i in range(8)]
        writer=self.client();self.assertEqual(writer.request(op='put_batch_commit',entries=entries),{'ok':False,'error':'resource_exhausted'})
        for e in entries:self.assertIsNone(self.ok(writer,op='get_latest',key=e['key'])['value'])
        for i,c in enumerate(held[:3]):
            self.assertEqual(self.ok(c,op='get',key=(100+i).to_bytes(8,'big').hex())['value'],'ab'*256);self.ok(c,op='rollback')
        result=self.ok(writer,op='put_batch_commit',entries=entries);self.assertTrue(all(x['ok'] for x in result['results']))
        self.stop();self.start('--max-staged-bytes','4096');reader=self.client()
        for e in entries:self.assertEqual(self.ok(reader,op='get_latest',key=e['key'])['value'],e['value'])
if __name__=='__main__':unittest.main()
