//! source: https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.0.0/examples/wifi/access_point_with_sta

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embedded_io::*;
use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    dma_rx_stream_buffer, main, ram,
    time::{self, Duration},
    timer::timg::TimerGroup,
};
use esp_println::{print, println};
use camera_tcp_server::{camera::cam_init, wifi::init_wifi_stack};

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[main]
fn main() -> ! {
    // TODO :why?
    // DEBUG - failed to transmit IP: device exhausted
    esp_println::logger::init_logger(log::LevelFilter::Error);
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    // all peripherals are `generated` at compile time as singletons
    // peripherals.WIFI
    // peripherals.GPIO
    let peripherals = esp_hal::init(config);

    // TODO: why reclaimed is necessary ?
    // - `reclaimed`: Memory reclaimed from the esp-idf bootloader.
    // just removing make it panic
    // TODO: why I get runtime panic ?
    // why this is not detected on compile time ?
    esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
    // just 64 was enough to start
    esp_alloc::heap_allocator!(size: 36 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);

    // An RTOS (Real-Time Operating System) implementation for esp-hal
    //
    // and start scheduler and pins context to first core
    // The current context will be converted into the main task, and will be pinned to the first core.
    //
    // context is `everything` from CPU register state; thread; function in this case main
    // CALLING it from `not` main might be UB (gemini)
    // HERE interrupts are enabled
    // This transition is necessary because an RTOS relies entirely
    // on interrupt-driven preemptions to pause your main code
    esp_rtos::start(timg0.timer0);

    let (ap_stack, controller) = init_wifi_stack(peripherals.WIFI);

    println!(
        "Start busy loop on main. Connect to the AP `esp-radio` and point your browser to http://192.168.2.1:8080/"
    );
    println!("Use a static IP in the range 192.168.2.2 .. 192.168.2.255, use gateway 192.168.2.1");
    // check for HTTP headers
    // .arduino15/packages/esp32/hardware/esp32/3.3.7/libraries/ESP32/examples/Camera/CameraWebServer/app_httpd.cpp
    let camera = cam_init(
        peripherals.LCD_CAM,
        peripherals.DMA_CH0,
        peripherals.I2C0,
        peripherals.GPIO4,
        peripherals.GPIO5,
        peripherals.GPIO15,
        peripherals.GPIO6,
        peripherals.GPIO7,
        peripherals.GPIO13,
        peripherals.GPIO11,
        peripherals.GPIO9,
        peripherals.GPIO8,
        peripherals.GPIO10,
        peripherals.GPIO12,
        peripherals.GPIO18,
        peripherals.GPIO17,
        peripherals.GPIO16,
    )
    .unwrap();

    // 9. Create sockets with stacks
    let mut rx_buffer = [0u8; 1536];
    // TX must be massive to act as a shock-absorber between the fast camera and slow Wi-Fi
    // 16KB to 32KB is highly recommended for MJPEG streaming.
    let mut tx_buffer = [0u8; 65536];
    let mut ap_socket = ap_stack.get_socket(&mut rx_buffer, &mut tx_buffer);

    let dma_rx_buf = dma_rx_stream_buffer!(160 * 1000);
    let mut transfer = camera.receive(dma_rx_buf).map_err(|e| e.0).unwrap();

    ap_socket.listen(8080).unwrap();

    loop {
        ap_socket.work();

        if !ap_socket.is_open() {
            ap_socket.listen(8080).unwrap();
        }

        // 11. Did someone connected ?
        if ap_socket.is_connected() {
            println!("Connected");

            let mut time_out = false;
            let deadline = time::Instant::now() + Duration::from_secs(20);
            // TODO: is 1024 magic number or requirment here ?
            let mut buffer = [0u8; 1024];
            let mut pos = 0;
            loop {
                if let Ok(len) = ap_socket.read(&mut buffer[pos..]) {
                    let to_print =
                        unsafe { core::str::from_utf8_unchecked(&buffer[..(pos + len)]) };

                    if to_print.contains("\r\n\r\n") {
                        print!("{}", to_print);
                        println!();
                        break;
                    }

                    pos += len;
                } else {
                    break;
                }

                if time::Instant::now() > deadline {
                    println!("Timeout");
                    time_out = true;
                    break;
                }
            }

            // 12. Let's send our video feed
            'start_again: loop {
                println!("Client connected! Starting video stream.");

                // send once per connection Main HTTP Header
                let header = b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\n\r\n";
                ap_socket.write_all(header).unwrap();

                loop {
                    // send once per frame Boundary Header
                    let frame_header = b"--frame\r\nContent-Type: image/jpeg\r\n\r\n";
                    ap_socket.write_all(frame_header).unwrap();

                    let mut jpeg_total_bytes = 0;

                    // Reads the DMA chunks until EOF for this specific frame
                    loop {
                        let (data, ends_with_eof) = transfer.peek_until_eof();

                        if data.is_empty() {
                            // good time advance network
                            ap_socket.work();
                            // controller.

                            if transfer.is_done() {
                                println!("Network lag detected. Dropping frame...");
                                let (camera, returned_buf) = transfer.stop();
                                transfer = camera.receive(returned_buf).map_err(|e| e.0).unwrap();
                                break;
                            }
                            continue;
                        }

                        let bytes_peeked = data.len();
                        jpeg_total_bytes += bytes_peeked;

                        // TODO: experiment wiht chunk sizes
                        // Stream the binary image chunks
                        for chunk in data.chunks(1024) {
                            if let Err(_) = ap_socket.write_all(chunk) {
                                // TODO: realod often fails here. fix me (better)
                                break 'start_again;
                            }
                        }

                        // Free the DMA buffer
                        transfer.consume(bytes_peeked);

                        // If this chunk contained the EOF signal, the frame is complete
                        if ends_with_eof {
                            println!("Frame sent. Total bytes: {}", jpeg_total_bytes);

                            // Close the frame payload with a carriage return
                            ap_socket.write_all(b"\r\n").unwrap();
                            ap_socket.flush().unwrap();
                            // Break out of the INNER loop.
                            break;
                        }
                    } // Inner Loop
                } // Outer Loop
            }

            ap_socket.close();

            println!("Done\n");
            println!();
        }

        let start = time::Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            ap_socket.work();
        }
    }
}


