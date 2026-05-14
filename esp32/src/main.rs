//! source: https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.0.0/examples/wifi/access_point_with_sta

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec;

use camera_tcp_server::camera::cam_init;
use log::{debug, info, trace};
use core::ops::Index;
// use edge_dhcp::io::server;

use embassy_executor::Spawner;

use core::{net::{Ipv4Addr,SocketAddrV4}, str::FromStr};
use embassy_futures::yield_now;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{
    IpListenEndpoint, Ipv4Cidr, Runner, Stack, StackResources, StaticConfigV4,
};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::{channel::Channel, signal::Signal};
use embassy_time::{Duration, Timer, with_timeout};
use esp_alloc::{self as _, HeapStats};
use esp_backtrace as _;
use esp_hal::system::Stack as ProcStack;
use esp_hal::{
    clock::CpuClock, interrupt::software::SoftwareInterruptControl, lcd_cam::cam::Camera, ram,
    rng::Rng, timer::timg::TimerGroup,
};
use esp_radio::wifi::{Config, ControllerConfig, Interface, WifiController, ap::AccessPointConfig};
use esp_rtos::embassy::Executor;
use shared::{JpegFrameChunk, MAX_CHUNK_SIZE};

use edge_dhcp::{
    io::{self, DEFAULT_SERVER_PORT},
    server::{Server, ServerOptions},
};
use edge_nal::UdpBind;
use edge_nal_embassy::{Udp, UdpBuffers};

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

// TODO: decide either this parsing or build.rs
// const fn parse_u16(s: &str) -> u16 {
//     let mut res: u16 = 0;
//     let mut i = 0;
//     let bytes = s.as_bytes();
//     while i < bytes.len() {
//         assert!(bytes[i] >= b'0' && bytes[i] <= b'9', "Error: Invalid port");
//         let digit = (bytes[i] - b'0') as u16;

//         let multiplied = match res.checked_mul(10) {
//             Some(v) => v,
//             None => panic!("Error: Port value exceeds u16::MAX (65535)"),
//         };

//         res = match multiplied.checked_add(digit) {
//             Some(v) => v,
//             None => panic!("Error: Port value exceeds u16::MAX (65535)"),
//         };
//         i += 1;
//     }
//     res
// }

// const DEFAULT_PORT_ENV: u16 = match option_env!("DEFAULT_PORT") {
//     Some(p) => parse_u16(p),
//     None => DEFAULT_PORT,
// };

// TODO: adjust numbers
include!(concat!(env!("OUT_DIR"), "/port.rs"));
const GW_IP_ADDR_ENV: Option<&'static str> = option_env!("GATEWAY_IP");
const DEFAULT_GW_IP_ADDR: &str = "192.168.2.1";
const AP_SSID_NAME: &str = "esp-radio-2";
const MAX_FRAME_SIZE: usize = 32768;
const DATA_CHUNK_SIZE: usize = 1024;
const TX_BUFF_NUMBER_CHUNKS: usize = MAX_FRAME_SIZE / DATA_CHUNK_SIZE;
// both need SRAM but DMA must be static
const HEAP_SIZE: usize = 100 * 1024 - DMA_RX_STREAM_BUF_SIZE;
const DMA_RX_STREAM_BUF_SIZE: usize = TX_BUFF_NUMBER_CHUNKS * DATA_CHUNK_SIZE;
const TCP_TX_BUFF_SIZE: usize = TX_BUFF_NUMBER_CHUNKS * DATA_CHUNK_SIZE;
const VIDEO_DATA_POOL_POOL_SIZE: usize = 16;
const CAMERA_STACK_SIZE: usize = 2048;
const MTU: usize = 1500;
const TCP_RX_BUFF_SIZE: usize = 15 * MTU;
pub enum CameraMessage {
    /// A chunk of video data. Contains the buffer and the number of valid bytes.
    VideoChunk(MyBox<[u8; DATA_CHUNK_SIZE]>, usize),

    /// Signal that the JPEG frame is completely finished
    EndOfFrame,

    /// Optional: Signal that the hardware crashed and needs a reset
    HardwareError,
}

static VIDEO_CHANNEL: Channel<CriticalSectionRawMutex, CameraMessage, VIDEO_DATA_POOL_POOL_SIZE> =
    Channel::new();
static VIDEO_DATA_POOL: Channel<
    CriticalSectionRawMutex,
    MyBox<[u8; DATA_CHUNK_SIZE]>,
    VIDEO_DATA_POOL_POOL_SIZE,
> = Channel::new();

struct MyBox<T>(Box<T>);

impl<T> Drop for MyBox<T> {
    fn drop(&mut self) {
        info!("MyBox is being dropped, cleaning up resources.");
    }
}

impl<T> MyBox<T> {
    pub fn new(val: T) -> Self {
        MyBox(Box::new(val))
    }
}

static STATION_CONNECTED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static LAST_CONNECTED_IP: Signal<CriticalSectionRawMutex, Ipv4Addr> = Signal::new();

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
    esp_alloc::heap_allocator!(size: HEAP_SIZE);

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
        Config::AccessPoint(AccessPointConfig::default().with_ssid(AP_SSID_NAME));

    let (controller, interfaces) = esp_radio::wifi::new(
        peripherals.WIFI,
        ControllerConfig::default().with_initial_config(access_point_config),
    )
    .expect("Failed to initialize WiFi");

    info!("Wifi started!");

    let device = interfaces.access_point;

    let gw_ip_addr_str = GW_IP_ADDR_ENV.unwrap_or(DEFAULT_GW_IP_ADDR);
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
        mk_static!(StackResources<5>, StackResources::<5>::new()),
        seed,
    );

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
    .expect("Failed to initialize camera");

    // to prevent unnecessary allocations and copying of buffers we create pool of buffers
    // for camera to write into and then send them to tcp task for writing to socket;
    // after writing to socket the buffer is returned back to pool;
    // this way we can achieve zero-copy streaming from camera to browser with backpressure
    // support (when network is congested the camera task will wait for an available buffer
    // instead of busy-looping and dropping frames)
    info!("POOL size {VIDEO_DATA_POOL_POOL_SIZE}");
    for _ in 0..VIDEO_DATA_POOL_POOL_SIZE {
        VIDEO_DATA_POOL.send(MyBox::new([0; DATA_CHUNK_SIZE])).await;
    }

    let app_core_stack = mk_static!(ProcStack<CAMERA_STACK_SIZE>, ProcStack::new());
    esp_rtos::start_second_core(
        peripherals.CPU_CTRL,
        sw_int.software_interrupt1,
        app_core_stack,
        move || {
            static EXECUTOR: static_cell::StaticCell<Executor> = static_cell::StaticCell::new();
            let executor = EXECUTOR.init(Executor::new());
            executor.run(|spawner| {
                spawner.spawn(camera_task(camera).expect("Failed to spawn camera"));
            });
        },
    );

    spawner.spawn(connection(controller).expect("Failed to spawn wifi controller."));
    // advances network stack
    spawner.spawn(net_task(runner).expect("Failed to spawn network task."));
    spawner.spawn(run_dhcp(stack, gw_ip_addr_str).expect("Failed to spawn dhcp."));

    // Optimized buffers for SVGA streaming
    let udp_frame_buffer = mk_static!([u8; 32786], [0u8; 32786]);
    let udp_rx_meta = mk_static!([PacketMetadata; 4], [PacketMetadata::EMPTY; 4]);
    let udp_rx_payload = mk_static!([u8; 1024], [0u8; 1024]);
    let udp_tx_meta = mk_static!([PacketMetadata; 20], [PacketMetadata::EMPTY; 20]);
    let udp_tx_payload = mk_static!([u8; 32768], [0u8; 32768]);

    spawner.spawn(
        udp_task(
            stack,
            udp_frame_buffer,
            udp_rx_meta,
            udp_rx_payload,
            udp_tx_meta,
            udp_tx_payload,
        )
        .unwrap(),
    );

    loop {
        if stack.is_link_up() {
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    info!("UDP Relay Stream active. Sending JpegFrameChunks to 192.168.2.2:{DEFAULT_PORT_ENV}");
    info!("DHCP is enabled so there's no need to configure a static IP, just in case:");
    while !stack.is_config_up() {
        Timer::after(Duration::from_millis(100)).await
    }

    stack
        .config_v4()
        .inspect(|c| println!("ipv4 config: {c:?}"));

    let stats: HeapStats = esp_alloc::HEAP.stats();
    // HeapStats implements the Display and defmt::Format traits, so you can
    // pretty-print the heap stats.
    info!("{}", stats);

    loop {
        core::future::pending::<()>().await;
    }
}

#[embassy_executor::task]
async fn run_dhcp(stack: Stack<'static>, gw_ip_addr: &'static str) {
    let ip = Ipv4Addr::from_str(gw_ip_addr).expect("dhcp task failed to parse gw ip");

    const RX_UDP_BUF_SIZE: usize = 1024;
    const TX_UDP_BUF_SIZE: usize = RX_UDP_BUF_SIZE;
    const DHCP_BUF_SIZE: usize = TX_UDP_BUF_SIZE;
    const NUMBER_OF_BUFFERS: usize = 2;
    const PACKETS_PER_BUFFER: usize = 2;
    let mut dhcp_scratch_buf = vec![0u8; DHCP_BUF_SIZE].into_boxed_slice();

    let mut gw_buf = [Ipv4Addr::UNSPECIFIED];

    let buffers =
        UdpBuffers::<NUMBER_OF_BUFFERS, RX_UDP_BUF_SIZE, TX_UDP_BUF_SIZE, PACKETS_PER_BUFFER>::new(
        );
    let unbound_socket = Udp::new(stack, &buffers);
    let mut bound_socket = unbound_socket
        .bind(core::net::SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            DEFAULT_SERVER_PORT,
        )))
        .await
        .expect("Failed to create socket for dhcp.");

    let mut dhcp_server: Server<_, 64> = Server::new(|| embassy_time::Instant::now().as_secs(), ip);

    STATION_CONNECTED.wait().await;
    STATION_CONNECTED.reset();

    loop {
        // io::server::run never returns, so we wrap it into timeout to be
        // able to get leased addresses and get our IP of the client
        let result = with_timeout(
            Duration::from_secs(2),
            io::server::run(
                &mut dhcp_server,
                &ServerOptions::new(ip, Some(&mut gw_buf)),
                &mut bound_socket,
                &mut buf,
            ),
        )
        .await;

        match result {
            Ok(Err(e)) => log::warn!("DHCP server error: {:?}", e),
            Err(e) => {
                // TimeoutError but someone connected (STATION_CONNECTED) but no DHCP request
                // just assume static client with this IP
                LAST_CONNECTED_IP.signal(Ipv4Addr::new(192, 168, 2, 2));
                println!("DHCP {e:?}");
            }
            _ => {}
        }

        // stream to the last one connected
        for (client_ip, _) in dhcp_server.leases.iter() {
            println!("Leased IP addr: {}", client_ip);
            LAST_CONNECTED_IP.signal(*client_ip);
        }
    }
}

#[embassy_executor::task]
async fn connection(controller: WifiController<'static>) {
    info!("start connection task");
    loop {
        let ev = controller
            .wait_for_access_point_connected_event_async()
            .await;
        match ev {
            Ok(esp_radio::wifi::AccessPointStationEventInfo::Connected(
                access_point_station_connected_info,
            )) => {
                info!(
                    "Station connected: {:?}",
                    access_point_station_connected_info
                );
                STATION_CONNECTED.signal(());
            }
            Ok(esp_radio::wifi::AccessPointStationEventInfo::Disconnected(
                access_point_station_disconnected_info,
            )) => {
                info!(
                    "Station disconnected: {:?}",
                    access_point_station_disconnected_info
                );
            }
            _ => (),
        }
        // Timer::after(Duration::from_millis(5000)).await
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface<'static>>) {
    runner.run().await
}

#[embassy_executor::task]
async fn camera_task(camera: Camera<'static>) {
    // TODO with embassy this got smaller why ?
    let dma_rx_buf = esp_hal::dma_rx_stream_buffer!(DMA_RX_STREAM_BUF_SIZE, DATA_CHUNK_SIZE);
    let mut transfer: esp_hal::lcd_cam::cam::CameraTransfer<'_, esp_hal::dma::DmaRxStreamBuf> =
        camera
            .receive(dma_rx_buf)
            .map_err(|e| e.0)
            .expect("Failed to receive from camera");

    info!("Started camera");
    loop {
        // must scope the borrowed data buffer
        // use it within scope for sending to channel
        let (bytes_to_consume, eof_reached, is_done) = {
            let (data, ends_with_eof) = transfer.peek_until_eof();

            if data.is_empty() {
                // No data yet, return early to drop the borrow
                // info!("data.is_empty()");
                (0, ends_with_eof, transfer.is_done())
            } else {
                for chunk in data.chunks(DATA_CHUNK_SIZE) {
                    let mut scratch_buf = VIDEO_DATA_POOL.receive().await;
                    scratch_buf.0[..chunk.len()].copy_from_slice(chunk);

                    // Use .send(...).await to provide backpressure and ensure frame integrity.
                    // This prevents the task from busy-looping when the network is congested
                    // and ensures the browser doesn't receive partial/corrupted JPEGs.
                    VIDEO_CHANNEL
                        .send(CameraMessage::VideoChunk(scratch_buf, chunk.len()))
                        .await;
                }

                (data.len(), ends_with_eof, transfer.is_done())
            }
        };

        if bytes_to_consume > 0 {
            transfer.consume(bytes_to_consume);
        } else {
            // If we had no data to consume, check if the transfer finished.
            if is_done {
                info!("camera's DMA is full - resetting it");
                let (camera, returned_buf) = transfer.stop();
                transfer = camera
                    .receive(returned_buf)
                    .map_err(|e| e.0)
                    .expect("Failed to reinitialize camera");
                VIDEO_CHANNEL.send(CameraMessage::EndOfFrame).await;
            }

            // nothing to consume
            // good time to yield
            // TODO: even with second core we still needs this
            // read about WFI - Wait For Interrupt instruction;
            // Task Watchdog Timer (TWDT)
            // https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/wdts.html
            yield_now().await;
            continue;
        }

        if eof_reached {
            VIDEO_CHANNEL.send(CameraMessage::EndOfFrame).await;
            yield_now().await;
        }
    }
}

#[derive(Eq, PartialEq)]
enum ConnectionState {
    GetNextRequest,
    StartStream,
}

#[embassy_executor::task]
async fn udp_task(
    stack: Stack<'static>,
    frame_buffer: &'static mut [u8],
    rx_meta: &'static mut [PacketMetadata; 4],
    rx_payload: &'static mut [u8; 1024],
    tx_meta: &'static mut [PacketMetadata; 20],
    tx_payload: &'static mut [u8; 32768],
) {
    info!("Start UDP task...");
    let mut socket = UdpSocket::new(
        stack,
        rx_meta,
        rx_payload,
        tx_meta,
        tx_payload,
    );
    socket.bind(DEFAULT_PORT_ENV).unwrap();

    let mut frame_id: u16 = 0;
    let mut current_pos = 0;
    let user_ip: Ipv4Addr = LAST_CONNECTED_IP.wait().await;
    LAST_CONNECTED_IP.reset();

    println!("user ip {user_ip}");
    let remote_endpoint = core::net::SocketAddr::V4(SocketAddrV4::new(user_ip, 5000));

    info!("UDP task waiting for a station to connect...");
    STATION_CONNECTED.wait().await;
    STATION_CONNECTED.reset();
    
    // Give the station a moment to initialize its network interface after connecting
    info!("Station detected! Waiting 2s for network stability...");
    Timer::after(Duration::from_secs(2)).await;

    loop {
        match VIDEO_CHANNEL.receive().await {
            CameraMessage::VideoChunk(buffer, length) => {
                if current_pos + length <= frame_buffer.len() {
                    frame_buffer[current_pos..current_pos + length]
                        .copy_from_slice(&buffer.0[..length]);
                    current_pos += length;
                } else {
                    println!("JPEG frame is too big for buffer {}", frame_buffer.len());
                }

            }
            CameraMessage::EndOfFrame => {
                if current_pos == 0 {
                    continue;
                }

                let total_chunks = current_pos.div_ceil(MAX_CHUNK_SIZE);
                let mut chunk_id = 0;

                for chunk in frame_buffer[..current_pos].chunks(MAX_CHUNK_SIZE) {
                    let mut payload = [0u8; MAX_CHUNK_SIZE];
                    payload[..chunk.len()].copy_from_slice(&chunk);

                    let chunk = JpegFrameChunk {
                        frame_id,
                        chunk_id: chunk_id as u8,
                        total_chunks: total_chunks as u8,
                        payload_len: chunk.len() as u16,
                        payload,
                    };
                    // info!("sending ");
                    if let Err(e) = socket.send_to(&chunk.to_bytes(), remote_endpoint).await {
                        info!("UDP send error: {:?}", e);
                        break;
                    }

                    if chunk_id == total_chunks - 1 {
                        info!("Frame {} sent ({} bytes)", frame_id, current_pos);
                    }

                    chunk_id += 1;
                }

                frame_id = frame_id.wrapping_add(1);
                current_pos = 0;
            }
            CameraMessage::HardwareError => {
                info!("Camera hardware error reported to UDP task.");
            }
        }
    }

}
