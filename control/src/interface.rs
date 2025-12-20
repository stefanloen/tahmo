use defmt::info;
use embassy_futures::select::{Either3, select3};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::Timer;
use core::str;

use crate::messages::{IntReqMsg, IntResMsg};
use crate::usb::{MyUsbClass, UsbWriter};
use crate::{usb_write, usb_writeln};

/*
TODO list:
Status 
- firmware version
- chip id
- Online time
- Current time
- Battery voltage
- Chip temperature

Current activity
- Solar panel input
- GPS signal strength
- Rockblock signal strength
- Memory status 

Setconfig
-
-
-
-

*/

enum InterfaceState {
    Disconnected,
    Idle,
    Batvolt,
    GetConst,
}

pub struct Interface {
    class: MyUsbClass,
    state: InterfaceState,
}

impl Interface {
    pub fn new(class: MyUsbClass) -> Self {
        Self { 
            class, 
            state: InterfaceState::Disconnected, 
        }
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

    let mut writer = UsbWriter::new(&mut sender);

    loop {  
        let mut rx_buf = [0u8; 64];

        let result = select3(
            control.control_changed(), 
            receiver.read_packet(&mut rx_buf),
            channel_req.receive()
        ).await;

        match result {
            Either3::First(_) => {
                if receiver.dtr() && let InterfaceState::Disconnected = interface.state {
                    interface.state = InterfaceState::Idle;
                    Timer::after_millis(50).await;
                    info!("[intf] Terminal ready");
                    usb_write!(writer, "Welcome to TAHMO-WLM. For a list of commands, use 'help' or '?'").await.ok();                
                } else if !receiver.dtr() && let InterfaceState::Idle = interface.state{
                    interface.state = InterfaceState::Disconnected;
                    info!("[intf] Terminal Disconnected");
                } else if !receiver.dtr() && let InterfaceState::Batvolt = interface.state{
                    interface.state = InterfaceState::Disconnected;
                    //Disconnect while getting batvolt
                    info!("[intf] Terminal Disconnected, abort getting battery voltage");
                }
            },
            Either3::Second(res) => {
                let command = match res {
                    Ok(n) => {
                        let line = str::from_utf8(&rx_buf[..n]).unwrap_or("").trim();
                        if line.is_empty() { 
                            continue;
                        }
                        line
                    },
                    Err(_) => {
                        continue;
                    }
                };

                match interface.state {
                    InterfaceState::Idle => {
                        match command {
                            "help" | "?" => {
                                usb_writeln!(writer,
"The following commands can be used
help | ? - Gives a list of commands
status - Give a full status update
batvolt - Get battery voltage
getconst - Get the constellation state
").await.ok();
                            }
                            "status" => {
                                usb_writeln!(writer, "Status has not yet been implemented").await.ok();
                            }
                            "batvolt" => {
                                channel_res.send(IntResMsg::GetBatVolt).await;
                                interface.state = InterfaceState::Batvolt;
                            }
                            "getconst" => {
                                usb_writeln!(writer, "Getting constellation state. Please wait...").await.ok();
                                channel_res.send(IntResMsg::GetConstellationState).await;
                                interface.state = InterfaceState::GetConst;
                            }
                            _ => {
                                usb_writeln!(writer, "Unknown command, for a list of commands, use 'help' or '?'").await.ok();
                            }
                        }
                    },
                    InterfaceState::Batvolt => {
                        // User input while getting batvolt, abort 
                        interface.state = InterfaceState::Idle
                    }
                    InterfaceState::GetConst => {
                        // TODO: Cancel properly by sending message to control to cancel
                        interface.state = InterfaceState::Idle
                    }
                    InterfaceState::Disconnected => {
                        unreachable!("Received USB message while being disconnected");
                    }
                }
                
            },
            Either3::Third(request) => {
                match interface.state {
                    InterfaceState::Idle | InterfaceState::Disconnected => {
                        // Ignoring commands from control
                    },
                    InterfaceState::Batvolt => {
                        match request {
                            IntReqMsg::BatVoltSuccess { voltage } => {
                                usb_writeln!(writer, "Battery millivolts: {}", {voltage}).await.ok();
                                interface.state = InterfaceState::Idle;
                            },
                            IntReqMsg::BatVoltFail => {
                                usb_writeln!(writer, "Could not get battery voltage").await.ok();
                                interface.state = InterfaceState::Idle;
                            },
                            _ => {
                                // Ignore other requests
                            }

                        };
                    }
                    InterfaceState::GetConst => {
                        match request {
                            IntReqMsg::ConstellationState { signal_bars_max, signal_level_max, constellation_visible } => {
                                usb_writeln!(writer, "Signal bars: {}", signal_bars_max).await.ok();
                                usb_writeln!(writer, "Signal level: {}", signal_level_max).await.ok();
                                usb_writeln!(writer, "Constellation visible: {}", constellation_visible).await.ok();
                                interface.state = InterfaceState::Idle;
                            }
                            IntReqMsg::ConstellationStateFail { error } => {
                                usb_writeln!(writer, "Getting constellation failed").await.ok();
                                interface.state = InterfaceState::Idle;
                            }

                            _ => {
                                // Ignore other requests
                            }
                        };
                    }
                }
            }
        }
    }
}