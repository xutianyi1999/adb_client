use std::io::{BufReader, BufWriter, Read, Write};
use std::str::FromStr;
use std::time::SystemTime;

use byteorder::ReadBytesExt;

use crate::{
    ADBDeviceExt,
    models::{
        ADBCommand, ADBHostCommand, ADBLocalCommand, AdbRequestStatus, HostFeatures, SyncCommand,
    },
    stream_transport::{ReadWrite, StreamTransport},
    Result, RustADBError,
};

const BUFFER_SIZE: usize = 65535;

/// Opens a new byte stream (yamux, TCP, etc.) for each device command.
/// Mirrors the original ADB client which opens a fresh TCP connection per call.
pub trait StreamOpener: Send + Sync {
    /// Open a new stream. Called before every `host:transport:...` command.
    fn open(&self) -> std::result::Result<Box<dyn ReadWrite>, String>;
}

/// ADB device communicating over a generic byte stream (yamux, etc.).
pub struct ADBStreamDevice {
    identifier: Option<String>,
    transport_id: Option<u32>,
    opener: Box<dyn StreamOpener>,
}

impl std::fmt::Debug for ADBStreamDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ADBStreamDevice")
            .field("identifier", &self.identifier)
            .field("transport_id", &self.transport_id)
            .finish()
    }
}

impl ADBStreamDevice {
    /// Create a new stream-backed ADB device.
    pub fn new(serial: String, opener: Box<dyn StreamOpener>) -> Self {
        Self {
            identifier: Some(serial),
            transport_id: None,
            opener,
        }
    }

    /// Open a fresh transport and select the device with `host:transport:...`.
    fn open_transport(&self) -> Result<StreamTransport> {
        let stream = self.opener.open()
            .map_err(|e| RustADBError::IOError(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
        let mut transport = StreamTransport::new();
        transport.connect(stream);

        let cmd = if let Some(id) = self.transport_id {
            ADBHostCommand::TransportId(id)
        } else if let Some(serial) = self.identifier.clone() {
            ADBHostCommand::TransportSerial(serial)
        } else {
            ADBHostCommand::TransportAny
        };
        transport.send_adb_request(&ADBCommand::Host(cmd))?;
        Ok(transport)
    }
}

impl ADBDeviceExt for ADBStreamDevice {
    fn shell_command(
        &mut self,
        command: &dyn AsRef<str>,
        stdout: Option<&mut dyn Write>,
        stderr: Option<&mut dyn Write>,
    ) -> Result<Option<u8>> {
        let supported_features = self.host_features();
        let use_shell_v2 = supported_features.as_ref().is_ok_and(|features| {
            features.contains(&HostFeatures::ShellV2) || features.contains(&HostFeatures::Cmd)
        });

        if use_shell_v2 {
            self.shell_command_v2(command, stdout, stderr)
        } else {
            self.shell_command_v1(command, stdout)
        }
    }

    fn shell(&mut self, _reader: &mut dyn Read, _writer: Box<dyn Write + Send>) -> Result<()> {
        unimplemented!("shell not supported via stream device")
    }

    fn exec(
        &mut self,
        _command: &str,
        _reader: &mut dyn Read,
        _writer: Box<dyn Write + Send>,
    ) -> Result<()> {
        unimplemented!("exec not supported via stream device")
    }

    fn stat(&mut self, _remote_path: &dyn AsRef<str>) -> Result<crate::models::AdbStatResponse> {
        unimplemented!("stat not supported via stream device")
    }

    fn pull(&mut self, _source: &dyn AsRef<str>, _output: &mut dyn Write) -> Result<()> {
        unimplemented!("pull not supported via stream device")
    }

    fn push(&mut self, stream: &mut dyn Read, path: &dyn AsRef<str>) -> Result<()> {
        let transport = self.open_transport()?;
        transport.send_adb_request(&ADBCommand::Local(ADBLocalCommand::Sync))?;
        transport.send_sync_request(&SyncCommand::Send)?;
        self.handle_send_command(&transport, stream, path)
    }

    fn list(&mut self, _path: &dyn AsRef<str>) -> Result<Vec<crate::models::ADBListItemType>> {
        unimplemented!("list not supported via stream device")
    }

    fn reboot(&mut self, _reboot_type: crate::models::RebootType) -> Result<()> {
        unimplemented!("reboot not supported via stream device")
    }

    fn remount(&mut self) -> Result<Vec<crate::models::RemountInfo>> {
        unimplemented!("remount not supported via stream device")
    }

    fn root(&mut self) -> Result<()> {
        unimplemented!("root not supported via stream device")
    }

    fn enable_verity(&mut self) -> Result<()> {
        unimplemented!("enable_verity not supported via stream device")
    }

    fn disable_verity(&mut self) -> Result<()> {
        unimplemented!("disable_verity not supported via stream device")
    }

    fn install(&mut self, _apk_path: &dyn AsRef<std::path::Path>, _user: Option<&str>) -> Result<()> {
        unimplemented!("install not supported via stream device")
    }

    fn uninstall(&mut self, _package: &dyn AsRef<str>, _user: Option<&str>) -> Result<()> {
        unimplemented!("uninstall not supported via stream device")
    }

    #[cfg(feature = "framebuffer")]
    fn framebuffer_inner(
        &mut self,
    ) -> Result<image::ImageBuffer<image::Rgba<u8>, Vec<u8>>> {
        unimplemented!("framebuffer not supported via stream device")
    }
}

impl ADBStreamDevice {
    fn host_features(&self) -> Result<Vec<HostFeatures>> {
        let transport = self.open_transport()?;
        let features = transport
            .proxy_connection(&ADBCommand::Host(ADBHostCommand::HostFeatures), true)?;
        Ok(features
            .split(|x| x.eq(&b','))
            .filter_map(|v| HostFeatures::try_from(v).ok())
            .collect())
    }

    /// Forward socket connection.
    pub fn forward(&self, remote: String, local: String) -> Result<()> {
        let transport = self.open_transport()?;
        transport
            .proxy_connection(
                &ADBCommand::Local(ADBLocalCommand::Forward(remote, local)),
                false,
            )
            .map(|_| ())
    }

    /// Remove a previously applied forward rule by its local endpoint.
    pub fn forward_remove(&self, local: String) -> Result<()> {
        let transport = self.open_transport()?;
        transport
            .proxy_connection(
                &ADBCommand::Local(ADBLocalCommand::ForwardRemove(local)),
                false,
            )
            .map(|_| ())
    }

    /// List all port forwards.
    pub fn forward_list(&self) -> Result<String> {
        let transport = self.open_transport()?;
        let raw = transport
            .proxy_connection(&ADBCommand::Host(ADBHostCommand::ListForward), true)?;
        String::from_utf8(raw).map_err(|e| RustADBError::IOError(std::io::Error::other(e)))
    }

    /// Remove all previously applied forward rules.
    pub fn forward_remove_all(&self) -> Result<()> {
        let transport = self.open_transport()?;
        transport
            .proxy_connection(&ADBCommand::Local(ADBLocalCommand::ForwardRemoveAll), false)
            .map(|_| ())
    }

    fn shell_command_v1(
        &self,
        command: &dyn AsRef<str>,
        mut stdout: Option<&mut dyn Write>,
    ) -> Result<Option<u8>> {
        let transport = self.open_transport()?;
        transport.send_adb_request(&ADBCommand::Local(
            ADBLocalCommand::ShellCommand(command.as_ref().to_string(), vec![]),
        ))?;

        let mut buffer = vec![0; BUFFER_SIZE].into_boxed_slice();
        let mut guard = transport.lock()?;
        let stream: &mut dyn Read = &mut *guard;
        loop {
            let n = match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => match e.kind() {
                    std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::BrokenPipe => break,
                    _ => return Err(RustADBError::IOError(e)),
                },
            };
            if let Some(ref mut stdout) = stdout {
                stdout.write_all(&buffer[..n])?;
            }
        }
        Ok(None)
    }

    fn shell_command_v2(
        &self,
        command: &dyn AsRef<str>,
        mut stdout: Option<&mut dyn Write>,
        mut stderr: Option<&mut dyn Write>,
    ) -> Result<Option<u8>> {
        let mut args = vec!["v2".to_string()];
        if let Ok(term) = std::env::var("TERM") {
            args.push(format!("TERM={term}"));
        }

        let transport = self.open_transport()?;
        transport.send_adb_request(&ADBCommand::Local(
            ADBLocalCommand::ShellCommand(command.as_ref().to_string(), args),
        ))?;

        #[derive(Eq, PartialEq)]
        enum ShellChannel {
            Stdout,
            Stderr,
            ExitStatus,
        }

        impl TryFrom<u8> for ShellChannel {
            type Error = std::io::Error;
            fn try_from(value: u8) -> std::result::Result<Self, Self::Error> {
                match value {
                    1 => Ok(Self::Stdout),
                    2 => Ok(Self::Stderr),
                    3 => Ok(Self::ExitStatus),
                    _ => Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Invalid channel",
                    )),
                }
            }
        }

        let mut exit = None;
        let mut guard = transport.lock()?;
        let stream: &mut dyn Read = &mut *guard;
        let mut input = std::io::BufReader::new(stream);
        let mut buffer = vec![0; BUFFER_SIZE].into_boxed_slice();

        loop {
            let mut pckt_metadata = vec![0; 5];
            if let Err(err) = input.read_exact(&mut pckt_metadata) {
                match err.kind() {
                    std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::BrokenPipe => {
                        return Ok(None)
                    }
                    _ => return Err(RustADBError::IOError(err)),
                }
            }

            let (channel, payload_size) = {
                let channel = pckt_metadata[0];
                let payload_size =
                    u32::from_le_bytes(pckt_metadata[1..5].try_into()?) as usize;
                (ShellChannel::try_from(channel)?, payload_size)
            };

            if payload_size == 0 {
                continue;
            }

            match channel {
                ShellChannel::Stdout | ShellChannel::Stderr => {
                    let mut remainder = payload_size;
                    while remainder > 0 {
                        let to_read = std::cmp::min(remainder, BUFFER_SIZE);
                        match input.read(&mut buffer[..to_read]) {
                            Ok(0) => return Ok(exit),
                            Ok(size) => {
                                match channel {
                                    ShellChannel::Stdout => {
                                        if let Some(ref mut stdout) = stdout {
                                            stdout.write_all(&buffer[..size])?;
                                        }
                                    }
                                    ShellChannel::Stderr => {
                                        if let Some(ref mut writer) = stderr.as_mut() {
                                            writer.write_all(&buffer[..size])?;
                                        } else if let Some(ref mut stdout) = stdout {
                                            stdout.write_all(&buffer[..size])?;
                                        }
                                    }
                                    ShellChannel::ExitStatus => {}
                                }
                                remainder -= size;
                            }
                            Err(e) => return Err(RustADBError::IOError(e)),
                        }
                    }
                }
                ShellChannel::ExitStatus => {
                    if payload_size != 1 {
                        return Err(RustADBError::ADBShellV2ParseError(format!(
                            "Spurious exit status packet with size of {payload_size} (should be 1)"
                        )));
                    }
                    match input.read_u8() {
                        Ok(status) => exit = Some(status),
                        Err(e) => match e.kind() {
                            std::io::ErrorKind::UnexpectedEof
                            | std::io::ErrorKind::BrokenPipe => return Ok(None),
                            _ => return Err(RustADBError::IOError(e)),
                        },
                    }
                }
            }
        }
    }

    fn handle_send_command<S: AsRef<str>>(
        &self,
        transport: &StreamTransport,
        mut input: impl Read,
        to: S,
    ) -> Result<()> {
        let to = to.as_ref().to_string() + ",0777";
        let to_as_bytes = to.as_bytes();

        let mut buffer = Vec::with_capacity(4 + to_as_bytes.len());
        buffer.extend_from_slice(&(u32::try_from(to.len()).unwrap_or(0)).to_le_bytes());
        buffer.extend_from_slice(to_as_bytes);

        let mut stream = transport.lock()?;
        stream.write_all(&buffer)?;

        struct SendWriter<'a>(&'a mut dyn Write);
        impl Write for SendWriter<'_> {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                let chunk_len = u32::try_from(buf.len()).map_err(std::io::Error::other)?;
                let mut v = Vec::with_capacity(8 + buf.len());
                v.extend_from_slice(b"DATA");
                v.extend_from_slice(&chunk_len.to_le_bytes());
                v.extend_from_slice(buf);
                self.0.write_all(&v)?;
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.0.flush()
            }
        }

        let writer = SendWriter(&mut *stream);
        std::io::copy(
            &mut BufReader::with_capacity(BUFFER_SIZE, &mut input),
            &mut BufWriter::with_capacity(BUFFER_SIZE, writer),
        )?;
        drop(stream);

        let Ok(last_modified) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) else {
            return Err(RustADBError::ADBRequestFailed(
                "SystemTime before UNIX EPOCH!".into(),
            ));
        };
        let mut done_buffer = Vec::with_capacity(8);
        done_buffer.extend_from_slice(b"DONE");
        done_buffer.extend_from_slice(&last_modified.as_secs().to_le_bytes());
        transport.lock()?.write_all(&done_buffer)?;

        let mut request_status = [0; 4];
        transport.lock()?.read_exact(&mut request_status)?;
        match AdbRequestStatus::from_str(std::str::from_utf8(&request_status)?)? {
            AdbRequestStatus::Fail => {
                let length = transport.get_body_length()?;
                let mut body = vec![
                    0;
                    length
                        .try_into()
                        .map_err(|_| RustADBError::ConversionError)?
                ];
                if length > 0 {
                    transport.lock()?.read_exact(&mut body)?;
                }
                Err(RustADBError::ADBRequestFailed(String::from_utf8(body)?))
            }
            AdbRequestStatus::Okay => Ok(()),
        }
    }
}
