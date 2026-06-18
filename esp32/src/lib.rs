#![no_std]

pub mod camera;
// pub mod wifi;

pub const GW_IP_ADDR_ENV: Option<&'static str> = option_env!("GATEWAY_IP");
pub const DEFAULT_GW_IP_ADDR: &str = "192.168.2.1";
pub const AP_SSID_NAME: &str = "esp-radio-2";
pub const MAX_FRAME_SIZE: usize = 65_536; // 64KB is max JPEG size, but we need some extra space for metadata and headers
pub const DATA_CHUNK_SIZE: usize = 1400;
pub const TX_BUFF_NUMBER_CHUNKS: usize = MAX_FRAME_SIZE / DATA_CHUNK_SIZE;
// both need SRAM but DMA must be static
pub const HEAP_SIZE: usize = 100 * 1024 - DMA_RX_STREAM_BUF_SIZE;
pub const RECLAIMED_HEAP_SIZE: usize = 64 * 1024;
pub const DMA_RX_STREAM_BUF_SIZE: usize = TX_BUFF_NUMBER_CHUNKS * DATA_CHUNK_SIZE;
pub const VIDEO_DATA_POOL_POOL_SIZE: usize = 32;
pub const CAMERA_STACK_SIZE: usize = 2048;
pub const UDP_RX_NUM_OF_PACKET_PER_BUFFER: usize = 4;
pub const UDP_RX_BUFFER_SIZE: usize = 1024;
pub const UDP_TX_NUM_OF_PACKET_PER_BUFFER: usize = 20;
pub const UDP_TX_BUFFER_SIZE: usize = 32768;
pub const MTU: usize = 1500;
