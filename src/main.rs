//! source: https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.0.0/examples/wifi/access_point_with_sta

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use camera_tcp_server::camera::cam_init;
use core::{net::Ipv4Addr, str::FromStr};

use embassy_executor::Spawner;

use embassy_futures::yield_now;
use embassy_net::{
    IpListenEndpoint, Ipv4Cidr, Runner, Stack, StackResources, StaticConfigV4, tcp::TcpSocket,
};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::system::Stack as ProcStack;
use esp_hal::{
    clock::CpuClock, interrupt::software::SoftwareInterruptControl, lcd_cam::cam::Camera, ram,
    rng::Rng, timer::timg::TimerGroup,
};
use esp_println::{print, println};
use esp_radio::wifi::{Config, ControllerConfig, Interface, WifiController, ap::AccessPointConfig};
use esp_rtos::embassy::Executor;
esp_bootloader_esp_idf::esp_app_desc!();

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]

// When you are okay with using a nightly compiler it's better to use https://docs.rs/static_cell/2.1.0/static_cell/macro.make_static.html
macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

const GW_IP_ADDR_ENV: Option<&'static str> = option_env!("GATEWAY_IP");

pub enum CameraMessage {
    /// A chunk of video data. Contains the buffer and the number of valid bytes.
    VideoChunk([u8; 1024], usize),

    /// Signal that the JPEG frame is completely finished
    EndOfFrame,

    /// Optional: Signal that the hardware crashed and needs a reset
    HardwareError,
}

static VIDEO_CHANNEL: Channel<CriticalSectionRawMutex, CameraMessage, 16> = Channel::new();

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // TODO :why?
    // DEBUG - failed to transmit IP: device exhausted
    esp_println::logger::init_logger(log::LevelFilter::Debug);
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    // all peripherals are `generated` at compile time as singletons
    // peripherals.WIFI
    // peripherals.GPIO
    let peripherals = esp_hal::init(config);

    // - `reclaimed`: Memory reclaimed from the esp-idf bootloader.
    esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 64 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);

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
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    let access_point_config =
        Config::AccessPoint(AccessPointConfig::default().with_ssid("esp-radio-2"));

    println!("Starting wifi");
    let (controller, interfaces) = esp_radio::wifi::new(
        peripherals.WIFI,
        ControllerConfig::default().with_initial_config(access_point_config),
    )
    .unwrap();
    println!("Wifi started!");

    let device = interfaces.access_point;

    let gw_ip_addr_str = GW_IP_ADDR_ENV.unwrap_or("192.168.2.1");
    let gw_ip_addr = Ipv4Addr::from_str(gw_ip_addr_str).expect("failed to parse gateway ip");

    let config = embassy_net::Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(gw_ip_addr, 24),
        gateway: Some(gw_ip_addr),
        dns_servers: Default::default(),
    });

    let rng = Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;

    // Init network stack
    let (stack, runner) = embassy_net::new(
        device,
        config,
        mk_static!(StackResources<3>, StackResources::<3>::new()),
        seed,
    );

    // TODO worker for camera ?
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

    // https://github.com/esp-rs/esp-hal/blob/main/examples/async/embassy_multicore/src/main.rs
    const CAMERA_STACK_SIZE: usize = 12288;
    let app_core_stack = mk_static!(ProcStack<CAMERA_STACK_SIZE>, ProcStack::new());
    esp_rtos::start_second_core(
        peripherals.CPU_CTRL,
        sw_int.software_interrupt1,
        app_core_stack,
        move || {
            static EXECUTOR: static_cell::StaticCell<Executor> = static_cell::StaticCell::new();
            let executor = EXECUTOR.init(Executor::new());
            executor.run(|spawner| {
                spawner.spawn(camera_task(camera).unwrap());
            });
        },
    );

    spawner.spawn(connection(controller).unwrap());
    spawner.spawn(net_task(runner).unwrap());
    spawner.spawn(run_dhcp(stack, gw_ip_addr_str).unwrap());
    spawner.spawn(tcp_task(stack).unwrap());

    loop {
        if stack.is_link_up() {
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }
    println!(
        "Connect to the AP `esp-radio` and point your browser to http://{gw_ip_addr_str}:8080/"
    );
    println!("DHCP is enabled so there's no need to configure a static IP, just in case:");
    while !stack.is_config_up() {
        Timer::after(Duration::from_millis(100)).await
    }
    stack
        .config_v4()
        .inspect(|c| println!("ipv4 config: {c:?}"));
    loop {
        core::future::pending::<()>().await;
    }
}

#[embassy_executor::task]
async fn run_dhcp(stack: Stack<'static>, gw_ip_addr: &'static str) {
    use core::net::{Ipv4Addr, SocketAddrV4};

    use edge_dhcp::{
        io::{self, DEFAULT_SERVER_PORT},
        server::{Server, ServerOptions},
    };
    use edge_nal::UdpBind;
    use edge_nal_embassy::{Udp, UdpBuffers};

    let ip = Ipv4Addr::from_str(gw_ip_addr).expect("dhcp task failed to parse gw ip");

    let mut buf = [0u8; 1500];

    let mut gw_buf = [Ipv4Addr::UNSPECIFIED];

    let buffers = UdpBuffers::<3, 1024, 1024, 10>::new();
    let unbound_socket = Udp::new(stack, &buffers);
    let mut bound_socket = unbound_socket
        .bind(core::net::SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            DEFAULT_SERVER_PORT,
        )))
        .await
        .unwrap();

    loop {
        _ = io::server::run(
            &mut Server::<_, 64>::new_with_et(ip),
            &ServerOptions::new(ip, Some(&mut gw_buf)),
            &mut bound_socket,
            &mut buf,
        )
        .await
        .inspect_err(|e| log::warn!("DHCP server error: {e:?}"));
        Timer::after(Duration::from_millis(500)).await;
    }
}

#[embassy_executor::task]
async fn connection(controller: WifiController<'static>) {
    println!("start connection task");
    loop {
        let ev = controller
            .wait_for_access_point_connected_event_async()
            .await;
        match ev {
            Ok(esp_radio::wifi::AccessPointStationEventInfo::Connected(
                access_point_station_connected_info,
            )) => {
                println!(
                    "Station connected: {:?}",
                    access_point_station_connected_info
                );
            }
            Ok(esp_radio::wifi::AccessPointStationEventInfo::Disconnected(
                access_point_station_disconnected_info,
            )) => {
                println!(
                    "Station disconnected: {:?}",
                    access_point_station_disconnected_info
                );
            }
            _ => (),
        }
        Timer::after(Duration::from_millis(5000)).await
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface<'static>>) {
    runner.run().await
}

#[embassy_executor::task]
async fn camera_task(camera: Camera<'static>) {
    // TODO with embassy this got smaller why ?
    let dma_rx_buf = esp_hal::dma_rx_stream_buffer!(110 * 1024);
    let mut transfer = camera.receive(dma_rx_buf).map_err(|e| e.0).unwrap();
    println!("Started camera");
    loop {
        // must scope the borrowed data buffer
        // use it within scope for sending to channel
        let (bytes_to_consume, eof_reached, is_done) = {
            let (data, ends_with_eof) = transfer.peek_until_eof();

            if data.is_empty() {
                // No data yet, return early to drop the borrow
                // println!("data.is_empty()");
                (0, ends_with_eof, transfer.is_done())
            } else {
                let mut fixed_array = [0u8; 1024];

                for chunk in data.chunks(1024) {
                    fixed_array[..chunk.len()].copy_from_slice(chunk);

                    // println!("sending chunk {}",chunk.len());
                    let _ =
                        VIDEO_CHANNEL.try_send(CameraMessage::VideoChunk(fixed_array, chunk.len()));
                }

                (data.len(), ends_with_eof, transfer.is_done())
            }
        };

        if bytes_to_consume > 0 {
            transfer.consume(bytes_to_consume);
        } else {
            // If we had no data to consume, check if the transfer finished.
            if is_done {
                // DMA is full - reset it
                println!("is_done");
                let (camera, returned_buf) = transfer.stop();
                transfer = camera.receive(returned_buf).map_err(|e| e.0).unwrap();
                let _ = VIDEO_CHANNEL.try_send(CameraMessage::EndOfFrame);
            }

            // nothing to consume
            // good time to yield
            // TODO: with second core we still needs this
            // read about WTI; TWDT
            yield_now().await;
            continue;
        }

        if eof_reached {
            let _ = VIDEO_CHANNEL.try_send(CameraMessage::EndOfFrame);
            yield_now().await;
        }
    }
}

#[embassy_executor::task]
async fn tcp_task(stack: Stack<'static>) {
    println!("Start tcp task...");
    let mut rx_buffer = [0; 1536];
    let mut tx_buffer = [0; 32768];

    let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
    socket.set_timeout(Some(embassy_time::Duration::from_secs(10)));

    'outer: loop {
        println!("Wait for connection...");
        let r = socket
            .accept(IpListenEndpoint {
                addr: None,
                port: 8080,
            })
            .await;
        println!("Connected...");

        if let Err(e) = r {
            println!("connect error: {:?}", e);
            continue;
        }

        let mut buffer = [0u8; 1024];
        let mut pos = 0;
        loop {
            match socket.read(&mut buffer[pos..]).await {
                Ok(0) => {
                    println!("read EOF");
                    socket.close();
                    let _ = socket.flush().await;
                    continue 'outer;
                }

                Ok(len) => {
                    let to_print =
                        unsafe { core::str::from_utf8_unchecked(&buffer[..(pos + len)]) };

                    if to_print.contains("\r\n\r\n") {
                        print!("{}", to_print);
                        println!();

                        if to_print.contains("GET /favicon.ico") {
                            let _ = socket
                                .write(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
                                .await;
                            socket.abort();
                            continue 'outer;
                        }

                        if to_print.contains("GET / HTTP/1.1") {
                            println!("Serving HTML Dashboard");
                            let header = b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: text/html\r\n\r\n";
                            let _ = socket.write(header).await;
                            let html_page = include_str!("../dashboard.html");
                            let _ = socket.write(html_page.as_bytes()).await;
                            socket.close();

                            let _ = socket.flush().await;
                            // Wait for the browser to reconnect and ask for /stream
                            continue 'outer;
                        }

                        if to_print.contains("GET /stream HTTP/1.1") {
                            println!("Browser requested the video stream!");
                            break;
                        }

                        // If it's anything else, close it
                        socket.close();
                        let _ = socket.flush().await;
                        continue 'outer;
                    }

                    pos += len;
                }
                Err(e) => {
                    println!("read error: {:?}", e);
                    socket.close();
                    continue 'outer;
                }
            }
        }

        println!("Starting video stream.");

        let header =
            b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\n\r\n";
        if socket.write(header).await.is_err() {
            println!("Client dropped before header was sent.");
            socket.close();
            let _ = socket.flush().await;
            continue 'outer;
        }
        println!("Header was sent.");
        let _ = socket.flush().await;

        loop {
            match VIDEO_CHANNEL.receive().await {
                CameraMessage::VideoChunk(buffer, length) => {
                    // println!("CameraMessage::VideoChunk {}", length);
                    if socket.write(&buffer[..length]).await.is_err() {
                        // Browser disconnected
                        break;
                    }
                    // flushing here was bad decision
                    // as it will ask for ACK for each buffer
                }
                CameraMessage::EndOfFrame => {
                    // println!("CameraMessage::EndOfFrame");
                    let boundary = b"\r\n--frame\r\nContent-Type: image/jpeg\r\n\r\n";
                    if socket.write(boundary).await.is_err() {
                        break;
                    }
                    let _ = socket.flush().await;
                }
                CameraMessage::HardwareError => {
                    println!("Camera died, closing connection to force client refresh.");
                    break;
                }
            }
        }

        let _ = socket.write(b"\r\n").await;
        socket.close();
        let _ = socket.flush().await;
        println!("Done\n");
        println!();
    }
}
