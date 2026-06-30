use std::io::{Error, ErrorKind, Read, Write};
use std::str::FromStr;

use parking_lot::Mutex;

use byteorder::{ByteOrder, LittleEndian};

use crate::{
    ADBTransport,
    models::{ADBCommand, AdbRequestStatus, SyncCommand},
    Result, RustADBError,
};

/// Trait alias for `Read + Write + Send`.
pub trait ReadWrite: Read + Write + Send {}
impl<T: Read + Write + Send> ReadWrite for T {}

/// ADB transport over a generic byte stream.
pub struct StreamTransport {
    stream: Option<Mutex<Box<dyn ReadWrite>>>,
}

impl std::fmt::Debug for StreamTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamTransport")
            .field("connected", &self.stream.is_some())
            .finish()
    }
}

impl StreamTransport {
    /// Create a new disconnected transport.
    pub const fn new() -> Self {
        Self { stream: None }
    }

    /// Attach a byte stream transport.
    pub fn connect(&mut self, stream: Box<dyn ReadWrite>) {
        self.stream = Some(Mutex::new(stream));
    }

    pub(crate)     fn lock(&self) -> Result<parking_lot::MutexGuard<'_, Box<dyn ReadWrite>>> {
        self.stream
            .as_ref()
            .ok_or_else(|| {
                RustADBError::IOError(Error::new(ErrorKind::NotConnected, "not connected"))
            })
            .map(|m| m.lock())
    }

    pub(crate) fn proxy_connection(
        &self,
        adb_command: &ADBCommand,
        with_response: bool,
    ) -> Result<Vec<u8>> {
        self.send_adb_request(adb_command)?;

        if with_response {
            let length = self.get_hex_body_length()?;
            let mut body = vec![
                0;
                length
                    .try_into()
                    .map_err(|_| RustADBError::ConversionError)?
            ];
            if length > 0 {
                self.lock()?.read_exact(&mut body)?;
            }
            Ok(body)
        } else {
            Ok(vec![])
        }
    }

    pub(crate) fn send_sync_request(&self, command: &SyncCommand) -> Result<()> {
        Ok(self.lock()?.write_all(command.to_string().as_bytes())?)
    }

    pub(crate) fn get_body_length(&self) -> Result<u32> {
        let length_buffer = self.read_body_length()?;
        Ok(LittleEndian::read_u32(&length_buffer))
    }

    fn read_body_length(&self) -> Result<[u8; 4]> {
        let mut length_buffer = [0; 4];
        self.lock()?.read_exact(&mut length_buffer)?;
        Ok(length_buffer)
    }

    fn get_hex_body_length(&self) -> Result<u32> {
        let length_buffer = self.read_body_length()?;
        Ok(u32::from_str_radix(
            std::str::from_utf8(&length_buffer)?,
            16,
        )?)
    }

    pub(crate) fn send_adb_request(&self, command: &ADBCommand) -> Result<()> {
        let adb_command_string = command.to_string();
        let adb_request = format!("{:04x}{}", adb_command_string.len(), adb_command_string);
        self.lock()?.write_all(adb_request.as_bytes())?;
        self.read_adb_response()
    }

    fn read_adb_response(&self) -> Result<()> {
        let mut request_status = [0; 4];
        self.lock()?.read_exact(&mut request_status)?;

        match AdbRequestStatus::from_str(std::str::from_utf8(&request_status)?)? {
            AdbRequestStatus::Fail => {
                let length = self.get_hex_body_length()?;
                let mut body = vec![
                    0;
                    length
                        .try_into()
                        .map_err(|_| RustADBError::ConversionError)?
                ];
                if length > 0 {
                    self.lock()?.read_exact(&mut body)?;
                }
                Err(RustADBError::ADBRequestFailed(String::from_utf8(body)?))
            }
            AdbRequestStatus::Okay => Ok(()),
        }
    }
}

impl ADBTransport for StreamTransport {
    fn disconnect(&mut self) -> Result<()> {
        self.stream = None;
        Ok(())
    }

    fn connect(&mut self) -> Result<()> {
        Ok(())
    }
}
