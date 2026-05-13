use calamine::{Data, DataType, HeaderRow, Range, Reader, Xlsx, open_workbook};
use serde::Serialize;

use crate::{
    config::{optional_static_str, required_f64, required_static_str},
    core::point::{DataPoint, Translator, Val},
};

#[derive(Debug, thiserror::Error)]
pub enum GpioConfigsError {
    #[error("Failed to open workbook: {0}")]
    OpenWorkbookError(#[from] calamine::XlsxError),
}

pub type GpioConfigs = Vec<GpioConfig>;

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub enum Direction {
    DI,
    DO,
}

impl TryFrom<&str> for Direction {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.eq_ignore_ascii_case("DI") {
            Ok(Direction::DI)
        } else if value.eq_ignore_ascii_case("DO") {
            Ok(Direction::DO)
        } else {
            Err(anyhow::anyhow!("Invalid direction: {}", value))
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GpioConfig {
    pub id: u32,
    pub key: &'static str,
    pub gpio: u16,
    pub direction: Direction,
    pub chip: &'static str,
    pub line: u16,
    pub name: Option<&'static str>,
    pub enable: bool,
    pub trans: Option<&'static Translator>,
}

impl GpioConfig {
    pub fn to_data_point(&self, value: u8) -> DataPoint {
        DataPoint {
            id: self.id,
            key: self.key,
            name: self.name.unwrap_or_default(),
            value: Val::U8(value),
            translator: self.trans,
            warn_bits: None,
            status_word: None,
        }
    }
}

pub(crate) fn build_configs(path: String) -> Result<GpioConfigs, GpioConfigsError> {
    let mut workbook: Xlsx<_> = open_workbook(path)?;
    let mut configs = Vec::new();
    let parse = |range: Range<Data>, configs: &mut Vec<GpioConfig>| {
        for row in range.rows() {
            let config = GpioConfig::build(row);
            match config {
                Ok(config) => {
                    configs.push(config);
                }
                Err(err) => {
                    tracing::error!("构建Linux GPIO配置失败: {}", err);
                }
            }
        }
    };
    if let Ok(range) = workbook
        .with_header_row(HeaderRow::Row(1))
        .worksheet_range("gpio")
    {
        parse(range, &mut configs);
    }
    Ok(configs)
}

impl GpioConfig {
    fn build(row: &[Data]) -> Result<Self, anyhow::Error> {
        let id = required_f64(row, 0, "id")? as u32;
        let key = required_static_str(row, 1, "KEY")?;
        let gpio = required_f64(row, 2, "GPIO")? as u16;
        let direction: Direction = required_static_str(row, 3, "DIRECTION")?.try_into()?;
        let chip = required_static_str(row, 4, "CHIP")?;
        let line = required_f64(row, 5, "LINE")? as u16;
        let name = optional_static_str(row, 6);
        let enable = required_f64(row, 7, "ENABLE")? != 0.0;
        let trans = row[8]
            .get_string()
            .and_then(|str| Translator::try_from(str).ok());
        let trans: Option<&'static Translator> = match trans {
            Some(t) => Some(Box::leak(Box::new(t))),
            None => None,
        };
        Ok(Self {
            id,
            key,
            gpio,
            direction,
            chip,
            line,
            name,
            enable,
            trans,
        })
    }
}
