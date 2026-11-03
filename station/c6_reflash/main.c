/* Reflash the FireBeetle 2 ESP32-P4's on-board ESP32-C6 over SDIO, from the P4.
 *
 * Based on esp-serial-flasher's esp32_sdio_example (Unlicense / CC0). The C6's
 * IO9 strap is not wired to the P4, so it has to be held low by hand (the test
 * pad, through about 1 kOhm, to GND) while this program resets the C6 through
 * GPIO54. It keeps trying until then; once the C6 comes up in download mode it
 * writes esp-hosted 2.7.0's slave firmware (bootloader at 0x0, partition table,
 * OTA data, app at 0x10000) and resets the C6 into it. */

#include <stdio.h>
#include "esp_log.h"
#include "esp32_sdio_port.h"
#include "esp_loader.h"
#include "example_common.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

extern const uint8_t bootloader_bin[];
extern const uint32_t bootloader_bin_size;
extern const uint8_t partition_table_bin[];
extern const uint32_t partition_table_bin_size;
extern const uint8_t ota_data_initial_bin[];
extern const uint32_t ota_data_initial_bin_size;
extern const uint8_t app_bin[];
extern const uint32_t app_bin_size;

static const char *TAG = "c6_reflash";

#define ATTEMPTS 100000 /* until IO9 is held: the C6 answered with CRC errors at 20 MHz 4-bit */
#define OTA_DATA_ADDRESS 0xd000

void app_main(void)
{
    esp_loader_t loader;
    esp32_sdio_port_t port = {
        .port.ops     = &esp32_sdio_ops,
        .slot         = SDMMC_HOST_SLOT_1,
        .max_freq_khz = 5000,         /* 4-bit but slow: at 20 MHz the data came back with CRC errors */
        .reset_pin    = GPIO_NUM_54, /* the C6's EN (DFRobot schematic V1.0) */
        .boot_pin     = GPIO_NUM_3,  /* not the C6's IO9: the board's user LED, a harmless stand-in */
        .bus_width    = SDIO_4BIT,
        .sdio_d0_pin  = GPIO_NUM_14,
        .sdio_d1_pin  = GPIO_NUM_15,
        .sdio_d2_pin  = GPIO_NUM_16,
        .sdio_d3_pin  = GPIO_NUM_17,
        .sdio_clk_pin = GPIO_NUM_18,
        .sdio_cmd_pin = GPIO_NUM_19,
    };

    if (esp_loader_init_sdio(&loader, &port.port) != ESP_LOADER_SUCCESS) {
        ESP_LOGE(TAG, "SDIO initialisation failed");
        return;
    }

    for (int attempt = 1; attempt <= ATTEMPTS; attempt++) {
        if (attempt % 20 == 1) {
            ESP_LOGI(TAG, "attempt %d: hold the C6's IO9 pad to GND now", attempt);
        }
        if (connect_to_target(&loader, 0) != ESP_LOADER_SUCCESS) {
            vTaskDelay(pdMS_TO_TICKS(500));
            continue;
        }
        ESP_LOGW(TAG, "connected: the C6 is in download mode -- IO9 can be let go");
        target_chip_t chip = esp_loader_get_target(&loader);
        ESP_LOGI(TAG, "writing the bootloader at 0x%lx", (unsigned long)get_bootloader_address(chip));
        flash_binary(&loader, bootloader_bin, bootloader_bin_size, get_bootloader_address(chip));
        ESP_LOGI(TAG, "writing the partition table");
        flash_binary(&loader, partition_table_bin, partition_table_bin_size, PARTITION_TABLE_ADDRESS);
        ESP_LOGI(TAG, "writing the OTA data");
        flash_binary(&loader, ota_data_initial_bin, ota_data_initial_bin_size, OTA_DATA_ADDRESS);
        ESP_LOGI(TAG, "writing esp-hosted's slave app at 0x%x", APPLICATION_ADDRESS);
        flash_binary(&loader, app_bin, app_bin_size, APPLICATION_ADDRESS);
        esp_loader_reset_target(&loader);
        ESP_LOGW(TAG, "done: the C6 is reset into esp-hosted 2.7.0. Flash wifi_check back.");
        return;
    }
    ESP_LOGE(TAG, "the C6 never came up in download mode: was IO9 held to GND?");
}
