use core::u32;

use defmt::{error, info};
use embassy_time::Timer;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use heapless::Vec;
use embassy_futures::select::{Either6, select6};
use core::fmt::Write;

use crate::realtime::RealTime;
use crate::messages::{MeasureReqMsg, ComputeReqMsg, CommReqMsg, MonReqMsg, IntReqMsg, MeasureResMsg, ComputeResMsg, CommResMsg, MonResMsg, IntResMsg};
use crate::storage::SectorStorage;
use crate::types::{Config, Sector, SectorList, SectorState, MAX_SECTORS};
use crate::{scheduler::*, StorageType};

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum RealtimeStatus {
    NotAvailable = 0,
    Requested = 1,
    Available = 2,
}

#[embassy_executor::task]
pub async fn task_control(
    measure_request_channel: &'static Channel<CriticalSectionRawMutex, MeasureReqMsg, 8>,
    compute_request_channel: &'static Channel<CriticalSectionRawMutex, ComputeReqMsg, 8>,
    comm_request_channel: &'static Channel<CriticalSectionRawMutex, CommReqMsg, 8>,
    mon_request_channel: &'static Channel<CriticalSectionRawMutex, MonReqMsg, 8>,
    int_request_channel: &'static Channel<CriticalSectionRawMutex, IntReqMsg, 8>,
    measure_response_channel: &'static Channel<CriticalSectionRawMutex, MeasureResMsg, 8>,
    compute_response_channel: &'static Channel<CriticalSectionRawMutex, ComputeResMsg, 8>,
    comm_response_channel: &'static Channel<CriticalSectionRawMutex, CommResMsg, 8>,
    mon_response_channel: &'static Channel<CriticalSectionRawMutex, MonResMsg, 8>,
    int_response_channel: &'static Channel<CriticalSectionRawMutex, IntResMsg, 8>,
    storage: &'static StorageType,
) {
    let mut config = Config::default(); // TODO, load from flash
    info!("[cont] starting");
    info!(
        "[cont] configuration: [{}] measurements ({})", 
        config.get_mid_times_as_str().as_str(),
        config.sector_mid_times.len()
    );
    info!("[cont] measurement time: {} s",config.bins_per_sector * config.seconds_per_bin);
    let mut realtime = RealTime::new();
    let scheduler = Scheduler::new();

    // List of tasks
    let mut realtime_status = RealtimeStatus::NotAvailable;
    let mut sectors: SectorList;
    let mut sleep_until_time: Option<u32> = None;
    let mut sleep_until_date: Option<u32> = None;
    let sector_storage = SectorStorage::new();

    // Latest battery and temperature
    let mut battery_mv: Option<u32> = None;
    let mut chip_c: Option<f32> = None;
    let mut charge_state_fraction: u8 = 0;
    let mut reset_charge_state_monitor = false;

    {
        let mut storage_lock = storage.lock().await;
        let storage = storage_lock.as_mut().expect("Storage should be initialized");
        let result = sector_storage.load(storage, true);
        if let Ok(loaded_sectors) = result {
            sectors = loaded_sectors;
            // sectors = SectorList::new();
            // sector_storage.save(storage, &sectors).expect("Should save sectors");

            info!("[cont] loaded {} sectors from storage", sectors.len());
        } else {
            sectors = SectorList::new();
            info!("[cont] no stored sectors found ({}), starting fresh", result.err().unwrap());
        }
    }

    loop {
        // Send out tasks
        if realtime_status == RealtimeStatus::NotAvailable {
            info!("[cont] requesting reference time from GNSS");
            realtime_status = RealtimeStatus::Requested;
            measure_request_channel.send(MeasureReqMsg::GetRefTime).await;
        } else if realtime_status == RealtimeStatus::Requested {
            // waiting for response
        } else if let Some(index) = sectors.get_idx_for_state(SectorState::TO_MEASURE) {
            let measuring_idxs: Vec<usize, MAX_SECTORS> = sectors.get_idxs_for_state(SectorState::MEASURING);

            let sector = sectors.get(index);
            info!("[cont] requesting measurement for sector {}", sector.get_uid());

            let preceding_sector = measuring_idxs.iter().find(|&i| {
                sectors.get(*i).is_succeeding(&sector)
            });
            let sleep_gnss = preceding_sector.is_none();

            let sector = sectors.get_mut(index);
            sector.state = SectorState::MEASURING;
            measure_request_channel.send(MeasureReqMsg::MeasureSector { sector: sector.clone(), config: config.clone(), sleep_gnss }).await;
            sectors.set_changed(true);
        }

        let list_to_compute = sectors.get_idxs_for_state(SectorState::TO_COMPUTE);
        if list_to_compute.len() > 0 {
            for &index in list_to_compute.iter() {
                let sector = sectors.get_mut(index);
                info!("[cont] requesting computation for sector {}", sector.get_uid());
                sector.state = SectorState::COMPUTING;
                compute_request_channel.send(ComputeReqMsg::Compute { sector: sector.clone(), config: config.clone() }).await;
            }
            sectors.set_changed(true);
        }

        let list_to_communicate = sectors.get_idxs_for_state(SectorState::TO_COMMUNICATE);
        if list_to_communicate.len() > 0 {
            let mut sectors_to_send = Vec::<Sector, MAX_SECTORS>::new();
            for &index in list_to_communicate.iter() {
                let sector = sectors.get_mut(index);
                info!("[cont] scheduling communication for sector {}", sector.get_uid());
                sector.state = SectorState::COMMUNICATING;
                sectors_to_send.push(sector.clone()).expect("Should fit");
            }
            sectors.set_changed(true);
            info!("[cont] requesting communication for {} sectors", sectors_to_send.len());
            comm_request_channel.send(CommReqMsg::Send { 
                sectors: sectors_to_send, 
                config: config.clone(), 
                battery_mv: battery_mv, 
                temp_c: chip_c,
                charge_state_fraction
            }).await;
        }

        // Reset charge state monitor
        if reset_charge_state_monitor {
            info!("[cont] Resetting charge state monitor");
            mon_request_channel.send(MonReqMsg::ResetChargeStateMonitor).await;
            reset_charge_state_monitor = false;
        }

        // Wait for responses
        save_sectors(storage, &mut sectors).await;

        let mut timer = if let Some(st) = sleep_until_time && let Some(sd) = sleep_until_date {
            realtime.get_timer(st, sd)
        } else {
            Timer::after_secs(u32::MAX as u64)
        };

        sectors.print_debug();

        info!("[cont] waiting for events or next timer");

        let result = select6(
            &mut timer,
            measure_response_channel.receive(),
            compute_response_channel.receive(),
            comm_response_channel.receive(),
            mon_response_channel.receive(),
            int_response_channel.receive()
        ).await;
        match result {
            Either6::First(_) => {
                info!("[cont] next sector timer expired, getting next sector");
                let sector_awaiting_idx = sectors.get_idx_for_state(SectorState::AWAITING);
                if let Some(idx) = sector_awaiting_idx {
                    let sector: &mut Sector = sectors.get_mut(idx);
                    sector.state = SectorState::TO_MEASURE;

                    let (next_sector, next_start_time, next_start_date) = scheduler.get_next_sector(&config, &realtime, sector);
                    sleep_until_time = Some(next_start_time);
                    sleep_until_date = Some(next_start_date);
                    sectors.push(next_sector);
                    sectors.set_changed(true);
                } else {
                    info!("No sector in AWAITING state when timer expired");
                }
            }
            Either6::Second(measure_res_msg) => {
                match measure_res_msg {
                    MeasureResMsg::RefTimeSuccess { deviation, date} => {
                        info!("[cont] received reference time from GNSS");
                        realtime.update_time(deviation);
                        realtime.update_date(date);
                        realtime_status = RealtimeStatus::Available;
                        let (first_sector, first_start_time, first_start_date) = scheduler.get_first_sector(&config, &realtime);
                        sleep_until_time = Some(first_start_time);
                        sleep_until_date = Some(first_start_date);
                        sectors.push(first_sector);
                        sectors.set_changed(true);
                    },
                    MeasureResMsg::RefTimeFail {} => {
                        info!("[cont] getting ref time failed");
                        // retry should already happen
                    }
                    MeasureResMsg::SectorSuccess { sector_uid, result } => {
                        info!("[cont] received sector measurement success from GNSS");
                        let sector = sectors.get_mut_uid(sector_uid).expect("Sector should exist");
                        sector.state = SectorState::TO_COMPUTE;
                        realtime.update_time(result.deviation);
                        realtime.update_date(result.date);
                        sector.update_coords(result.lat, result.lon);
                        sectors.set_changed(true);
                    },
                    MeasureResMsg::SectorFail { sector_uid, error } => {
                        error!("Measurement failed: {:?}", error);
                        // delete sector
                        let sector_uid_to_delete = {
                            let sector = sectors.get_mut_uid(sector_uid).expect("Sector should exist");
                            sector.get_uid()
                        };
                        sectors.delete_uid(sector_uid_to_delete);
                        sectors.set_changed(true);
                    }
                }
            }
            Either6::Third(compute_res_msg) => {
                match compute_res_msg {
                    ComputeResMsg::Success { sector_uid } => {
                        info!("[cont] received computation success from core 1");
                        let sector = sectors.get_mut_uid(sector_uid).expect("Sector should exist");
                        sector.state = SectorState::TO_COMMUNICATE;
                        sectors.set_changed(true);
                    }
                    ComputeResMsg::ComputeFail { sector_uid, error } => {
                        error!("Computation failed: {}", error);
                        // recompute
                        let sector = sectors.get_mut_uid(sector_uid).expect("Sector should exist");
                        sector.state = SectorState::TO_COMPUTE;
                        sectors.set_changed(true);
                    }
                }
            }
            Either6::Fourth(comm_res_msg) => {
                match comm_res_msg {
                    CommResMsg::Success { sector_uids } => {
                        info!("[cont] received communication success from core 1");
                        for &sector_uid in sector_uids.iter() {
                            sectors.delete_uid(sector_uid);
                        }
                        reset_charge_state_monitor = true;
                        sectors.set_changed(true);
                    }
                    CommResMsg::Fail { sector_uids, error } => {
                        error!("Communication failed: {:?}", error);
                        // recommicate
                        for &sector_uid in sector_uids.iter() {
                            let sector = sectors.get_mut_uid(sector_uid).expect("Sector should exist");
                            sector.state = SectorState::TO_COMMUNICATE;
                        }
                        sectors.set_changed(true);
                    }
                    CommResMsg::ConstellationState { signal_bars_max, signal_level_max, constellation_visible } => {
                        int_request_channel.send(IntReqMsg::ConstellationState { signal_bars_max, signal_level_max, constellation_visible }).await;
                    },
                    CommResMsg::ConstellationStateFail { error } => {
                        int_request_channel.send(IntReqMsg::ConstellationStateFail { error } ).await;
                    }
                }
            }
            Either6::Fifth(mon_res_msg) => {
                match mon_res_msg {
                    MonResMsg::BatVoltSuccess { voltage } => battery_mv = Some(voltage),
                    MonResMsg::BatVoltFail => battery_mv = None,
                    MonResMsg::TempSuccess { temp_c } => chip_c = Some(temp_c),
                    MonResMsg::TempFail => chip_c = None,
                    MonResMsg::ChargeStateFraction { fraction } => charge_state_fraction = fraction,
                }
            }
            Either6::Sixth(int_res_msg) => {
                match int_res_msg {
                    IntResMsg::GetBatVolt => {
                        // Immediately respond with battery voltage
                        // This will need more complex state machine for responses from interface that
                        // need drastic change of control state
                        match battery_mv {
                            Some(voltage) => {
                                int_request_channel.send(IntReqMsg::BatVoltSuccess { voltage }).await;
                            },
                            _ => {
                                int_request_channel.send(IntReqMsg::BatVoltFail).await;
                            }
                        }
                    }
                    IntResMsg::GetConstellationState => {
                        // Request constellation state from the comms task
                        // TODO: Check if comms is actually available
                        comm_request_channel.send(CommReqMsg::GetConstellationState).await;
                    }
                    IntResMsg::GetConfig => {
                        int_request_channel.send(IntReqMsg::GiveConfig {config: config.clone() }).await;
                    }
                    IntResMsg::SetConfig{ config: new_config} => {
                        //TODO abort measurements, computing and communication and empty sectorlist
                        let safe_to_change_config = sectors.iter().all(|s| s.state == SectorState::AWAITING); 

                        if safe_to_change_config{
                            config = new_config; //TODO store in flash
                            sectors.clear();
                            sectors.set_changed(true);
                            realtime_status = RealtimeStatus::NotAvailable;

                            sleep_until_time = None;
                            sleep_until_date = None;

                            info!("[cont] config is updated")
                        } else {
                            info!("[cont] cannot update config")
                        }
                    }
                    IntResMsg::GetState => {
                        let mut s = heapless::String::<2048>::new();
                        writeln!(s, "--- SectorList Report ---").ok();
                        for part in sectors.iter() {
                            writeln!(s, "{:?}", part).ok(); 
                        }
                        writeln!(s, "--- End of Report ---").ok();
                        
                        int_request_channel.send(IntReqMsg::GiveState { str: s }).await;
                    }
                }
            }
        }
    }
}

pub async fn save_sectors(storage: &'static StorageType, sectors: &mut SectorList) {
    // if sectors.len() == 0 {
    //     info!("[cont] no sectors to save, not saving");
    //     return;
    // }
    if sectors.has_changed() == false {
        info!("[cont] no changes to sectors, not saving");
        return;
    }
    sectors.set_changed(false);
    info!("[cont] saving {} sectors to storage", sectors.len());
    let mut storage_lock = storage.lock().await;
    let storage = storage_lock.as_mut().expect("Storage should be initialized");
    let sector_storage = SectorStorage::new();
    sector_storage.save(storage, sectors).expect("Should save sectors");
}