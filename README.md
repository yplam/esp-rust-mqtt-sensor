# esp-rust-mqtt-sensor
HomeAssistant MQTT CO2 sensor running on ESP32C3 in Rust

## Schematic

ESP32 ---- SenseAir S8

```
ESP32 gpio2 -> S8 UART_RxD
ESP32 gpio3 <- S8 UART_TxD
```


## Build and run

Edit config options in cfg.toml and then:

```
$ source ~/esp/esp-idf/export.sh
$ cargo build --release
$ espflash --monitor /dev/ttyACM0 target/riscv32imc-esp-espidf/release/esp-rust-mqtt-sensor

```
