use face_id::detector::ScrfdDetector;
use log::{debug, info};
use shared::{JpegFrameChunk, STRUCT_CHUNK_SIZE};
use simple_logger::SimpleLogger;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicI32;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::broadcast;
use tokio::time;

mod face;
use crate::face::face_detection;

async fn fps(counter: Arc<AtomicI32>) {
    let mut interval = time::interval(time::Duration::from_secs(1));
    interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let fps = counter.swap(0, std::sync::atomic::Ordering::Relaxed);
        info!("FPS {}", fps);
    }
}

async fn udp_receiver(udp_tx: broadcast::Sender<Vec<u8>>, frames: Arc<AtomicI32>) {
    let socket = UdpSocket::bind("0.0.0.0:5000")
        .await
        .expect("Failed to bind UDP socket");
    info!("Relay listening for UDP chunks on 0.0.0.0:5000...");

    let mut buf = [0u8; STRUCT_CHUNK_SIZE];
    let mut pending_frames: HashMap<u16, Vec<Option<Vec<u8>>>> = HashMap::new();

    loop {
        let (nbytes, src) = socket
            .recv_from(&mut buf)
            .await
            .expect("Failed to receive UDP packet");
        debug!("DEBUG: Received {} bytes from {}", nbytes, src);

        if let Some(chunk) = JpegFrameChunk::from_bytes(&buf[..nbytes]) {
            if pending_frames.len() > 10 {
                let current_id = chunk.frame_id;
                pending_frames.retain(|&id, _| {
                    let diff = if current_id >= id {
                        current_id - id
                    } else {
                        u16::MAX - id + current_id
                    };
                    diff < 5
                });
            }

            let entry = pending_frames
                .entry(chunk.frame_id)
                .or_insert_with(|| vec![None; chunk.total_chunks as usize]);

            if let Some(slot) = entry.get_mut(chunk.chunk_id as usize) {
                if slot.is_none() {
                    *slot = Some(chunk.payload[..chunk.payload_len as usize].to_vec());
                }
            }

            let received_count = entry.iter().filter(|c| c.is_some()).count();
            debug!(
                "\rFrame {}: Chunks {}/{} from {}      ",
                chunk.frame_id, received_count, chunk.total_chunks, src
            );
            // std::io::stdout().flush().unwrap();

            if entry.iter().all(|c| c.is_some()) {
                let full_image: Vec<u8> = entry
                    .iter()
                    .filter_map(|c| c.as_ref())
                    .flatten()
                    .cloned()
                    .collect();

                debug!(
                    "\n[OK] Frame {} reassembled! Size: {} bytes",
                    chunk.frame_id,
                    full_image.len()
                );

                frames.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                // Send it to image processing
                let _ = udp_tx.send(full_image);

                pending_frames.remove(&chunk.frame_id);
            }
        }
    }
}

async fn tcp_broadcast(mut socket: TcpStream, mut rx: broadcast::Receiver<Vec<u8>>) {
    let header = "HTTP/1.1 200 OK\r\n\
                          Content-Type: multipart/x-mixed-replace; boundary=frame\r\n\
                          Cache-Control: no-cache\r\n\
                          Connection: close\r\n\r\n";
    if socket.write_all(header.as_bytes()).await.is_err() {
        return;
    }

    loop {
        match rx.recv().await {
            Ok(frame) => {
                let frame_header = format!(
                    "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                    frame.len()
                );
                if socket.write_all(frame_header.as_bytes()).await.is_err() {
                    break;
                }
                if socket.write_all(&frame).await.is_err() {
                    break;
                }
                if socket.write_all(b"\r\n").await.is_err() {
                    break;
                }
            }
            // Handle case where browser is too slow and skips frames
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => break,
        }
    }
}

#[tokio::main]
async fn main() {
    // Create a broadcast channel for reassembled JPEG frames.
    // We use a buffer of 16 frames; slow clients will be dropped if they lag too far.
    SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .init()
        .unwrap();

    let (upd_image_tx, _) = broadcast::channel::<Vec<u8>>(16536);
    let (image_tcp_tx, _) = broadcast::channel::<Vec<u8>>(16536);
    let udp_to_image_tx = upd_image_tx.clone();
    let image_to_tcp = image_tcp_tx.clone();

    let detector = ScrfdDetector::from_hf().build().await.unwrap();

    let counter = Arc::new(AtomicI32::new(0));
    let frames = counter.clone();

    tokio::spawn(fps(counter));
    tokio::spawn(udp_receiver(udp_to_image_tx, frames));

    let from_udp = upd_image_tx.subscribe();

    tokio::spawn(face_detection(detector, from_udp, image_to_tcp));

    let listener = TcpListener::bind("0.0.0.0:8081")
        .await
        .expect("Failed to bind TCP listener");
    info!("Browser stream available at http://localhost:8081");

    while let Ok((socket, _)) = listener.accept().await {
        let rx = image_tcp_tx.subscribe();
        tokio::spawn(tcp_broadcast(socket, rx));
    }
}
