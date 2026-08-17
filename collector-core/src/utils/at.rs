use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_serial::{SerialPortBuilderExt, SerialStream};

#[derive(Debug, thiserror::Error)]
pub enum AtCommandError {
    #[error("{0}")]
    SerialError(#[from] tokio_serial::Error),
    #[error("{0}")]
    IoError(#[from] std::io::Error),
}

pub struct AtCommand {
    reader: BufReader<SerialStream>,
}

impl AtCommand {
    pub fn open(path: &str, baudrate: u32) -> Result<Self, AtCommandError> {
        let serial = tokio_serial::new(path, baudrate).open_native_async()?;
        Ok(Self {
            reader: BufReader::new(serial),
        })
    }

    pub async fn command(&mut self, cmd: &str) -> Result<String, AtCommandError> {
        self.reader.write_all(cmd.as_bytes()).await?;
        self.reader.write_all(b"\r\n").await?;
        self.reader.flush().await?;

        let mut response = String::new();
        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).await?;
            if n == 0 {
                break;
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            response.push_str(trimmed);
            response.push('\n');

            if trimmed == "OK" || trimmed == "ERROR" || trimmed.starts_with("+CME ERROR") {
                break;
            }
        }

        Ok(response)
    }
}
