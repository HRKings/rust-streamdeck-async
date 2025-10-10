//! Elgato Streamdeck library
//!
//! Library for interacting with Elgato Stream Decks through [hidapi](https://crates.io/crates/hidapi).
//! Heavily based on [python-elgato-streamdeck](https://github.com/abcminiuser/python-elgato-streamdeck) and partially on
//! [streamdeck library for rust](https://github.com/ryankurte/rust-streamdeck).

#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]

use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::iter::zip;
use std::str::Utf8Error;
use std::sync::RwLock;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use futures_lite::stream::StreamExt;

use crate::images::{convert_image, ImageRect};
use async_hid::{Device, DeviceReader, DeviceWriter, HidBackend, HidError, HidResult};
use image::{DynamicImage, ImageError};

use crate::info::{is_vendor_familiar, Kind};
use crate::util::{extract_str, flip_key_index, get_feature_report, read_button_states, read_data, read_encoder_input, read_lcd_input, send_feature_report, write_data};

/// Various information about Stream Deck devices
pub mod info;
/// Utility functions for working with Stream Deck devices
pub mod util;
/// Image processing functions
pub mod images;

/// Creates an instance of the HidApi
///
/// Can be used if you don't want to link hidapi crate into your project
pub fn new_hidapi() -> HidBackend {
    HidBackend::default()
}

/// Returns a list of devices as (Kind, Serial Number) that could be found using HidApi.
pub async fn list_devices(hidapi: &HidBackend) -> HidResult<HashMap<String, Kind>> {
    let devices = hidapi
        .enumerate()
        .await?
        .filter_map(|d| {
            if !is_vendor_familiar(&d.vendor_id) {
                return None;
            }

            if let Some(serial) = d.serial_number.clone() {
                Some((serial, Kind::from_vid_pid(d.vendor_id, d.product_id)?))
            } else {
                None
            }
        })
        .collect::<HashMap<_, _>>()
        .await;

    Ok(devices)
}

/// Type of input that the device produced
#[derive(Clone, Debug)]
pub enum StreamDeckInput {
    /// No data was passed from the device
    NoData,

    /// Button was pressed
    ButtonStateChange(Vec<bool>),

    /// Encoder/Knob was pressed
    EncoderStateChange(Vec<bool>),

    /// Encoder/Knob was twisted/turned
    EncoderTwist(Vec<i8>),

    /// Touch screen received short press
    TouchScreenPress(u16, u16),

    /// Touch screen received long press
    TouchScreenLongPress(u16, u16),

    /// Touch screen received a swipe
    TouchScreenSwipe((u16, u16), (u16, u16)),
}

impl StreamDeckInput {
    /// Checks if there's data received or not
    pub fn is_empty(&self) -> bool {
        matches!(self, StreamDeckInput::NoData)
    }
}

/// Interface for a Stream Deck device
pub struct StreamDeck {
    /// Kind of the device
    kind: Kind,
    /// Connected HIDDevice
    device: Device,
    /// HIDDevice Writer
    device_writer: DeviceWriter,
    /// HIDDevice Reader
    device_reader: DeviceReader,
    /// Temporarily cache the image before sending it to the device
    image_cache: RwLock<Vec<ImageCache>>,
}

#[derive(Clone)]
struct ImageCache {
    key: u8,
    image_data: Vec<u8>,
}

/// Static functions of the struct
impl StreamDeck {
    /// Attempts to connect to the device
    pub async fn connect(hidapi: &HidBackend, kind: Kind, serial: &str) -> Result<StreamDeck, StreamDeckError> {
        let device_option = hidapi
            .enumerate()
            .await?
            .find(|d| d.vendor_id == kind.vendor_id() && d.product_id == kind.product_id() && d.serial_number == Some(serial.to_string()))
            .await;

        if let Some(device) = device_option {
            let (device_reader, device_writer) = device.open().await?;

            Ok(StreamDeck {
                kind,
                device,
                device_reader,
                device_writer,
                image_cache: RwLock::new(vec![]),
            })
        } else {
            Err(StreamDeckError::Device404)
        }
    }
}

/// Instance methods of the struct
impl StreamDeck {
    /// Returns kind of the Stream Deck
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Returns manufacturer string of the device
    pub fn manufacturer(&self) -> Result<String, StreamDeckError> {
        todo!();
        // Ok(self.device.get_manufacturer_string()?.unwrap_or_else(|| "Unknown".to_string()))
    }

    /// Returns product string of the device
    pub fn product(&self) -> Result<&str, StreamDeckError> {
        Ok(&self.device.name)
    }

    /// Returns serial number of the device
    pub async fn serial_number(&mut self) -> Result<String, StreamDeckError> {
        match self.kind {
            Kind::Original | Kind::Mini => {
                let bytes = get_feature_report(&mut self.device_reader, 0x03, 17).await?;
                Ok(extract_str(&bytes[5..])?)
            }

            Kind::MiniMk2 | Kind::MiniMk2Module => {
                let bytes = get_feature_report(&mut self.device_reader, 0x03, 32).await?;
                Ok(extract_str(&bytes[5..])?)
            }

            _ => {
                let bytes = get_feature_report(&mut self.device_reader, 0x06, 32).await?;
                Ok(extract_str(&bytes[2..])?)
            }
        }
        .map(|s| s.replace('\u{0001}', ""))
    }

    /// Returns firmware version of the StreamDeck
    pub async fn firmware_version(&mut self) -> Result<String, StreamDeckError> {
        match self.kind {
            Kind::Original | Kind::Mini | Kind::MiniMk2 => {
                let bytes = get_feature_report(&mut self.device_reader, 0x04, 17).await?;
                Ok(extract_str(&bytes[5..])?)
            }

            Kind::MiniMk2Module => {
                let bytes = get_feature_report(&mut self.device_reader, 0xA1, 17).await?;
                Ok(extract_str(&bytes[5..])?)
            }

            _ => {
                let bytes = get_feature_report(&mut self.device_reader, 0x05, 32).await?;
                Ok(extract_str(&bytes[6..])?)
            }
        }
    }

    /// Reads all possible input from Stream Deck device
    pub async fn read_input(&mut self, timeout: Option<Duration>) -> Result<StreamDeckInput, StreamDeckError> {
        match &self.kind {
            Kind::Plus => {
                let data = read_data(&mut self.device_reader, 14.max(5 + self.kind.encoder_count() as usize), timeout).await?;

                if data[0] == 0 {
                    return Ok(StreamDeckInput::NoData);
                }

                match &data[1] {
                    0x0 => Ok(StreamDeckInput::ButtonStateChange(read_button_states(&self.kind, &data))),

                    0x2 => Ok(read_lcd_input(&data)?),

                    0x3 => Ok(read_encoder_input(&self.kind, &data)?),

                    _ => Err(StreamDeckError::BadData),
                }
            }

            _ => {
                let data = match self.kind {
                    Kind::Original | Kind::Mini | Kind::MiniMk2 | Kind::MiniMk2Module => read_data(&mut self.device_reader, 1 + self.kind.key_count() as usize, timeout).await,
                    _ => read_data(&mut self.device_reader, 4 + self.kind.key_count() as usize + self.kind.touchpoint_count() as usize, timeout).await,
                }?;

                if data[0] == 0 {
                    return Ok(StreamDeckInput::NoData);
                }

                Ok(StreamDeckInput::ButtonStateChange(read_button_states(&self.kind, &data)))
            }
        }
    }

    /// Resets the device
    pub async fn reset(&mut self) -> Result<(), StreamDeckError> {
        match self.kind {
            Kind::Original | Kind::Mini | Kind::MiniMk2 | Kind::MiniMk2Module => {
                let mut buf = vec![0x0B, 0x63];

                buf.extend(vec![0u8; 15]);

                Ok(send_feature_report(&mut self.device_writer, buf.as_slice()).await?)
            }

            _ => {
                let mut buf = vec![0x03, 0x02];

                buf.extend(vec![0u8; 30]);

                Ok(send_feature_report(&mut self.device_writer, buf.as_slice()).await?)
            }
        }
    }

    /// Sets brightness of the device, value range is 0 - 100
    pub async fn set_brightness(&mut self, percent: u8) -> Result<(), StreamDeckError> {
        let percent = percent.clamp(0, 100);

        match self.kind {
            Kind::Original | Kind::Mini | Kind::MiniMk2 | Kind::MiniMk2Module => {
                let mut buf = vec![0x05, 0x55, 0xaa, 0xd1, 0x01, percent];

                buf.extend(vec![0u8; 11]);

                Ok(send_feature_report(&mut self.device_writer, buf.as_slice()).await?)
            }

            _ => {
                let mut buf = vec![0x03, 0x08, percent];

                buf.extend(vec![0u8; 29]);

                Ok(send_feature_report(&mut self.device_writer, buf.as_slice()).await?)
            }
        }
    }

    async fn send_image(&mut self, key: u8, image_data: &[u8]) -> Result<(), StreamDeckError> {
        let kind = self.kind.clone();

        if key >= kind.key_count() {
            return Err(StreamDeckError::InvalidKeyIndex);
        }

        let key = if let Kind::Original = &kind { flip_key_index(&kind, key) } else { key };

        if !&kind.is_visual() {
            return Err(StreamDeckError::NoScreen);
        }

        self.write_image_data_reports(image_data, WriteImageParameters::for_key(kind, image_data.len()), |page_number, this_length, last_package| match kind {
            Kind::Original => vec![0x02, 0x01, (page_number + 1) as u8, 0, if last_package { 1 } else { 0 }, key + 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],

            Kind::Mini | Kind::MiniMk2 | Kind::MiniMk2Module => vec![0x02, 0x01, page_number as u8, 0, if last_package { 1 } else { 0 }, key + 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],

            _ => vec![
                0x02,
                0x07,
                key,
                if last_package { 1 } else { 0 },
                (this_length & 0xff) as u8,
                (this_length >> 8) as u8,
                (page_number & 0xff) as u8,
                (page_number >> 8) as u8,
            ],
        })
        .await?;
        Ok(())
    }

    /// Writes image data to Stream Deck device, changes must be flushed with `.flush()` before
    /// they will appear on the device!
    pub fn write_image(&mut self, key: u8, image_data: &[u8]) -> Result<(), StreamDeckError> {
        let cache_entry = ImageCache { key, image_data: image_data.to_vec() };

        self.image_cache.write()?.push(cache_entry);

        Ok(())
    }

    /// Writes image data to Stream Deck device's lcd strip/screen as region.
    /// Only Stream Deck Plus supports writing LCD regions, for Stream Deck Neo use write_lcd_fill
    pub async fn write_lcd(&mut self, x: u16, y: u16, rect: &ImageRect) -> Result<(), StreamDeckError> {
        match self.kind {
            Kind::Plus => (),
            _ => return Err(StreamDeckError::UnsupportedOperation),
        }

        self.write_image_data_reports(
            rect.data.as_slice(),
            WriteImageParameters {
                image_report_length: 1024,
                image_report_payload_length: 1024 - 16,
            },
            |page_number, this_length, last_package| {
                vec![
                    0x02,
                    0x0c,
                    (x & 0xff) as u8,
                    (x >> 8) as u8,
                    (y & 0xff) as u8,
                    (y >> 8) as u8,
                    (rect.w & 0xff) as u8,
                    (rect.w >> 8) as u8,
                    (rect.h & 0xff) as u8,
                    (rect.h >> 8) as u8,
                    if last_package { 1 } else { 0 },
                    (page_number & 0xff) as u8,
                    (page_number >> 8) as u8,
                    (this_length & 0xff) as u8,
                    (this_length >> 8) as u8,
                    0,
                ]
            },
        )
        .await
    }

    /// Writes image data to Stream Deck device's lcd strip/screen as full fill
    ///
    /// You can convert your images into proper image_data like this:
    /// ```
    /// use elgato_streamdeck::images::convert_image_with_format;
    /// let image_data = convert_image_with_format(device.kind().lcd_image_format(), image).unwrap();
    /// device.write_lcd_fill(&image_data);
    /// ```
    pub async fn write_lcd_fill(&mut self, image_data: &[u8]) -> Result<(), StreamDeckError> {
        match self.kind {
            Kind::Neo => {
                self.write_image_data_reports(
                    image_data,
                    WriteImageParameters {
                        image_report_length: 1024,
                        image_report_payload_length: 1024 - 8,
                    },
                    |page_number, this_length, last_package| {
                        vec![
                            0x02,
                            0x0b,
                            0,
                            if last_package { 1 } else { 0 },
                            (this_length & 0xff) as u8,
                            (this_length >> 8) as u8,
                            (page_number & 0xff) as u8,
                            (page_number >> 8) as u8,
                        ]
                    },
                )
                .await
            }

            Kind::Plus => {
                let (w, h) = self.kind.lcd_strip_size().unwrap();

                self.write_image_data_reports(
                    image_data,
                    WriteImageParameters {
                        image_report_length: 1024,
                        image_report_payload_length: 1024 - 16,
                    },
                    |page_number, this_length, last_package| {
                        vec![
                            0x02,
                            0x0c,
                            0,
                            0,
                            0,
                            0,
                            (w & 0xff) as u8,
                            (w >> 8) as u8,
                            (h & 0xff) as u8,
                            (h >> 8) as u8,
                            if last_package { 1 } else { 0 },
                            (page_number & 0xff) as u8,
                            (page_number >> 8) as u8,
                            (this_length & 0xff) as u8,
                            (this_length >> 8) as u8,
                            0,
                        ]
                    },
                )
                .await
            }

            _ => Err(StreamDeckError::UnsupportedOperation),
        }
    }

    /// Sets button's image to blank, changes must be flushed with `.flush()` before
    /// they will appear on the device!
    pub async fn clear_button_image(&mut self, key: u8) -> Result<(), StreamDeckError> {
        self.send_image(key, &self.kind.blank_image()).await
    }

    /// Sets blank images to every button, changes must be flushed with `.flush()` before
    /// they will appear on the device!
    pub async fn clear_all_button_images(&mut self) -> Result<(), StreamDeckError> {
        for i in 0..self.kind.key_count() {
            self.clear_button_image(i).await?
        }
        Ok(())
    }

    /// Sets specified button's image, changes must be flushed with `.flush()` before
    /// they will appear on the device!
    pub fn set_button_image(&mut self, key: u8, image: DynamicImage) -> Result<(), StreamDeckError> {
        let image_data = convert_image(self.kind, image)?;
        self.write_image(key, &image_data)?;
        Ok(())
    }

    /// Sets specified touch point's led strip color
    pub async fn set_touchpoint_color(&mut self, point: u8, red: u8, green: u8, blue: u8) -> Result<(), StreamDeckError> {
        if point >= self.kind.touchpoint_count() {
            return Err(StreamDeckError::InvalidTouchPointIndex);
        }

        let mut buf = vec![0x03, 0x06];

        let touchpoint_index: u8 = point + self.kind.key_count();
        buf.extend(vec![touchpoint_index]);
        buf.extend(vec![red, green, blue]);

        Ok(send_feature_report(&mut self.device_writer, buf.as_slice()).await?)
    }

    /// Flushes the button's image to the device
    pub async fn flush(&mut self) -> Result<(), StreamDeckError> {
        let image_cache_snapshot: Vec<_> = {
            let image_cache = self.image_cache.read()?;
            image_cache.iter().cloned().collect::<Vec<_>>() // clone the entries you need
        };

        if image_cache_snapshot.is_empty() {
            return Ok(());
        }

        for image in image_cache_snapshot.iter() {
            self.send_image(image.key, &image.image_data).await?;
        }

        self.image_cache.write()?.clear();

        Ok(())
    }

    /// Returns button state reader for this device
    pub fn get_reader(self: Self) -> DeviceStateReader {
        let key_count = self.kind.key_count().clone();
        let touchpoint_count = self.kind.touchpoint_count().clone();
        let encoder_count = self.kind.encoder_count().clone();

        DeviceStateReader {
            device: Arc::new(Mutex::new(self)),
            states: Mutex::new(DeviceState {
                buttons: vec![false; key_count as usize + touchpoint_count as usize],
                encoders: vec![false; encoder_count as usize],
            }),
        }
    }

    async fn write_image_data_reports<T>(&mut self, image_data: &[u8], parameters: WriteImageParameters, header_fn: T) -> Result<(), StreamDeckError>
    where
        T: Fn(usize, usize, bool) -> Vec<u8>,
    {
        let image_report_length = parameters.image_report_length;
        let image_report_payload_length = parameters.image_report_payload_length;

        let mut page_number = 0;
        let mut bytes_remaining = image_data.len();

        while bytes_remaining > 0 {
            let this_length = bytes_remaining.min(image_report_payload_length);
            let bytes_sent = page_number * image_report_payload_length;

            // Selecting header based on device
            let mut buf: Vec<u8> = header_fn(page_number, this_length, this_length == bytes_remaining);

            buf.extend(&image_data[bytes_sent..bytes_sent + this_length]);

            // Adding padding
            buf.extend(vec![0u8; image_report_length - buf.len()]);

            write_data(&mut self.device_writer, &buf).await?;

            bytes_remaining -= this_length;
            page_number += 1;
        }

        Ok(())
    }
}

#[derive(Clone, Copy)]
struct WriteImageParameters {
    pub image_report_length: usize,
    pub image_report_payload_length: usize,
}

impl WriteImageParameters {
    pub fn for_key(kind: Kind, image_data_len: usize) -> Self {
        let image_report_length = match kind {
            Kind::Original => 8191,
            _ => 1024,
        };

        let image_report_header_length = match kind {
            Kind::Original | Kind::Mini | Kind::MiniMk2 | Kind::MiniMk2Module => 16,
            _ => 8,
        };

        let image_report_payload_length = match kind {
            Kind::Original => image_data_len / 2,
            _ => image_report_length - image_report_header_length,
        };

        Self {
            image_report_length,
            image_report_payload_length,
        }
    }
}

/// Errors that can occur while working with Stream Decks
#[derive(Debug)]
pub enum StreamDeckError {
    /// HidApi error
    HidError(HidError),

    /// Failed to convert bytes into string
    Utf8Error(Utf8Error),

    /// Failed to encode image
    ImageError(ImageError),

    /// Reader mutex was poisoned
    PoisonError,

    /// No device was found
    Device404,

    /// There's literally nowhere to write the image
    NoScreen,

    /// Key index is invalid
    InvalidKeyIndex,

    /// Key index is invalid
    InvalidTouchPointIndex,

    /// Unrecognized Product ID
    UnrecognizedPID,

    /// The device doesn't support doing that
    UnsupportedOperation,

    /// Stream Deck sent unexpected data
    BadData,
}

impl Display for StreamDeckError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Error for StreamDeckError {}

impl From<HidError> for StreamDeckError {
    fn from(e: HidError) -> Self {
        Self::HidError(e)
    }
}

impl From<Utf8Error> for StreamDeckError {
    fn from(e: Utf8Error) -> Self {
        Self::Utf8Error(e)
    }
}

impl From<ImageError> for StreamDeckError {
    fn from(e: ImageError) -> Self {
        Self::ImageError(e)
    }
}

impl<T> From<PoisonError<T>> for StreamDeckError {
    fn from(_value: PoisonError<T>) -> Self {
        Self::PoisonError
    }
}

/// Tells what changed in button states
#[derive(Copy, Clone, Debug, Hash)]
pub enum DeviceStateUpdate {
    /// Button got pressed down
    ButtonDown(u8),

    /// Button got released
    ButtonUp(u8),

    /// Encoder got pressed down
    EncoderDown(u8),

    /// Encoder was released from being pressed down
    EncoderUp(u8),

    /// Encoder was twisted
    EncoderTwist(u8, i8),

    /// Touch Point got pressed down
    TouchPointDown(u8),

    /// Touch Point got released
    TouchPointUp(u8),

    /// Touch screen received short press
    TouchScreenPress(u16, u16),

    /// Touch screen received long press
    TouchScreenLongPress(u16, u16),

    /// Touch screen received a swipe
    TouchScreenSwipe((u16, u16), (u16, u16)),
}

#[derive(Default)]
struct DeviceState {
    /// Buttons include Touch Points state
    pub buttons: Vec<bool>,
    pub encoders: Vec<bool>,
}

/// Button reader that keeps state of the Stream Deck and returns events instead of full states
pub struct DeviceStateReader {
    pub device: Arc<Mutex<StreamDeck>>,
    states: Mutex<DeviceState>,
}

impl DeviceStateReader {
    /// Reads states and returns updates
    pub async fn read(&mut self, timeout: Option<Duration>) -> Result<Vec<DeviceStateUpdate>, StreamDeckError> {
        let input = self.device.lock()?.read_input(timeout).await?;
        let mut my_states = self.states.lock()?;

        let mut updates = vec![];

        match input {
            StreamDeckInput::ButtonStateChange(buttons) => {
                for (index, (their, mine)) in zip(buttons.iter(), my_states.buttons.iter()).enumerate() {
                    if their != mine {
                        let key_count = self.device.lock()?.kind.key_count();
                        if index < key_count as usize {
                            if *their {
                                updates.push(DeviceStateUpdate::ButtonDown(index as u8));
                            } else {
                                updates.push(DeviceStateUpdate::ButtonUp(index as u8));
                            }
                        } else if *their {
                            updates.push(DeviceStateUpdate::TouchPointDown(index as u8 - key_count));
                        } else {
                            updates.push(DeviceStateUpdate::TouchPointUp(index as u8 - key_count));
                        }
                    }
                }

                my_states.buttons = buttons;
            }

            StreamDeckInput::EncoderStateChange(encoders) => {
                for (index, (their, mine)) in zip(encoders.iter(), my_states.encoders.iter()).enumerate() {
                    if *their != *mine {
                        if *their {
                            updates.push(DeviceStateUpdate::EncoderDown(index as u8));
                        } else {
                            updates.push(DeviceStateUpdate::EncoderUp(index as u8));
                        }
                    }
                }

                my_states.encoders = encoders;
            }

            StreamDeckInput::EncoderTwist(twist) => {
                for (index, change) in twist.iter().enumerate() {
                    if *change != 0 {
                        updates.push(DeviceStateUpdate::EncoderTwist(index as u8, *change));
                    }
                }
            }

            StreamDeckInput::TouchScreenPress(x, y) => {
                updates.push(DeviceStateUpdate::TouchScreenPress(x, y));
            }

            StreamDeckInput::TouchScreenLongPress(x, y) => {
                updates.push(DeviceStateUpdate::TouchScreenLongPress(x, y));
            }

            StreamDeckInput::TouchScreenSwipe(s, e) => {
                updates.push(DeviceStateUpdate::TouchScreenSwipe(s, e));
            }

            _ => {}
        }

        drop(my_states);

        Ok(updates)
    }
}
