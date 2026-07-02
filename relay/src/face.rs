use face_id::detector::ScrfdDetector;
use imageproc::rect::Rect;
use log::{debug, error};
use tokio::sync::broadcast;

pub async fn face_detection(
    mut detector: ScrfdDetector,
    from_udp: broadcast::Receiver<Vec<u8>>,
    image_to_tcp: broadcast::Sender<Vec<u8>>,
) {
    let mut rec: broadcast::Receiver<Vec<u8>> = from_udp.resubscribe();
    loop {
        match rec.recv().await {
            Ok(frame) => {
                let mut image = image::DynamicImage::from(image::load_from_memory(&frame).unwrap());

                let faces = detector.detect(&image).unwrap();

                let absolute_coordinates_of_face: Vec<(i32, i32, u32, u32)> = faces
                    .iter()
                    .map(|i| i.to_absolute(image.width(), image.height()))
                    .map(|b| {
                        (
                            b.bbox.x1 as i32,
                            b.bbox.y1 as i32,
                            // height and width of bounding box
                            (b.bbox.x2 as i32 - b.bbox.x1 as i32) as u32,
                            (b.bbox.y2 as i32 - b.bbox.y1 as i32) as u32,
                        )
                    })
                    .collect();
                debug!("abs: {:#?}", absolute_coordinates_of_face[0]);
                if absolute_coordinates_of_face.is_empty() {
                    debug!("No faces detected in image");
                    let mut jpeg_data = Vec::new();

                    if image
                        .write_to(
                            &mut std::io::Cursor::new(&mut jpeg_data),
                            image::ImageFormat::Jpeg,
                        )
                        .is_ok()
                    {
                        // Only send if there is at least one subscriber
                        if image_to_tcp.receiver_count() > 0 {
                            let _ = image_to_tcp.send(jpeg_data);
                            debug!("Sent JPEG frame to channel");
                        }
                    }
                    continue;
                }

                imageproc::drawing::draw_hollow_rect_mut(
                    &mut image,
                    Rect::at(absolute_coordinates_of_face[0].0, absolute_coordinates_of_face[0].1)
                        .of_size(absolute_coordinates_of_face[0].2, absolute_coordinates_of_face[0].3),
                    image::Rgba([255, 0, 0, 255]),
                );
                debug!("Detected {} faces in image", faces.len(),);
                let mut jpeg_data = Vec::new();
                if image
                    .write_to(
                        &mut std::io::Cursor::new(&mut jpeg_data),
                        image::ImageFormat::Jpeg,
                    )
                    .is_ok()
                {
                    // Only send if there is at least one subscriber
                    if image_to_tcp.receiver_count() > 0 {
                        let _ = image_to_tcp.send(jpeg_data);
                        debug!("Sent JPEG frame to channel");
                    }
                }
            }
            // Handle case where browser is too slow and skips frames
            Err(broadcast::error::RecvError::Lagged(_)) => {
                debug!("Browser lagged behind, skipping frames");
                continue;
            }
            Err(_) => {
                error!("Failed to receive frame from broadcast channel");
            }
        }
    }
}
