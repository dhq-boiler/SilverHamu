import pathlib,subprocess,sys,tempfile,unittest
from unittest.mock import patch
from probe import sync_readback
ROOT=pathlib.Path(__file__).resolve().parents[2]
class Paths(unittest.TestCase):
    def test_ER_probe_name_collision_preserves_existing_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            p=pathlib.Path(tmp)/'probe';p.write_bytes(b'original')
            with self.assertRaises(FileExistsError):sync_readback(p,b'new')
            self.assertEqual(p.read_bytes(),b'original')
    def test_EQ_ER_probe_cleans_only_its_own_file_after_sync_or_failure(self):
        with tempfile.TemporaryDirectory() as tmp:
            p=pathlib.Path(tmp)/'probe'
            self.assertEqual(len(sync_readback(p,b'new')),64);self.assertFalse(p.exists())
            with patch('probe.os.fsync',side_effect=OSError('injected sync failure')):
                with self.assertRaises(OSError):sync_readback(p,b'new')
            self.assertFalse(p.exists())
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
