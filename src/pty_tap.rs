//! Transparent non-blocking PTY relay (`OSC 52` write-only + `OSC 133` timing).

use std::ffi::{c_char, c_int, c_short, c_ulong, c_void};
use std::io;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use crate::ffi::*;
use crate::i18n::UiText;
use crate::osc_parser::{OscCommandTimer, OscEvent, OscStreamParser};
use crate::telemetry::format_turn_duration;

const RELAY_BUF_CAP: usize = 32 * 1024;
const PENDING_HIGH_WATER: usize = 64 * 1024;
const DEFAULT_NOTIFY_SECS: u64 = 15;
const TIOCSCTTY: c_ulong = 0x540E;
const TIOCSWINSZ: c_ulong = 0x5414;
const F_SETFD: c_int = 2;
const F_GETFL: c_int = 3;
const F_SETFL: c_int = 4;
const FD_CLOEXEC: c_int = 1;
const O_NONBLOCK: c_int = 0o4000;
const TCSANOW: c_int = 0;
const IUTF8: u32 = 0o040000;
const POLLIN: c_short = 0x001;
const POLLOUT: c_short = 0x004;
const POLLERR: c_short = 0x008;
const POLLHUP: c_short = 0x010;
const G_CONNECT_AFTER: c_int = 1;

type VtePty = c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct Winsize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PollFd {
    fd: c_int,
    events: c_short,
    revents: c_short,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Termios {
    c_iflag: u32,
    c_oflag: u32,
    c_cflag: u32,
    c_lflag: u32,
    c_line: u8,
    c_cc: [u8; 32],
    c_ispeed: u32,
    c_ospeed: u32,
}

extern "C" {
    fn openpty(
        amaster: *mut c_int,
        aslave: *mut c_int,
        name: *mut c_char,
        termp: *const Termios,
        winp: *const Winsize,
    ) -> c_int;
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    fn poll(fds: *mut PollFd, nfds: c_ulong, timeout: c_int) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
    fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    fn close(fd: c_int) -> c_int;
    fn dup2(oldfd: c_int, newfd: c_int) -> c_int;
    fn setsid() -> i32;
    fn tcgetattr(fd: c_int, termios_p: *mut Termios) -> c_int;
    fn tcsetattr(fd: c_int, optional_actions: c_int, termios_p: *const Termios) -> c_int;
    fn cfmakeraw(termios_p: *mut Termios);
    fn vte_pty_new_foreign_sync(fd: c_int, cancellable: *mut c_void, error: *mut *mut c_void) -> *mut VtePty;
    fn vte_pty_set_utf8(pty: *mut VtePty, utf8: c_int) -> c_int;
    fn vte_terminal_set_pty(terminal: *mut VteTerminal, pty: *mut VtePty);
    fn vte_terminal_watch_child(terminal: *mut VteTerminal, child_pid: i32);
}

const OSC52_COPIED_PREFIX: UiText = UiText::new(
    c"Скопировано по OSC 52 · ",
    c"Copied via OSC 52 · ",
);
const OSC52_BYTES_UNIT: UiText = UiText::new(c"байт", c"bytes");
const OSC133_NOTIFY_TITLE: UiText = UiText::new(
    c"AkhTerm · Команда завершена",
    c"AkhTerm · Command finished",
);

static OSC52_ENABLED: AtomicBool = AtomicBool::new(true);
static NOTIFY_THRESHOLD_SECS: AtomicU64 = AtomicU64::new(DEFAULT_NOTIFY_SECS);
static FOOTER_HOOK: Mutex<Option<fn(&str)>> = Mutex::new(None);
static OSC133_HOOK: Mutex<Option<fn(Osc133Completion)>> = Mutex::new(None);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Osc133Completion {
    pub vte_addr: usize,
    pub exit_code: Option<i32>,
    pub elapsed_secs: u64,
    pub status_text: String,
}

#[derive(Clone, Default)]
pub struct PtyTapSink {
    pub on_osc52_toast: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    pub on_osc133_done: Option<Arc<dyn Fn(Osc133Completion) + Send + Sync>>,
    pub skip_gtk_clipboard: bool,
}

pub fn set_osc52_enabled(enabled: bool) {
    OSC52_ENABLED.store(enabled, Ordering::Release);
}

pub fn is_osc52_enabled() -> bool {
    OSC52_ENABLED.load(Ordering::Acquire)
}

pub fn set_notify_threshold_secs(secs: u64) {
    NOTIFY_THRESHOLD_SECS.store(secs, Ordering::Release);
}

pub fn notify_threshold_secs() -> u64 {
    NOTIFY_THRESHOLD_SECS.load(Ordering::Acquire)
}

pub fn current_footer_hook() -> Option<fn(&str)> {
    FOOTER_HOOK.lock().ok().and_then(|guard| *guard)
}

pub fn current_osc133_hook() -> Option<fn(Osc133Completion)> {
    OSC133_HOOK.lock().ok().and_then(|guard| *guard)
}

pub fn format_osc52_toast(byte_len: usize) -> String {
    format!("{}{byte_len} {}", OSC52_COPIED_PREFIX.as_str(), OSC52_BYTES_UNIT.as_str())
}

pub fn format_osc133_status(exit_code: Option<i32>, elapsed_secs: u64) -> String {
    let dur = format_turn_duration(elapsed_secs);
    match exit_code {
        None | Some(0) => format!("✓ · {dur}"),
        Some(code) => format!("✗ {code} · {dur}"),
    }
}

pub fn osc133_notify_title() -> &'static str {
    OSC133_NOTIFY_TITLE.as_str()
}

pub fn format_osc133_notify_body(
    tab_title: &str,
    exit_code: Option<i32>,
    elapsed_secs: u64,
) -> String {
    let status = format_osc133_status(exit_code, elapsed_secs);
    if tab_title.trim().is_empty() {
        status
    } else {
        format!("{tab_title} · {status}")
    }
}

pub fn should_send_osc133_notify(
    elapsed_secs: u64,
    threshold_secs: u64,
    is_focused: bool,
) -> bool {
    threshold_secs > 0 && elapsed_secs >= threshold_secs && !is_focused
}

struct Osc52IdleJob {
    text: String,
    sink: PtyTapSink,
}

struct Osc133IdleJob {
    vte_addr: usize,
    exit_code: Option<i32>,
    elapsed_secs: u64,
    sink: PtyTapSink,
}

fn write_gtk_clipboard_text(text: &str) {
    if unsafe { gdk_display_get_default() }.is_null() {
        return;
    }
    let clean = text.replace('\0', "");
    if let Ok(c_len) = c_int::try_from(clean.len()) {
        unsafe {
            let clip = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
            if !clip.is_null() {
                gtk_clipboard_set_text(clip, clean.as_ptr() as *const c_char, c_len);
            }
        }
    }
}

fn emit_footer_toast(msg: &str, sink: &PtyTapSink) {
    if let Some(cb) = &sink.on_osc52_toast {
        cb(msg);
        return;
    }
    if let Ok(guard) = FOOTER_HOOK.lock() {
        if let Some(hook) = *guard {
            hook(msg);
        }
    }
}

extern "C" fn on_osc52_idle(data: *mut c_void) -> c_int {
    if !data.is_null() && is_osc52_enabled() {
        let job = unsafe { *Box::from_raw(data.cast::<Osc52IdleJob>()) };
        if !job.sink.skip_gtk_clipboard {
            write_gtk_clipboard_text(&job.text);
        }
        emit_footer_toast(&format_osc52_toast(job.text.len()), &job.sink);
    } else if !data.is_null() {
        unsafe { drop(Box::from_raw(data.cast::<Osc52IdleJob>())) };
    }
    0
}

extern "C" fn on_osc133_idle(data: *mut c_void) -> c_int {
    if data.is_null() {
        return 0;
    }
    let job = unsafe { *Box::from_raw(data.cast::<Osc133IdleJob>()) };
    let done = Osc133Completion {
        vte_addr: job.vte_addr,
        exit_code: job.exit_code,
        elapsed_secs: job.elapsed_secs,
        status_text: format_osc133_status(job.exit_code, job.elapsed_secs),
    };
    dispatch_osc133_completion(done, &job.sink);
    0
}

fn dispatch_osc133_completion(done: Osc133Completion, sink: &PtyTapSink) {
    if let Some(cb) = &sink.on_osc133_done {
        cb(done);
        return;
    }
    if let Ok(guard) = OSC133_HOOK.lock() {
        if let Some(hook) = *guard {
            hook(done);
        }
    }
}

fn schedule_osc52_write(text: String, sink: &PtyTapSink) {
    if !is_osc52_enabled() {
        return;
    }
    let boxed = Box::into_raw(Box::new(Osc52IdleJob { text, sink: sink.clone() }));
    unsafe { g_idle_add(on_osc52_idle, boxed.cast()) };
}

fn schedule_osc133_done(
    vte_addr: usize,
    exit_code: Option<i32>,
    elapsed_secs: u64,
    sink: &PtyTapSink,
) {
    let boxed = Box::into_raw(Box::new(Osc133IdleJob {
        vte_addr,
        exit_code,
        elapsed_secs,
        sink: sink.clone(),
    }));
    unsafe { g_idle_add(on_osc133_idle, boxed.cast()) };
}

fn set_cloexec(fd: c_int) {
    unsafe {
        fcntl(fd, F_SETFD, FD_CLOEXEC);
    }
}

fn set_nonblocking(fd: c_int) {
    unsafe {
        let flags = fcntl(fd, F_GETFL, 0);
        if flags >= 0 {
            fcntl(fd, F_SETFL, flags | O_NONBLOCK);
        }
    }
}

fn zeroed_termios() -> Termios {
    Termios {
        c_iflag: 0,
        c_oflag: 0,
        c_cflag: 0,
        c_lflag: 0,
        c_line: 0,
        c_cc: [0; 32],
        c_ispeed: 0,
        c_ospeed: 0,
    }
}

fn set_raw_termios(fd: c_int) {
    let mut tio = zeroed_termios();
    unsafe {
        if tcgetattr(fd, &mut tio) == 0 {
            cfmakeraw(&mut tio);
            tcsetattr(fd, TCSANOW, &tio);
        }
    }
}

fn enable_iutf8(fd: c_int) {
    let mut tio = zeroed_termios();
    unsafe {
        if tcgetattr(fd, &mut tio) == 0 {
            tio.c_iflag |= IUTF8;
            tcsetattr(fd, TCSANOW, &tio);
        }
    }
}

fn open_pty_pair(ws: &Winsize) -> io::Result<(c_int, c_int)> {
    let (mut master, mut slave) = (-1, -1);
    let rc = unsafe { openpty(&mut master, &mut slave, ptr::null_mut(), ptr::null(), ws) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    set_cloexec(master);
    set_cloexec(slave);
    Ok((master, slave))
}

fn default_winsize_for_vte(vte: *mut GtkWidget) -> Winsize {
    let (rows, cols, width, height) = if vte.is_null() {
        (24, 80, 0, 0)
    } else {
        unsafe {
            (
                vte_terminal_get_row_count(vte).clamp(1, u16::MAX as isize) as u16,
                vte_terminal_get_column_count(vte).clamp(1, u16::MAX as isize) as u16,
                gtk_widget_get_allocated_width(vte).clamp(0, u16::MAX as c_int) as u16,
                gtk_widget_get_allocated_height(vte).clamp(0, u16::MAX as c_int) as u16,
            )
        }
    };
    Winsize { ws_row: rows, ws_col: cols, ws_xpixel: width, ws_ypixel: height }
}

struct ResizeCtx {
    master_fd: Arc<Mutex<Option<c_int>>>,
}

fn apply_winsize_to_master(master_fd: &Mutex<Option<c_int>>, ws: &Winsize) {
    if let Ok(guard) = master_fd.lock() {
        if let Some(fd) = *guard {
            unsafe { ioctl(fd, TIOCSWINSZ, ws) };
        }
    }
}

extern "C" fn on_vte_size_allocate(
    vte: *mut GtkWidget,
    _alloc: *mut c_void,
    user_data: *mut c_void,
) {
    if vte.is_null() || user_data.is_null() {
        return;
    }
    let ctx = unsafe { &*user_data.cast::<ResizeCtx>() };
    let ws = default_winsize_for_vte(vte);
    apply_winsize_to_master(&ctx.master_fd, &ws);
}

extern "C" fn drop_resize_ctx(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        unsafe { drop(Box::from_raw(data.cast::<ResizeCtx>())) };
    }
}

fn connect_vte_resize(vte: *mut GtkWidget, master_fd: Arc<Mutex<Option<c_int>>>) {
    if vte.is_null() {
        return;
    }
    let ctx = Box::into_raw(Box::new(ResizeCtx { master_fd }));
    unsafe {
        g_signal_connect_data(
            vte,
            c"size-allocate".as_ptr(),
            on_vte_size_allocate as *const c_void,
            ctx.cast(),
            drop_resize_ctx as *const c_void,
            G_CONNECT_AFTER,
        );
    }
}

fn attach_foreign_pty_to_vte(vte: *mut GtkWidget, vte_master: c_int) -> bool {
    unsafe {
        let pty = vte_pty_new_foreign_sync(vte_master, ptr::null_mut(), ptr::null_mut());
        if pty.is_null() {
            close(vte_master);
            return false;
        }
        vte_pty_set_utf8(pty, 1);
        vte_terminal_set_pty(vte, pty);
        g_object_unref(pty);
        true
    }
}

fn prepare_child_ctty(slave_fd: c_int) -> io::Result<()> {
    unsafe {
        let _ = setsid();
        let _ = ioctl(slave_fd, TIOCSCTTY, 0 as c_int);
        if dup2(slave_fd, 0) < 0 || dup2(slave_fd, 1) < 0 || dup2(slave_fd, 2) < 0 {
            return Err(io::Error::last_os_error());
        }
        if slave_fd > 2 {
            close(slave_fd);
        }
    }
    Ok(())
}

fn spawn_child_process(cwd: &str, argv: &[&str], child_slave: c_int) -> Option<i32> {
    let prog = *argv.first()?;
    let mut cmd = Command::new(prog);
    cmd.args(&argv[1..])
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor");
    if !cwd.is_empty() {
        cmd.current_dir(cwd);
    }
    unsafe {
        cmd.pre_exec(move || prepare_child_ctty(child_slave));
    }
    let child = cmd.spawn().ok()?;
    Some(child.id() as i32)
}

#[derive(Default)]
struct PipeBuf {
    data: Vec<u8>,
    off: usize,
}

impl PipeBuf {
    fn pending(&self) -> &[u8] {
        &self.data[self.off..]
    }

    fn is_empty(&self) -> bool {
        self.off >= self.data.len()
    }

    fn len(&self) -> usize {
        self.data.len().saturating_sub(self.off)
    }

    fn consume(&mut self, n: usize) {
        self.off += n;
        if self.off >= self.data.len() {
            self.data.clear();
            self.off = 0;
        } else if self.off >= RELAY_BUF_CAP {
            self.data.drain(..self.off);
            self.off = 0;
        }
    }
}

fn is_would_block(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

fn read_nonblock(fd: c_int, scratch: &mut [u8]) -> Option<usize> {
    let n = unsafe { read(fd, scratch.as_mut_ptr().cast(), scratch.len()) };
    if n > 0 {
        return Some(n as usize);
    }
    if n == 0 {
        return None;
    }
    let err = io::Error::last_os_error();
    if is_would_block(&err) { Some(0) } else { None }
}

fn flush_pipe_buf(fd: c_int, buf: &mut PipeBuf) -> bool {
    while !buf.is_empty() {
        let slice = buf.pending();
        let n = unsafe { write(fd, slice.as_ptr().cast(), slice.len()) };
        if n > 0 {
            buf.consume(n as usize);
            continue;
        }
        let err = io::Error::last_os_error();
        return is_would_block(&err);
    }
    true
}

struct RelayState {
    vte_fd: c_int,
    child_fd: c_int,
    resize_handle: Arc<Mutex<Option<c_int>>>,
    vte_addr: usize,
    sink: PtyTapSink,
    parser: OscStreamParser,
    timer: OscCommandTimer,
    to_vte: PipeBuf,
    to_child: PipeBuf,
    vte_eof: bool,
    child_eof: bool,
}

impl RelayState {
    fn handle_osc_events(&mut self, events: &[OscEvent]) {
        let now = Instant::now();
        for ev in events {
            match ev {
                OscEvent::Osc52Write(text) => schedule_osc52_write(text.clone(), &self.sink),
                OscEvent::Osc52BlockedQuery => {}
                OscEvent::CommandStart => self.timer.on_start(now),
                OscEvent::CommandFinish { exit_code } => {
                    let (code, secs) = self.timer.on_finish(*exit_code, now);
                    schedule_osc133_done(self.vte_addr, code, secs, &self.sink);
                }
            }
        }
    }

    fn drain_child_output(&mut self, scratch: &mut [u8], events: &mut Vec<OscEvent>) {
        loop {
            match read_nonblock(self.child_fd, scratch) {
                Some(0) => break,
                Some(n) => {
                    events.clear();
                    self.parser.feed(&scratch[..n], &mut self.to_vte.data, events);
                    self.handle_osc_events(events);
                }
                None => {
                    self.child_eof = true;
                    break;
                }
            }
        }
    }

    fn drain_vte_input(&mut self, scratch: &mut [u8]) {
        loop {
            match read_nonblock(self.vte_fd, scratch) {
                Some(0) => break,
                Some(n) => self.to_child.data.extend_from_slice(&scratch[..n]),
                None => {
                    self.vte_eof = true;
                    break;
                }
            }
        }
    }

    fn build_poll_fds(&self) -> [PollFd; 2] {
        let c_in = (!self.child_eof && self.to_vte.len() < PENDING_HIGH_WATER)
            .then_some(POLLIN)
            .unwrap_or(0);
        let c_out = (!self.to_child.is_empty()).then_some(POLLOUT).unwrap_or(0);
        let v_in = (!self.vte_eof && self.to_child.len() < PENDING_HIGH_WATER)
            .then_some(POLLIN)
            .unwrap_or(0);
        let v_out = (!self.to_vte.is_empty()).then_some(POLLOUT).unwrap_or(0);
        [
            PollFd { fd: self.child_fd, events: c_in | c_out, revents: 0 },
            PollFd { fd: self.vte_fd, events: v_in | v_out, revents: 0 },
        ]
    }

    fn step_io(&mut self, fds: &[PollFd; 2], scratch: &mut [u8], events: &mut Vec<OscEvent>) {
        if (fds[0].revents & (POLLIN | POLLHUP | POLLERR)) != 0 {
            self.drain_child_output(scratch, events);
        }
        if (fds[1].revents & (POLLIN | POLLHUP | POLLERR)) != 0 {
            self.drain_vte_input(scratch);
        }
        if !flush_pipe_buf(self.vte_fd, &mut self.to_vte) {
            self.vte_eof = true;
        }
        if !flush_pipe_buf(self.child_fd, &mut self.to_child) {
            self.child_eof = true;
        }
    }

    fn is_done(&self) -> bool {
        self.vte_eof || (self.child_eof && self.to_vte.is_empty())
    }

    fn run(mut self) {
        let mut scratch = vec![0u8; RELAY_BUF_CAP];
        let mut events = Vec::new();
        while !self.is_done() {
            let mut fds = self.build_poll_fds();
            let rc = unsafe { poll(fds.as_mut_ptr(), 2, -1) };
            if rc < 0 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            self.step_io(&fds, &mut scratch, &mut events);
        }
        self.shutdown_fds();
    }

    fn shutdown_fds(self) {
        if let Ok(mut guard) = self.resize_handle.lock() {
            *guard = None;
        }
        unsafe {
            close(self.vte_fd);
            close(self.child_fd);
        }
    }
}

fn spawn_relay_thread(
    vte_slave: c_int,
    child_master: c_int,
    resize_handle: Arc<Mutex<Option<c_int>>>,
    vte_addr: usize,
    sink: PtyTapSink,
) {
    let state = RelayState {
        vte_fd: vte_slave,
        child_fd: child_master,
        resize_handle,
        vte_addr,
        sink,
        parser: OscStreamParser::new(),
        timer: OscCommandTimer::default(),
        to_vte: PipeBuf::default(),
        to_child: PipeBuf::default(),
        vte_eof: false,
        child_eof: false,
    };
    let _ = thread::Builder::new()
        .name("akhterm-pty-tap".to_owned())
        .spawn(move || state.run());
}

fn close_fd_pair(a: c_int, b: c_int) {
    unsafe {
        if a >= 0 {
            close(a);
        }
        if b >= 0 {
            close(b);
        }
    }
}

pub fn spawn_vte_with_tap(
    vte: *mut GtkWidget,
    cwd: &str,
    argv: &[&str],
    sink: PtyTapSink,
) -> i32 {
    if vte.is_null() || argv.is_empty() {
        return 0;
    }
    let ws = default_winsize_for_vte(vte);
    let Ok((vte_master, vte_slave)) = open_pty_pair(&ws) else {
        return 0;
    };
    let Ok((child_master, child_slave)) = open_pty_pair(&ws) else {
        close_fd_pair(vte_master, vte_slave);
        return 0;
    };
    prepare_tap_fds(vte_slave, child_master, child_slave);
    if !attach_foreign_pty_to_vte(vte, vte_master) {
        close_fd_pair(vte_slave, child_master);
        close_fd_pair(child_slave, -1);
        return 0;
    }
    finish_tap_spawn(vte, cwd, argv, vte_slave, child_master, child_slave, sink)
}

fn prepare_tap_fds(vte_slave: c_int, child_master: c_int, child_slave: c_int) {
    set_raw_termios(vte_slave);
    enable_iutf8(child_slave);
    set_nonblocking(vte_slave);
    set_nonblocking(child_master);
}

fn finish_tap_spawn(
    vte: *mut GtkWidget,
    cwd: &str,
    argv: &[&str],
    vte_slave: c_int,
    child_master: c_int,
    child_slave: c_int,
    sink: PtyTapSink,
) -> i32 {
    let pid_opt = spawn_child_process(cwd, argv, child_slave);
    unsafe { close(child_slave) };
    let Some(pid) = pid_opt else {
        close_fd_pair(vte_slave, child_master);
        return 0;
    };
    let resize_handle = Arc::new(Mutex::new(Some(child_master)));
    connect_vte_resize(vte, Arc::clone(&resize_handle));
    unsafe { vte_terminal_watch_child(vte, pid) };
    spawn_relay_thread(vte_slave, child_master, resize_handle, vte as usize, sink);
    pid
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;
    use std::time::Duration;

    use super::*;
    use crate::i18n::{configured_locale, set_locale, UiLocale};
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};

    #[test]
    fn runtime_toggles_and_bilingual_formatters_work() {
        let prev_loc = configured_locale();
        set_osc52_enabled(false);
        assert!(!is_osc52_enabled());
        set_osc52_enabled(true);
        assert!(is_osc52_enabled());

        set_notify_threshold_secs(0);
        assert!(!should_send_osc133_notify(60, notify_threshold_secs(), false));
        set_notify_threshold_secs(15);
        assert!(!should_send_osc133_notify(14, 15, false));
        assert!(!should_send_osc133_notify(20, 15, true));
        assert!(should_send_osc133_notify(15, 15, false));

        set_locale(UiLocale::Ru);
        assert_eq!(format_osc52_toast(42), "Скопировано по OSC 52 · 42 байт");
        set_locale(UiLocale::En);
        assert_eq!(format_osc52_toast(42), "Copied via OSC 52 · 42 bytes");
        set_locale(prev_loc);

        assert_eq!(format_osc133_status(Some(0), 18), "✓ · 18s");
        assert_eq!(format_osc133_status(None, 5), "✓ · 5s");
        assert_eq!(format_osc133_status(Some(130), 72), "✗ 130 · 1m 12s");
    }

    fn read_clipboard_text_blocking() -> Option<String> {
        unsafe {
            let clip = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
            let ptr = gtk_clipboard_wait_for_text(clip);
            if ptr.is_null() {
                return None;
            }
            let text = CStr::from_ptr(ptr).to_string_lossy().into_owned();
            g_free(ptr.cast());
            Some(text)
        }
    }

    fn read_vte_screen_text(vte: *mut GtkWidget) -> String {
        unsafe {
            let ptr = vte_terminal_get_text_format(vte, VTE_FORMAT_TEXT);
            if ptr.is_null() {
                return String::new();
            }
            let text = CStr::from_ptr(ptr).to_string_lossy().into_owned();
            g_free(ptr.cast());
            text
        }
    }

    #[test]
    fn headless_vte_pty_tap_relays_osc52_blocks_queries_and_tracks_osc133() {
        with_headless_gtk(|| {
            set_osc52_enabled(true);
            let prev_loc = configured_locale();
            set_locale(UiLocale::Ru);
            let toasts = Arc::new(Mutex::new(Vec::<String>::new()));
            let completions = Arc::new(Mutex::new(Vec::<Osc133Completion>::new()));
            let sink = make_test_sink(&toasts, &completions);
            let (win, vte) = spawn_tap_probe_window(sink);
            pump_until_done(&completions, vte, "TAP_OK");
            assert_eq!(read_clipboard_text_blocking().as_deref(), Some("Привет ЦУ"));
            assert_eq!(toasts.lock().unwrap().as_slice(), &["Скопировано по OSC 52 · 17 байт"]);
            let screen = read_vte_screen_text(vte);
            assert!(screen.contains("TAP_OK"), "screen={screen:?}");
            assert!(!screen.contains('?'), "query leaked to screen={screen:?}");
            let done = completions.lock().unwrap();
            assert_eq!(done.len(), 1);
            assert_eq!(done[0].exit_code, Some(42));
            assert!(done[0].status_text.starts_with("✗ 42 · "));
            unsafe { gtk_widget_destroy(win) };
            pump_gtk_events();
            set_locale(prev_loc);
        });
    }

    #[test]
    fn headless_vte_pty_tap_syncs_winsize_and_respects_osc52_disable() {
        with_headless_gtk(|| {
            set_osc52_enabled(false);
            let toasts = Arc::new(Mutex::new(Vec::<String>::new()));
            let completions = Arc::new(Mutex::new(Vec::<Osc133Completion>::new()));
            let sink = make_test_sink(&toasts, &completions);
            let (win, vte) = spawn_resize_probe_window(sink);
            pump_until_done(&completions, vte, "SIZE=");
            assert!(toasts.lock().unwrap().is_empty());
            let screen = read_vte_screen_text(vte);
            assert!(screen.contains("SIZE="), "screen={screen:?}");
            assert!(!screen.contains("SIZE=0 0"), "zero winsize: {screen:?}");
            unsafe { gtk_widget_destroy(win) };
            pump_gtk_events();
            set_osc52_enabled(true);
        });
    }

    fn make_test_sink(
        toasts: &Arc<Mutex<Vec<String>>>,
        completions: &Arc<Mutex<Vec<Osc133Completion>>>,
    ) -> PtyTapSink {
        let t_out = Arc::clone(toasts);
        let c_out = Arc::clone(completions);
        PtyTapSink {
            on_osc52_toast: Some(Arc::new(move |msg| t_out.lock().unwrap().push(msg.to_owned()))),
            on_osc133_done: Some(Arc::new(move |done| c_out.lock().unwrap().push(done))),
            skip_gtk_clipboard: false,
        }
    }

    fn spawn_tap_probe_window(sink: PtyTapSink) -> (*mut GtkWidget, *mut GtkWidget) {
        let script = concat!(
            "printf '\\033]52;c;0J/RgNC40LLQtdGCINCm0KM=\\007';",
            "printf '\\033]52;c;?\\007';",
            "printf '\\033]133;C\\007';",
            "printf 'TAP_OK\\n';",
            "printf '\\033]133;D;42\\007';",
            "sleep 0.15"
        );
        spawn_script_window(sink, script, 640, 240)
    }

    fn spawn_resize_probe_window(sink: PtyTapSink) -> (*mut GtkWidget, *mut GtkWidget) {
        let script = concat!(
            "printf '\\033]52;c;ZGlzYWJsZWQ=\\007';",
            "printf '\\033]133;C\\007';",
            "sleep 0.08;",
            "printf 'SIZE=%s\\n' \"$(stty size)\";",
            "printf '\\033]133;D;0\\007';",
            "sleep 0.15"
        );
        spawn_script_window(sink, script, 820, 360)
    }

    fn spawn_script_window(
        sink: PtyTapSink,
        script: &str,
        width: c_int,
        height: c_int,
    ) -> (*mut GtkWidget, *mut GtkWidget) {
        unsafe {
            let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
            gtk_window_set_default_size(win, width, height);
            let vte = vte_terminal_new();
            gtk_container_add(win, vte);
            gtk_widget_show_all(win);
            pump_gtk_events();
            let pid = spawn_vte_with_tap(vte, "/tmp", &["sh", "-c", script], sink);
            assert!(pid > 0);
            (win, vte)
        }
    }

    fn pump_until_done(
        completions: &Arc<Mutex<Vec<Osc133Completion>>>,
        vte: *mut GtkWidget,
        needle: &str,
    ) {
        for _ in 0..100 {
            pump_gtk_events();
            let has_done = !completions.lock().unwrap().is_empty();
            let has_text = read_vte_screen_text(vte).contains(needle);
            if has_done && has_text {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        pump_gtk_events();
    }

    #[test]
    fn pty_tap_meets_grade_b_mi_target() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("pty_tap.rs"));
        assert!(mi >= 60.0, "pty_tap.rs MI {mi:.2} < 60.0");
    }
}
