"""Report arithmetic-mean WAL seconds and median performance; keep trial-level distributions."""
import argparse,csv,gzip,json
from pathlib import Path
from statistics import mean,median
from partition import KINDS,PHASES,records,save

def read(p):
    with p.open() as f:return list(csv.DictReader(f))

def prior_overlap(raw):
    result=[]
    for f in sorted(raw.glob("*.log.gz")):
        rep,variant=f.name.split(".")[0].split("-")
        text=gzip.decompress(f.read_bytes()).decode()
        windows=records(text,"PUBLICATION");attempts=records(text,"ATTEMPT")
        syncs=sorted((a[6],a[7]) for a in attempts if a[-1])
        total=sum(b-a for a,b in syncs)
        row=dict(mode="standard" if variant=="off" else "fifo",rep=int(rep))
        for name,i,j in zip(PHASES[:-1],(1,3,4,5,6,10),(2,4,5,6,10,11)):
            row[name]=sum(max(0,min(b,w[j])-max(a,w[i])) for a,b in syncs for w in windows)/1e9
        row["outside"]=total/1e9-sum(row[k] for k in PHASES[:-1]);row["total"]=total/1e9
        result.append(row)
    assert len(result)==6
    return result

def stat(rows,mode,key,fn=mean):
    return fn(float(r[key]) for r in rows if r["mode"]==mode)

def bar_chart(path,title,sub,metrics):
    svg=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 970 375" role="img"><title>'+title+'</title><rect width="970" height="375" fill="white"/><g font-family="sans-serif" fill="#203247">']
    def label(x,y,s,z=14):svg.append(f'<text x="{x}" y="{y}" font-size="{z}">{s}</text>')
    label(16,32,title,23);label(16,57,sub)
    for i,(name,values) in enumerate(metrics):
        x,y=16+480*(i%2),95+132*(i//2);label(x,y,name,17)
        for j,(policy,v,color) in enumerate(zip(("Standard","FIFO"),values,("#536e83","#237669"))):
            yy=y+30+32*j;width=225*v/max(values) if max(values)>0 else 0
            label(x,yy,policy);svg.append(f'<rect x="{x+75}" y="{yy-13}" width="{width:.2f}" height="15" fill="{color}"/>');label(x+81+width,yy,f"{v:,.3f}" if max(values)<1 else f"{v:,.2f}")
    path.write_text("\n".join(svg+["</g></svg>",""]),encoding="utf8")

def main():
    p=argparse.ArgumentParser()
    for arg in ("out","pilot","previous"):p.add_argument(arg,type=Path)
    args=p.parse_args();out=args.out;r=read(out/"trials.csv");cross=read(out/"cross.csv");old=prior_overlap(args.previous)
    save(out/"previous-phase.csv",old)
    metrics=[("Update p99 (ms)","p99_ms",median),("Worst update (ms)","max_ms",max),("Updates/s to last response","updates_s",median),("Updates/s including drain","drained_s",median)]
    bar_chart(out/"performance.svg","63 independent checkpoints with guest tracing","1M rows, four writers, two CPUs, 4 GiB. Three trial medians except the maximum across all trials.",[(label,[stat(r,m,k,fn) for m in ("standard","fifo")]) for label,k,fn in metrics])
    bar_chart(out/"waits.svg","Where the writers waited inside WAL synchronization","Seconds per 16,384-update run, mean of three trials. Each panel has its own scale.",[(label,[stat(r,m,k) for m in ("standard","fifo")]) for label,k in (("Journal commit sleep","journal_s"),("File writeback sleep","writeback_s"),("Wakeup to switch-in","wake_wait_s"),("Scheduled on guest CPU","scheduled_s"))])
    matrix=[]
    for phase in PHASES:
        x=dict(phase=phase)
        for m in ("standard","fifo"):
            filtered=[q for q in cross if q["mode"]==m and q["phase"]==phase]
            for k in KINDS:x[m+"_"+k]=sum(int(q["ns"]) for q in filtered if q["kind"]==k)/3e9
            x[m+"_total"]=sum(x[m+"_"+k] for k in KINDS)
        matrix.append(x)
    save(out/"phase-means.csv",matrix)
    deltas=[(row["phase"],stat(old,"fifo",row["phase"])-stat(old,"standard",row["phase"]),
             row["fifo_total"]-row["standard_total"]) for row in matrix]
    peak=max(abs(x) for _,v,w in deltas for x in (v,w)) or 1
    svg=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 960 480" role="img"><title>Shift in WAL sync time by checkpoint phase</title><rect width="960" height="480" fill="white"/><g font-family="sans-serif" fill="#203247">']
    def txt(x,y,content,size=14):
        svg.append(f'<text x="{x}" y="{y}" font-size="{size}">{content}</text>')
    txt(17,30,"Checkpoint overlap of WAL sync time",23)
    txt(17,55,"FIFO minus standard, mean seconds per trial. Left of zero means less overlap with FIFO.")
    for i,(name,color) in enumerate([("Previous application timings","#526f87"),("Current kernel-traced trials","#237669")]):
        x=18+i*300;svg.append(f'<rect x="{x}" y="73" width="16" height="13" fill="{color}"/>');txt(x+22,85,name)
    center=540
    svg.append(f'<line x1="{center}" y1="97" x2="{center}" y2="455" stroke="#a2acb1"/>')
    for i,((phase,v,w),label) in enumerate(zip(deltas,("Prefix preparation","Encode and write snapshot","Sync snapshot","Before cutover lock","Cutover lock held","After unlock","Outside checkpoints"))):
        y=116+i*49;txt(18,y+7,label)
        for j,(val,color) in enumerate([(v,"#526f87"),(w,"#237669")]):
            width=270*abs(val)/peak;x=center-width if val<0 else center;yy=y+j*15
            svg.append(f'<rect x="{x:.2f}" y="{yy-10}" width="{width:.2f}" height="10" fill="{color}"/>')
            txt(round(x-61 if val<0 else x+width+6),yy,f"{val:+.2f}",12)
    (out/"overlap.svg").write_text("\n".join(svg+["</g></svg>",""]),encoding="utf8")
    for m in ("standard","fifo"):
        for k in KINDS:assert abs(sum(x[m+"_"+k] for x in matrix)-stat(r,m,k+"_s"))<1e-7
    stages=[]
    for m in ("standard","fifo"):
        for phase in PHASES:
            count=total=0
            for f in (out/"matches").glob("*-"+m+"-trace.csv.gz"):
                with gzip.open(f,"rt") as stream:
                    for s in csv.DictReader(stream):
                        if s["start_phase"]==phase:count+=1;total+=int(s["end_ns"])-int(s["start_ns"])
            stages.append(dict(mode=m,start_phase=phase,mean_calls=count/3,mean_call_ms=total/count/1e6 if count else 0))
    save(out/"start-phases.csv",stages)
    pilot=read(args.pilot/"trials.csv")
    overhead=[]
    for m in ("standard","fifo"):
        a,b=([i for i in pilot if i["mode"]==m and i["observer"]==f] for f in ("plain","trace"))
        for k in ("updates_s","drained_s","p99_ms","max_ms"):
            x,y=(median(float(q[k]) for q in group) for group in (a,b))
            overhead.append(dict(mode=m,metric=k,plain=x,trace=y,change_pct=100*(y/x-1)))
    save(out/"pilot-overhead.csv",overhead)
if __name__=="__main__":main()
