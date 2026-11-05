"""Read the running kernel's request size instead of guessing a layout."""
import struct
def request_size(data):
 magic,version,flags,hlen,toff,tlen,soff,slen=struct.unpack_from('<HBBIIIII',data)
 if magic!=0xeb9f or version!=1 or hlen<24:raise ValueError('unsupported BTF')
 end=hlen+toff+tlen;strings=data[hlen+soff:hlen+soff+slen];pos=hlen+toff;found=[]
 if end>len(data) or len(strings)!=slen:raise ValueError('truncated BTF')
 while pos<end:
  name,info,size=struct.unpack_from('<III',data,pos);kind=(info>>24)&31;vlen=info&65535;pos+=12
  lengths={1:4,2:0,3:12,4:12*vlen,5:12*vlen,6:8*vlen,7:0,8:0,9:0,10:0,11:0,12:0,13:8*vlen,14:4,15:12*vlen,16:0,17:4,18:0,19:12*vlen}
  if kind not in lengths or name>=len(strings):raise ValueError('invalid BTF type')
  stop=strings.find(b'\0',name)
  if stop<0:raise ValueError('unterminated name')
  if kind==4 and strings[name:stop]==b'request':found.append(size)
  pos+=lengths[kind]
 if pos!=end or len(found)!=1 or not 64<=found[0]<=8192:raise ValueError('ambiguous request layout')
 return found[0]
