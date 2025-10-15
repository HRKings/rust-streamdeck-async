#[cfg(not(feature = "stream"))]
compile_error!("The `stream` feature must be enabled to compile this example.");

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use image::open;

use elgato_streamdeck_async::{DeviceStateUpdate, StreamDeck, list_devices, new_hidapi};
use elgato_streamdeck_async::images::{convert_image_with_format, ImageRect};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tokio_stream::{StreamExt, StreamMap};

struct StreamDeckAnimated {
    pub device: Arc<StreamDeck>,
    pub animation_thread: JoinHandle<()>,
}

#[tokio::main]
async fn main() {
    // Create instance of HidApi
    let hid = new_hidapi();
    let streamdecks = Arc::new(RwLock::new(HashMap::new()));

    let devices = list_devices(&hid).await.expect("Could not list devices");

    // Use image-rs to load an image
    let image = open("examples/no-place-like-localhost.jpg").unwrap();

    for (serial, kind) in devices {
        println!("{:?} {} {}", kind, serial, kind.product_id());

        // Connect to the device
        let device = Arc::new(StreamDeck::connect(&hid, kind, &serial).await.expect("Could not connect to device"));

        // Print out some info from the device
        println!("Connected to '{}' with version '{}'", device.serial_number().await.unwrap(), device.firmware_version().await.unwrap());

        device.set_brightness(50).await.unwrap();
        device.clear_all_button_images().await.unwrap();

        let image = image.clone();
        let alternative = image.grayscale().brighten(-50);

        println!("Key count: {}", kind.key_count());
        // Write it to the device
        for i in 0..kind.key_count() {
            device.set_button_image(i, image.clone()).await.expect("Could not set button image");
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

        // Flush
        device.flush().await.unwrap();

        let device_for_animation = device.clone();

        // Start new task to animate the button images
        let animation_thread = tokio::spawn(async move {
            let mut index = 0;
            let mut previous = 0;

            loop {
                device_for_animation.set_button_image(index, image.clone()).await.expect("Could not set button image");
                device_for_animation.set_button_image(previous, alternative.clone()).await.expect("Could not set button image");

                device_for_animation.flush().await.unwrap();

                sleep(Duration::from_millis(50)).await; // 50ms = 20fps

                previous = index;

                index += 1;
                if index >= kind.key_count() {
                    index = 0;
                }
            }
        });

        streamdecks.write().await.insert(
            device.serial_number().await.unwrap().clone(),
            StreamDeckAnimated {
                device: device.clone(),
                animation_thread,
            },
        );
    }

    let mut stream_map = StreamMap::new();
    let devices_read = streamdecks.read().await;

    for (path, streamdeck) in devices_read.iter() {
        let reader = streamdeck.device.clone().get_reader().await.expect("Could not get reader");
        let stream = reader.into_stream(Some(Duration::from_millis(10)));

        stream_map.insert(path, stream);
    }

    // Read state changes
    while let Some((serial, update_result)) = stream_map.next().await {
        let devices_read = streamdecks.read().await;
        let streamdeck = devices_read.get(serial).unwrap();

        let small = match streamdeck.device.kind().lcd_strip_size() {
            Some((w, h)) => {
                let min = w.min(h) as u32;
                let scaled_image = image.clone().resize_to_fill(min, min, image::imageops::Nearest);
                Some(ImageRect::from_image(scaled_image).unwrap())
            }
            None => None,
        };

        if let Ok(update) = update_result {
            match update {
                DeviceStateUpdate::ButtonDown(key) => {
                    println!("Button {} down. Device: {}", key, serial);
                }
                DeviceStateUpdate::ButtonUp(key) => {
                    println!("Button {} up. Device: {}", key, serial);

                    // Ensures this is the press of the last button
                    if key == streamdeck.device.kind().key_count() - 1 {
                        // Abort animation thread
                        streamdeck.animation_thread.abort();

                        // Reset and shutdown the device
                        streamdeck.device.reset().await.expect("could not reset deck");
                        stream_map.remove(serial);
                    }
                }
                DeviceStateUpdate::EncoderTwist(dial, ticks) => {
                    println!("Dial {} twisted by {}. Device: {}", dial, ticks, serial);
                }
                DeviceStateUpdate::EncoderDown(dial) => {
                    println!("Dial {} down. Device: {}", dial, serial);
                }
                DeviceStateUpdate::EncoderUp(dial) => {
                    println!("Dial {} up. Device: {}", dial, serial);
                }

                DeviceStateUpdate::TouchPointDown(point) => {
                    println!("Touch point {} down. Device: {}", point, serial);
                }
                DeviceStateUpdate::TouchPointUp(point) => {
                    println!("Touch point {} up. Device: {}", point, serial);
                }

                DeviceStateUpdate::TouchScreenPress(x, y) => {
                    println!("Touch Screen press at {x}, {y}. Device: {}", serial);
                    if let Some(small) = &small {
                        streamdeck.device.write_lcd(x, y, small).await.unwrap();
                    }
                }

                DeviceStateUpdate::TouchScreenLongPress(x, y) => {
                    println!("Touch Screen long press at {x}, {y}. Device: {}", serial);
                }

                DeviceStateUpdate::TouchScreenSwipe((sx, sy), (ex, ey)) => {
                    println!("Touch Screen swipe from {sx}, {sy} to {ex}, {ey}. Device: {}", serial);
                }
            }
        }
    }
}
