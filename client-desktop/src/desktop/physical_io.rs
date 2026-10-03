//! Input-safety cancellation is independent of the ordinary UI FIFO. A stop
//! belongs to one local grant serial, so delayed callbacks cannot close a
//! newer authority. Cancelling a send preserves the worker's exact command
//! queue; the caller drops the old socket and reconnects as an observer.
use crate::physical_dispatch::PhysicalControl;
use rui_transport::TransportError;
use std::future::Future;
use tokio::sync::watch;

pub async fn guarded<F, T>(
    future: F,
    authority: Option<PhysicalControl>,
    priority: &mut watch::Receiver<Option<PhysicalControl>>,
    stopped: &mut bool,
) -> Result<T, TransportError>
where
    F: Future<Output = Result<T, TransportError>>,
{
    let Some(authority) = authority else {
        return future.await;
    };
    tokio::pin!(future);
    loop {
        if *priority.borrow_and_update() == Some(authority) {
            *stopped = true;
            return Err(TransportError::Closed);
        }
        tokio::select! {
            biased;
            changed = priority.changed() => {
                if changed.is_err() {
                    *stopped = true;
                    return Err(TransportError::Closed);
                }
            },
            result = &mut future => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_queue::CommandQueue;
    use rui_protocol_core::{commands::CommandFamily, resources::MemoryBudget};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn priority_stop_cancels_a_polled_backpressured_send_even_when_normal_fifo_is_full() {
        let budget = MemoryBudget::new(128 * 1024);
        let mut exact = CommandQueue::new([1; 16], budget.clone(), 4096).unwrap();
        let (normal, mut fifo) = tokio::sync::mpsc::channel(32);
        for i in 0..32 {
            normal.try_send(i).unwrap();
        }
        assert_eq!(normal.capacity(), 0);
        let (mut writer, mut reader) = tokio::io::duplex(4);
        let writer_memory = budget.reserve(4096).unwrap();
        let scope = PhysicalControl {
            connection_generation: 2,
            grant_serial: 3,
        };
        let (stop, mut signal) = watch::channel(None);
        let (polled, mut observed) = tokio::sync::mpsc::channel(1);
        let mut stopped = false;
        {
            let send = async {
                exact
                    .encode(CommandFamily::RawInput, 16, |bytes| {
                        bytes.extend_from_slice(&[9; 16]);
                        Ok(())
                    })
                    .unwrap();
                polled.try_send(()).unwrap();
                writer
                    .write_all(exact.latest().unwrap().bytes())
                    .await
                    .map_err(|_| TransportError::Io)
            };
            let guarded = guarded(send, Some(scope), &mut signal, &mut stopped);
            tokio::pin!(guarded);
            tokio::select! {
                _=&mut guarded => panic!("send cannot finish before draining its bounded writer"),
                _=observed.recv() => {},
            }
            let mut prefix = [0; 4];
            reader.read_exact(&mut prefix).await.unwrap();
            assert_eq!(prefix, [9; 4]);
            stop.send_replace(Some(scope));
            assert!(matches!(
                tokio::time::timeout(std::time::Duration::from_secs(1), &mut guarded)
                    .await
                    .unwrap(),
                Err(TransportError::Closed)
            ));
        }
        assert!(stopped);
        // The scoped future has been cancelled, not restarted with new bytes.
        assert_eq!(exact.len(), 1);
        assert_eq!(exact.latest().unwrap().bytes(), &[9; 16]);
        assert_eq!(exact.next_sequence(), 2);
        assert_eq!(fifo.try_recv().unwrap(), 0);
        assert!(budget.used() <= budget.limit());
        drop(writer);
        drop(reader);
        drop(writer_memory);
        drop(exact);
        assert_eq!(budget.used(), 0);
    }
    #[tokio::test]
    async fn an_old_grant_stop_cannot_cancel_new_owner_work() {
        let current = PhysicalControl {
            connection_generation: 4,
            grant_serial: 9,
        };
        let old = PhysicalControl {
            grant_serial: 8,
            ..current
        };
        let (_tx, mut rx) = watch::channel(Some(old));
        let mut stopped = false;
        assert_eq!(
            guarded(
                async { Ok::<_, TransportError>(17) },
                Some(current),
                &mut rx,
                &mut stopped
            )
            .await
            .unwrap(),
            17
        );
        assert!(!stopped);
    }
}
