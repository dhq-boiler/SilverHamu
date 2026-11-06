"""Verify that only the root default marker differs in resolution."""
import argparse,json,pathlib

def compare(a,b):
    def nodes(m):
        root=m['resolve']['root']
        return {n['id']:sorted(f for f in n['features'] if not(n['id']==root and f=='default')) for n in m['resolve']['nodes']}
    assert a['resolve']['root']==b['resolve']['root'],'different root package'
    assert nodes(a)==nodes(b),'resolved dependency features differ beyond root default'
    def packages(m):return sorted((p['id'],p['name'],p['version'],p.get('source')) for p in m['packages'])
    assert packages(a)==packages(b),'package identities differ'
    return {'normalized_features_equal':True,'packages_equal':True,
        'explicit_root_features':next(n['features'] for n in a['resolve']['nodes'] if n['id']==a['resolve']['root']),
        'default_root_features':next(n['features'] for n in b['resolve']['nodes'] if n['id']==b['resolve']['root'])}

def main():
    p=argparse.ArgumentParser();p.add_argument('builds',type=pathlib.Path);a=p.parse_args();r=a.builds
    result=compare(*[json.loads((r/v/'metadata.json').read_text()) for v in ['explicit','default']])
    for name in ['rustc.txt','cargo.txt']:
        assert (r/'explicit'/name).read_bytes()==(r/'default'/name).read_bytes(),name
    result['toolchains_equal']=True
    result['binaries']={v:(r/v/'binaries.sha256').read_text().splitlines() for v in ['explicit','default']}
    result['text']={v:(r/v/'text.sha256').read_text().split()[0] for v in ['explicit','default']}
    (r/'comparison.json').write_text(json.dumps(result,indent=2));print(json.dumps(result,indent=2))
if __name__=='__main__':main()
