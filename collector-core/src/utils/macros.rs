#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        $crate::utils::log::OP_LOGGER.push(
            $crate::utils::log::OpLogLevel::Info,
            format!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        $crate::utils::log::OP_LOGGER.push(
            $crate::utils::log::OpLogLevel::Warn,
            format!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        $crate::utils::log::OP_LOGGER.push(
            $crate::utils::log::OpLogLevel::Error,
            format!($($arg)*),
        )
    };
}
