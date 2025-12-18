use embassy_rp::otp::get_chipid;
use embassy_rp::peripherals::USB;
use embassy_rp::usb::Driver;
use embassy_usb::{Builder, Config, UsbDevice};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use static_cell::StaticCell;

use crate::utils::uid_to_bytes;

// Type aliases to keep signatures clean
type MyDriver = Driver<'static, USB>;
pub type MyUsbClass = CdcAcmClass<'static, MyDriver>;
pub type MyUsbDevice = UsbDevice<'static, MyDriver>;

pub struct UsbContext {
    pub class: MyUsbClass,
    pub device: MyUsbDevice,
}

impl UsbContext {
    pub fn new(driver: MyDriver) -> Self {
        static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
        static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
        static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
        static STATE: StaticCell<State> = StaticCell::new();
        static SERIAL_NUMBER: StaticCell<[u8; 16]> = StaticCell::new();

        let serial_bytes = uid_to_bytes(get_chipid().unwrap());
        let static_serial = core::str::from_utf8(SERIAL_NUMBER.init(serial_bytes)).unwrap(); 

        let config = {
            // Vendor ID: 1209 (pid.codes), Product ID: 0001 (Test PID)
            let mut config = Config::new(0x1209, 0x0001); 
            config.manufacturer = Some("TAHMO");
            config.product = Some("TAHMO WLM");
            config.serial_number = Some(static_serial);
            config.max_power = 500;
            config.max_packet_size_0 = 64;
            config
        };

        let mut builder = Builder::new(
            driver,
            config,
            CONFIG_DESC.init([0; 256]),
            BOS_DESC.init([0; 256]),
            &mut [],
            CONTROL_BUF.init([0; 64]),
        );

        let class = {
            static STATE: StaticCell<State> = StaticCell::new();
            let state = STATE.init(State::new());
            CdcAcmClass::new(&mut builder, state, 64)
        };

        let device = builder.build();

        Self { class, device }
    }
}

#[embassy_executor::task]
pub async fn usb_task(mut device: MyUsbDevice) -> ! {
    device.run().await
}
