// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

//! Native Python bindings for UMAMI's command socket (`Client`) and
//! shared-memory histogram (`Shm`).

use std::ffi::{c_int, c_void};
use std::ptr;
use std::time::Duration;

use numpy::{Element, PyArray1, PyArrayDescr};
use pyo3::create_exception;
use pyo3::exceptions::{PyBufferError, PyRuntimeError, PyValueError};
use pyo3::ffi;
use pyo3::prelude::*;
use pythonize::{depythonize, pythonize};

use umami::{
    ClientError, Command, CommandReply, Event, EventType, HistoConfig, ParamMap, ShmReader,
};

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
/// Exports the histogram bins only via the buffer protocol (header fields
/// all have their own typed property above). The mapping unmaps once the
/// last reference -- including any numpy view -- goes away; no `close()`.
#[pyclass(module = "umami_client", subclass)]
struct Shm {
    reader: ShmReader,
    // needed persistently for the buffer API
    shape: [ffi::Py_ssize_t; 3],
    strides: [ffi::Py_ssize_t; 3],
}

#[pymethods]
impl Shm {
    #[new]
    fn new(name: &str) -> PyResult<Self> {
        let reader = ShmReader::open(name)
            .map_err(|e| PyRuntimeError::new_err(format!("{e:#}")))?;
        let (nx, ny, nt) = (reader.nx() as isize, reader.ny() as isize, reader.nt() as isize);
        let itemsize = size_of::<u32>() as isize;
        // C-contiguous (nt, ny, nx), matching the bin index math in shm.rs's
        // ShmBox::add_histo: off = t*nx*ny + y*nx + x.
        Ok(Self {
            reader,
            shape: [nt, ny, nx],
            strides: [ny * nx * itemsize, nx * itemsize, itemsize],
        })
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
    /// Standard PyO3 buffer-protocol slot. `view.obj` keeps `slf` -- and
    /// therefore the mapping and the `shape`/`strides` arrays pointed into
    /// below -- alive for as long as the buffer exists; nothing else needs
    /// releasing, so there is no `__releasebuffer__`.
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

        let (ptr, len, shape, strides) = {
            let borrowed = slf.borrow();
            let data = borrowed.reader.histo_data();
            (data.as_ptr().cast::<u8>(), std::mem::size_of_val(data),
             borrowed.shape.as_ptr().cast_mut(), borrowed.strides.as_ptr().cast_mut())
        };

        unsafe {
            (*view).obj = slf.into_any().into_ptr();
            (*view).buf = ptr as *mut c_void;
            (*view).len = len as isize;
            (*view).readonly = 1;
            (*view).itemsize = size_of::<u32>() as isize;
            (*view).format = if (flags & ffi::PyBUF_FORMAT) == ffi::PyBUF_FORMAT {
                // little-endian unsigned int, standard (4-byte) size
                c"<I".as_ptr().cast_mut()
            } else {
                ptr::null_mut()
            };
            (*view).ndim = 3;
            (*view).shape = if (flags & ffi::PyBUF_ND) == ffi::PyBUF_ND {
                shape
            } else {
                ptr::null_mut()
            };
            (*view).strides = if (flags & ffi::PyBUF_STRIDES) == ffi::PyBUF_STRIDES {
                strides
            } else {
                ptr::null_mut()
            };
            (*view).suboffsets = ptr::null_mut();
            (*view).internal = ptr::null_mut();
        }
        Ok(())
    }
}

/// `EventType`'s discriminant plus its one possible payload byte
/// (`Monitor`/`AuxSignal`'s `num`, `Edge`/`Gate`'s `up`).
fn evtype_tag_arg(evtype: EventType) -> (u8, u8) {
    match evtype {
        EventType::Neutron => (0x01, 0),
        EventType::Monitor { num } => (0x02, num),
        EventType::Edge { up } => (0x10, up as u8),
        EventType::Gate { up } => (0x11, up as u8),
        EventType::Tzero => (0x12, 0),
        EventType::AuxSignal { num } => (0x13, num),
        EventType::Heartbeat => (0x80, 0),
        EventType::Void => (0xff, 0),
    }
}

/// Decodes one archived `Vec<Event>` batch (as produced by the `ext_process`
/// output) into a `Vec` of records, deserializing each `Event`.
fn decode_batch(buf: &[u8]) -> PyResult<Vec<Event>> {
    let archived = rkyv::access::<rkyv::Archived<Vec<Event>>, rkyv::rancor::Error>(buf)
        .map_err(|e| PyValueError::new_err(format!("Corrupt event batch: {e}")))?;
    archived
        .iter()
        .map(|ev| {
            rkyv::deserialize::<Event, rkyv::rancor::Error>(ev)
                .map_err(|e| PyValueError::new_err(format!("Corrupt event: {e}")))
        })
        .collect()
}

/// All of an `Event`'s fields, as a Numpy Element ready structure.
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct EventRecord {
    time_ns: i64,
    rel_time_ns: i64,
    channel: u32,
    ampl: u32,
    x: u16,
    y: u16,
    t: u16,
    i: u16,
    flags: u16,
    evtype: u8,
    evtype_arg: u8,
}

impl From<Event> for EventRecord {
    fn from(ev: Event) -> Self {
        let (evtype, evtype_arg) = evtype_tag_arg(ev.evtype);
        Self {
            time_ns: ev.time.as_nanos(),
            rel_time_ns: ev.rel_time.as_nanos(),
            channel: ev.channel.0,
            ampl: ev.ampl.0,
            x: ev.histo.x,
            y: ev.histo.y,
            t: ev.histo.t,
            i: ev.histo.i,
            flags: ev.flags.bits(),
            evtype,
            evtype_arg,
        }
    }
}

unsafe impl Element for EventRecord {
    const IS_COPY: bool = true;

    fn get_dtype(py: Python<'_>) -> Bound<'_, PyArrayDescr> {
        PyArrayDescr::new(py, [
            ("time_ns", "<i8"), ("rel_time_ns", "<i8"),
            ("channel", "<u4"), ("ampl", "<u4"),
            ("x", "<u2"), ("y", "<u2"), ("t", "<u2"), ("i", "<u2"),
            ("flags", "<u2"), ("evtype", "u1"), ("evtype_arg", "u1"),
        ]).expect("EventRecord's dtype spec is well-formed")
    }

    fn clone_ref(&self, _py: Python<'_>) -> Self {
        *self
    }
}

/// The subset of `EventRecord` a live view most commonly needs -- see
/// `decode_events_xy`.
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct EventXY {
    rel_time_ns: i64,
    x: u16,
    y: u16,
}

impl From<Event> for EventXY {
    fn from(ev: Event) -> Self {
        Self { rel_time_ns: ev.rel_time.as_nanos(), x: ev.histo.x, y: ev.histo.y }
    }
}

unsafe impl Element for EventXY {
    const IS_COPY: bool = true;

    fn get_dtype(py: Python<'_>) -> Bound<'_, PyArrayDescr> {
        PyArrayDescr::new(py, [("rel_time_ns", "<i8"), ("x", "<u2"), ("y", "<u2")])
            .expect("EventXY's dtype spec is well-formed")
    }

    fn clone_ref(&self, _py: Python<'_>) -> Self {
        *self
    }
}

/// Decodes one archived event batch (as sent by the `ext_process` output)
/// into a numpy structured array with every field: `time_ns`, `rel_time_ns`,
/// `channel`, `ampl`, `x`, `y`, `t`, `i`, `flags`, `evtype`, `evtype_arg`.
#[pyfunction]
fn decode_events<'py>(py: Python<'py>, buf: &[u8]) -> PyResult<Bound<'py, PyArray1<EventRecord>>> {
    let records: Vec<EventRecord> = decode_batch(buf)?.into_iter().map(EventRecord::from).collect();
    Ok(PyArray1::from_vec(py, records))
}

/// Decodes one archived event batch into a numpy structured array with just
/// `rel_time_ns`, `x`, `y`.
#[pyfunction]
fn decode_events_xy<'py>(py: Python<'py>, buf: &[u8]) -> PyResult<Bound<'py, PyArray1<EventXY>>> {
    let records: Vec<EventXY> = decode_batch(buf)?.into_iter().map(EventXY::from).collect();
    Ok(PyArray1::from_vec(py, records))
}

/// A writable UMAMI histogram shared-memory segment, for publishing an
/// external process's own output.
#[pyclass(module = "umami_client", subclass)]
struct ShmWriter {
    inner: umami::ShmWriter,
    shape: [ffi::Py_ssize_t; 3],
    strides: [ffi::Py_ssize_t; 3],
}

#[pymethods]
impl ShmWriter {
    #[new]
    fn new(name: &str, nx: u16, ny: u16, nt: u16) -> PyResult<Self> {
        let config = HistoConfig { nx: nx as usize, ny: ny as usize, max_nt: nt as usize,
                                   max_ni: 0 };
        let mut inner = umami::ShmWriter::create(name, &config)
            .map_err(|e| PyRuntimeError::new_err(format!("{e:#}")))?;
        inner.set_initialized();
        let itemsize = size_of::<u32>() as isize;
        // matches Shm::new's (nt, ny, nx) shape/strides convention
        Ok(Self {
            inner,
            shape: [nt as isize, ny as isize, nx as isize],
            strides: [ny as isize * nx as isize * itemsize, nx as isize * itemsize, itemsize],
        })
    }

    fn set_run_id(&mut self, run_id: &str) {
        self.inner.set_run_id(run_id);
    }

    fn set_running(&mut self, running: bool) {
        self.inner.set_running(running);
    }

    fn clear_histo(&mut self) {
        self.inner.clear_histo();
    }

    /// # Safety
    ///
    /// Same lifetime argument as `Shm::__getbuffer__`: `view.obj` keeps
    /// `slf` -- and therefore the mapping -- alive for as long as the
    /// buffer exists.
    unsafe fn __getbuffer__(
        slf: Bound<'_, Self>,
        view: *mut ffi::Py_buffer,
        flags: c_int,
    ) -> PyResult<()> {
        if view.is_null() {
            return Err(PyBufferError::new_err("Py_buffer view is null"));
        }

        let (ptr, len, shape, strides) = {
            let mut borrowed = slf.borrow_mut();
            let data = borrowed.inner.histo_data_mut();
            let len = std::mem::size_of_val(data);
            (data.as_mut_ptr().cast::<u8>(), len,
             borrowed.shape.as_ptr().cast_mut(), borrowed.strides.as_ptr().cast_mut())
        };

        unsafe {
            (*view).obj = slf.into_any().into_ptr();
            (*view).buf = ptr as *mut c_void;
            (*view).len = len as isize;
            (*view).readonly = 0;
            (*view).itemsize = size_of::<u32>() as isize;
            (*view).format = if (flags & ffi::PyBUF_FORMAT) == ffi::PyBUF_FORMAT {
                c"<I".as_ptr().cast_mut()
            } else {
                ptr::null_mut()
            };
            (*view).ndim = 3;
            (*view).shape = if (flags & ffi::PyBUF_ND) == ffi::PyBUF_ND {
                shape
            } else {
                ptr::null_mut()
            };
            (*view).strides = if (flags & ffi::PyBUF_STRIDES) == ffi::PyBUF_STRIDES {
                strides
            } else {
                ptr::null_mut()
            };
            (*view).suboffsets = ptr::null_mut();
            (*view).internal = ptr::null_mut();
        }
        Ok(())
    }
}

#[pymodule]
fn umami_client(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Client>()?;
    m.add_class::<Shm>()?;
    m.add_class::<ShmWriter>()?;
    m.add_function(wrap_pyfunction!(decode_events, m)?)?;
    m.add_function(wrap_pyfunction!(decode_events_xy, m)?)?;
    m.add("UmamiClientError", m.py().get_type::<UmamiClientError>())?;
    m.add("UmamiError", m.py().get_type::<UmamiError>())?;
    m.add("UmamiTimeout", m.py().get_type::<UmamiTimeout>())?;
    m.add("UmamiConnectionError", m.py().get_type::<UmamiConnectionError>())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use numpy::PyArrayMethods;
    use umami::{EventTime, ShmReader};
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn make_event(sec: u32, nsec: u32, rel_nsec: u32, channel: u32, ampl: u32,
                  x: u16, y: u16, t: u16, evtype: EventType) -> Event {
        let mut ev = Event::new(evtype)
            .with_channel(channel)
            .with_abs_time(EventTime::from_sec_nsec(sec, nsec))
            .with_rel_time(EventTime::from_sec_nsec(0, rel_nsec))
            .with_ampl(ampl);
        ev.histo.x = x;
        ev.histo.y = y;
        ev.histo.t = t;
        ev
    }

    #[test]
    fn test_decode_events_roundtrips_all_fields() {
        let events = vec![
            make_event(1, 500_000_000, 250_000_000, 5, 1234, 10, 20, 3, EventType::Neutron),
            make_event(2, 0, 0, 7, 0, 0, 0, 0, EventType::Monitor { num: 3 }),
        ];
        let bytes = rkyv::to_bytes::<rkyv::rancor::Failure>(&events).unwrap();

        Python::attach(|py| {
            let array = decode_events(py, &bytes).unwrap();
            let records = array.to_vec().unwrap();
            assert_eq!(records.len(), 2);

            assert_eq!({ records[0].time_ns }, 1_500_000_000);
            assert_eq!({ records[0].rel_time_ns }, 250_000_000);
            assert_eq!({ records[0].channel }, 5);
            assert_eq!({ records[0].ampl }, 1234);
            assert_eq!({ records[0].x }, 10);
            assert_eq!({ records[0].y }, 20);
            assert_eq!({ records[0].t }, 3);
            assert_eq!({ records[0].evtype }, 0x01);
            assert_eq!({ records[0].evtype_arg }, 0);

            assert_eq!({ records[1].channel }, 7);
            assert_eq!({ records[1].evtype }, 0x02);
            assert_eq!({ records[1].evtype_arg }, 3);
        });
    }

    #[test]
    fn test_decode_events_xy_has_only_rel_time_x_y() {
        let events = vec![
            make_event(1, 0, 750_000_000, 5, 1234, 42, 99, 3, EventType::Neutron),
        ];
        let bytes = rkyv::to_bytes::<rkyv::rancor::Failure>(&events).unwrap();

        Python::attach(|py| {
            let array = decode_events_xy(py, &bytes).unwrap();
            let records = array.to_vec().unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!({ records[0].rel_time_ns }, 750_000_000);
            assert_eq!({ records[0].x }, 42);
            assert_eq!({ records[0].y }, 99);
        });
    }

    #[test]
    fn test_decode_events_rejects_corrupt_bytes() {
        Python::attach(|py| {
            assert!(decode_events(py, b"not a valid archive").is_err());
        });
    }

    #[test]
    fn test_shm_writer_roundtrips_via_buffer_protocol_and_shm_reads_it() {
        let name = format!("umami_client_test_shmwriter_{}", std::process::id());
        Python::attach(|py| {
            let writer = Py::new(py, ShmWriter::new(&name, 4, 4, 2).unwrap()).unwrap();
            {
                let mut w = writer.borrow_mut(py);
                w.set_run_id("run_042");
                w.set_running(true);
            }
            let np = py.import("numpy").unwrap();
            let arr = np.call_method1("asarray", (writer.clone_ref(py),)).unwrap();
            // bin (t=1, y=2, x=3) -> flat offset matches shm.rs's own index math
            arr.call_method1("__setitem__", ((1, 2, 3), 7u32)).unwrap();

            let reader = ShmReader::open(&name).unwrap();
            assert_eq!(reader.run_id(), "run_042");
            assert!(reader.running());
            let off = 4 * 4 + 2 * 4 + 3; // t=1, y=2, x=3, nx=ny=4
            assert_eq!(reader.histo_data()[off], 7);
        });
        // ShmWriter has no unlink-on-drop guard (unlike the umami crate's
        // own #[cfg(test)] ShmGuard, which isn't visible across the crate
        // boundary), so clean up the segment by hand.
        nix::sys::mman::shm_unlink(name.as_bytes()).ok();
    }
}
