#define _GNU_SOURCE
#define _LARGEFILE64_SOURCE
#include <dlfcn.h>
#include <unistd.h>
#include <time.h>
#include <errno.h>
#include <stdint.h>
#include <stdatomic.h>
#include <sys/types.h>

/* Fixed storage prevents the observer from allocating during measured calls. */
#define CAP 200000
#define FDS 4096
typedef struct { int64_t req,kind,fd,offset,bytes,result,error; uint64_t start,end; } Event;
static Event events[CAP];
static _Atomic unsigned used,overflow,closed;
static _Atomic int request=-1;
static int tracked[FDS];
static int (*real_fsync)(int),(*real_fdatasync)(int),(*real_close)(int);
static ssize_t (*real_write)(int,const void*,size_t),(*real_pwrite)(int,const void*,size_t,off_t),(*real_pwrite64)(int,const void*,size_t,off64_t);
static int (*real_ftruncate)(int,off_t),(*real_ftruncate64)(int,off64_t);
static uint64_t now(void){struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return (uint64_t)t.tv_sec*1000000000+t.tv_nsec;}
__attribute__((constructor)) static void init(void){
 real_fsync=dlsym(RTLD_NEXT,"fsync");real_fdatasync=dlsym(RTLD_NEXT,"fdatasync");real_close=dlsym(RTLD_NEXT,"close");
 real_write=dlsym(RTLD_NEXT,"write");real_pwrite=dlsym(RTLD_NEXT,"pwrite");real_pwrite64=dlsym(RTLD_NEXT,"pwrite64");
 real_ftruncate=dlsym(RTLD_NEXT,"ftruncate");real_ftruncate64=dlsym(RTLD_NEXT,"ftruncate64");
}
int trace_track(int fd){if(fd<0||fd>=FDS)return -1;tracked[fd]=1;return 0;}
void trace_request(int r){atomic_store(&request,r);}
unsigned trace_count(void){return atomic_load(&used);}
unsigned trace_overflow(void){return atomic_load(&overflow);}
unsigned trace_closed(void){return atomic_load(&closed);}
const Event *trace_events(void){return events;}
static int enabled(int fd){return fd>=0&&fd<FDS&&tracked[fd]&&atomic_load(&request)>=0;}
static void record(int r,int k,int fd,int64_t off,int64_t bytes,int64_t ret,int err,uint64_t start,uint64_t end){
 unsigned n=atomic_fetch_add(&used,1);if(n>=CAP){atomic_fetch_add(&overflow,1);return;}
 events[n]=(Event){r,k,fd,off,bytes,ret,err,start,end};
}
#define SYNC(name,kind) int name(int fd){if(!enabled(fd))return real_##name(fd);int r=atomic_load(&request);uint64_t s=now();int v=real_##name(fd);int e=errno;uint64_t t=now();record(r,kind,fd,-1,0,v,v<0?e:0,s,t);errno=e;return v;}
SYNC(fsync,1)
SYNC(fdatasync,2)
ssize_t write(int fd,const void*b,size_t n){if(!enabled(fd))return real_write(fd,b,n);int r=atomic_load(&request);off_t off=lseek(fd,0,SEEK_CUR);uint64_t s=now();ssize_t v=real_write(fd,b,n);int e=errno;uint64_t t=now();record(r,3,fd,off,n,v,v<0?e:0,s,t);errno=e;return v;}
#define PWRITE(name,type) ssize_t name(int fd,const void*b,size_t n,type off){if(!enabled(fd))return real_##name(fd,b,n,off);int r=atomic_load(&request);uint64_t s=now();ssize_t v=real_##name(fd,b,n,off);int e=errno;uint64_t t=now();record(r,4,fd,off,n,v,v<0?e:0,s,t);errno=e;return v;}
PWRITE(pwrite,off_t)
PWRITE(pwrite64,off64_t)
#define TRUNC(name,type) int name(int fd,type size){if(!enabled(fd))return real_##name(fd,size);int r=atomic_load(&request);uint64_t s=now();int v=real_##name(fd,size);int e=errno;uint64_t t=now();record(r,5,fd,size,0,v,v<0?e:0,s,t);errno=e;return v;}
TRUNC(ftruncate,off_t)
TRUNC(ftruncate64,off64_t)
int close(int fd){if(fd>=0&&fd<FDS&&tracked[fd]){if(atomic_load(&request)>=0)atomic_fetch_add(&closed,1);tracked[fd]=0;}return real_close(fd);}

