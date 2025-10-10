use std::sync::Arc;
use std::time::Duration;

use image::open;

use elgato_streamdeck::{DeviceStateUpdate, list_devices, new_hidapi, StreamDeck};
use elgato_streamdeck::images::{convert_image_with_format, ImageRect};

#[tokio::main]
async fn main() {
    // Create instance of HidApi
    let hid = new_hidapi();

    for (serial, kind) in list_devices(&hid).await.expect("Could not list HID devices") {
        println!("Found: {:?} {} {}", kind, serial, kind.product_id());

        // Connect to the device
        let mut device = StreamDeck::connect(&hid, kind, &serial).await.expect("Failed to connect");
        // Print out some info from the device
        println!(
            "Connected to '{}' with version '{}'",
            device.serial_number().await.expect("Could not get serial number"),
            device.firmware_version().await.expect("Could not get firmware version")
        );

        device.set_brightness(50).await.expect("Could not set the brightness");
        device.clear_all_button_images().await.unwrap();
        // Use image-rs to load an image
        let image = open("examples/no-place-like-localhost.jpg").unwrap();

        println!("Key count: {}", kind.key_count());
        // Write it to the device
        for i in 0..kind.key_count() {
            device.set_button_image(i, image.clone()).unwrap();
        }

        println!("Touch point count: {}", kind.touchpoint_count());
        for i in 0..kind.touchpoint_count() {
            device.set_touchpoint_color(i, 255, 255, 255).await.unwrap();
        }

        if let Some(format) = device.kind().lcd_image_format() {
            let scaled_image = image.clone().resize_to_fill(format.size.0 as u32, format.size.1 as u32, image::imageops::FilterType::Nearest);
            let converted_image = convert_image_with_format(format, scaled_image).unwrap();
            let _ = device.write_lcd_fill(&converted_image);
        }

        let small = match device.kind().lcd_strip_size() {
            Some((w, h)) => {
                let min = w.min(h) as u32;
                let scaled_image = image.clone().resize_to_fill(min, min, image::imageops::Nearest);
                Some(ImageRect::from_image(scaled_image).unwrap())
            }
            None => None,
        };

        // Flush
        device.flush().await.unwrap();

        // #[allow(clippy::arc_with_non_send_sync)]
        // let device = Arc::new(device);

        let kind = device.kind().clone();
        {
            let mut reader = device.get_reader();

            'infinite: loop {
                let updates = match reader.read(Some(Duration::from_secs_f64(100.0))).await {
                    Ok(updates) => updates,
                    Err(_) => break,
                };

                for update in updates {
                    match update {
                        DeviceStateUpdate::ButtonDown(key) => {
                            println!("Button {} down", key);
                        }
                        DeviceStateUpdate::ButtonUp(key) => {
                            println!("Button {} up", key);
                            if key == kind.key_count() - 1 {
                                break 'infinite;
                            }
                        }
                        DeviceStateUpdate::EncoderTwist(dial, ticks) => {
                            println!("Dial {} twisted by {}", dial, ticks);
                        }
                        DeviceStateUpdate::EncoderDown(dial) => {
                            println!("Dial {} down", dial);
                        }
                        DeviceStateUpdate::EncoderUp(dial) => {
                            println!("Dial {} up", dial);
                        }

                        DeviceStateUpdate::TouchPointDown(point) => {
                            println!("Touch point {} down", point);
                        }
                        DeviceStateUpdate::TouchPointUp(point) => {
                            println!("Touch point {} up", point);
                        }

                        DeviceStateUpdate::TouchScreenPress(x, y) => {
                            println!("Touch Screen press at {x}, {y}");
                            if let Some(small) = &small {
                                reader.device.lock().expect("could not lock device").write_lcd(x, y, small).await.unwrap();
                            }
                        }

                        DeviceStateUpdate::TouchScreenLongPress(x, y) => {
                            println!("Touch Screen long press at {x}, {y}")
                        }

                        DeviceStateUpdate::TouchScreenSwipe((sx, sy), (ex, ey)) => {
                            println!("Touch Screen swipe from {sx}, {sy} to {ex}, {ey}")
                        }
                    }
                }
            }

            drop(reader);
        }
    }
}
