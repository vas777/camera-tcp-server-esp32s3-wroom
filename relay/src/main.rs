use std::collections::HashMap;
use tokio::net::UdpSocket;
use shared::{JpegFrameChunk, STRUCT_CHUNK_SIZE};

#[tokio::main]
async fn main() {
    // Bind to the address the ESP32 is targeting.
    // "0.0.0.0" allows listening on all interfaces, including the one 
    // connected to the ESP32 Access Point.
    let socket = UdpSocket::bind("0.0.0.0:5000").await.expect("Failed to bind UDP socket");
    println!("Relay listening on 0.0.0.0:5000...");

    // Buffer to hold raw packet data. Must be at least STRUCT_CHUNK_SIZE.
    let mut buf = [0u8; STRUCT_CHUNK_SIZE];
    
    // frame_id -> list of chunks (Option to track missing ones)
    let mut pending_frames: HashMap<u16, Vec<Option<Vec<u8>>>> = HashMap::new();
    let mut last_processed_frame = 0u16;

    loop {
        let (nbytes, src) = socket.recv_from(&mut buf).await.expect("Failed to receive UDP packet");
        // Heartbeat: print for every packet received to verify the network path
        println!("DEBUG: Received {} bytes from {}", nbytes, src);

        if let Some(chunk) = JpegFrameChunk::from_bytes(&buf[..nbytes]) {
            // Use a threshold to prevent memory leaks from incomplete frames
            if pending_frames.len() > 10 {
                let current_id = chunk.frame_id;
                pending_frames.retain(|&id, _| {
                    // Keep only frames that are "recent" relative to current frame_id
                    let diff = if current_id >= id { current_id - id } else { u16::MAX - id + current_id };
                    diff < 5
                });
            }

            let entry = pending_frames
                .entry(chunk.frame_id)
                .or_insert_with(|| vec![None; chunk.total_chunks as usize]);

            // Store the payload slice if we haven't received this chunk yet
            if (chunk.chunk_id as usize) < entry.len() && entry[chunk.chunk_id as usize].is_none() {
                entry[chunk.chunk_id as usize] = Some(chunk.payload[..chunk.payload_len as usize].to_vec());
            }

            let received_count = entry.iter().filter(|c| c.is_some()).count();
            print!("\rFrame {}: Chunks {}/{} from {}      ", chunk.frame_id, received_count, chunk.total_chunks, src);
            use std::io::Write;
            std::io::stdout().flush().unwrap();

            // Check if all chunks for this frame have arrived
            if entry.iter().all(|c| c.is_some()) {
                let full_image: Vec<u8> = entry
                    .iter()
                    .filter_map(|c| c.as_ref())
                    .flatten()
                    .cloned()
                    .collect();
                
                println!("\n[OK] Frame {} reassembled! Size: {} bytes", chunk.frame_id, full_image.len());
                last_processed_frame = chunk.frame_id;
                
                // Cleanup finished frame
                pending_frames.remove(&chunk.frame_id);
            }
        }
    }
}
