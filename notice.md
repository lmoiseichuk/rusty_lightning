# Notice

lightning is under the MIT License -- see [LICENSE](LICENSE) -- except the parts listed below, which keep their own.

| Part | Where | Licence |
|---|---|---|
| The AS3935 register driver, ported to Rust from DFRobot's MicroPython library `DFRobot_AS3935_Lib.py` and its example `DFRobot_AS3935_detailed.py` | `src/as3935.rs` | MIT, Copyright (c) 2020 DFRobot; the example's own header says GNU Lesser General Public License |
| Modified copies of that DFRobot library and example | `doc/experiments/DFRobot_AS3935_Lib.py`, `doc/experiments/deep_demo.py` | GNU Lesser General Public License, as their headers say, Copyright DFRobot 2018 |
| The civil-date algorithm, reworked from Howard Hinnant's `civil_from_days` | `src/civil.rs` | public domain |
| The values of ESP-IDF v5.3.4's `WIFI_INIT_CONFIG_DEFAULT()`, written out by hand from `esp_wifi.h` | `src/portal.rs`, `default_wifi_init_config` | Apache-2.0, Espressif |
| The C6 reflash program, reworked from Espressif's esp-serial-flasher `examples/esp32_sdio_example` | `station/c6_reflash/main.c` | Unlicense OR CC0-1.0, Espressif |
| GitHub's `Rust.gitignore` template | `.gitignore`, lines 1-21 | CC0 1.0 |

Firmware built from this repository also contains third-party libraries -- ESP-IDF and its components, the Rust standard library and the Rust crates it depends on -- each under its own licence, as its source states.
