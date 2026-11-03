# Rewriting the FireBeetle's ESP32-C6 from the P4

The FireBeetle 2 ESP32-P4's on-board ESP32-C6 is its Wi-Fi. On our board it
shipped with an image it could not boot. The P4 then reports
`sdmmc_card_init failed` and error 0x107 when esp_hosted tries to reach it.
DFRobot's forum has the same fault on the DFR1172: the image was written at
0x10000 instead of 0x0.

This rewrites the C6 from the P4 itself, over the SDIO lines the two already
share. No UART wires: only the C6's IO9 strap has to be held low by hand,
because it is not wired to the P4. It was done on 2026-11-02, and Wi-Fi came up
at once.

## The pads

Six round test pads sit beside the C6, unlabelled. The schematic (DFR1172 V1.0,
page 7) names them C6_RX0, C6_TX0, C6_IO9, C6_RST, ESP_3V3 and GND, but the
board does not lay them out in that order. **IO9 and GND are the two pads at
opposite ends of the group.** Hold IO9 to GND through about 1 kΩ: through the
resistor, touching a wrong pad does no harm.

## Steps

1. **The C6's firmware:** esp-hosted 2.7.0's slave, for the ESP32-C6, over SDIO.
   That is the pairing the station's `esp_hosted = 2.7.0` host expects. It needs
   an ESP-IDF v5.5.5 environment:

       idf.py create-project-from-example "espressif/esp_hosted==2.7.0:slave"
       cd slave && idf.py set-target esp32c6 && idf.py build

2. **The flasher:** esp-serial-flasher's `examples/esp32_sdio_example` (built at
   commit ddf3cdd8fa87, 2026-10-22). Clone the repository into a directory
   named `esp-serial-flasher`: ESP-IDF names the component after its directory.
   Then make four changes to the example:
   - put this folder's `main.c` in place of the example's `main/main.c`;
   - copy the slave's `build/bootloader/bootloader.bin`,
     `build/partition_table/partition-table.bin`, `build/network_adapter.bin`
     (as `app.bin`) and `build/ota_data_initial.bin` into `target-firmware/`;
   - add `ota_data_initial.bin` to the list in `main/CMakeLists.txt`;
   - append to `sdkconfig.defaults`:

         CONFIG_ESP32P4_SELECTS_REV_LESS_V3=y
         CONFIG_ESP32P4_REV_MIN_100=y
         CONFIG_ESPTOOLPY_FLASHSIZE_16MB=y
         CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y

   Build it for `esp32p4` and flash it to the P4 over its USB.

3. **The hold:** with the P4 running the flasher and its log open, hold IO9 to
   GND. Within a second or two the log says `connected: the C6 is in download
   mode`. Let go; the P4 writes and verifies the four images in about five
   seconds and resets the C6.

4. **Back to the station:** flash `wifi_check` again. The P4's log should say
   `Identified slave [esp32c6]`, then a scan.

## What did not work first

| Tried | Outcome |
| --- | --- |
| esp_hosted with the P4's internal pull-ups on the SDIO lines | no change: not the cause |
| The SDIO clock at 10 MHz and then 400 kHz | no change: not signal integrity |
| The flasher on a 1-bit, 5 MHz bus | no reply at all from the C6 in download mode |
| The flasher at 4-bit, 20 MHz or 5 MHz, with IO9 not yet found | sporadic CRC errors (0x109) whether IO9 was held or not; the hold went to the wrong pad |
| 4-bit, 5 MHz, IO9 and GND on the end pads | connected and written at once |
