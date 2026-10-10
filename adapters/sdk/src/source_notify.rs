//! Bounded, separate datagram wakeups for the private source owner.
use rui_protocol_core::resources::{MemoryBudget, MemoryReservation};
use std::{
    io,
    os::{
        fd::AsRawFd,
        unix::{net::UnixDatagram, process::CommandExt},
    },
    process::Command,
};

pub(crate) struct DirtyNotifications {
    socket: UnixDatagram,
    incarnation: u64,
    last: u64,
    _memory: MemoryReservation,
}
impl DirtyNotifications {
    pub(crate) fn new(budget: &MemoryBudget, incarnation: u64) -> io::Result<(Self, UnixDatagram)> {
        let memory = budget.reserve(64 << 10).map_err(io::Error::other)?;
        let (socket, peer) = UnixDatagram::pair()?;
        for endpoint in [&socket, &peer] {
            endpoint.set_nonblocking(true)?;
            for option in [libc::SO_SNDBUF, libc::SO_RCVBUF] {
                let size: libc::c_int = 8192;
                // SAFETY: owned socket, valid fixed-size socket-option value.
                if unsafe {
                    libc::setsockopt(
                        endpoint.as_raw_fd(),
                        libc::SOL_SOCKET,
                        option,
                        (&size as *const libc::c_int).cast(),
                        std::mem::size_of_val(&size) as libc::socklen_t,
                    )
                } != 0
                {
                    return Err(io::Error::last_os_error());
                }
            }
        }
        Ok((
            Self {
                socket,
                incarnation,
                last: 0,
                _memory: memory,
            },
            peer,
        ))
    }
    pub(crate) fn inherit(peer: &UnixDatagram, command: &mut Command) {
        let fd = peer.as_raw_fd();
        command.env("RUI_PRIVATE_DIRTY_FD", fd.to_string());
        // SAFETY: post-fork callback only performs async-signal-safe fcntl.
        // The caller retains the peer until spawn completes.
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
    pub(crate) fn acknowledged(&mut self, generation: u64) {
        self.last = self.last.max(generation);
    }
    pub(crate) fn take(&mut self) -> io::Result<bool> {
        let mut changed = false;
        // One producer normally coalesces to a single wakeup. Also bound work
        // for malformed/overflow fixture traffic rather than draining forever.
        for _ in 0..64 {
            let mut bytes = [0u8; 17];
            match self.socket.recv(&mut bytes) {
                Ok(16) => {
                    let incarnation = u64::from_be_bytes(bytes[..8].try_into().unwrap());
                    let generation = u64::from_be_bytes(bytes[8..16].try_into().unwrap());
                    if incarnation == self.incarnation && generation > self.last {
                        self.last = generation;
                        changed = true;
                    }
                }
                Ok(_) => return Err(io::Error::other("invalid dirty notification")),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn send(peer: &UnixDatagram, incarnation: u64, generation: u64) {
        peer.send(&[incarnation.to_be_bytes(), generation.to_be_bytes()].concat())
            .unwrap();
    }
    #[test]
    fn coalescing_stale_incarnation_and_memory_lifetime() {
        let budget = MemoryBudget::new(64 << 10);
        let (mut notifications, peer) = DirtyNotifications::new(&budget, 2).unwrap();
        send(&peer, 1, 100);
        send(&peer, 2, 1);
        send(&peer, 2, 3);
        send(&peer, 2, 2);
        assert!(notifications.take().unwrap());
        assert!(!notifications.take().unwrap());
        send(&peer, 2, 3);
        assert!(!notifications.take().unwrap());
        send(&peer, 2, 4);
        assert!(notifications.take().unwrap());
        assert_eq!(budget.used(), 64 << 10);
        drop(notifications);
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn malformed_datagrams_reject_without_entering_reply_stream() {
        let budget = MemoryBudget::new(64 << 10);
        let (mut notifications, peer) = DirtyNotifications::new(&budget, 1).unwrap();
        peer.send(&[0; 32]).unwrap();
        assert!(notifications.take().is_err());
    }
}
