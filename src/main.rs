use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use crc::{Crc, CRC_16_MODBUS};
use embedded_svc::mqtt::client::{Connection, Publish, QoS};
use embedded_svc::wifi::{ClientConfiguration, Configuration, Wifi};
use esp_idf_hal::gpio;
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::serial::{config, Pins, Serial};
use esp_idf_hal::units::Hertz;
use esp_idf_svc::mqtt::client::{EspMqttClient, MqttClientConfiguration};
use esp_idf_svc::netif::EspNetifStack;
use esp_idf_svc::nvs::EspDefaultNvs;
use esp_idf_svc::sysloop::EspSysLoopStack;
use esp_idf_svc::wifi::EspWifi;
use esp_idf_sys::*;
use log::*;
use serde::{Deserialize, Serialize};

const CRC_MODBUS: Crc<u16> = Crc::<u16>::new(&CRC_16_MODBUS);

#[toml_cfg::toml_config]
pub struct Config {
    #[default("")]
    mqtt_user: &'static str,
    #[default("")]
    mqtt_pass: &'static str,
    #[default("")]
    broker_url: &'static str,
    #[default("")]
    wifi_ssid: &'static str,
    #[default("")]
    wifi_pass: &'static str,
    #[default("")]
    discovery_topic: &'static str,
    #[default("")]
    state_topic: &'static str,
}

#[derive(Serialize, Deserialize, Debug)]
struct DiscoveryMessage<'a> {
    name: &'a str,
    state_topic: &'a str,
    state_class: &'a str,
    device_class: &'a str,
    unit_of_measurement: &'a str,
    unique_id: &'a str,
}

fn main() -> Result<()> {
    esp_idf_sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    let app_config = CONFIG;
    info!("Starting mqtt sensor...");

    let netif_stack = Arc::new(EspNetifStack::new()?);
    let sys_loop_stack = Arc::new(EspSysLoopStack::new()?);
    let default_nvs = Arc::new(EspDefaultNvs::new()?);
    let mut wifi = EspWifi::new(netif_stack, sys_loop_stack, default_nvs)?;
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: app_config.wifi_ssid.into(),
        password: app_config.wifi_pass.into(),
        ..Default::default()
    }))?;
    wifi.wait_status(|status| !status.is_transitional());

    let mut mac_addr: [u8; 6] = [0; 6];
    unsafe {
        esp_read_mac(mac_addr.as_mut_ptr(), esp_mac_type_t_ESP_MAC_WIFI_SOFTAP);
    }
    let device_id = String::from_iter(mac_addr.map(|v| format!("{:x}", v)));
    let mqtt_discovery_topic = String::from(app_config.discovery_topic).replace("{}", &device_id);
    let mqtt_state_topic = String::from(app_config.state_topic).replace("{}", &device_id);
    let discovery_message = DiscoveryMessage {
        name: "CO2 Sensor",
        state_topic: &mqtt_state_topic,
        state_class: "measurement",
        device_class: "carbon_dioxide",
        unit_of_measurement: "ppm",
        unique_id: &device_id,
    };
    let discovery_payload = serde_json::to_vec(&discovery_message)?;

    let conf = MqttClientConfiguration {
        crt_bundle_attach: Some(esp_idf_sys::esp_crt_bundle_attach),
        username: Some(app_config.mqtt_user),
        password: Some(app_config.mqtt_pass),
        keep_alive_interval: Some(Duration::from_secs(30)),
        ..Default::default()
    };

    let peripherals = Peripherals::take().unwrap();
    let uart1 = peripherals.uart1;
    let tx = peripherals.pins.gpio2;
    let rx = peripherals.pins.gpio3;

    let serial = Serial::new(
        uart1,
        Pins {
            tx,
            rx,
            cts: None::<gpio::GpioPin<gpio::Input>>,
            rts: None::<gpio::GpioPin<gpio::Output>>,
        },
        config::Config::new().baudrate(Hertz(9600)),
    )?;

    let (mut tx, mut rx) = serial.split();
    loop {
        match EspMqttClient::new_with_conn(app_config.broker_url, &conf) {
            Ok((mut client, mut connection)) => {
                thread::spawn(move || {
                    while let Some(msg) = connection.next() {
                        match msg {
                            Err(e) => info!("MQTT Message ERROR: {}", e),
                            Ok(_) => {},
                        }
                    }
                });
                if let Err(e) = client.publish(
                    &mqtt_discovery_topic,
                    QoS::AtLeastOnce,
                    false,
                    &discovery_payload,
                ) {
                    warn!("public discovery error {}", e);
                    thread::sleep(Duration::from_secs(30));
                    continue;
                }
                loop {
                    if let Err(e) =
                        tx.write_bytes(&[0xfe, 0x04, 0x00, 0x03, 0x00, 0x01, 0xd5, 0xc5])
                    {
                        warn!("write sensor error {:?}", e);
                        thread::sleep(Duration::from_secs(5));
                        continue;
                    }
                    let mut buff: [u8; 7] = [0; 7];
                    if let Err(e) = rx.read_bytes_blocking(&mut buff, Duration::from_secs(1)) {
                        warn!("read sensor error {:?}", e);
                        thread::sleep(Duration::from_secs(5));
                        continue;
                    }
                    let value = buff[3] as u16 * 256 + buff[4] as u16;
                    let checksum = CRC_MODBUS.checksum(&buff[..5]);
                    let data_checksum = buff[6] as u16 * 256 + buff[5] as u16;
                    if checksum == data_checksum {
                        println!("co2: {} ppm", value);
                        match client.publish(
                            &mqtt_state_topic,
                            QoS::AtLeastOnce,
                            false,
                            format!("{}", value).as_ref(),
                        ) {
                            Ok(_) => {
                                info!("public ok")
                            }
                            Err(e) => {
                                warn!("public error {}", e);
                                break;
                            }
                        }
                    }
                    thread::sleep(Duration::from_secs(10));
                }
            }
            Err(e) => {
                warn!("mqtt connect with error {:?}", e);
                thread::sleep(Duration::from_secs(30));
            }
        }
    }
}
