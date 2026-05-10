# What is it ?
In its current state, it is a TCP (over Wi-Fi) web camera that uses the ESP32-S3-WROOM SoC. It uses Espressif Rust (no-std) crates, as well as Embassy (async).

# Why is it ?
For self-development. It turns out that embedded development with Rust for ESP is very rewarding, fun and one might say straightforward.

# How to run it ?
1. You will need to source esp32s3-wroom SoC with ov3660 camera.
2. Install esp toolchain (check `cheatsheet.sh`).
3. Init environment (`setup.sh`).
4. Flush it `cargo run --release --bin  camera-tcp-server`.
5. Follow the instructions in console.
6. Behold yourself on a screen.

# Structure
- `src/camera` - init and camera's `magic` init arrays
- `src/wifi` - was used for synchronous version with smoltcp
- `src/main.rs` - contains main loop and embassy tasks.
    - tcp_task - listens for connection on port 8080 and forwards JPEG frames into that connection.
    - camera_task - uses `try_send` to send JPEG frame, so if tcp_task is `overloaded` camera won't be blocked
    - CameraMessage - simple communication `protocol`; for now just distinguish frame vs endOfFrame


# Examples used
Espressif have an exellent collections of [examples](https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.0.0/examples).
Some of them were used for this project.
Main ones are:
- WiFi set up - [link](https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.0.0/examples/wifi/embassy_access_point)
- Camera set up - [link](https://github.com/esp-rs/esp-hal/blob/main/qa-test/src/bin/lcd_cam_ov2640.rs)
- Camera config (as esp32s3-wroom has ov3660 camera) - [link](https://github.com/espressif/esp32-camera/blob/master/sensors/private_include/ov3660_settings.h)
- Init of second core - [link](https://github.com/esp-rs/esp-hal/tree/main/examples/async/embassy_multicore)

# TODO/IDEAs
 - [ ] Make it a USB camera so finished product.
 - [ ] Improve stability and performance.
    - [ ] Switch to UDP socket instead of TCP.
    - [ ] Ensure connection on first try.
    - [ ] Latency vs bandwidth config.
- [ ] Camera configuration functions.
- [ ] Try adding microphone.
- [ ] In addition to USB, also transmit live to the `internet` (basically a stress test of chip?).
- [ ] Try adding face recognition functionality.

# Try UDP with relay

ESP32 -> Jpeg Frames/UDP -> relay <-> WebSocket/TCP OR webRTC/UDP -> Browser

1. Read full jpeg frame from camera
2. Calculate total_chunks
3. Send individual chunks like JpegFrameChunk to relay
4. Relay assembles frame if possible (missing chunks are discarded)
3. Relay for now uses embedded HTML to serve video stream