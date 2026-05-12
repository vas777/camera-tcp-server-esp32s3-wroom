use std::collections::HashMap;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::broadcast;
use shared::{JpegFrameChunk, STRUCT_CHUNK_SIZE};
use std::io::Write;

#[tokio::main]
async fn main() {
    // Create a broadcast channel for reassembled JPEG frames.
    // We use a buffer of 16 frames; slow clients will be dropped if they lag too far.
    let (tx, _) = broadcast::channel::<Vec<u8>>(16);
    let udp_tx = tx.clone();

    // Task 1: UDP Receiver and Frame Reassembler
    tokio::spawn(async move {
        let socket = UdpSocket::bind("0.0.0.0:5000")
            .await
            .expect("Failed to bind UDP socket");
        println!("Relay listening for UDP chunks on 0.0.0.0:5000...");

        let mut buf = [0u8; STRUCT_CHUNK_SIZE];
        let mut pending_frames: HashMap<u16, Vec<Option<Vec<u8>>>> = HashMap::new();

        loop {
            let (nbytes, src) = socket
                .recv_from(&mut buf)
                .await
                .expect("Failed to receive UDP packet");
            println!("DEBUG: Received {} bytes from {}", nbytes, src);

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

                if (chunk.chunk_id as usize) < entry.len() && entry[chunk.chunk_id as usize].is_none() {
                    entry[chunk.chunk_id as usize] = Some(chunk.payload[..chunk.payload_len as usize].to_vec());
                }

                let received_count = entry.iter().filter(|c| c.is_some()).count();
                print!(
                    "\rFrame {}: Chunks {}/{} from {}      ",
                    chunk.frame_id, received_count, chunk.total_chunks, src
                );
                std::io::stdout().flush().unwrap();

                if entry.iter().all(|c| c.is_some()) {
                    let full_image: Vec<u8> = entry
                        .iter()
                        .filter_map(|c| c.as_ref())
                        .flatten()
                        .cloned()
                        .collect();

                    println!(
                        "\n[OK] Frame {} reassembled! Size: {} bytes",
                        chunk.frame_id,
                        full_image.len()
                    );

                    // Broadcast the complete frame to all connected browsers
                    let _ = udp_tx.send(full_image);

                    pending_frames.remove(&chunk.frame_id);
                }
            }
        }
    });

    // Task 2: TCP Server for MJPEG streaming
    let listener = TcpListener::bind("0.0.0.0:8081")
        .await
        .expect("Failed to bind TCP listener");
    println!("Browser stream available at http://localhost:8081");

    while let Ok((mut socket, _)) = listener.accept().await {
        let mut rx = tx.subscribe();
        tokio::spawn(async move {
            let header = "HTTP/1.1 200 OK\r\n\
                          Content-Type: multipart/x-mixed-replace; boundary=frame\r\n\
                          Cache-Control: no-cache\r\n\
                          Connection: close\r\n\r\n";
            if socket.write_all(header.as_bytes()).await.is_err() { return; }

            loop {
                match rx.recv().await {
                    Ok(frame) => {
                        let frame_header = format!(
                            "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                            frame.len()
                        );
                        if socket.write_all(frame_header.as_bytes()).await.is_err() { break; }
                        if socket.write_all(&frame).await.is_err() { break; }
                        if socket.write_all(b"\r\n").await.is_err() { break; }
                    }
                    // Handle case where browser is too slow and skips frames
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
    }
}
