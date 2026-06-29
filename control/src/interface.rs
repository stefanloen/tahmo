use defmt::info;
use embassy_futures::select::{Either3, select3};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::Timer;
use core::str;
use core::str::FromStr;
use heapless::Vec;

use crate::messages::{IntReqMsg, IntResMsg};
use crate::usb::{MyUsbClass, UsbWriter};
use crate::{StorageType, usb_write, usb_writeln, utils};
use crate::types::{MAX_MIDPOINTS, SECONDS_PER_DAY};
use crate::{compute::{Record, BUF_BYTES}, storage::{BinStorage, MeasurementStorage, SectorStorage}, types::{CONTAINER_SIZE, NUM_BINS, NUM_MEASUREMENTS}};


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
    GetTemp,
    GetConst,
    GetConfig,
    GetState,

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
    storage: &'static StorageType
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
                    info!("[intf] Terminal Disconnected, abort getting battery voltage");
                } else if !receiver.dtr() && let InterfaceState::GetTemp = interface.state{
                    interface.state = InterfaceState::Disconnected;
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
gettemp - Get chip temperature
getconst - Get the constellation state
getconfig - Get the current configuration
setconfig - Set configuration
download - Download memory
").await.ok();
                            }
                            "status" => {
                                channel_res.send(IntResMsg::GetState).await;
                                interface.state = InterfaceState::GetState;
                            }
                            "batvolt" => {
                                channel_res.send(IntResMsg::GetBatVolt).await;
                                interface.state = InterfaceState::Batvolt;
                            }
                            "gettemp" => {
                                channel_res.send(IntResMsg::GetTemp).await;
                                interface.state = InterfaceState::GetTemp;
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
measurements <timepoint> <n> <duration>, example use: 'setconfig measurements 00:00:00 4 60'
").await.ok();
                                    }
                                    _ => {
                                        usb_writeln!(writer, "Unknown or missing setting, for a list of settings, use 'setconfig help' or 'setconfig ?'").await.ok();
                                    }
                                }
                            }

                            "download" => {
                                download(storage, &mut writer).await;
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
                    InterfaceState::GetTemp => {
                        match request {
                            IntReqMsg::TempSuccess {temp} => {
                                usb_writeln!(writer, "Chip temperature: {:.0}C", {temp}).await.ok();
                                interface.state = InterfaceState::Idle;
                            },
                            IntReqMsg::TempFail =>{
                                usb_writeln!(writer, "Could not get chip temperature").await.ok();
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
                    },
                    InterfaceState::GetState {} => {
                        match request {
                            IntReqMsg::GiveState { str } => {
                                for line in str.lines() {
                                    usb_writeln!(writer, "{}", line).await.ok();
                                }
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

pub async fn download(storage: &'static StorageType, writer: &mut UsbWriter<'_, embassy_rp::usb::Driver<'static, embassy_rp::peripherals::USB>>) {
    let mut storage_lock = storage.lock().await;
    let storage = storage_lock.as_mut().expect("Storage should be initialized");

    let sector_storage = SectorStorage::new();
    let sectors = sector_storage.load(storage, false);
    if let Ok(sectors) = sectors {
        usb_writeln!(writer, "SECTOR:idx, uid, state, midpoint_idx, measurement_idx, start_bin_idx, start_time, lat, lon").await.ok();
        for (i, sector) in sectors.iter().enumerate() {
            usb_writeln!(
                writer, 
                "SECTOR:{}, {}, {:?}, {}, {}, {}, {}, {}, {}", 
                i,
                sector.get_uid(),
                sector.state,
                sector.get_midpoint_index(),
                sector.get_measurement_index(),
                sector.get_start_bin_index(),
                sector.get_start_time(),
                sector.get_lat(),
                sector.get_lon(),
            ).await.ok();
        }
    }

    usb_writeln!(writer, "MEASUREMENT:idx, uid, mean, std, num_seen, start_time, end_time, lat, lon").await.ok();
    usb_writeln!(writer, "OBSERVATION:meas_idx, obs_idx, sat_id, start_time, end_time, used, max_rh, max_amp, mean_amp, num_recs, max_rh_2, max_amp_2").await.ok();
    let measurement_storage = MeasurementStorage::new();
    for i in 0..NUM_MEASUREMENTS {
        if let Some(measurement) = measurement_storage.read(storage, i as u32) {
            if measurement.uid == 0 {
                continue;
            }
            usb_writeln!(writer, "MEASUREMENT:{}, {}, {}, {}, {}, {}, {}, {}, {}",
                i,
                measurement.uid,
                measurement.mean,
                measurement.std,
                measurement.num_seen,
                measurement.start_time,
                measurement.end_time,
                measurement.lat,
                measurement.lon
            ).await.ok();
            for (j, observation) in measurement.observations.iter().enumerate() {
                if observation.sat_id == u16::MAX {
                    continue;
                }
                usb_writeln!(writer,"OBSERVATION:{}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}",
                    i,
                    j,
                    observation.sat_id,
                    observation.start_time,
                    observation.end_time,
                    observation.used,
                    observation.max_rh,
                    observation.max_amp,
                    observation.mean_amp,
                    observation.num_recs,
                    observation.max_rh_2,
                    observation.max_amp_2
                ).await.ok();

                Timer::after_millis(1).await;
            }
        }
    }

    usb_writeln!(writer, "DATA:bin_idx,time,id,satellite,network,band,elevation,azimuth,snr").await.ok();
    let bin_storage = BinStorage::new();

    for i in 0..NUM_BINS {
        let mut buffer = [0u8; CONTAINER_SIZE];
        let result = bin_storage.read(storage, i as u32, &mut buffer);
        if let Ok(_) = result {
            let mut words = buffer.chunks_exact(4);

            while let Some(hdr_b) = words.next() {
                let header = u32::from_le_bytes([hdr_b[0], hdr_b[1], hdr_b[2], hdr_b[3]]);
                let time = (header >> 8) as u16;
                let num = (header & 0xFF) as u8;

                if time == u16::MAX || num == 0 {
                    continue;
                }

                for _ in 0..num {
                    if let Some(smp_b) = words.next() {
                        let sample = Record::from_sample(u32::from_le_bytes([smp_b[0], smp_b[1], smp_b[2], smp_b[3]]));

                        usb_writeln!(writer,"DATA:{},{},{},{},{},{},{},{},{}",
                            i,
                            time,
                            sample.get_id(),
                            sample.get_satellite(),
                            sample.get_network(),
                            sample.get_band(),
                            sample.get_elevation(),
                            sample.get_azimuth(),
                            sample.get_snr()
                        ).await.ok();
                    } else {
                        break;
                    }
                }

                Timer::after_millis(1).await;
            }
        } else {
            info!("[main] no data for bin {}: {:?}", i, result.err().unwrap());
        }
    }

    usb_writeln!(writer, "download finished").await.ok();
}