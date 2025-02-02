//! Errors returned by OpenSSL library.
//!
//! OpenSSL errors are stored in an `ErrorStack`.  Most methods in the crate
//! returns a `Result<T, ErrorStack>` type.
//!
//! # Examples
//!
//! ```
//! use openssl::error::ErrorStack;
//! use openssl::bn::BigNum;
//!
//! let an_error = BigNum::from_dec_str("Cannot parse letters");
//! match an_error {
//!     Ok(_)  => (),
//!     Err(e) => println!("Parsing Error: {:?}", e),
//! }
//! ```
use cfg_if::cfg_if;
use libc::{c_char, c_int, c_uint, c_ulong};
use std::borrow::Cow;
use std::convert::TryInto;
use std::error;
use std::ffi::CStr;
use std::fmt;
use std::io;
use std::ptr;
use std::str;

#[cfg(not(boringssl))]
type ErrType = c_ulong;
#[cfg(boringssl)]
type ErrType = c_uint;

/// Collection of [`Error`]s from OpenSSL.
///
/// [`Error`]: struct.Error.html
#[derive(Debug, Clone)]
pub struct ErrorStack(Vec<Error>);

impl ErrorStack {
    /// Returns the contents of the OpenSSL error stack.
    pub fn get() -> ErrorStack {
        let mut vec = vec![];
        while let Some(err) = Error::get() {
            vec.push(err);
        }
        ErrorStack(vec)
    }

    /// Pushes the errors back onto the OpenSSL error stack.
    pub fn put(&self) {
        for error in self.errors() {
            error.put();
        }
    }
}

impl ErrorStack {
    /// Returns the errors in the stack.
    pub fn errors(&self) -> &[Error] {
        &self.0
    }
}

impl fmt::Display for ErrorStack {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return fmt.write_str("OpenSSL error");
        }

        let mut first = true;
        for err in &self.0 {
            if !first {
                fmt.write_str(", ")?;
            }
            write!(fmt, "{}", err)?;
            first = false;
        }
        Ok(())
    }
}

impl error::Error for ErrorStack {}

impl From<ErrorStack> for io::Error {
    fn from(e: ErrorStack) -> io::Error {
        io::Error::new(io::ErrorKind::Other, e)
    }
}

impl From<ErrorStack> for fmt::Error {
    fn from(_: ErrorStack) -> fmt::Error {
        fmt::Error
    }
}

/// An error reported from OpenSSL.
#[derive(Clone)]
pub struct Error {
    code: ErrType,
    file: *const c_char,
    line: c_int,
    func: *const c_char,
    data: Option<Cow<'static, str>>,
}

unsafe impl Sync for Error {}
unsafe impl Send for Error {}

impl Error {
    /// Returns the first error on the OpenSSL error stack.
    pub fn get() -> Option<Error> {
        unsafe {
            ffi::init();

            let mut file = ptr::null();
            let mut line = 0;
            let mut func = ptr::null();
            let mut data = ptr::null();
            let mut flags = 0;
            match ERR_get_error_all(&mut file, &mut line, &mut func, &mut data, &mut flags) {
                0 => None,
                code => {
                    // The memory referenced by data is only valid until that slot is overwritten
                    // in the error stack, so we'll need to copy it off if it's dynamic
                    let data = if flags & ffi::ERR_TXT_STRING != 0 {
                        let bytes = CStr::from_ptr(data as *const _).to_bytes();
                        let data = str::from_utf8(bytes).unwrap();
                        #[cfg(not(boringssl))]
                        let data = if flags & ffi::ERR_TXT_MALLOCED != 0 {
                            Cow::Owned(data.to_string())
                        } else {
                            Cow::Borrowed(data)
                        };
                        #[cfg(boringssl)]
                        let data = Cow::Borrowed(data);
                        Some(data)
                    } else {
                        None
                    };
                    Some(Error {
                        code,
                        file,
                        line,
                        func,
                        data,
                    })
                }
            }
        }
    }

    /// Pushes the error back onto the OpenSSL error stack.
    pub fn put(&self) {
        self.put_error();

        unsafe {
            let data = match self.data {
                Some(Cow::Borrowed(data)) => Some((data.as_ptr() as *mut c_char, 0)),
                Some(Cow::Owned(ref data)) => {
                    let ptr = ffi::CRYPTO_malloc(
                        (data.len() + 1) as _,
                        concat!(file!(), "\0").as_ptr() as _,
                        line!() as _,
                    ) as *mut c_char;
                    if ptr.is_null() {
                        None
                    } else {
                        ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
                        *ptr.add(data.len()) = 0;
                        Some((ptr, ffi::ERR_TXT_MALLOCED))
                    }
                }
                None => None,
            };
            if let Some((ptr, flags)) = data {
                ffi::ERR_set_error_data(ptr, flags | ffi::ERR_TXT_STRING);
            }
        }
    }

    #[cfg(ossl300)]
    fn put_error(&self) {
        unsafe {
            ffi::ERR_new();
            ffi::ERR_set_debug(self.file, self.line, self.func);
            ffi::ERR_set_error(
                ffi::ERR_GET_LIB(self.code),
                ffi::ERR_GET_REASON(self.code),
                ptr::null(),
            );
        }
    }

    #[cfg(not(ossl300))]
    fn put_error(&self) {
        unsafe {
            ffi::ERR_put_error(
                ffi::ERR_GET_LIB(self.code),
                ffi::ERR_GET_FUNC(self.code),
                ffi::ERR_GET_REASON(self.code),
                self.file,
                self.line.try_into().unwrap(),
            );
        }
    }

    /// Returns the raw OpenSSL error code for this error.
    pub fn code(&self) -> ErrType {
        self.code
    }

    /// Returns the name of the library reporting the error, if available.
    pub fn library(&self) -> Option<&'static str> {
        unsafe {
            let cstr = ffi::ERR_lib_error_string(self.code);
            if cstr.is_null() {
                return None;
            }
            let bytes = CStr::from_ptr(cstr as *const _).to_bytes();
            Some(str::from_utf8(bytes).unwrap())
        }
    }

    /// Returns the name of the function reporting the error.
    pub fn function(&self) -> Option<&'static str> {
        unsafe {
            if self.func.is_null() {
                return None;
            }
            let bytes = CStr::from_ptr(self.func).to_bytes();
            Some(str::from_utf8(bytes).unwrap())
        }
    }

    /// Returns the reason for the error.
    pub fn reason(&self) -> Option<&'static str> {
        unsafe {
            let cstr = ffi::ERR_reason_error_string(self.code);
            if cstr.is_null() {
                return None;
            }
            let bytes = CStr::from_ptr(cstr as *const _).to_bytes();
            Some(str::from_utf8(bytes).unwrap())
        }
    }

    /// Returns the name of the source file which encountered the error.
    pub fn file(&self) -> &'static str {
        unsafe {
            assert!(!self.file.is_null());
            let bytes = CStr::from_ptr(self.file as *const _).to_bytes();
            str::from_utf8(bytes).unwrap()
        }
    }

    /// Returns the line in the source file which encountered the error.
    pub fn line(&self) -> u32 {
        self.line as u32
    }

    /// Returns additional data describing the error.
    #[allow(clippy::option_as_ref_deref)]
    pub fn data(&self) -> Option<&str> {
        self.data.as_ref().map(|s| &**s)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut builder = fmt.debug_struct("Error");
        builder.field("code", &self.code());
        if let Some(library) = self.library() {
            builder.field("library", &library);
        }
        if let Some(function) = self.function() {
            builder.field("function", &function);
        }
        if let Some(reason) = self.reason() {
            builder.field("reason", &reason);
        }
        builder.field("file", &self.file());
        builder.field("line", &self.line());
        if let Some(data) = self.data() {
            builder.field("data", &data);
        }
        builder.finish()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "error:{:08X}", self.code())?;
        match self.library() {
            Some(l) => write!(fmt, ":{}", l)?,
            None => write!(fmt, ":lib({})", ffi::ERR_GET_LIB(self.code()))?,
        }
        match self.function() {
            Some(f) => write!(fmt, ":{}", f)?,
            None => write!(fmt, ":func({})", ffi::ERR_GET_FUNC(self.code()))?,
        }
        match self.reason() {
            Some(r) => write!(fmt, ":{}", r)?,
            None => write!(fmt, ":reason({})", ffi::ERR_GET_REASON(self.code()))?,
        }
        write!(
            fmt,
            ":{}:{}:{}",
            self.file(),
            self.line(),
            self.data().unwrap_or("")
        )
    }
}

impl error::Error for Error {}

cfg_if! {
    if #[cfg(ossl300)] {
        use ffi::ERR_get_error_all;
    } else {
        #[allow(bad_style)]
        unsafe extern "C" fn ERR_get_error_all(
            file: *mut *const c_char,
            line: *mut c_int,
            func: *mut *const c_char,
            data: *mut *const c_char,
            flags: *mut c_int,
        ) -> ErrType {
            let code = ffi::ERR_get_error_line_data(file, line, data, flags);
            *func = ffi::ERR_func_error_string(code);
            code
        }
    }
}
