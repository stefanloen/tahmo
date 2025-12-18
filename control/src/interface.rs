use defmt::info;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use crate::messages::{IntReqMsg, IntResMsg};
use crate::usb::MyUsbClass;
use core::str;

pub struct Interface {
    class: MyUsbClass,
}

impl Interface {
    pub fn new(class: MyUsbClass) -> Self {
        Self { class }
    }
}

#[embassy_executor::task]
pub async fn interface_task(
    channel_req: &'static Channel<CriticalSectionRawMutex, IntReqMsg, 8>,
    channel_res: &'static Channel<CriticalSectionRawMutex, IntResMsg, 8>,
    mut interface: Interface,
) {
    loop {
        interface.class.wait_connection().await;
        info!("USB Connected");
        
        loop {
            if interface.class.dtr() {
                break;
            }
            embassy_time::Timer::after_millis(50).await;
        }

        info!("Terminal ready");
        let _ = interface.class.write_packet(b"Terminal Ready. Type a command.\r\n").await;

        loop {
            let mut rx_buf = [0u8; 64];

            match interface.class.read_packet(&mut rx_buf).await {
                Ok(n) => {

                    let line = str::from_utf8(&rx_buf[..n]).unwrap_or("").trim();
                    if line.is_empty() { continue; }

                    match line {
                        "ping" => {
                            let _ = interface.class.write_packet(b"pong\r\n").await;
                        }
                        _ => {
                            let _ = interface.class.write_packet(b"Unknown command\r\n").await;
                        }
                    }
                }
                Err(_) => break, 
            }
        }
        info!("USB Disconnected");
    }
}