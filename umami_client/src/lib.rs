// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

//! Native Python bindings for UMAMI's command socket (`Client`) and
//! shared-memory histogram (`Shm`).

use std::ffi::{c_int, c_void, CString};
use std::ptr;
use std::time::Duration;

use pyo3::create_exception;
use pyo3::exceptions::{PyBufferError, PyRuntimeError, PyValueError};
use pyo3::ffi;
use pyo3::prelude::*;
use pythonize::{depythonize, pythonize};

use umami::{ClientError, Command, CommandReply, ParamMap, ShmReader};

create_exception!(umami_client, UmamiClientError, pyo3::exceptions::PyException,
    "Base class for all umami_client errors.");
create_exception!(umami_client, UmamiError, UmamiClientError,
    "The running UMAMI instance rejected a command. `args` is \
     `(module, message)`, where `module` is the name of the module that \
     raised the error or `None` for an instance-wide error.");
create_exception!(umami_client, UmamiTimeout, UmamiClientError,
    "No reply was received within the client's configured timeout.");
create_exception!(umami_client, UmamiConnectionError, UmamiClientError,
    "The command socket could not be reached.");

fn client_error_to_py(e: ClientError) -> PyErr {
    match e {
        ClientError::Timeout =>
            UmamiTimeout::new_err("No reply received (timeout)"),
        ClientError::Connection(io_err) =>
            UmamiConnectionError::new_err(io_err.to_string()),
        ClientError::Other(err) =>
            UmamiConnectionError::new_err(format!("{err:#}")),
    }
}

/// A connection to a running UMAMI instance's command socket.
#[pyclass(module = "umami_client", subclass)]
struct Client {
    inner: umami::Client,
}

impl Client {
    /// Sends `cmd` and turns an `Error` reply into a raised `UmamiError`;
    /// callers only see `Ok`/`Data` replies.
    fn dispatch(&mut self, cmd: Command) -> PyResult<CommandReply> {
        let reply = self.inner.send(&cmd).map_err(client_error_to_py)?;
        if let CommandReply::Error { module, message } = reply {
            // these errors map to UmamiError
            return Err(UmamiError::new_err((module.map(|m| m.to_string()), message)));
        }
        Ok(reply)
    }

    fn dispatch_ok(&mut self, cmd: Command) -> PyResult<()> {
        self.dispatch(cmd).map(drop)
    }

    fn dispatch_data<'py>(&mut self, py: Python<'py>, cmd: Command)
                          -> PyResult<Bound<'py, PyAny>> {
        match self.dispatch(cmd)? {
            CommandReply::Data { value } => pythonize(py, &value)
                .map_err(|e| PyValueError::new_err(
                    format!("Converting reply to Python: {e}")
                )),
            _ => Ok(py.None().into_bound(py)),
        }
    }
}

#[pymethods]
impl Client {
    #[new]
    #[pyo3(signature = (ipc_name, timeout=2.0))]
    fn new(ipc_name: &str, timeout: f64) -> PyResult<Self> {
        let inner = umami::Client::with_timeout(ipc_name, Duration::from_secs_f64(timeout))
            .map_err(|e| UmamiConnectionError::new_err(format!("{e:#}")))?;
        Ok(Self { inner })
    }

    /// Whether the last command succeeded in reaching the instance -- does
    /// not itself probe the socket; the next `send`-based call reconnects
    /// automatically if this is `False`.
    #[getter]
    fn connected(&self) -> bool {
        self.inner.connected()
    }

    /// Rebinds a fresh local socket address and reconnects immediately,
    /// rather than waiting for the next call to notice `connected` is false.
    fn reconnect(&mut self) -> PyResult<()> {
        self.inner.reconnect().map_err(|e| UmamiConnectionError::new_err(format!("{e:#}")))
    }

    fn ping(&mut self) -> PyResult<String> {
        match self.dispatch(Command::Ping)? {
            CommandReply::Data { value } => Ok(
                value.as_str().unwrap_or_default().to_string()
            ),
            _ => Ok(String::new()),
        }
    }

    fn clear(&mut self) -> PyResult<()> {
        self.dispatch_ok(Command::Clear)
    }

    fn start(&mut self, run_id: String) -> PyResult<()> {
        self.dispatch_ok(Command::Start { run_id })
    }

    fn stop(&mut self) -> PyResult<()> {
        self.dispatch_ok(Command::Stop)
    }

    fn reset(&mut self) -> PyResult<()> {
        self.dispatch_ok(Command::Reset)
    }

    fn get_state<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.dispatch_data(py, Command::GetState)
    }

    fn set_raw_dump(&mut self, enable: bool, path: String) -> PyResult<()> {
        self.dispatch_ok(Command::SetRawDump { enable, path })
    }

    fn get_modes<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.dispatch_data(py, Command::GetModes)
    }

    fn set_mode(&mut self, name: String) -> PyResult<()> {
        self.dispatch_ok(Command::SetMode { name })
    }

    #[pyo3(signature = (full=false))]
    fn get_params<'py>(&mut self, py: Python<'py>, full: bool)
                       -> PyResult<Bound<'py, PyAny>> {
        self.dispatch_data(py, Command::GetParams { full })
    }

    fn set_params(&mut self, params: Bound<'_, PyAny>) -> PyResult<()> {
        let params: ParamMap = depythonize(&params)
            .map_err(|e| PyValueError::new_err(format!("Invalid params: {e}")))?;
        self.dispatch_ok(Command::SetParams { params })
    }

    fn save_histo(&mut self, path: String, max_nt: usize) -> PyResult<()> {
        self.dispatch_ok(Command::SaveHisto { path, max_nt })
    }

    #[pyo3(signature = (path=None))]
    fn save_config(&mut self, path: Option<String>) -> PyResult<()> {
        self.dispatch_ok(Command::SaveConfig { path })
    }

    /// Sends a caller-built JSON command verbatim and returns the raw JSON
    /// reply, bypassing all typed conversion -- for callers that want to speak
    /// the wire protocol directly.
    fn send_json(&mut self, json: &str) -> PyResult<String> {
        let cmd: Command = serde_json::from_str(json)
            .map_err(|e| PyValueError::new_err(format!("Invalid command JSON: {e}")))?;
        let reply = self.dispatch(cmd)?;
        serde_json::to_string(&reply)
            .map_err(|e| PyValueError::new_err(format!("Serializing reply: {e}")))
    }
}

/// Read-only view of a UMAMI shared-memory histogram segment.
///
/// Exports the entire mapped segment (header followed by histogram bins) via
/// the buffer protocol, so `np.frombuffer(shm, dtype, count, offset)` works
/// with the byte offsets documented in `shm.rs`'s `ShmInterface`. The mapping
/// unmaps once the last reference -- including any numpy view still holding
/// a buffer export -- goes away; there is no explicit `close()`.
#[pyclass(module = "umami_client", subclass)]
struct Shm {
    reader: ShmReader,
}

#[pymethods]
impl Shm {
    #[new]
    fn new(name: &str) -> PyResult<Self> {
        let reader = ShmReader::open(name)
            .map_err(|e| PyRuntimeError::new_err(format!("{e:#}")))?;
        Ok(Self { reader })
    }

    #[getter] fn nx(&self) -> u16 { self.reader.nx() }
    #[getter] fn ny(&self) -> u16 { self.reader.ny() }
    #[getter] fn nt(&self) -> u16 { self.reader.nt() }
    #[getter] fn ni(&self) -> u16 { self.reader.ni() }
    #[getter] fn run_id(&self) -> String { self.reader.run_id() }
    #[getter] fn run_start(&self) -> u32 { self.reader.run_start() }
    #[getter] fn running(&self) -> bool { self.reader.running() }
    #[getter] fn total_events(&self) -> u64 { self.reader.total_events() }
    #[getter] fn total_neutrons(&self) -> u64 { self.reader.total_neutrons() }
    #[getter] fn lifetime_ns(&self) -> i64 { self.reader.lifetime_ns() }
    #[getter] fn tzero_count(&self) -> u64 { self.reader.tzero_count() }
    #[getter] fn monitor_counts(&self) -> Vec<u64> {
        self.reader.monitor_counts().to_vec()
    }

    /// # Safety
    ///
    /// Standard PyO3 buffer-protocol slot; see `__releasebuffer__` and the
    /// module doc above for the lifetime argument (view.obj keeps `slf`,
    /// and therefore the mapping, alive for as long as the buffer exists).
    unsafe fn __getbuffer__(
        slf: Bound<'_, Self>,
        view: *mut ffi::Py_buffer,
        flags: c_int,
    ) -> PyResult<()> {
        if view.is_null() {
            return Err(PyBufferError::new_err("Py_buffer view is null"));
        }
        if (flags & ffi::PyBUF_WRITABLE) == ffi::PyBUF_WRITABLE {
            return Err(PyBufferError::new_err("umami_client.Shm is read-only"));
        }

        let (ptr, len) = {
            let borrowed = slf.borrow();
            let bytes = borrowed.reader.as_bytes();
            (bytes.as_ptr(), bytes.len())
        };

        // TODO: change to match the actual array properties (u32 values, 3 dimensions etc)
        unsafe {
            (*view).obj = slf.into_any().into_ptr();
            (*view).buf = ptr as *mut c_void;
            (*view).len = len as isize;
            (*view).readonly = 1;
            (*view).itemsize = 1;
            (*view).format = if (flags & ffi::PyBUF_FORMAT) == ffi::PyBUF_FORMAT {
                // TODO: this can just as well be a static constant, no?
                CString::new("B").expect("no interior NUL").into_raw()
            } else {
                ptr::null_mut()
            };
            (*view).ndim = 1;
            (*view).shape = if (flags & ffi::PyBUF_ND) == ffi::PyBUF_ND {
                &mut (*view).len
            } else {
                ptr::null_mut()
            };
            (*view).strides = if (flags & ffi::PyBUF_STRIDES) == ffi::PyBUF_STRIDES {
                &mut (*view).itemsize
            } else {
                ptr::null_mut()
            };
            (*view).suboffsets = ptr::null_mut();
            (*view).internal = ptr::null_mut();
        }
        Ok(())
    }

    /// # Safety
    ///
    /// Standard PyO3 buffer-protocol slot, only ever called by the Python
    /// runtime with a view this class itself filled in.
    unsafe fn __releasebuffer__(&self, view: *mut ffi::Py_buffer) {
        unsafe {
            if !(*view).format.is_null() {
                drop(CString::from_raw((*view).format));
            }
        }
    }
}

#[pymodule]
fn umami_client(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Client>()?;
    m.add_class::<Shm>()?;
    m.add("UmamiClientError", m.py().get_type::<UmamiClientError>())?;
    m.add("UmamiError", m.py().get_type::<UmamiError>())?;
    m.add("UmamiTimeout", m.py().get_type::<UmamiTimeout>())?;
    m.add("UmamiConnectionError", m.py().get_type::<UmamiConnectionError>())?;
    m.add("SHM_HEADER_SIZE", umami::SHM_HEADER_SIZE)?;
    Ok(())
}
