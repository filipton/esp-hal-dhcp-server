# esp-hal-dhcp-server
Simple dhcp server for embassy tested on esp-hal (esp32-s3 - WiFiAP with android phone).

[![crates.io](https://img.shields.io/crates/v/esp-hal-dhcp-server.svg)](https://crates.io/crates/esp-hal-dhcp-server)
[![MIT license](https://img.shields.io/github/license/mashape/apistatus.svg)]()

## Example
To see full example look inside `./example` dir, you can cd into it and run it as normal crate

```rust
// ...
// spawn your dhcp_server task
spawner.spawn(dhcp_server(stack)).ok();

log::info!("Closing dhcp server after 2m...");
Timer::after(Duration::from_secs(120)).await;
log::info!("Closing dhcp server...");

// you can close your server using builtin SIGNAL
esp_hal_dhcp::dhcp_close();
// ...

// ...
#[embassy_executor::task]
async fn dhcp_server(stack: Stack<'static>) {
    let config = DhcpServerConfig {
        ip: Ipv4Addr::new(192, 168, 2, 1),
        lease_time: Duration::from_secs(3600),
        gateways: &[],
        subnet: None,
        dns: &[],
    };

    let mut leaser = SimpleDhcpLeaser {
        start: Ipv4Addr::new(192, 168, 2, 50),
        end: Ipv4Addr::new(192, 168, 2, 200),
        leases: Default::default(),
    };
    esp_hal_dhcp::run_dhcp_server(stack, config, &mut leaser).await;
}
```
