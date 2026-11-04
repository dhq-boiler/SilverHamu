"""Reuse original benchmark transactions; add read-only settings to metadata."""
import argparse, json, sys
sys.path.insert(0, "/bench-src")
import runner as base
p=argparse.ArgumentParser()
p.add_argument("action")
p.add_argument("engine")
p.add_argument("--workload")
p.add_argument("--clients", type=int)
p.add_argument("--rep", type=int)
a=p.parse_args()
if a.action=="init":
    base.initialize(a.engine)
    result={"initialized":base.ROWS}
elif a.action=="verify": result=base.verify(a.engine)
elif a.action=="measure": result=base.measure(a.engine,a.workload,a.clients,a.rep)
else:
    result=base.metadata(a.engine)
    if a.engine=="postgres":
        adapter=base.SQL(a.engine)
        for name in ["wal_sync_method","full_page_writes","track_wal_io_timing"]:
            adapter.cur.execute("SHOW "+name)
            result[name]=adapter.cur.fetchone()[0]
        adapter.rollback();adapter.close()
print(json.dumps(result))
