use defmt::info;
use defmt::Format;
use embassy_futures::select::Either;
use embassy_time::with_timeout;
use embassy_time::Duration;
use embassy_futures::select::select;
use embassy_time::Timer;

use crate::gpio;
use crate::adc;

pub struct Battery {
    pin_low_bat: gpio::Input<'static>,
    pin_powergood: gpio::Input<'static>,
    pin_status: gpio::Input<'static>,
    pin_voltage: adc::Channel<'static>,
    pin_temp: adc::Channel<'static>,
    adc: adc::Adc<'static, adc::Async>,
}

impl Battery {
    pub fn new(
            pin_low_bat: gpio::Input<'static>,
            pin_powergood: gpio::Input<'static>, 
            pin_status: gpio::Input<'static>, 
            pin_voltage: adc::Channel<'static>, 
            pin_temp: adc::Channel<'static>,
            adc: adc::Adc<'static, adc::Async>,
            ) -> Self {
        Battery { 
            pin_low_bat, 
            pin_powergood, 
            pin_status, 
            pin_voltage, 
            pin_temp,
            adc,
        }
    }

    pub async fn get_battery_voltage(&mut self) -> Result<u32, adc::Error> {
        const NUM_SAMPLES: usize = 5;
        const SAMPLE_DELAY_MS: u64 = 10;

        let mut sum: u32 = 0;
        let mut actual_num: usize = 0;

        for _ in 0..NUM_SAMPLES {
            match with_timeout(Duration::from_millis(100), self.adc.read(&mut self.pin_voltage)).await {
                Ok(res) => {
                    sum += res? as u32;
                    actual_num += 1;
                },
                Err(e) => {}
            };
            embassy_time::Timer::after_millis(SAMPLE_DELAY_MS).await;
        }
        
        if actual_num == 0 {
            return Err(adc::Error::ConversionFailed);
        }

        // Compute average ADC reading
        let avg_raw = sum as f32 / actual_num as f32;

        // Convert to mV (assuming 3.3 V reference, 12-bit ADC)
        let voltage_mv = avg_raw * 3300.0 / 4096.0;

        // Compensate for 1:1 voltage divider
        Ok((voltage_mv * 2.0) as u32)
    }

    pub async fn get_chip_temperature(&mut self) -> Result<f32, adc::Error> {
        const NUM_SAMPLES: usize = 5;
        const SAMPLE_DELAY_MS: u64 = 10;

        let mut sum: u32 = 0;
        let mut actual_num: usize = 0;

        for _ in 0..NUM_SAMPLES {
            match with_timeout(Duration::from_millis(100), self.adc.read(&mut self.pin_temp)).await {
                Ok(res) => {
                    sum += res? as u32;
                    actual_num += 1;
                },
                Err(e) => {}
            };
            embassy_time::Timer::after_millis(SAMPLE_DELAY_MS).await;
        }

        if actual_num == 0 {
            return Err(adc::Error::ConversionFailed);
        }

        // Compute average ADC reading
        let avg_raw = sum as f32 / actual_num as f32;

        // Convert to mV (assuming 3.3 V reference, 12-bit ADC)
        let temp_c = convert_to_celsius(avg_raw);

        Ok(temp_c)
    }

    pub async fn get_state(&mut self) -> (ChargeState, bool) {
        let mut timer_future = Timer::after(Duration::from_millis(1200));
        let mut edge_future = self.pin_status.wait_for_any_edge();

        let mut edge_count = 0;
        loop {
            match select( edge_future, &mut timer_future).await {
                Either::First(_) => {
                    edge_count +=1;
                    edge_future = self.pin_status.wait_for_any_edge();
                },
                Either::Second(_) => {
                    break;
                }
            }
        }

        let pin_conditions = (edge_count >= 2, self.pin_powergood.is_high(), self.pin_status.is_high());

        let chargestate = match pin_conditions {
            (true, _, _) => ChargeState::Fault,    // Fault takes priority
            (false, true, _) => ChargeState::NoInput,
            (false, false, false) => ChargeState::Charging,
            (false, false, true) => ChargeState::Standby,

        };

        let bat_low = self.pin_low_bat.is_low();

        (chargestate, bat_low) //Bat low
    }

}

fn convert_to_celsius(raw_temp: f32) -> f32 {
    // According to chapter 12.4.6 Temperature Sensor in RP235x datasheet
    let temp = 27.0 - (raw_temp as f32 * 3.3 / 4096.0 - 0.706) / 0.001721;
    let sign = if temp < 0.0 { -1.0 } else { 1.0 };
    let rounded_temp_x10: i16 = ((temp * 10.0) + 0.5 * sign) as i16;
    (rounded_temp_x10 as f32) / 10.0
}

#[derive(Format, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChargeState {
    Unknown,
    Fault, // Stat=Blinking (charging is blocked)
    NoInput, // PG=High (no power available)
    Charging, // PG=Low, Stat=Low (power available and charging)
    Standby // PG=Low, Stat=High (power available not charging)
}