use defmt::info;
use crate::usb::MyUsbClass;

pub struct EchoInterface {
    class: MyUsbClass,
}

impl EchoInterface {
    pub fn new(class: MyUsbClass) -> Self {
        Self { class }
    }

    pub async fn run(&mut self) -> ! {
        let mut buf = [0; 64];
        loop {
            // Wait for the USB cable to be plugged in and port opened
            self.class.wait_connection().await;
            info!("Connected");

            loop {
                match self.class.read_packet(&mut buf).await {
                    Ok(n) => {
                        let data = &buf[..n];
                        if self.class.write_packet(data).await.is_err() { break; }
                        if self.class.write_packet(b" - echoed\r\n").await.is_err() { break; }
                    }
                    Err(_) => break, // Connection lost
                }
            }
            info!("Disconnected");
        }
    }
}

#[embassy_executor::task]
pub async fn interface_task(class: MyUsbClass) {
    let mut echo = EchoInterface::new(class);
    echo.run().await;
}