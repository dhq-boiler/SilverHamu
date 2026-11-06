import pathlib,subprocess,sys,tempfile,unittest
ROOT=pathlib.Path(__file__).resolve().parents[2]
class Paths(unittest.TestCase):
    def call(self,*args):return subprocess.run([sys.executable,str(ROOT/'benchmarks/resource-envelope/run.py'),*map(str,args)],capture_output=True,text=True)
    def test_ER_existing_bind_directory_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=pathlib.Path(tmp);data=root/'existing';data.mkdir();sentinel=data/'keep';sentinel.write_text('original')
            p=self.call('--out',root/'out','--storage-mode','bind','--bind-root',data)
            self.assertNotEqual(p.returncode,0);self.assertEqual(sentinel.read_text(),'original');self.assertFalse((root/'out').exists())
    def test_BV_bind_root_and_storage_mode_must_agree(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=pathlib.Path(tmp)
            for args in [('--storage-mode','bind'),('--bind-root',root/'data'),('--storage-mode','volume','--bind-root',root/'data')]:
                p=self.call('--out',root/'out',*args);self.assertNotEqual(p.returncode,0);self.assertIn('--bind-root is required only for bind storage',p.stderr)
            self.assertFalse((root/'out').exists());self.assertFalse((root/'data').exists())
if __name__=='__main__':unittest.main()
