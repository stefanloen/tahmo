use embassy_rp::otp::get_chipid;
use embassy_rp::peripherals::USB;
use embassy_rp::usb::Driver;
use embassy_usb::{Builder, Config, UsbDevice};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use static_cell::StaticCell;
use heapless::String;

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
            let state = STATE.init(State::new());
            CdcAcmClass::new(&mut builder, state, 64)
        };

        let device = builder.build();

        Self { class, device }
    }
}

#[macro_export]
macro_rules! usb_write {
    ($writer:expr, $($arg:tt)*) => {
        $writer.write_fmt(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! usb_writeln {
    ($writer:expr, $($arg:tt)*) => {
        $writer.writeln_fmt(format_args!($($arg)*))
    };
}

pub struct UsbWriter<'a, D: embassy_usb::driver::Driver<'static>> {
    sender: &'a mut embassy_usb::class::cdc_acm::Sender<'static, D>,
}

impl<'a, D: embassy_usb::driver::Driver<'static>> UsbWriter<'a, D> {
    pub fn new(sender: &'a mut embassy_usb::class::cdc_acm::Sender<'static, D>) -> Self {
        Self { sender }
    }

    pub async fn write_fmt(&mut self, args: core::fmt::Arguments<'_>) -> Result<(), ()> {
        let mut buf: String<512> = String::new();
        if core::fmt::write(&mut buf, args).is_ok() {
            for chunk in buf.as_bytes().chunks(64) {
                self.sender.write_packet(chunk).await.map_err(|_| ())?;
            }
            Ok(())
        } else {
            Err(())
        }
    }

    pub async fn writeln_fmt(&mut self, args: core::fmt::Arguments<'_>) -> Result<(), ()> {
        self.write_fmt(args).await?;
        self.sender.write_packet(b"\r\n").await.map_err(|_| ())?;
        Ok(())
    }
}

#[embassy_executor::task]
pub async fn usb_task(mut device: MyUsbDevice) -> ! {
    device.run().await
}
