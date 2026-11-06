"""Keep start-up bursts and identical-image controls separate from causation."""
import importlib.util,json,pathlib,sys
ROOT=pathlib.Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('aged_default_tail',ROOT/'benchmarks/aged-service/analyze.py')
aged=importlib.util.module_from_spec(spec);spec.loader.exec_module(aged)

def tail(records,start):
    assert len({x['seq'] for x in records})==len(records),'duplicate request'
    out={}
    for kind in ['read','write']:
        requested=[x for x in records if x['kind']==kind]
        ok=[x for x in requested if x['status']=='ok']
        assert all(start<=x['planned']<=x['enqueued']<=x['began']<=x['ended'] for x in ok),'reversed request clock'
        samples=[(x['ended']-x['planned'])/1e6 for x in ok]
        out[kind]={'requested':len(requested),'completed':len(ok),
            'at_least_ms':{str(t):sum(x>=t for x in samples) for t in [10,20,50]},
            'first_second_ms':aged.stats([(x['ended']-x['planned'])/1e6 for x in ok if x['planned']<start+10**9]),
            'later_ms':aged.stats([(x['ended']-x['planned'])/1e6 for x in ok if x['planned']>=start+10**9]),
            'before_call_ms':aged.stats([(x['began']-x['planned'])/1e6 for x in ok]),
            'call_ms':aged.stats([(x['ended']-x['began'])/1e6 for x in ok])}
    return out

def summarize(root):
    result=aged.summarize(root/'raw',pin_reader=True)
    assert len(result['trials'])==10 and result['requests']==160000
    assert result['full_checks']==50 and result['seed_full_checks']==10
    ids=json.loads((root/'raw/image-ids.json').read_text())
    assert ids['same-a']==ids['same-b']==ids['default'],'control is not identical'
    for trial in result['trials']:
        f=root/'raw'/trial['label'];m=json.loads((f/'results.json').read_text())[0]
        env=json.loads((f/'environment.json').read_text())
        assert env['image']==ids[trial['variant']],'image changed during comparison'
        trial['tails']={}
        for work in ['read','mixed']:
            raw=json.loads(aged.data(f/m['name']/(work+'-raw.json')))
            trial['tails'][work]=tail(raw['records'],raw['start_ns'])
    return result

if __name__=='__main__':
    root=pathlib.Path(sys.argv[1]);r=summarize(root);(root/'summary.json').write_text(json.dumps(r,indent=2))
