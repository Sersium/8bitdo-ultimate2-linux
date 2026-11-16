# Native protocol and validation

The Rust app targets the **Ultimate 2 Wireless**, not the Ultimate 2C, Ultimate 2 Bluetooth or first-generation Ultimate. The retained Python code supports additional models.

## Sources

- [Original configurator](https://github.com/MiguelSanzBr/8bitdo-ultimate2-linux) and its [protocol notes](PROTOCOL.md).
- [SDL 8BitDo driver](https://github.com/libsdl-org/SDL/blob/main/src/joystick/hidapi/SDL_hidapi_8bitdo.c): raw motion scaling, axes and DInput rumble.
- [Official WebHID updater](https://web.8bitdo.com/): firmware catalogue, image format and bootloader commands. Inspected asset versions: `index-BXAl9J5z.js`, `connectDevice-CqNxlsyD.js`, `request-ZgT0Hy98.js`.
- Official Ultimate Software V2 1.38 was inspected for interoperability, including macro layouts and edition-dependent lighting. Vendor source/binaries/assets are excluded from this repository.

## Framing and configuration

Normal requests are 64 bytes: `81 class cmd:u16 sub:u16 len:u16 crc16:u16 total:u32 offset:u32 data`. Multibyte header integers are little endian; CRC is Modbus CRC16 over written data. Maximum payload is 46 bytes; configuration reads use 45. Class-4 replies are `02 04 04 00 request_cmd:u16 ...`; class-5 replies use status word **0**, not 4. Configuration replies echo the request header/CRC, so that CRC is not a checksum of returned bytes.

Configuration is 1592 bytes. Read/write/commit commands are 2/1/6; commit subcommand is `0x0123`. Runtime fields 12..20 are preserved. Writes stop report streaming, compare the original configuration, commit, read back and resume streaming even on errors. Theme commands are 0x40/0x41; Off is wire value **6 on edition 2**, otherwise 5. Native backups store the raw configuration, active theme and all active macro steps, not just macro metadata.

Edition 2 is WUCHANG: Fallen Feathers, 3 is Honkai: Star Rail Evernight/lilac, 4 is Clear Blue. Standard edition numbers are not assigned guessed colours.

## Macros

Three profiles, four macros per profile. Profile metadata starts at `500 + 216*slot`; records at `508 + 216*slot + 52*index`. Record fields: UTF-16BE name (32 bytes), mode u8, special u8, step count u16, record offset u16, reserved u16, trigger u32, cycles u32, interval u32.

Step bytes are five little-endian u16 values: duration, key mask, LT-high/RT-low, left X-low/Y-high, right X-low/Y-high. Sticks centre on 127; maximum 200 steps. Steps occupy separate 4096-byte flash blocks. Subcommand is `(gamepad_mode << 8) | slot`; base address is `index*4096`. Read 0x102 in chunks of 32, erase 0x104, write 0x103 with zero padding to 32 bytes. Verify steps before committing the metadata. No duplicate trigger resolution is performed automatically.

## Firmware

Official HTTPS catalogue: `support.8bitdo.com/restapi/link/v1/client/firmware/select`, JSON `firmware_type` 94 (controller) or 95 (dongle), `beta:0`. Image downloads are restricted to the official support host. Metadata MD5 is stale for these models and is not used as an integrity claim.

Images have one 28-byte header: version u32, destination u32, payload length u32, target PID u32, vendor checksum u32, two reserved u32. Validate bounded length, exact size/version, target 6012/6013, address 0x5000, reserved zeros, and the Telink `KNLT` marker. The vendor whole-image checksum algorithm is not established; TLS and packet CRCs provide transport protection, not a cryptographic firmware signature check.

Identify the actual firmware target with class 5 command 8; USB 310b is shared by controller and receiver. Enter boot with `81 05 00 51 00` (this is **not** an input-mode switch). Boot USB 3208 has no HID report IDs: write a 65-byte hidapi buffer with zero report prefix and trailing zero; incoming 64-byte replies have prefix zero, normalized internally to the application reply prefix.

The boot sequence is: identify exact target, erase **4**, write **3** in 46-byte chunks, save **0xc4** with payload length/version, reset **7**, then check the reconnected running version. An error stops the sequence; the app does not erase before matching the target. Window closing is blocked during the update. After an interrupted flash, reconnect in boot mode and retry the same target; don't flash a controller image to a receiver.

## Validation on 2026-11-16

- Ultimate 2 WUCHANG controller 1.10: configuration, edition, lighting Off, full backup and non-flashing bootloader entry/identity/reset verified on hardware.
- Receiver 1.06: bootloader entry/target identity/reset verified without erase/write commands. Main UI reads its own version separately from the controller.
- USB/XInput physical input query: class 4 command 3, subcommand 0x123, returns a 12-byte state (u32 physical key mask, LX/LY/RX/RY, RT/LT, reserved). Queued replies are drained before each request; final input changes remain pending until the UI paints them.
- Receiver DInput: real 34-byte reports, about 1000 reports/s; user confirmed wireframe/angle feedback follows movement.
- Official controller 1.10 (145436 bytes) and receiver 1.06 (40476 bytes) download/format validation passed.
- Same-version updates are rejected in the UI and again immediately before bootloader entry. Recovery is available when the device is already in boot mode and has no running firmware version.
- Quaternion orientation uses a consistent right-handed controller frame; geometry flips screen Y only at projection. Still calibration removes gyro bias; gravity corrects tilt. Yaw remains relative.
- Firmware write sequencing for both targets and wrong-target protection are tested with a simulated transport. **No actual firmware flash has been performed.**
- Macro encoding, storage addressing, readback and metadata preservation are tested with a simulated transport. Actual macro writes remain untested.
- Polling adjustment and automatic host input-mode switching are not implemented: no working Ultimate 2 command has been verified. Physical mode selection is documented in the UI.

## Developer checks

```sh
cargo fmt --check --manifest-path native/Cargo.toml
cargo clippy --locked --manifest-path native/Cargo.toml --all-targets --all-features -- -D warnings
cargo test --locked --manifest-path native/Cargo.toml
```

`--list`, `--info`, `--backup FILE.json`, `--check-updates` and `--verify-downloads` provide diagnostics. `--probe-boot controller|dongle` enters and exits boot mode without erasing/writing; `--probe-macros` reads one macro block without writing. Do not run multiple configurators against the same device.

Build with `--features screenshots`, then run `U2_SCREENSHOT_TO=shot.png ultimate2-native --page=Motion` to capture the real native interface. This optional development feature is excluded from the normal build.
