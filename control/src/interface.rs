use defmt::info;
use embassy_futures::select::{Either3, select3};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::Timer;
use core::str;
use core::str::FromStr;
use heapless::Vec;

use crate::messages::{IntReqMsg, IntResMsg};
use crate::usb::{MyUsbClass, UsbWriter};
use crate::{usb_write, usb_writeln, utils};
use crate::types::{MAX_MIDPOINTS, SECONDS_PER_DAY};

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
    GetConfig,

    SetElevation {
        min: u32,
        max: u32,
    },

    SetAzimuth {
        min: u32,
        max: u32,
    },

    SetHeight {
        min: f32,
        max: f32,
    },

    SetMeasurements {
        mid_times: Vec::<u32, MAX_MIDPOINTS>,
        bins_per_sector: u32,
    }
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
                
                let mut parts = match res {
                    Ok(n) => {
                        let line = str::from_utf8(&rx_buf[..n]).unwrap_or("").trim();
                        if line.is_empty() { 
                            continue;
                        }
                        line.split_whitespace()
                    },
                    Err(_) => {
                        continue;
                    }
                };

                match interface.state {
                    InterfaceState::Idle => {
                        match parts.next().unwrap_or("") {
                            "help" | "?" => {
                                usb_writeln!(writer,
"The following commands can be used
help | ? - Gives a list of commands
status - Give a full status update
batvolt - Get battery voltage
getconst - Get the constellation state
getconfig - Get the current configuration
setconfig - Set configuration
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
                            "getconfig" => {
                                channel_res.send(IntResMsg::GetConfig).await;
                                interface.state = InterfaceState::GetConfig;
                            }
                            "setconfig" => {
                                match parts.next().unwrap_or("") {
                                    "elevation" => {
                                        let min_val = parse_in_range(&mut writer, parts.next(), 0, 90, "Min elevation").await;
                                        let max_val = parse_in_range(&mut writer, parts.next(), 0, 90, "Max elevation").await;

                                        let (Some(min), Some(max)) = (min_val, max_val) else {
                                            continue;
                                        };

                                        if min >= max {
                                            usb_writeln!(writer, "Error: Min must be less than Max").await.ok();
                                            continue;
                                        }

                                        channel_res.send(IntResMsg::GetConfig).await;
                                        interface.state = InterfaceState::SetElevation { min, max }                                        
                                    }
                                    "azimuth" => {
                                        let min_val = parse_in_range(&mut writer, parts.next(), 0, 360, "Min azimuth").await;
                                        let max_val = parse_in_range(&mut writer, parts.next(), 0, 360, "Max azimuth").await;

                                        let (Some(min), Some(max)) = (min_val, max_val) else {
                                            continue;
                                        };

                                        channel_res.send(IntResMsg::GetConfig).await;
                                        interface.state = InterfaceState::SetAzimuth { min, max }
                                    }
                                    "height" => {
                                        let min_val = parse_in_range(&mut writer, parts.next(), 0.0, 50.0, "Min height").await;
                                        let max_val = parse_in_range(&mut writer, parts.next(), 0.0, 50.0, "Max height").await;

                                        let (Some(min), Some(max)) = (min_val, max_val) else { 
                                            continue; 
                                        };

                                        if min >= max {
                                            usb_writeln!(writer, "Error: Min must be less than Max").await.ok();
                                            continue;
                                        }

                                        channel_res.send(IntResMsg::GetConfig).await;
                                        interface.state = InterfaceState::SetHeight { min, max }
                                    }
                                    "measurements" => {
                                        let timepoint_val = parts.next()
                                                                        .and_then(|s| utils::time_str_to_seconds(s))
                                                                        .filter(|&t| t >= 0 && t < 60*60*24);
                                        
                                        if timepoint_val.is_none() {
                                            usb_writeln!(writer, "Error: timepoint is missing or incorrect").await.ok();
                                        }
                                        
                                        let n_per_day_val = parse_in_range(&mut writer, parts.next(), 1, MAX_MIDPOINTS, "Measurements per day").await;
                                        let duration_val = parse_in_range(&mut writer, parts.next(), 20, 240, "Measurement duration").await;

                                        let (Some(timepoint), Some(n_per_day), Some(duration)) = (timepoint_val, n_per_day_val, duration_val) else {
                                            continue;
                                        };
                                        
                                        let interval = SECONDS_PER_DAY / n_per_day as u32;
                                        let gap_between_measurements =  interval as i32 - duration * 60;

                                        if gap_between_measurements < 30 { // TODO remove magic number
                                            usb_writeln!(writer, "Error: Invalid measurements configuration").await.ok();
                                            continue;
                                        }
                                        
                                        if duration % 20 != 0 {
                                            usb_writeln!(writer, "Error: Duration is not a multiple of 20").await.ok();
                                            continue;
                                        }

                                        let bins_per_sector = duration as u32 / 20;

                                        let earliest_mid_time = timepoint % (interval as u32);
                                        let mut mid_times = Vec::<u32, MAX_MIDPOINTS>::new();
                                    
                                        for i in 0..n_per_day{
                                            let mid_time = (earliest_mid_time + i as u32 *interval as u32) % SECONDS_PER_DAY;
                                            mid_times.push(mid_time).unwrap();
                                        }

                                        channel_res.send(IntResMsg::GetConfig).await;
                                        interface.state = InterfaceState::SetMeasurements { mid_times, bins_per_sector}

                                    }
                                    "help" | "?" => {
                                        usb_writeln!(writer,
"The following setting can be used
help | ? - Gives a list of settings
elevation <min> <max>, example use: 'setconfig elevation 5 30'
azimuth <min> <max>, example use: 'setconfig azimuth 120 240'
height <min> <max>, example use: 'setconfig height 0.5 15'
measurements <timepoint> <n> <duration>, example use: 'setconfig measurements 00:00 4 60'
").await.ok();
                                    }
                                    _ => {
                                        usb_writeln!(writer, "Unknown or missing setting, for a list of settings, use 'setconfig help' or 'setconfig ?'").await.ok();
                                    }
                                }
                            }

                            _ => {
                                usb_writeln!(writer, "Unknown command, for a list of commands, use 'help' or '?'").await.ok();
                            }
                        }
                    },
                    InterfaceState::GetConst => {
                        // TODO: Cancel properly by sending message to control to cancel
                        interface.state = InterfaceState::Idle
                    }
                    InterfaceState::Disconnected => {
                        unreachable!("Received USB message while being disconnected");
                    }
                    _ => {
                        // User input while in a noncritical state
                        usb_writeln!(writer, "Aborted").await.ok();
                        interface.state = InterfaceState::Idle
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
                    },
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
                    },
                    InterfaceState::GetConfig => {
                        match request {
                            IntReqMsg::GiveConfig { config } => {
                                usb_writeln!(writer, "Elevation range: {} deg to {} deg", config.post_min_elevation, config.post_max_elevation).await.ok();
                                usb_writeln!(writer, "Azimuth range: {} deg to {} deg", config.post_min_azimuth, config.post_max_azimuth).await.ok();
                                usb_writeln!(writer, "Relative height range: {}m to {}m", config.min_relative_height, config.max_relative_height).await.ok();
                                usb_writeln!(writer, "Measurement duration: {} minutes", config.bins_per_sector * 20).await.ok();
                                usb_writeln!(writer, "Mid times: {}", config.get_mid_times_as_str()).await.ok();
                                interface.state = InterfaceState::Idle;
                            }

                            _ => {
                                // Ignore other requests
                            }
                        }
                    },
                    InterfaceState::SetElevation { min, max } => {
                        match request {
                            IntReqMsg::GiveConfig { mut config } => {
                                config.pre_min_elevation = min;
                                config.post_min_elevation = min;
                                config.pre_max_elevation = max;
                                config.post_max_elevation = max;
                                
                                usb_writeln!(writer, "Config changed successfully").await.ok();
                                usb_writeln!(writer, "Elevation range: {} deg to {} deg", min,max).await.ok();

                                channel_res.send(IntResMsg::SetConfig { config }).await;
                                interface.state = InterfaceState::Idle;
                            }

                            _ => {
                                // Ignore other requests
                            }
                        }
                    },
                    InterfaceState::SetAzimuth { min, max } => {
                        match request {
                            IntReqMsg::GiveConfig { mut config } => {
                                config.pre_min_azimuth = min;
                                config.post_min_azimuth = min;
                                config.pre_max_azimuth = max;
                                config.post_max_azimuth = max;
                                
                                usb_writeln!(writer, "Config changed successfully").await.ok();
                                usb_writeln!(writer, "Azimuth range: {} deg to {} deg", min,max).await.ok();

                                channel_res.send(IntResMsg::SetConfig { config }).await;
                                interface.state = InterfaceState::Idle;
                            }

                            _ => {
                                // Ignore other requests
                            }
                        }
                    },
                    InterfaceState::SetHeight { min, max } => {
                        match request {
                            IntReqMsg::GiveConfig { mut config } => {
                                config.min_relative_height = min;
                                config.max_relative_height = max;
                                
                                usb_writeln!(writer, "Config changed successfully").await.ok();
                                usb_writeln!(writer, "Relative height range: {}m to {}m", min, max).await.ok();

                                channel_res.send(IntResMsg::SetConfig { config }).await;
                                interface.state = InterfaceState::Idle;
                            }

                            _ => {
                                // Ignore other requests
                            }
                        }
                    },
                    InterfaceState::SetMeasurements { ref mid_times, bins_per_sector} => {
                        match request {
                            IntReqMsg::GiveConfig { mut config } => {
                                config.sector_mid_times = mid_times.clone();
                                config.bins_per_sector = bins_per_sector;

                                usb_writeln!(writer, "Config changed successfully").await.ok();
                                usb_writeln!(writer, "Mid times: {}", config.get_mid_times_as_str()).await.ok();
                                usb_writeln!(writer, "Measurement duration: {} minutes", bins_per_sector * 20).await.ok();

                                channel_res.send(IntResMsg::SetConfig { config: config }).await;
                                interface.state = InterfaceState::Idle;
                            }

                            _ => {
                                // Ignore other requests
                            }
                        }
                    }
                }
            }
        }
    }
}

fn parse_num<T: FromStr> (s: Option<&str>) -> Option<T> {
    s.and_then(|string| string.parse::<T>().ok())
}

async fn parse_in_range<T>(
    writer: &mut UsbWriter<'_, embassy_rp::usb::Driver<'static, embassy_rp::peripherals::USB>>, 
    s: Option<&str>, 
    min: T, 
    max: T, 
    label: &str
) -> Option<T> 
where
T: FromStr + PartialOrd + core::fmt::Display + Copy
{
    let val = parse_num(s);

    match val {
        Some(v) if v >= min && v <= max => Some(v),
        Some(_) => {
            usb_writeln!(writer, "Error: {} must be between {} and {}", label, min, max).await.ok();
            None
        }
        None => {
            usb_writeln!(writer, "Error: {} is missing or of incorrect type", label).await.ok();
            None
        }
    }
}