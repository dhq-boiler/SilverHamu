"""Select only run-unique threads even when a trace prefix lacks the cached name."""
import re
PREFIX=re.compile(r'^\s*(.+)-(\d+)\s+\[')
def target_ids(line,tag):
    ids=set();match=PREFIX.match(line)
    if match and match[1].strip()==tag:ids.add(int(match[2]))
    ids.update(int(x) for x in re.findall(r'(?:prev_comm|next_comm|comm)='+re.escape(tag)+r' (?:prev_pid|next_pid|pid)=(\d+)',line))
    return ids

def exportable(line,tids):
    if line.startswith('#'):return True
    match=PREFIX.match(line)
    if match and int(match[2]) in tids:return True
    if 'sched_switch:' in line or 'sched_wakeup:' in line:
        return any(int(x) in tids for x in re.findall(r'(?:prev_pid|next_pid|pid)=(\d+)',line))
    return False
