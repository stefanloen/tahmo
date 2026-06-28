use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_futures::select::{select3, Either3};
use defmt::info;
use embassy_time::{Duration, Instant, Timer};
use core::cmp::max;
use crate::{battery::{Battery, ChargeState}, messages::{MonReqMsg, MonResMsg}};

const ADC_POLL_INTERVAL: u64 = 30;
const CHARGE_POLL_INTERVAL: u64 = 30;

#[embassy_executor::task]
pub async fn task_monitor(
    channel_req: &'static Channel<CriticalSectionRawMutex, MonReqMsg, 8>,
    channel_res: &'static Channel<CriticalSectionRawMutex, MonResMsg, 8>,
    mut battery: Battery,
) {
    let mut battery_mv: f32 = 0.0;
    let mut chip_temp_c: f32 = 0.0;
    let mut next_adc_measurement = Instant::now();

    let mut charge_state_monitor = ChargeStateMonitor::new();
    let mut next_charge_poll = Instant::now();

    info!("[moni] starting");
    
    loop {
        let mut adc_timer = Timer::at(next_adc_measurement);
        let mut charge_timer = Timer::at(next_charge_poll);

        let result = select3(
            &mut adc_timer,
            channel_req.receive(),
            &mut charge_timer
        ).await;
        match result {
            Either3::First(_) => {
                let start = Instant::now();
                match battery.get_battery_voltage().await {
                    Ok(volts) => {
                        battery_mv = calc_emwa(volts as f32, battery_mv, 0.02, 0.0, 4200.0);
                        info!("[moni] Battery millivolts {} (emwa {}) in {} ms", volts, battery_mv as u32, (Instant::now() - start).as_millis());
                        channel_res.send(MonResMsg::BatVoltSuccess { voltage: battery_mv as u32 }).await;
                    }
                    Err(_) => {
                        channel_res.send(MonResMsg::BatVoltFail).await;
                    }
                }

                let start = Instant::now();
                match battery.get_chip_temperature().await {
                    Ok(temp) => {
                        chip_temp_c = calc_emwa(temp, chip_temp_c, 0.02, -50.0, 85.0);
                        info!("[moni] Chip temperature: {} (emwa {}) in {} ms", temp, chip_temp_c, (Instant::now() - start).as_millis());
                        channel_res.send(MonResMsg::TempSuccess { temp_c: chip_temp_c }).await;
                    }
                    Err(_) => {
                        channel_res.send(MonResMsg::TempFail).await;
                    }
                }

                next_adc_measurement = next_adc_measurement.saturating_add(Duration::from_secs(ADC_POLL_INTERVAL));
            }
            Either3::Second(message) => {
                match message {
                    MonReqMsg::GetBatVolt => {
                        channel_res.send(MonResMsg::BatVoltSuccess { voltage: battery_mv as u32 }).await;
                    }
                    MonReqMsg::GetTemp => {
                        channel_res.send(MonResMsg::TempSuccess { temp_c: chip_temp_c }).await;
                    }
                    MonReqMsg::ResetChargeStateMonitor => {
                        charge_state_monitor.reset();
                    }

                }
            }
            Either3::Third(_) => {
                let (charge_state, bat_low) = battery.get_state().await;
                
                info!("[moni] Charge controller state: {:?}", charge_state);
                charge_state_monitor.set_state(charge_state);

                let statefraction = charge_state_monitor.get_fraction();
                channel_res.send(MonResMsg::ChargeState { fraction: statefraction, bat_low: bat_low }).await;

                let (a,b,c,d) = unpack_fractions(statefraction);
                info!(
                    "[moni] ChargeState fraction bytes: Fault: {:?}, NoInput: {:?}, Charging: {:?}, Standby: {:?}",
                    a, b, c, d
                );
                info!("[moni] Battery low: {:?}", bat_low);

                next_charge_poll = next_charge_poll.saturating_add(Duration::from_secs(CHARGE_POLL_INTERVAL));
            }
        }
    }
}

fn calc_emwa(now: f32, previous: f32, k: f32, min: f32, max: f32) -> f32 {
    if now <= min {
        previous
    } else if now >= max {
        previous
    } else if previous == 0.0 {
        now
    } else {
        now * k + previous * (1.0-k)
    }
}

struct ChargeStateMonitor {
    time_fault: u64,
    time_noinput: u64,
    time_charging: u64,
    time_standby: u64,

    last_time: Instant,
    current_state: ChargeState,
}

impl ChargeStateMonitor {
    pub fn new () -> Self{
        Self {
            time_fault: 0,
            time_noinput: 0,
            time_charging: 0,
            time_standby: 0,
            last_time: Instant::now(),
            current_state: ChargeState::Unknown
        }
    }

    pub fn reset(&mut self) {
        self.time_fault = 0;
        self.time_noinput = 0;
        self.time_charging = 0;
        self.time_standby = 0;

        self.last_time = Instant::now();
        self.current_state = ChargeState::Unknown;
    }

    pub fn set_state(&mut self, charge_state: ChargeState){
        let passed_time = (Instant::now() - self.last_time).as_secs();

        //Make sure that even <1 seconds in a state is represented
        match self.current_state {
            ChargeState::Unknown => {},
            ChargeState::Fault => self.time_fault = (self.time_fault+passed_time).max(1),
            ChargeState::NoInput => self.time_noinput = (self.time_noinput+passed_time).max(1),
            ChargeState::Charging => self.time_charging = (self.time_charging+passed_time).max(1),
            ChargeState::Standby => self.time_standby = (self.time_standby+passed_time).max(1),
        }

        self.current_state = charge_state;
        self.last_time = Instant::now();
    }

    pub fn get_state(&mut self) -> ChargeState {
        self.current_state
    }

    pub fn get_fraction(&mut self) -> [u8; 4] {
        // Function needs minimal changing to allow use of less bytes
        self.set_state(self.current_state);

        let max_time = max(
            max(self.time_fault, self.time_noinput),
            max(self.time_charging, self.time_standby),
        ) as f32;

        if max_time == 0.0 {
            return [0, 0, 0, 0];
        } 

        let a = self.time_fault as f32 / max_time;
        let b = self.time_noinput as f32 / max_time;
        let c = self.time_charging as f32 / max_time;
        let d = self.time_standby as f32 / max_time;

        let qa = non_std_ceil(a * 255.0).min(255.0) as u8;
        let qb = non_std_ceil(b * 255.0).min(255.0) as u8;
        let qc = non_std_ceil(c * 255.0).min(255.0) as u8;
        let qd = non_std_ceil(d * 255.0).min(255.0) as u8;

        [qa, qb, qc, qd]
    }

}

fn non_std_ceil(x: f32) -> f32 {
    let truncated = x as i32 as f32;

    if x == truncated {
        x
    } else if x > 0.0 {
        truncated + 1.0
    } else {
        truncated
    }
}

fn unpack_fractions(bytes: [u8; 4]) -> (u8, u8, u8, u8) {
    let a = bytes[0];
    let b = bytes[1];
    let c = bytes[2];
    let d = bytes[3];
    (a, b, c, d)
}