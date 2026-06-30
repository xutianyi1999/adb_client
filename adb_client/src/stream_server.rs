use crate::{
    Result,
    models::{ADBCommand, ADBHostCommand},
    server::DeviceLong,
    stream_transport::{ReadWrite, StreamTransport},
};

/// ADB server over a generic byte stream (yamux, etc.).
pub struct ADBStreamServer {
    transport: StreamTransport,
}

impl std::fmt::Debug for ADBStreamServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ADBStreamServer").finish()
    }
}

impl ADBStreamServer {
    /// Create a new ADB server over the given byte stream.
    pub fn new(stream: Box<dyn ReadWrite>) -> Self {
        let mut transport = StreamTransport::new();
        transport.connect(stream);
        Self { transport }
    }

    /// Send `host:devices-long` and parse the response.
    pub fn devices_long(&self) -> Result<Vec<DeviceLong>> {
        let raw = self
            .transport
            .proxy_connection(&ADBCommand::Host(ADBHostCommand::DevicesLong), true)?;

        let mut devices = vec![];
        for entry in raw.split(|x| x.eq(&b'\n')) {
            if entry.is_empty() {
                break;
            }
            devices.push(DeviceLong::try_from(entry)?);
        }
        Ok(devices)
    }
}
