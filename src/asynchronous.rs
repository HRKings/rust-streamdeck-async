//! Async implementation of Stream Deck interface
//!
//! This module provides an async wrapper around the synchronous hidapi library
//! by using a dedicated worker thread that owns the HidDevice and communicates
//! via channels with async code.

use std::sync::Arc;
use std::time::Duration;
use std::thread::JoinHandle;

use hidapi::{HidApi, HidDevice};
use image::DynamicImage;
use tokio::sync::{mpsc, oneshot};

use crate::images::{convert_image, ImageRect};
use crate::{Kind, StreamDeckError, StreamDeckInput, DeviceStateUpdate};

/// Commands that can be sent to the worker thread
enum Command {
    ReadInput {
        timeout: Option<Duration>,
        response: oneshot::Sender<Result<StreamDeckInput, StreamDeckError>>,
    },
    Reset {
        response: oneshot::Sender<Result<(), StreamDeckError>>,
    },
    SetBrightness {
        percent: u8,
        response: oneshot::Sender<Result<(), StreamDeckError>>,
    },
    WriteImage {
        key: u8,
        image_data: Arc<[u8]>,
        response: oneshot::Sender<Result<(), StreamDeckError>>,
    },
    WriteLcd {
        x: u16,
        y: u16,
        rect: ImageRect,
        response: oneshot::Sender<Result<(), StreamDeckError>>,
    },
    WriteLcdFill {
        image_data: Arc<[u8]>,
        response: oneshot::Sender<Result<(), StreamDeckError>>,
    },
    ClearButton {
        key: u8,
        response: oneshot::Sender<Result<(), StreamDeckError>>,
    },
    ClearAllButtons {
        response: oneshot::Sender<Result<(), StreamDeckError>>,
    },
    SetTouchpointColor {
        point: u8,
        red: u8,
        green: u8,
        blue: u8,
        response: oneshot::Sender<Result<(), StreamDeckError>>,
    },
    Flush {
        response: oneshot::Sender<Result<(), StreamDeckError>>,
    },
    GetManufacturer {
        response: oneshot::Sender<Result<String, StreamDeckError>>,
    },
    GetProduct {
        response: oneshot::Sender<Result<String, StreamDeckError>>,
    },
    GetSerialNumber {
        response: oneshot::Sender<Result<String, StreamDeckError>>,
    },
    GetFirmwareVersion {
        response: oneshot::Sender<Result<String, StreamDeckError>>,
    },
    Shutdown,
}

/// Async interface for Stream Deck device
pub struct AsyncStreamDeck {
    kind: Kind,
    command_tx: mpsc::Sender<Command>,
    worker_handle: Option<JoinHandle<()>>,
}

impl AsyncStreamDeck {
    /// Connects to a Stream Deck device asynchronously
    ///
    /// This spawns a worker thread that owns the HidDevice and handles all I/O operations
    pub fn connect(hidapi: &HidApi, kind: Kind, serial: &str) -> Result<Self, StreamDeckError> {
        let device = hidapi.open_serial(kind.vendor_id(), kind.product_id(), serial)?;

        let (cmd_tx, cmd_rx) = mpsc::channel(32);

        let worker_handle = std::thread::spawn(move || {
            Self::worker_loop(device, kind, cmd_rx);
        });

        Ok(AsyncStreamDeck {
            kind,
            command_tx: cmd_tx,
            worker_handle: Some(worker_handle),
        })
    }

    /// Returns the kind of this Stream Deck device
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Worker thread that owns the HidDevice and processes commands
    fn worker_loop(device: HidDevice, kind: Kind, mut cmd_rx: mpsc::Receiver<Command>) {
        // Use the synchronous StreamDeck for actual operations
        let sync_deck = crate::StreamDeck {
            kind,
            device,
            image_cache: std::sync::RwLock::new(vec![]),
        };

        while let Some(cmd) = cmd_rx.blocking_recv() {
            match cmd {
                Command::Shutdown => break,

                Command::ReadInput { timeout, response } => {
                    let result = sync_deck.read_input(timeout);
                    let _ = response.send(result);
                }

                Command::Reset { response } => {
                    let result = sync_deck.reset();
                    let _ = response.send(result);
                }

                Command::SetBrightness { percent, response } => {
                    let result = sync_deck.set_brightness(percent);
                    let _ = response.send(result);
                }

                Command::WriteImage { key, image_data, response } => {
                    let result = sync_deck.write_image(key, &image_data);
                    let _ = response.send(result);
                }

                Command::WriteLcd { x, y, rect, response } => {
                    let result = sync_deck.write_lcd(x, y, &rect);
                    let _ = response.send(result);
                }

                Command::WriteLcdFill { image_data, response } => {
                    let result = sync_deck.write_lcd_fill(&image_data);
                    let _ = response.send(result);
                }

                Command::ClearButton { key, response } => {
                    let result = sync_deck.clear_button_image(key);
                    let _ = response.send(result);
                }

                Command::ClearAllButtons { response } => {
                    let result = sync_deck.clear_all_button_images();
                    let _ = response.send(result);
                }

                Command::SetTouchpointColor { point, red, green, blue, response } => {
                    let result = sync_deck.set_touchpoint_color(point, red, green, blue);
                    let _ = response.send(result);
                }

                Command::Flush { response } => {
                    let result = sync_deck.flush();
                    let _ = response.send(result);
                }

                Command::GetManufacturer { response } => {
                    let result = sync_deck.manufacturer();
                    let _ = response.send(result);
                }

                Command::GetProduct { response } => {
                    let result = sync_deck.product();
                    let _ = response.send(result);
                }

                Command::GetSerialNumber { response } => {
                    let result = sync_deck.serial_number();
                    let _ = response.send(result);
                }

                Command::GetFirmwareVersion { response } => {
                    let result = sync_deck.firmware_version();
                    let _ = response.send(result);
                }
            }
        }
    }

    /// Sends a command and waits for response
    async fn send_command<T>(&self, cmd_builder: impl FnOnce(oneshot::Sender<T>) -> Command) -> Result<T, StreamDeckError> {
        let (tx, rx) = oneshot::channel();

        self.command_tx.send(cmd_builder(tx)).await.map_err(|_| StreamDeckError::WorkerThreadSendError)?;

        rx.await.map_err(|_| StreamDeckError::WorkerThreadSendError)
    }

    /// Reads input from the Stream Deck device
    pub async fn read_input(&self, timeout: Option<Duration>) -> Result<StreamDeckInput, StreamDeckError> {
        self.send_command(|response| Command::ReadInput { timeout, response }).await?
    }

    /// Resets the device
    pub async fn reset(&self) -> Result<(), StreamDeckError> {
        self.send_command(|response| Command::Reset { response }).await?
    }

    /// Sets the brightness of the device (0-100)
    pub async fn set_brightness(&self, percent: u8) -> Result<(), StreamDeckError> {
        self.send_command(|response| Command::SetBrightness { percent, response }).await?
    }

    /// Writes raw image data to a button
    ///
    /// Changes must be flushed with `.flush()` before they appear on the device
    pub async fn write_image(&self, key: u8, image_data: &[u8]) -> Result<(), StreamDeckError> {
        let image_data = Arc::from(image_data);
        self.send_command(|response| Command::WriteImage { key, image_data, response }).await?
    }

    /// Writes image data to the LCD strip/screen as a region (Stream Deck Plus only)
    pub async fn write_lcd(&self, x: u16, y: u16, rect: ImageRect) -> Result<(), StreamDeckError> {
        self.send_command(|response| Command::WriteLcd { x, y, rect, response }).await?
    }

    /// Writes image data to fill the entire LCD strip/screen
    pub async fn write_lcd_fill(&self, image_data: &[u8]) -> Result<(), StreamDeckError> {
        let image_data = Arc::from(image_data);
        self.send_command(|response| Command::WriteLcdFill { image_data, response }).await?
    }

    /// Clears a button's image
    ///
    /// Changes must be flushed with `.flush()` before they appear on the device
    pub async fn clear_button_image(&self, key: u8) -> Result<(), StreamDeckError> {
        self.send_command(|response| Command::ClearButton { key, response }).await?
    }

    /// Clears all button images
    ///
    /// Changes must be flushed with `.flush()` before they appear on the device
    pub async fn clear_all_button_images(&self) -> Result<(), StreamDeckError> {
        self.send_command(|response| Command::ClearAllButtons { response }).await?
    }

    /// Sets a button's image from a DynamicImage
    ///
    /// Changes must be flushed with `.flush()` before they appear on the device
    pub async fn set_button_image(&self, key: u8, image: DynamicImage) -> Result<(), StreamDeckError> {
        let image_data = convert_image(self.kind, image)?;
        self.write_image(key, &image_data).await
    }

    /// Sets a touchpoint's LED strip color
    pub async fn set_touchpoint_color(&self, point: u8, red: u8, green: u8, blue: u8) -> Result<(), StreamDeckError> {
        self.send_command(|response| Command::SetTouchpointColor { point, red, green, blue, response }).await?
    }

    /// Flushes all pending image changes to the device
    pub async fn flush(&self) -> Result<(), StreamDeckError> {
        self.send_command(|response| Command::Flush { response }).await?
    }

    /// Gets the manufacturer string
    pub async fn manufacturer(&self) -> Result<String, StreamDeckError> {
        self.send_command(|response| Command::GetManufacturer { response }).await?
    }

    /// Gets the product string
    pub async fn product(&self) -> Result<String, StreamDeckError> {
        self.send_command(|response| Command::GetProduct { response }).await?
    }

    /// Gets the serial number
    pub async fn serial_number(&self) -> Result<String, StreamDeckError> {
        self.send_command(|response| Command::GetSerialNumber { response }).await?
    }

    /// Gets the firmware version
    pub async fn firmware_version(&self) -> Result<String, StreamDeckError> {
        self.send_command(|response| Command::GetFirmwareVersion { response }).await?
    }

    /// Creates a state reader that emits device state updates as a stream
    ///
    /// Note: This takes `&self` not `self`, so you can continue using the device
    /// after creating a reader. The reader internally keeps an Arc to the device.
    pub fn get_reader(&self) -> AsyncDeviceStateReader
    where
        Self: Sized,
    {
        let (tx, rx) = mpsc::channel(100);

        // Clone the command sender to create an independent communication channel
        let cmd_tx = self.command_tx.clone();
        let kind = self.kind;

        let handle = tokio::spawn(async move {
            let mut button_states = vec![false; kind.key_count() as usize + kind.touchpoint_count() as usize];
            let mut encoder_states = vec![false; kind.encoder_count() as usize];

            loop {
                // Create a temporary deck-like structure to send commands
                let (response_tx, response_rx) = oneshot::channel();

                if cmd_tx
                    .send(Command::ReadInput {
                        timeout: Some(Duration::from_millis(10)),
                        response: response_tx,
                    })
                    .await
                    .is_err()
                {
                    break; // Command channel closed, device is gone
                }

                match response_rx.await {
                    Ok(Ok(input)) => {
                        let updates = Self::process_input(input, &mut button_states, &mut encoder_states, kind.key_count());

                        for update in updates {
                            if tx.send(update).await.is_err() {
                                return; // Receiver dropped, exit loop
                            }
                        }
                    }
                    Ok(Err(_)) | Err(_) => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
            }
        });

        AsyncDeviceStateReader {
            updates_rx: rx,
            _worker_handle: handle,
        }
    }

    /// Processes input and generates state updates
    fn process_input(input: StreamDeckInput, button_states: &mut Vec<bool>, encoder_states: &mut Vec<bool>, key_count: u8) -> Vec<DeviceStateUpdate> {
        let mut updates = Vec::new();

        match input {
            StreamDeckInput::ButtonStateChange(new_states) => {
                let len = new_states.len().min(button_states.len());

                for i in 0..len {
                    if new_states[i] != button_states[i] {
                        if i < key_count as usize {
                            if new_states[i] {
                                updates.push(DeviceStateUpdate::ButtonDown(i as u8));
                            } else {
                                updates.push(DeviceStateUpdate::ButtonUp(i as u8));
                            }
                        } else if new_states[i] {
                            updates.push(DeviceStateUpdate::TouchPointDown(i as u8 - key_count));
                        } else {
                            updates.push(DeviceStateUpdate::TouchPointUp(i as u8 - key_count));
                        }
                    }
                }

                button_states.copy_from_slice(&new_states);
            }

            StreamDeckInput::EncoderStateChange(new_states) => {
                for (i, (&new, &old)) in new_states.iter().zip(encoder_states.iter()).enumerate() {
                    if new != old {
                        if new {
                            updates.push(DeviceStateUpdate::EncoderDown(i as u8));
                        } else {
                            updates.push(DeviceStateUpdate::EncoderUp(i as u8));
                        }
                    }
                }
                encoder_states.copy_from_slice(&new_states);
            }

            StreamDeckInput::EncoderTwist(twists) => {
                for (i, &twist) in twists.iter().enumerate() {
                    if twist != 0 {
                        updates.push(DeviceStateUpdate::EncoderTwist(i as u8, twist));
                    }
                }
            }

            StreamDeckInput::TouchScreenPress(x, y) => {
                updates.push(DeviceStateUpdate::TouchScreenPress(x, y));
            }

            StreamDeckInput::TouchScreenLongPress(x, y) => {
                updates.push(DeviceStateUpdate::TouchScreenLongPress(x, y));
            }

            StreamDeckInput::TouchScreenSwipe(start, end) => {
                updates.push(DeviceStateUpdate::TouchScreenSwipe(start, end));
            }

            StreamDeckInput::NoData => {}
        }

        updates
    }
}

impl Drop for AsyncStreamDeck {
    fn drop(&mut self) {
        // Send shutdown signal
        let _ = self.command_tx.try_send(Command::Shutdown);

        // Wait for worker thread to finish
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Async reader for device state changes
pub struct AsyncDeviceStateReader {
    updates_rx: mpsc::Receiver<DeviceStateUpdate>,
    _worker_handle: tokio::task::JoinHandle<()>,
}

impl AsyncDeviceStateReader {
    /// Reads the next state update
    ///
    /// Returns None when the reader is closed
    pub async fn read(&mut self) -> Option<DeviceStateUpdate> {
        self.updates_rx.recv().await
    }

    /// Converts this reader into a Stream
    #[cfg(feature = "tokio-stream")]
    pub fn into_stream(self) -> impl tokio_stream::Stream<Item = DeviceStateUpdate> {
        tokio_stream::wrappers::ReceiverStream::new(self.updates_rx)
    }
}
