//! Linux shared-memory lifecycle fence for the owned cooperating source.
//!
//! ABI v1 is a sealed 4096-byte memfd: aligned native-endian atomic u64 at0,
//! immutable source generation at8, magic RUIFENC1 at16. Zero denies dispatch.
//! Host release stores pair with the source's libatomic acquire loads. Neither
//! Python byte decoding nor socket delivery provides the atomic authority read.
use rui_protocol_core::resources::{MemoryBudget, MemoryReservation};
use std::{
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    process::Command,
    ptr::NonNull,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

const PAGE: usize = 4096;
struct Mapping(NonNull<AtomicU64>);
// SAFETY: the mapping has page alignment, initialized immutable header bytes,
// and one mutable AtomicU64. Every mutable access uses that atomic ABI. Arc
// ownership keeps the mapping alive until no host reference can access it.
unsafe impl Send for Mapping {}
// SAFETY: the only concurrently mutated bytes are the aligned AtomicU64.
unsafe impl Sync for Mapping {}
impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: this is the exact successful mmap address/length; no host
        // references survive the final owner and the source owns its own map.
        unsafe {
            libc::munmap(self.0.as_ptr().cast(), PAGE);
        }
    }
}
struct FenceData {
    mapping: Mapping,
    reader: OwnedFd,
    issued: Mutex<u64>,
    _memory: MemoryReservation,
}
#[derive(Clone)]
pub(crate) struct NativeFence(Arc<FenceData>);
impl NativeFence {
    pub(crate) fn new(budget: &MemoryBudget, generation: u64) -> io::Result<Self> {
        // SAFETY: read-only process constant, no pointers or allocation.
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if generation == 0
            || !cfg!(target_has_atomic = "64")
            || page_size != PAGE as libc::c_long
            || std::mem::size_of::<AtomicU64>() != 8
            || std::mem::align_of::<AtomicU64>() != 8
        {
            return Err(io::Error::other("unsupported native fence ABI"));
        }
        let memory = budget.reserve(8192).map_err(io::Error::other)?;
        // SAFETY: constant NUL-terminated name and supported Linux flags.
        let fd = unsafe {
            libc::memfd_create(
                c"rui-native-control".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful creation transferred ownership of a new fd.
        let writer = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: our owned memfd is sized once before any reader is issued.
        if unsafe { libc::ftruncate(fd, PAGE as libc::off_t) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: valid owned fd and one bounded shared page, offset zero.
        let address = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                PAGE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if address == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let mapping = Mapping(
            NonNull::new(address.cast()).ok_or_else(|| io::Error::other("null fence mapping"))?,
        );
        // SAFETY: aligned, uniquely initialized page before publication.
        unsafe {
            mapping.0.as_ptr().write(AtomicU64::new(0));
            address.cast::<u8>().add(8).cast::<u64>().write(generation);
            std::ptr::copy_nonoverlapping(b"RUIFENC1".as_ptr(), address.cast::<u8>().add(16), 8);
        }
        // This path addresses only the just-created owned fd. The inherited
        // descriptor is read-only; there is no filesystem source/fence path.
        let reader: OwnedFd = std::fs::File::open(format!("/proc/self/fd/{fd}"))?.into();
        let seals =
            libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_FUTURE_WRITE | libc::F_SEAL_SEAL;
        // SAFETY: only seals are modified; the existing host mapping remains
        // writable and new writable mappings/writes/size changes are forbidden.
        if unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, seals) } < 0 {
            return Err(io::Error::last_os_error());
        }
        drop(writer);
        Ok(Self(Arc::new(FenceData {
            mapping,
            reader,
            issued: Mutex::new(0),
            _memory: memory,
        })))
    }
    fn word(&self) -> &AtomicU64 {
        // SAFETY: Arc retains the initialized aligned mapping for this borrow.
        unsafe { self.0.mapping.0.as_ref() }
    }
    /// A retired generation is never revived, including an exact grant retry.
    #[cfg(test)]
    pub(crate) fn grant(&self, generation: u64) -> io::Result<()> {
        self.grant_if(generation, || true)
    }
    pub(crate) fn grant_if(
        &self,
        generation: u64,
        authorized: impl FnOnce() -> bool,
    ) -> io::Result<()> {
        // Priority invalidation uses the same tiny host critical section. The
        // source's final load remains lock-free; no lock spans a pipe request.
        let mut issued = self
            .0
            .issued
            .lock()
            .map_err(|_| io::Error::other("poisoned native fence"))?;
        if generation == 0 || generation <= *issued || !authorized() {
            return Err(io::Error::other("retired native control generation"));
        }
        *issued = generation;
        self.word().store(generation, Ordering::Release);
        Ok(())
    }
    pub(crate) fn current(&self) -> u64 {
        self.word().load(Ordering::Acquire)
    }
    pub(crate) fn invalidate(&self) {
        self.invalidate_if(|| true);
    }
    pub(crate) fn invalidate_if(&self, authorized: impl FnOnce() -> bool) {
        let _issued = self
            .0
            .issued
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if authorized() {
            self.word().store(0, Ordering::Release);
        }
    }
    pub(crate) fn inherit(&self, command: &mut Command) {
        let fd = self.0.reader.as_raw_fd();
        command.env("RUI_PRIVATE_CONTROL_FD", fd.to_string());
        // SAFETY: post-fork callback performs only async-signal-safe fcntl and
        // returns errno; Arc owner in NativePipe outlives spawn/source teardown.
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retired_fence_generation_never_revives_and_clone_owns_charge() {
        let budget = MemoryBudget::new(8192);
        let fence = NativeFence::new(&budget, 1).unwrap();
        assert_eq!(fence.current(), 0);
        fence.grant(7).unwrap();
        let priority = fence.clone();
        std::thread::spawn(move || priority.invalidate())
            .join()
            .unwrap();
        assert_eq!(fence.current(), 0);
        assert!(fence.grant(7).is_err());
        assert!(fence.grant(6).is_err());
        fence.grant(8).unwrap();
        let keeper = fence.clone();
        drop(fence);
        assert_eq!(budget.used(), 8192);
        drop(keeper);
        assert_eq!(budget.used(), 0);
    }
}
