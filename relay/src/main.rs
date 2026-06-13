use face_id::analyzer::FaceAnalyzer;
use face_id::detector::ScrfdDetector;
use imageproc::rect::Rect;
use ort::ep::{CUDA, CoreML, DirectML, TensorRT};
use shared::{JpegFrameChunk, STRUCT_CHUNK_SIZE};
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::AtomicI32;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::broadcast;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    // Create a broadcast channel for reassembled JPEG frames.
    // We use a buffer of 16 frames; slow clients will be dropped if they lag too far.
    let (tx, _) = broadcast::channel::<Vec<u8>>(32);
    let udp_tx = tx.clone();

    let mut detector = ScrfdDetector::from_hf().build().await?;

    tokio::spawn(async move {
        let mut image_folder =
            std::fs::read_dir("saved_images").expect("Failed to read saved_images directory");

        loop {
            for entry in image_folder {
                if let Ok(entry) = entry {
                    println!("Processing image: {}", entry.path().display());
                    let mut image = image::open(entry.path()).expect("Failed to open image");
                    // let r = image.rotate90();

                    let faces = detector.detect(&image).unwrap();

                    let abs_face_coord: Vec<(i32, i32, u32, u32)> = faces
                        .iter()
                        .map(|i| i.to_absolute(image.width(), image.height()))
                        .map(|b| {
                            (
                                b.bbox.x1 as i32,
                                b.bbox.y1 as i32,
                                (b.bbox.x2 as i32 - b.bbox.x1 as i32) as u32,
                                (b.bbox.y2 as i32 - b.bbox.y1 as i32) as u32,
                            )
                        })
                        .collect();
                    // println!("abs: {:#?}", abs_face_coord[0]);
                    if abs_face_coord.is_empty() {
                        println!("No faces detected in image: {}", entry.path().display());
                        let mut jpeg_data = Vec::new();
                        if image
                            .write_to(
                                &mut std::io::Cursor::new(&mut jpeg_data),
                                image::ImageFormat::Jpeg,
                            )
                            .is_ok()
                        {
                            // Only send if there is at least one subscriber
                            if udp_tx.receiver_count() > 0 {
                                let _ = udp_tx.send(jpeg_data);
                                println!("Sent JPEG frame to channel");
                            }
                        }
                        continue;
                    }

                    imageproc::drawing::draw_hollow_rect_mut(
                        &mut image,
                        Rect::at(abs_face_coord[0].0, abs_face_coord[0].1)
                            .of_size(abs_face_coord[0].2, abs_face_coord[0].3),
                        image::Rgba([255, 0, 0, 255]),
                    );
                    println!(
                        "Detected {} faces in image: {}",
                        faces.len(),
                        entry.path().display()
                    );
                    let mut jpeg_data = Vec::new();
                    if image
                        .write_to(
                            &mut std::io::Cursor::new(&mut jpeg_data),
                            image::ImageFormat::Jpeg,
                        )
                        .is_ok()
                    {
                        // Only send if there is at least one subscriber
                        if udp_tx.receiver_count() > 0 {
                            let _ = udp_tx.send(jpeg_data);
                            println!("Sent JPEG frame to channel");
                        }
                        //     // let r = udp_tx.send(image.into_bytes());
                        //     // match r {
                        //     //     Ok(_) => println!("Sent frame to browser"),
                        //     //     Err(e) => println!("Failed to send frame to browser: {}", e),
                    }
                }
            }
            image_folder =
                std::fs::read_dir("saved_images").expect("Failed to read saved_images directory");
        }
    });
    // let mut counter = Arc::new(AtomicI32::new(0));
    // let counter_clone = counter.clone();

    // tokio::spawn(async move {
    //     let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    //     loop {
    //         interval.tick().await;
    //         let count = counter_clone.swap(0, std::sync::atomic::Ordering::SeqCst);
    //         println!("FPS: {}", count);
    //     }
    // });

    // // Task 1: UDP Receiver and Frame Reassembler
    // tokio::spawn(async move {
    //     let socket = UdpSocket::bind("0.0.0.0:5000")
    //         .await
    //         .expect("Failed to bind UDP socket");
    //     // println!("Relay listening for UDP chunks on 0.0.0.0:5000...");

    //     let mut buf = [0u8; STRUCT_CHUNK_SIZE];
    //     let mut pending_frames: HashMap<u16, Vec<Option<Vec<u8>>>> = HashMap::new();

    //     loop {
    //         let (nbytes, src) = socket
    //             .recv_from(&mut buf)
    //             .await
    //             .expect("Failed to receive UDP packet");
    //         // println!("DEBUG: Received {} bytes from {}", nbytes, src);

    //         if let Some(chunk) = JpegFrameChunk::from_bytes(&buf[..nbytes]) {
    //             if pending_frames.len() > 10 {
    //                 let current_id = chunk.frame_id;
    //                 pending_frames.retain(|&id, _| {
    //                     let diff = if current_id >= id {
    //                         current_id - id
    //                     } else {
    //                         u16::MAX - id + current_id
    //                     };
    //                     diff < 5
    //                 });
    //             }

    //             let entry = pending_frames
    //                 .entry(chunk.frame_id)
    //                 .or_insert_with(|| vec![None; chunk.total_chunks as usize]);

    //             if let Some(slot) = entry.get_mut(chunk.chunk_id as usize) {
    //                 if slot.is_none() {
    //                     *slot = Some(chunk.payload[..chunk.payload_len as usize].to_vec());
    //                 }
    //             }

    //             let received_count = entry.iter().filter(|c| c.is_some()).count();
    //             print!(
    //                 "\rFrame {}: Chunks {}/{} from {}      ",
    //                 chunk.frame_id, received_count, chunk.total_chunks, src
    //             );
    //             std::io::stdout().flush().unwrap();

    //             if entry.iter().all(|c| c.is_some()) {
    //                 let full_image: Vec<u8> = entry
    //                     .iter()
    //                     .filter_map(|c| c.as_ref())
    //                     .flatten()
    //                     .cloned()
    //                     .collect();

    //                 // println!(
    //                     // "\n[OK] Frame {} reassembled! Size: {} bytes",
    //                     // chunk.frame_id,
    //                     // full_image.len()
    //                 // );
    //                 counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    //                 // Broadcast the complete frame to all connected browsers
    //                 let _ = udp_tx.send(full_image);

    //                 pending_frames.remove(&chunk.frame_id);
    //             }
    //         }
    //     }
    // });

    // Task 2: TCP Server for MJPEG streaming
    let listener = TcpListener::bind("0.0.0.0:8081")
        .await
        .expect("Failed to bind TCP listener");
    println!("Browser stream available at http://localhost:8081");

    while let Ok((mut socket, _)) = listener.accept().await {
        let mut rx = tx.subscribe();
        tokio::spawn(async move {
            println!("New browser connected for MJPEG stream");
            let header = "HTTP/1.1 200 OK\r\n\
                          Content-Type: multipart/x-mixed-replace; boundary=frame\r\n\
                          Cache-Control: no-cache\r\n\
                          Connection: close\r\n\r\n";
            if socket.write_all(header.as_bytes()).await.is_err() {
                println!("Failed to send HTTP header to browser");
                return Err::<(), ()>(());
            }

            loop {
                match rx.recv().await {
                    Ok(frame) => {
                        println!("Sending frame of size {} bytes to browser", frame.len());
                        let frame_header = format!(
                            "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                            frame.len()
                        );
                        if socket.write_all(frame_header.as_bytes()).await.is_err() {
                            break Err(());
                        }
                        if socket.write_all(&frame).await.is_err() {
                            break Err(());
                        }
                        if socket.write_all(b"\r\n").await.is_err() {
                            break Err(());
                        }
                    }
                    // Handle case where browser is too slow and skips frames
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        println!("Browser lagged behind, skipping frames");
                        continue;
                    }
                    Err(_) => {
                        println!("Failed to receive frame from broadcast channel");
                        return Err(());
                    }
                }
            }
        });
    }
    Ok(())
}
