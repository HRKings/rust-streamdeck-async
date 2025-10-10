#[cfg(not(feature = "async"))]
compile_error!("The `async` feature must be enabled to compile this example.");

use std::sync::Arc;
use std::time::Duration;
use image::open;

use elgato_streamdeck::{DeviceStateUpdate, list_devices, new_hidapi, AsyncStreamDeck};
use elgato_streamdeck::images::{convert_image_with_format, ImageRect};
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    // Create instance of HidApi
    match new_hidapi() {
        Ok(hid) => {
            // Refresh device list
            for (kind, serial) in list_devices(&hid) {
                println!("{:?} {} {}", kind, serial, kind.product_id());

                // Connect to the device
                let device: Arc<AsyncStreamDeck> = Arc::new(AsyncStreamDeck::connect(&hid, kind, &serial).expect("Failed to connect"));
                // Print out some info from the device
                println!("Connected to '{}' with version '{}'", device.serial_number().await.unwrap(), device.firmware_version().await.unwrap());

                device.set_brightness(50).await.unwrap();
                device.clear_all_button_images().await.unwrap();

                // Use image-rs to load an image
                let image = open("examples/no-place-like-localhost.jpg").unwrap();
                let alternative = image.grayscale().brighten(-50);

                println!("Key count: {}", kind.key_count());
                // Write it to the device
                for i in 0..kind.key_count() {
                    device.set_button_image(i, image.clone()).await.unwrap();
                }

                println!("Touch point count: {}", kind.touchpoint_count());
                for i in 0..kind.touchpoint_count() {
                    device.set_touchpoint_color(i, 255, 255, 255).await.unwrap();
                }

                if let Some(format) = device.kind().lcd_image_format() {
                    let scaled_image = image.clone().resize_to_fill(format.size.0 as u32, format.size.1 as u32, image::imageops::FilterType::Nearest);
                    let converted_image = convert_image_with_format(format, scaled_image).unwrap();
                    let _ = device.write_lcd_fill(&converted_image).await;
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

                let device_for_animation = device.clone();

                // Start new task to animate the button images
                tokio::spawn(async move {
                    let mut index = 0;
                    let mut previous = 0;

                    loop {
                        device_for_animation.set_button_image(index, image.clone()).await.unwrap();
                        device_for_animation.set_button_image(previous, alternative.clone()).await.unwrap();

                        device_for_animation.flush().await.unwrap();

                        sleep(Duration::from_secs_f32(0.5)).await;

                        previous = index;

                        index += 1;
                        if index >= kind.key_count() {
                            index = 0;
                        }
                    }
                });

                'infinite: loop {
                    // Read state changes
                    let mut reader = device.get_reader();
                    while let Some(update) = reader.read().await {
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
                                    device.write_lcd(x, y, small.clone()).await.unwrap();
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
            }
        }
        Err(e) => eprintln!("Failed to create HidApi instance: {}", e),
    }
}
