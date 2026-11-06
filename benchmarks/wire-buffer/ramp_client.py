import argparse,importlib.util,json,pathlib,runpy,sys
ROOT=pathlib.Path(__file__).resolve().parents[1]
s=importlib.util.spec_from_file_location('resource_base',ROOT/'resource-envelope/client.py');base=importlib.util.module_from_spec(s);s.loader.exec_module(base)
def stop_reason(previous,current):
    if current['errors']:return 'operation_error'
    if current['queue_rejected']:return 'queue_refusal'
    if previous is not None and current['p99_ms']>previous*2:return 'p99_more_than_doubled'
    return None

def main():
    if len(sys.argv)<2 or sys.argv[1]!='measure':runpy.run_path(str(ROOT/'resource-envelope/client.py'),run_name='__main__');return
    p=argparse.ArgumentParser();p.add_argument('action');p.add_argument('--work');p.add_argument('--ops');p.add_argument('--rate');p.add_argument('--pin-reader',type=int);a=p.parse_args()
    trials=[];previous=None;reason=None
    for rate in [1000,2000,4000,8000]:
        m=base.measure('read',rate*10,rate,64,bool(a.pin_reader));records=m['records'];m['producer_p99_ms']=base.quantile([x['enqueued']-x['planned'] for x in records],.99)
        m['client_queue_p99_ms']=base.quantile([x['began']-x['enqueued'] for x in records if x['status']=='ok'],.99)
        m['call_p99_ms']=base.quantile([x['ended']-x['began'] for x in records if x['status']=='ok'],.99)
        last=max(x['enqueued'] for x in records);m['generation_span_s']=(last-m['start_ns'])/1e9
        m['producer_missed_rate']=m['generation_span_s']>10/.95
        reason=stop_reason(previous,m);trials.append(m);previous=m['p99_ms']
        if reason:break
    result={'work':'ramp','trials':trials,'stop_reason':reason,'errors':[e for m in trials for e in m['errors']]}
    (base.DATA/'ramp-raw.json').write_text(json.dumps(result))
    for m in trials:m.pop('records')
    print(json.dumps(result))
if __name__=='__main__':main()
