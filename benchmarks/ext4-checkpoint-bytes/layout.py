"""Resolve probe field offsets from this guest's BTF before enabling tracing."""
import struct

def fields(data, wanted):
    magic,version,_,h,to,tl,so,sl=struct.unpack_from("<HBBIIIII",data)
    if (magic,version)!=(0xeb9f,1): raise ValueError("unsupported BTF")
    strings=data[h+so:h+so+sl]
    def name(i): return strings[i:strings.index(0,i)].decode()
    pos=h+to; result={}
    while pos<h+to+tl:
        n,info,size=struct.unpack_from("<III",data,pos); pos+=12
        kind=(info>>24)&31; count=info&65535
        length={1:4,2:0,3:12,4:12*count,5:12*count,6:8*count,7:0,8:0,9:0,10:0,11:0,12:0,13:8*count,14:4,15:12*count,16:0,17:4,18:0,19:12*count}[kind]
        if kind==4 and name(n) in wanted:
            for i in range(count):
                member,_,bits=struct.unpack_from("<III",data,pos+12*i)
                key=(name(n),name(member))
                if key[1] in wanted[key[0]]:
                    if bits%8 or bits>=size*8 or key in result:raise ValueError(key)
                    result[key]=bits//8
        pos+=length
    assert len(result)==sum(map(len,wanted.values()))
    return result

def bio_probe():
    from pathlib import Path
    wanted={"bio":["bi_private","bi_iter","bi_end_io"],"bvec_iter":["bi_size"],
            "ext4_io_end":["inode"],"inode":["i_ino","i_sb"],"super_block":["s_dev"],
            "ext4_inode_info":["vfs_inode","i_sync_tid"]}
    f=fields(Path("/sys/kernel/btf/vmlinux").read_bytes(),wanted)
    bio="$arg1"
    end=f"+{f['bio','bi_private']}({bio})"
    inode=f"+{f['ext4_io_end','inode']}({end})"
    sb=f"+{f['inode','i_sb']}({inode})"
    return (f"endfn=+{f['bio','bi_end_io']}({bio}):symstr bytes=+{f['bio','bi_iter']+f['bvec_iter','bi_size']}({bio}):u32 "
            f"ino=+{f['inode','i_ino']}({inode}):u64 dev=+{f['super_block','s_dev']}({sb}):u32 "
            f"tid={f['ext4_inode_info','i_sync_tid']-f['ext4_inode_info','vfs_inode']:+d}({inode}):u32")
