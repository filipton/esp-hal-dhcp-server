#![no_std]

pub use edge_dhcp::Ipv4Addr;
use embassy_net::{
    udp::{BindError, PacketMetadata, UdpSocket},
    Stack,
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use server::DhcpServer;
use structs::DhcpLeaser;
use structs::DhcpServerConfig;

pub mod server;
pub mod simple_leaser;
pub mod structs;

pub type CloseSignal = Signal<CriticalSectionRawMutex, ()>;
pub static CLOSE_SIGNAL: CloseSignal = Signal::new();

pub async fn run_dhcp_server(
    stack: Stack<'static>,
    config: DhcpServerConfig<'_>,
    leaser: &'_ mut dyn DhcpLeaser,
) -> Result<(), BindError> {
    let mut rx_buffer = [0; 1024];
    let mut tx_buffer = [0; 1024];
    let mut rx_meta = [PacketMetadata::EMPTY; 16];
    let mut tx_meta = [PacketMetadata::EMPTY; 16];
    let sock = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );

    let mut server = DhcpServer::new(config, leaser, sock)?;
    embassy_futures::select::select(server.run(), CLOSE_SIGNAL.wait()).await;
    Ok(())
}

pub fn dhcp_close() {
    CLOSE_SIGNAL.signal(());
}
