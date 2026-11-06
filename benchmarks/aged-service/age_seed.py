"""Age only a newly created synthetic fixture, without a final checkpoint."""
import hashlib, json, pathlib, time
from runner import Native, ROWS, key, value, verify

def main():
    root=pathlib.Path('/data');expected=root/'updated.json'
    assert json.loads(expected.read_text())=={} and not (root/'aged-seed.json').exists()
    started=time.monotonic();db=Native()
    for start in range(0,ROWS,1000):
        db.begin()
        for i in range(start,min(ROWS,start+1000)):db.put(key(i),value(i,1))
        db.commit()
    db.close();Native.shutdown()
    expected.write_text(json.dumps({str(i):1 for i in range(ROWS)}))
    result={'rows':ROWS,'updated_rows':ROWS,'batch_rows':1000,'checkpoint_after_aging':False,
            'elapsed_s':time.monotonic()-started,'verification':verify('catdb-embedded'),
            'wal_bytes':(root/'catdb.wal').stat().st_size,
            'wal_sha256':hashlib.file_digest((root/'catdb.wal').open('rb'),'sha256').hexdigest()}
    (root/'aged-seed.json').write_text(json.dumps(result,indent=2));print(json.dumps(result))

if __name__=='__main__':main()
