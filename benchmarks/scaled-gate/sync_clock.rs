#[cfg(target_pointer_width = "64")]
use std::ffi::c_long;
#[cfg(target_pointer_width = "64")]
use std::io;
use std::time::Instant;

#[cfg(target_pointer_width = "64")]
#[repr(C)]
struct Timespec {
    seconds: c_long,
    nanos: c_long,
}
#[cfg(target_pointer_width = "64")]
unsafe extern "C" {
    fn clock_gettime(clock: i32, time: *mut Timespec) -> i32;
}

#[cfg(target_pointer_width = "64")]
pub(super) fn clock_anchor(origin: Instant) {
    let mut best = (u64::MAX, 0, 0);
    for _ in 0..32 {
        let before = origin.elapsed().as_nanos() as u64;
        let mut ts = Timespec {
            seconds: 0,
            nanos: 0,
        };
        let rc = unsafe { clock_gettime(1, &mut ts) };
        let after = origin.elapsed().as_nanos() as u64;
        assert_eq!(rc, 0, "{}", io::Error::last_os_error());
        let absolute = ts.seconds as u64 * 1_000_000_000 + ts.nanos as u64;
        best = best.min((after - before, absolute - after, absolute - before));
    }
    println!("OS_CLOCK,{},{},{}", best.0, best.1, best.2);
}
#[cfg(not(target_pointer_width = "64"))]
pub(super) fn clock_anchor(_: Instant) {
    panic!("sync clock capture requires a 64-bit Linux guest");
}
pub(super) fn identify(tag: &str, lane: usize) {
    assert!(tag.len() == 10 && tag.bytes().all(|c| c.is_ascii_hexdigit()));
    let name = format!("cw{tag}{lane}");
    std::fs::write("/proc/thread-self/comm", &name).unwrap();
    let tid = std::fs::read_link("/proc/thread-self").unwrap();
    println!(
        "OS_LANE,{lane},{},{}",
        tid.file_name().unwrap().to_str().unwrap(),
        name
    );
}
