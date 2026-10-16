#![cfg_attr(not(test), no_std)]

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
/// Signalled by [`dhcp_close`] to stop the running [`run_dhcp_server`].
pub static CLOSE_SIGNAL: CloseSignal = Signal::new();

/// Runs a DHCP server on `stack` until [`dhcp_close`] is called.
///
/// Returns an error only if the UDP socket cannot bind port 67.
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

    // a close requested while no server was running must not stop this one
    CLOSE_SIGNAL.reset();
    let mut server = DhcpServer::new(config, leaser, sock)?;
    embassy_futures::select::select(server.run(), CLOSE_SIGNAL.wait()).await;
    Ok(())
}

/// Stops the running [`run_dhcp_server`].
pub fn dhcp_close() {
    CLOSE_SIGNAL.signal(());
}
