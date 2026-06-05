/* Linker memory layout for the RP2040 (as on the Raspberry Pi Pico WH).
 *
 * The RP2040 has no internal program flash; it boots from external QSPI flash
 * via a 256-byte second-stage bootloader ("boot2") that must live at the very
 * start of flash with a checksum in its final word. cortex-m-rt's link.x places
 * the `.boot2` section first, then the vector table, then .text, etc.
 *
 * Pico / Pico W / Pico WH carry a 2 MiB QSPI flash and 264 KiB of SRAM.
 */
MEMORY {
    /* boot2 occupies the first 0x100 bytes of flash; FLASH below starts after
     * it so the vector table is correctly offset. */
    BOOT2 : ORIGIN = 0x10000000, LENGTH = 0x100
    FLASH : ORIGIN = 0x10000100, LENGTH = 2048K - 0x100

    /* 264 KiB SRAM. The RP2040 splits this into striped banks (SRAM0..3) plus
     * two 4 KiB banks (SRAM4/5); for a single-core Embassy app a flat RAM
     * region is fine. */
    RAM   : ORIGIN = 0x20000000, LENGTH = 264K
}

/* Place the second-stage bootloader in its own section at the start of flash.
 * embassy-rp / rp2040-boot2 emit a symbol that we copy here. */
SECTIONS {
    .boot2 ORIGIN(BOOT2) :
    {
        KEEP(*(.boot2));
    } > BOOT2
} INSERT BEFORE .text;
