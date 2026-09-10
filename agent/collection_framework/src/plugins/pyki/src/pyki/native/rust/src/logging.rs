#[repr(C)]
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    DEBUG = 0,
    INFO = 1,
    WARNING = 2,
    ERROR = 3,
    OFF = 4,
}

unsafe extern "C" {
    pub fn get_pyki_native_log_level() -> LogLevel;

    pub fn pyki_log(
        level: LogLevel,
        filename: *const std::os::raw::c_char,
        line: u32,
        message: *const std::os::raw::c_char,
    ) -> std::os::raw::c_void;
}

macro_rules! log {
    ($level: expr, $fmt: literal $(, $arg: expr)*) => {
        unsafe {
            if ($level >= get_pyki_native_log_level()) {
                let str = format!($fmt $(, $arg)*);
                use std::ffi::CString;
                let c_file = CString::new(file!()).unwrap();
                let c_message = CString::new(str).unwrap();
                pyki_log($level, c_file.as_ptr(), line!(), c_message.as_ptr());
            }
        }
    };
}

#[macro_export]
macro_rules! DEBUG {
    ($fmt: literal $(, $arg: expr)*) => {
        log!(LogLevel::DEBUG, $fmt $(, $arg)*);
    };
}
#[macro_export]
macro_rules! INFO{
    ($fmt: literal $(, $arg: expr)*) => {
        log!(LogLevel::INFO, $fmt $(, $arg)*);
    };
}

#[macro_export]
macro_rules! WARNING{
    ($fmt: literal $(, $arg: expr)*) => {
        log!(LogLevel::WARNING, $fmt $(, $arg)*);
    };
}

#[macro_export]
macro_rules! ERROR {
    ($fmt: literal $(, $arg: expr)*) => {
        log!(LogLevel::ERROR, $fmt $(, $arg)*);
    };
}
