//! Opaque bounded byte-rate relay. TLS remains end-to-end; no packets decoded.
use crate::Error;
use rui_protocol_core::resources::MemoryBudget;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
    task::{JoinHandle, JoinSet},
};
pub const BYTES_PER_SECOND: u64 = 32 * 1024;
pub struct Relay {
    pub address: SocketAddr,
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}
impl Relay {
    pub async fn new(target: SocketAddr, budget: &MemoryBudget) -> Result<Self, Error> {
        let memory = Arc::new(budget.reserve(128 * 1024)?);
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (stop, mut stopped) = watch::channel(false);
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    _=stopped.changed()=>break,
                    Some(_)=connections.join_next(),if !connections.is_empty()=>{},
                    accepted=listener.accept(),if connections.len()<4=>{
                        let Ok((peer,_))=accepted else {break};
                        let memory=memory.clone();let mut stopped=stopped.clone();
                        connections.spawn(async move {
                            let _memory=memory;
                            let Ok(source)=TcpStream::connect(target).await else{return};
                            let (mut pr,mut pw)=peer.into_split();let (mut sr,mut sw)=source.into_split();
                            async fn copy<R:tokio::io::AsyncRead+Unpin,W:tokio::io::AsyncWrite+Unpin>(r:&mut R,w:&mut W)->std::io::Result<()> {
                                let mut buffer=[0u8;2048];
                                loop {
                                    let length=r.read(&mut buffer).await?;
                                    if length==0 {w.shutdown().await?;return Ok(());}
                                    tokio::time::sleep(Duration::from_secs_f64(length as f64/BYTES_PER_SECOND as f64)).await;
                                    w.write_all(&buffer[..length]).await?;
                                }
                            }
                            tokio::select! {_=stopped.changed()=>{},_=copy(&mut pr,&mut sw)=>{},_=copy(&mut sr,&mut pw)=>{}}
                        });
                    }
                }
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        });
        Ok(Self {
            address,
            stop,
            task,
        })
    }
    pub async fn shutdown(self) {
        self.stop.send_replace(true);
        let _ = self.task.await;
    }
}
