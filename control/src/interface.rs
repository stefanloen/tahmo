use defmt::info;
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::Timer;
use core::str;

use crate::messages::{IntReqMsg, IntResMsg};
use crate::usb::{MyUsbClass, UsbWriter};
use crate::{usb_write, usb_writeln};

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
    let (mut sender, 
        mut receiver, 
        mut control) 
        = interface.class.split_with_control();

    let mut dtr = false;
    let mut writer = UsbWriter::new(&mut sender);

    loop {  
        let mut rx_buf = [0u8; 64];

        let result = select(
            control.control_changed(), 
            receiver.read_packet(&mut rx_buf)
        ).await;

        match result {
            Either::First(_) => {
                if receiver.dtr() && !dtr {
                    dtr = receiver.dtr();
                    Timer::after_millis(50).await;
                    info!("[intf] Terminal ready");
                    usb_write!(writer, "Terminal Ready. Type a command.").await.ok();                
                } else if !receiver.dtr() && dtr{
                    dtr = receiver.dtr();
                    info!("[intf] Terminal Disconnected");
                }
            },
            Either::Second(res) => {
                match res {
                Ok(n) => {

                    let line = str::from_utf8(&rx_buf[..n]).unwrap_or("").trim();
                    if line.is_empty() { continue; }

                    match line {
                        "ping" => {
                            usb_writeln!(writer, "Pong").await.ok();
                        }
                        _ => {
                            usb_writeln!(writer, "Unknown command.").await.ok();
                        }
                    }
                }
                Err(_) => break, 
                }
            }
        }
    }
}