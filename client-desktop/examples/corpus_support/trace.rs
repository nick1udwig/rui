//! Explicit local oracle export; never a transport format or ordinary log.
use crate::Error;
use rui_protocol_core::{
    resources::{MemoryBudget, MemoryReservation, ResourceContext},
    transcript::Direction,
};
use rui_transport::diagnostics::Diagnostics;
use sha2::{Digest, Sha256};
use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt, path::Path};

const FILE_LIMIT: usize = 64 << 20;
const SEGMENT_LIMIT: usize = 8;
struct Writer {
    file: std::fs::File,
    hash: Sha256,
    bytes: usize,
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let total = self
            .bytes
            .checked_add(bytes.len())
            .filter(|n| *n <= FILE_LIMIT)
            .ok_or_else(|| std::io::Error::other("native trace export ceiling"))?;
        self.file.write_all(bytes)?;
        self.hash.update(bytes);
        self.bytes = total;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
pub struct Trace {
    writer: Writer,
    context: Option<ResourceContext>,
    segment: usize,
    last: u64,
    packets: usize,
    _memory: MemoryReservation,
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
impl Trace {
    pub fn new(path: &Path, budget: &MemoryBudget) -> Result<Self, Error> {
        // Streaming hex scratch and fixed metadata are charged independently
        // from the view-scoped recorder, before creating export buffers.
        let memory = budget.reserve(32 * 1024)?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        Ok(Self {
            writer: Writer {
                file,
                hash: Sha256::new(),
                bytes: 0,
            },
            context: None,
            segment: 0,
            last: 0,
            packets: 0,
            _memory: memory,
        })
    }
    pub fn export(
        &mut self,
        diagnostics: &Diagnostics,
        context: ResourceContext,
    ) -> Result<(), Error> {
        if self.context != Some(context) {
            if self.context.is_some() {
                return Err("trace context changed without closing segment".into());
            }
            self.segment = self
                .segment
                .checked_add(1)
                .filter(|n| *n <= SEGMENT_LIMIT)
                .ok_or("native trace segment ceiling")?;
            serde_json::to_writer(
                &mut self.writer,
                &serde_json::json!({
                    "record":"scope","segment":self.segment,
                    "session_id":hex(&context.scope.session_id),"epoch":hex(&context.scope.epoch),
                    "principal_id":hex(&context.scope.principal_id),"view_id":hex(&context.view_id),
                    "connection":"post-auth","handshake_credentials_excluded":true,
                }),
            )?;
            self.writer.write_all(b"\n")?;
            self.context = Some(context);
        }
        diagnostics.inspect(context, |entries| -> Result<(), Error> {
            for entry in entries {
                if entry.sequence <= self.last { continue; }
                if entry.sequence != self.last + 1 { return Err("native trace retained-prefix gap".into()); }
                let exact=entry.exact_bytes().ok_or("native exact trace policy unavailable")?;
                if exact.len()!=entry.wire_bytes {return Err("native trace length mismatch".into());}
                // Write only small metadata through serde; exact bytes use a
                // fixed stack hex buffer, without a second whole-packet copy.
                let metadata=serde_json::json!({"record":"packet","segment":self.segment,
                    "sequence":entry.sequence,"direction":match entry.direction {
                        Direction::HostToClient=>"host-to-client",Direction::ClientToHost=>"client-to-host"},
                    "kind":entry.kind,"wire_bytes":entry.wire_bytes,"sha256":hex(&entry.digest)});
                let encoded=serde_json::to_vec(&metadata)?;
                self.writer.write_all(&encoded[..encoded.len()-1])?;
                self.writer.write_all(b",\"exact_packet_hex\":\"")?;
                let mut buffer=[0u8;4096];
                const DIGITS:&[u8;16]=b"0123456789abcdef";
                for chunk in exact.chunks(2048) {
                    for (i,b) in chunk.iter().enumerate() {
                        buffer[2*i]=DIGITS[(b>>4) as usize];buffer[2*i+1]=DIGITS[(b&15) as usize];
                    }
                    self.writer.write_all(&buffer[..2*chunk.len()])?;
                }
                self.writer.write_all(b"\"}\n")?;
                self.last=entry.sequence;self.packets+=1;
            }
            Ok(())
        })??;
        self.writer.flush()?;
        Ok(())
    }
    pub fn end_segment(&mut self, reason: &'static str) -> Result<(), Error> {
        if self.context.is_none() {
            return Err("native trace segment unavailable".into());
        }
        serde_json::to_writer(
            &mut self.writer,
            &serde_json::json!({"record":"segment-end",
            "segment":self.segment,"through_sequence":self.last,"reason":reason,
            "scope":"exact retained prefix; packets after export are not claimed"}),
        )?;
        self.writer.write_all(b"\n")?;
        self.context = None;
        self.last = 0;
        Ok(())
    }
    pub fn proof(&mut self) -> Result<serde_json::Value, Error> {
        self.writer.flush()?;
        Ok(
            serde_json::json!({"sha256":hex(&self.writer.hash.clone().finalize()),
            "bytes":self.writer.bytes,"packets":self.packets,"segments":self.segment,
            "file_byte_ceiling":FILE_LIMIT,"segment_ceiling":SEGMENT_LIMIT,
            "policy":"AuthorizedDiagnosticBytes; post-auth only; mode0600"}),
        )
    }
}
