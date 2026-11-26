//! The Windows side of the integrated title bar (like Windows Terminal or VS Code): the app icon
//! and Windows 11 caption buttons drawn by the app ([`Caption`]), while the window keeps its native
//! frame (`WS_CAPTION | WS_THICKFRAME`, so Aero Snap, Win+Arrow, Snap Layouts, Aero Shake, the
//! shadow and the min/max animations stay the system's), and a window procedure subclass in front
//! of winit's removes only the caption from the non-client area and answers hit tests from the
//! regions egui published, the way Windows Terminal does.
//!
//! - `WM_NCCALCSIZE`: the default calculation, then the client top moves back up to the window top.
//!   Left, right and bottom keep their invisible resize borders. Maximized windows overhang the
//!   monitor by the frame, so the top keeps that overhang; a maximized window leaves 2 px at the
//!   edge of an auto-hide taskbar so the taskbar can still slide in.
//! - `WM_NCHITTEST`: the default answer for the side and bottom borders, else
//!   [`hit_test`](super::hit_test::hit_test): top resize band, `HTMINBUTTON` / `HTMAXBUTTON` (Snap
//!   Layouts) / `HTCLOSE`, `HTSYSMENU` for the icon, `HTCAPTION` for the drag area.
//! - The caption buttons then get non-client mouse messages; their hover and press state lives
//!   here for egui to paint, and a release over the pressed button sends the `SC_*` command a
//!   native button would.
//! - winit sizes the window as if it had a caption (its `AdjustWindowRectEx` includes one). The
//!   subclass takes that caption height off again at start-up, in `WM_GETMINMAXINFO` and during
//!   `WM_DPICHANGED`, so the inner size stays what egui asked for when moving between monitors.
//!
//! Everything runs on the UI thread (window messages go to the thread that created the window,
//! which is the one egui draws on), so the shared state is thread-local `Cell`s: no locks.
//!
//! Each message arrives as a [`Delivered`]: made once from the window procedure's arguments, it
//! forwards them and reads the structs behind `lparam` for the messages that carry one, so the
//! handlers themselves stay safe code.
#![expect(
    unsafe_code,
    reason = "a Win32 window procedure subclass; each unsafe block makes one call or one read"
)]

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicIsize, Ordering};

use egui::{Rangef, Rect, Ui, pos2, vec2};
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, ScreenToClient,
};
use windows_sys::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForWindow, GetSystemMetricsForDpi,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::Shell::{
    ABE_BOTTOM, ABE_LEFT, ABE_RIGHT, ABE_TOP, ABM_GETAUTOHIDEBAREX, ABM_GETSTATE, ABS_AUTOHIDE,
    APPBARDATA, SHAppBarMessage,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, GWL_EXSTYLE, GWL_STYLE, GWLP_WNDPROC, GetClientRect, GetWindowLongW,
    GetWindowRect, HTCAPTION, HTCLIENT, HTCLOSE, HTMAXBUTTON, HTMINBUTTON, HTSYSMENU, HTTOP,
    HTTOPLEFT, HTTOPRIGHT, IsZoomed, MINMAXINFO, NCCALCSIZE_PARAMS, PostMessageW, SC_CLOSE,
    SC_MAXIMIZE, SC_MINIMIZE, SC_RESTORE, SM_CXPADDEDBORDER, SM_CYSIZEFRAME, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SYSTEM_METRICS_INDEX,
    SetWindowLongPtrW, SetWindowPos, WINDOW_LONG_PTR_INDEX, WINDOWPOS, WM_DPICHANGED,
    WM_GETMINMAXINFO, WM_MOUSEMOVE, WM_NCCALCSIZE, WM_NCDESTROY, WM_NCHITTEST, WM_NCLBUTTONDBLCLK,
    WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_SYSCOMMAND,
    WM_WINDOWPOSCHANGING, WNDPROC,
};

use super::app_mark;
use super::caption_buttons::{self, ButtonPointer};
use super::hit_test::{self, CaptionButton, CaptionRegions, Hit, WindowFrame};

const ICON_SIZE: f32 = 16.0;
/// Left of the icon.
const ICON_GAP: f32 = 12.0;
/// Rows kept free at the edge of an auto-hide taskbar on a maximized window; with none, Windows
/// takes the window for a full-screen app and the taskbar never slides in.
const AUTO_HIDE_TASKBAR_GAP: i32 = 2;

/// winit's window procedure (or the one in front of it), called for everything not handled here.
static ORIGINAL_PROC: AtomicIsize = AtomicIsize::new(0);

thread_local! {
    static REGIONS: Cell<Option<CaptionRegions>> = const { Cell::new(None) };
    static POINTER: Cell<ButtonPointer> = const { Cell::new(ButtonPointer::NONE) };
    static IS_TRACKING_LEAVE: Cell<bool> = const { Cell::new(false) };
    static IS_CHANGING_DPI: Cell<bool> = const { Cell::new(false) };
    static CONTEXT: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
}

/// The window's own parts of an integrated title bar: the app icon at the left end (it opens the
/// system menu) and the caption buttons at the right end.
pub struct Caption {
    bar: Rect,
    icon: Rect,
    buttons: [Rect; 3],
}

impl Caption {
    /// Paints the icon (when `has_icon`) and the caption buttons into `bar`.
    pub fn show(ui: &mut Ui, bar: Rect, has_icon: bool) -> Self {
        let icon = if has_icon {
            let icon = Rect::from_center_size(
                pos2(bar.left() + ICON_GAP + ICON_SIZE / 2.0, bar.center().y),
                vec2(ICON_SIZE, ICON_SIZE),
            );
            paint_icon(ui, icon);
            icon
        } else {
            Rect::NOTHING
        };
        let buttons = caption_buttons::show(ui, bar);
        Self { bar, icon, buttons }
    }

    /// The part of the bar the app's content may take: after the icon, before the buttons.
    pub fn free(&self) -> Rangef {
        let left = if self.icon.is_positive() { self.icon.right() } else { self.bar.left() };
        Rangef::new(left, self.buttons[0].left())
    }

    /// Makes the bar the window's caption for the next hit tests: dragging it moves the window
    /// except over `holes` (the content's widgets).
    pub fn publish(self, ctx: &egui::Context, holes: &[Rect]) {
        REGIONS.set(Some(CaptionRegions::from_points(
            self.bar,
            self.icon,
            self.buttons,
            holes,
            ctx.pixels_per_point(),
        )));
    }
}

/// The app mark, dimmed while the window is inactive like the rest of a Windows title bar.
fn paint_icon(ui: &Ui, rect: Rect) {
    let is_active = ui.ctx().input(|input| input.viewport().focused.unwrap_or(true));
    let tint = if is_active { egui::Color32::WHITE } else { egui::Color32::from_white_alpha(140) };
    app_mark::paint(ui, rect, tint);
}

/// Integrates the title bar into the window of `creation`. False when there is no Win32 window
/// (tests) or the subclass could not be set; the window then keeps its native caption.
pub fn integrate(creation: &eframe::CreationContext<'_>) -> bool {
    let Ok(handle) = creation.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return false;
    };
    let hwnd = handle.hwnd.get() as HWND;
    CONTEXT.with_borrow_mut(|context| *context = Some(creation.egui_ctx.clone()));
    // SAFETY: `hwnd` is this app's live top-level window, owned by this thread; `wnd_proc` matches
    // WNDPROC and forwards everything it does not handle to the procedure it replaces.
    let previous =
        unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, wnd_proc as *const () as usize as isize) };
    if previous == 0 {
        CONTEXT.with_borrow_mut(|context| *context = None);
        return false;
    }
    ORIGINAL_PROC.store(previous, Ordering::Relaxed);
    // winit created the window with room for a caption; the caption goes, the inner size stays.
    let window = window_rect(hwnd);
    let (width, mut height) = (window.right - window.left, window.bottom - window.top);
    let mut flags =
        SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE;
    if is_zoomed(hwnd) {
        flags |= SWP_NOSIZE;
    } else {
        height -= caption_height(hwnd);
    }
    // SAFETY: moves no window but this app's own; no window to insert after.
    unsafe { SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, width, height, flags) };
    true
}

/// Which caption button the pointer is over or holds down.
pub fn pointer() -> ButtonPointer {
    POINTER.get()
}

/// What the bar published last frame, as the window procedure reads it.
#[cfg(any(test, feature = "test-support"))]
pub fn published() -> Option<CaptionRegions> {
    REGIONS.get()
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the system calls this procedure with this window's messages.
    let mut delivered = unsafe { Delivered::new(hwnd, message, wparam, lparam) };
    match message {
        WM_NCCALCSIZE if wparam != 0 => calculate_size(&mut delivered),
        WM_NCHITTEST => hit_test_message(&delivered),
        WM_NCMOUSEMOVE => {
            track_leave(hwnd);
            set_hovered(button_of(wparam));
            delivered.forward()
        }
        WM_NCMOUSELEAVE => {
            IS_TRACKING_LEAVE.set(false);
            update_pointer(ButtonPointer::NONE);
            delivered.forward()
        }
        WM_MOUSEMOVE => {
            set_hovered(None);
            delivered.forward()
        }
        // The system's own button tracking would draw classic buttons over the client area.
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if button_of(wparam).is_some() => {
            update_pointer(ButtonPointer {
                hovered: button_of(wparam),
                pressed: button_of(wparam),
            });
            0
        }
        WM_NCLBUTTONUP if button_of(wparam).is_some() => {
            let button = button_of(wparam);
            if POINTER.get().pressed == button
                && let Some(button) = button
            {
                run(hwnd, button);
            }
            update_pointer(ButtonPointer { hovered: button, pressed: None });
            0
        }
        WM_DPICHANGED => {
            IS_CHANGING_DPI.set(true);
            let result = delivered.forward();
            IS_CHANGING_DPI.set(false);
            result
        }
        // winit sizes the window for the new DPI with a caption; take it off again.
        WM_WINDOWPOSCHANGING if IS_CHANGING_DPI.get() && !is_zoomed(hwnd) => {
            if let Some(position) = delivered.window_pos()
                && position.flags & SWP_NOSIZE == 0
            {
                position.cy -= caption_height(hwnd);
            }
            delivered.forward()
        }
        WM_GETMINMAXINFO => {
            let result = delivered.forward();
            let caption = caption_height(hwnd);
            // Only a minimum winit set from the inner size includes the caption.
            if let Some(info) = delivered.min_max_info()
                && info.ptMinTrackSize.y > caption * 2
            {
                info.ptMinTrackSize.y -= caption;
            }
            result
        }
        WM_NCDESTROY => {
            let original = ORIGINAL_PROC.load(Ordering::Relaxed);
            // SAFETY: puts back the procedure `integrate` replaced, on this app's own window.
            unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, original) };
            REGIONS.set(None);
            CONTEXT.with_borrow_mut(|context| *context = None);
            delivered.forward()
        }
        _ => delivered.forward(),
    }
}

/// The default non-client size, then the client top moves back up to the window top.
fn calculate_size(delivered: &mut Delivered) -> LRESULT {
    let Some(window) = delivered.calc_size_params().map(|params| params.rgrc[0]) else {
        return delivered.forward();
    };
    let result = delivered.forward();
    let hwnd = delivered.hwnd;
    let is_maximized = is_zoomed(hwnd);
    if let Some(params) = delivered.calc_size_params() {
        let client = &mut params.rgrc[0];
        if is_maximized {
            // The overhang below the monitor equals the one above it.
            client.top = window.top + (window.bottom - client.bottom);
            leave_room_for_auto_hide_taskbar(hwnd, client);
        } else {
            client.top = window.top;
        }
    }
    result
}

/// The default answer outside the client area, else the regions the bar published.
fn hit_test_message(delivered: &Delivered) -> LRESULT {
    let native = delivered.forward();
    if native != HTCLIENT as LRESULT {
        return native;
    }
    let Some(regions) = REGIONS.get() else {
        return native;
    };
    let hwnd = delivered.hwnd;
    let lparam = delivered.lparam;
    let point = screen_to_client(hwnd, POINT { x: signed_low(lparam), y: signed_high(lparam) });
    let client = client_rect(hwnd);
    let frame = WindowFrame {
        is_maximized: is_zoomed(hwnd),
        resize_border: resize_border(hwnd),
        width: client.right - client.left,
    };
    hit_code(hit_test::hit_test(point.x, point.y, &regions, frame)) as LRESULT
}

/// One message the system delivered to [`wnd_proc`], made only from that procedure's own
/// arguments: the pointers in `lparam` are valid as documented for `message` while it runs.
struct Delivered {
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
}

impl Delivered {
    /// # Safety
    ///
    /// The arguments are the ones the system passed to the window procedure, and the value is used
    /// only while that call runs.
    unsafe fn new(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> Self {
        Self { hwnd, message, wparam, lparam }
    }

    /// Hands the message unchanged to the procedure this subclass replaced.
    fn forward(&self) -> LRESULT {
        let Self { hwnd, message, wparam, lparam } = *self;
        // SAFETY: the arguments are the system's for this call, as `new` requires.
        unsafe { CallWindowProcW(original_proc(), hwnd, message, wparam, lparam) }
    }

    /// `WM_NCCALCSIZE` with `wparam` set: the proposed window rect, then the client rect.
    fn calc_size_params(&mut self) -> Option<&mut NCCALCSIZE_PARAMS> {
        let lparam = self.lparam;
        (self.message == WM_NCCALCSIZE && self.wparam != 0)
            // SAFETY: for this message `lparam` points to the system's NCCALCSIZE_PARAMS, and the
            // `&mut self` borrow keeps a second reference from being made meanwhile.
            .then(|| unsafe { &mut *(lparam as *mut NCCALCSIZE_PARAMS) })
    }

    /// `WM_WINDOWPOSCHANGING`: the position and size about to be applied.
    fn window_pos(&mut self) -> Option<&mut WINDOWPOS> {
        let lparam = self.lparam;
        (self.message == WM_WINDOWPOSCHANGING)
            // SAFETY: for this message `lparam` points to the system's WINDOWPOS.
            .then(|| unsafe { &mut *(lparam as *mut WINDOWPOS) })
    }

    /// `WM_GETMINMAXINFO`: the sizes the window may take.
    fn min_max_info(&mut self) -> Option<&mut MINMAXINFO> {
        let lparam = self.lparam;
        (self.message == WM_GETMINMAXINFO)
            // SAFETY: for this message `lparam` points to the system's MINMAXINFO.
            .then(|| unsafe { &mut *(lparam as *mut MINMAXINFO) })
    }
}

fn original_proc() -> WNDPROC {
    let original = ORIGINAL_PROC.load(Ordering::Relaxed);
    // SAFETY: `ORIGINAL_PROC` holds the WNDPROC that `integrate` replaced, or 0 (`None`).
    unsafe { std::mem::transmute::<isize, WNDPROC>(original) }
}

fn hit_code(hit: Hit) -> u32 {
    match hit {
        Hit::Client => HTCLIENT,
        Hit::Caption => HTCAPTION,
        Hit::SystemMenu => HTSYSMENU,
        Hit::Button(CaptionButton::Minimize) => HTMINBUTTON,
        Hit::Button(CaptionButton::Maximize) => HTMAXBUTTON,
        Hit::Button(CaptionButton::Close) => HTCLOSE,
        Hit::Top => HTTOP,
        Hit::TopLeft => HTTOPLEFT,
        Hit::TopRight => HTTOPRIGHT,
    }
}

/// The caption button of a non-client message's hit-test code.
fn button_of(wparam: WPARAM) -> Option<CaptionButton> {
    match wparam as u32 {
        HTMINBUTTON => Some(CaptionButton::Minimize),
        HTMAXBUTTON => Some(CaptionButton::Maximize),
        HTCLOSE => Some(CaptionButton::Close),
        _ => None,
    }
}

/// What a click on `button` does: the same system commands as the native buttons, so the
/// animations and the window state stay the system's.
fn run(hwnd: HWND, button: CaptionButton) {
    let command = match button {
        CaptionButton::Minimize => SC_MINIMIZE,
        CaptionButton::Maximize if is_zoomed(hwnd) => SC_RESTORE,
        CaptionButton::Maximize => SC_MAXIMIZE,
        CaptionButton::Close => SC_CLOSE,
    };
    // SAFETY: posts a system command to this app's own window; no pointers.
    unsafe { PostMessageW(hwnd, WM_SYSCOMMAND, command as WPARAM, 0) };
}

fn set_hovered(button: Option<CaptionButton>) {
    let current = POINTER.get();
    // Leaving a held button drops the press, as on a native button.
    let pressed = current.pressed.filter(|pressed| Some(*pressed) == button);
    update_pointer(ButtonPointer { hovered: button, pressed });
}

fn update_pointer(pointer: ButtonPointer) {
    if POINTER.replace(pointer) != pointer {
        // `try_borrow`: never re-enter a borrow if a repaint request ever dispatched a message.
        CONTEXT.with(|context| {
            if let Ok(context) = context.try_borrow()
                && let Some(context) = context.as_ref()
            {
                context.request_repaint();
            }
        });
    }
}

/// Asks for `WM_NCMOUSELEAVE` once the pointer leaves the non-client area.
fn track_leave(hwnd: HWND) {
    if IS_TRACKING_LEAVE.get() {
        return;
    }
    let mut track = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE | TME_NONCLIENT,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    // SAFETY: the struct carries its size and lives for the call.
    let is_tracking = unsafe { TrackMouseEvent(&raw mut track) } != 0;
    IS_TRACKING_LEAVE.set(is_tracking);
}

/// The caption height (plus top frame) the default frame adds above the client area at the
/// window's current DPI: what winit's size calculations include and the subclass removes.
fn caption_height(hwnd: HWND) -> i32 {
    let style = window_long(hwnd, GWL_STYLE);
    let ex_style = window_long(hwnd, GWL_EXSTYLE);
    let dpi = dpi_of(hwnd);
    let mut rect = RECT::default();
    // SAFETY: adjusts the local RECT, which lives for the call.
    let is_adjusted =
        unsafe { AdjustWindowRectExForDpi(&raw mut rect, style, 0, ex_style, dpi) } != 0;
    if is_adjusted { -rect.top } else { 0 }
}

/// Thickness of the resize frame at the window's DPI, also the height of the top resize band.
fn resize_border(hwnd: HWND) -> i32 {
    let dpi = dpi_of(hwnd);
    system_metric(SM_CYSIZEFRAME, dpi) + system_metric(SM_CXPADDEDBORDER, dpi)
}

fn leave_room_for_auto_hide_taskbar(hwnd: HWND, client: &mut RECT) {
    let mut state =
        APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, ..Default::default() };
    // SAFETY: `state` carries its size and lives for the call.
    let taskbar_state = unsafe { SHAppBarMessage(ABM_GETSTATE, &raw mut state) } as u32;
    if taskbar_state & ABS_AUTOHIDE == 0 {
        return;
    }
    // SAFETY: a query for this app's own window; the nearest monitor always exists.
    let nearest = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut monitor =
        MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    // SAFETY: `monitor` carries its size and lives for the call.
    if unsafe { GetMonitorInfoW(nearest, &raw mut monitor) } == 0 {
        return;
    }
    for edge in [ABE_TOP, ABE_BOTTOM, ABE_LEFT, ABE_RIGHT] {
        let mut bar = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            uEdge: edge,
            rc: monitor.rcMonitor,
            ..Default::default()
        };
        // SAFETY: `bar` carries its size and lives for the call.
        if unsafe { SHAppBarMessage(ABM_GETAUTOHIDEBAREX, &raw mut bar) } == 0 {
            continue;
        }
        match edge {
            ABE_TOP => client.top += AUTO_HIDE_TASKBAR_GAP,
            ABE_BOTTOM => client.bottom -= AUTO_HIDE_TASKBAR_GAP,
            ABE_LEFT => client.left += AUTO_HIDE_TASKBAR_GAP,
            _ => client.right -= AUTO_HIDE_TASKBAR_GAP,
        }
    }
}

/// The window is maximized.
fn is_zoomed(hwnd: HWND) -> bool {
    // SAFETY: a state query; no pointers.
    unsafe { IsZoomed(hwnd) != 0 }
}

/// The window's rect on screen, including its frame.
fn window_rect(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    // SAFETY: writes the local RECT, which lives for the call.
    unsafe { GetWindowRect(hwnd, &raw mut rect) };
    rect
}

/// The window's client rect, in client coordinates.
fn client_rect(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    // SAFETY: writes the local RECT, which lives for the call.
    unsafe { GetClientRect(hwnd, &raw mut rect) };
    rect
}

/// `point` on screen in the window's client coordinates.
fn screen_to_client(hwnd: HWND, mut point: POINT) -> POINT {
    // SAFETY: converts the local POINT in place, which lives for the call.
    unsafe { ScreenToClient(hwnd, &raw mut point) };
    point
}

/// One of the window's style words.
fn window_long(hwnd: HWND, index: WINDOW_LONG_PTR_INDEX) -> u32 {
    // SAFETY: a style query; no pointers.
    unsafe { GetWindowLongW(hwnd, index) as u32 }
}

/// The window's DPI.
fn dpi_of(hwnd: HWND) -> u32 {
    // SAFETY: a query; no pointers.
    unsafe { GetDpiForWindow(hwnd) }
}

/// A system metric at `dpi`.
fn system_metric(index: SYSTEM_METRICS_INDEX, dpi: u32) -> i32 {
    // SAFETY: a query; no pointers.
    unsafe { GetSystemMetricsForDpi(index, dpi) }
}

fn signed_low(lparam: LPARAM) -> i32 {
    i32::from((lparam & 0xFFFF) as u16 as i16)
}

fn signed_high(lparam: LPARAM) -> i32 {
    i32::from(((lparam >> 16) & 0xFFFF) as u16 as i16)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The structs behind `lparam` are read only for the message that carries them; any other
    /// message would hand out a reference to whatever `lparam` holds.
    #[test]
    fn lparam_structs_are_read_only_for_their_message() {
        let mut params = NCCALCSIZE_PARAMS::default();
        let lparam = (&raw mut params) as LPARAM;
        let deliver = |message, wparam| {
            // SAFETY: `params` outlives every value made here, and nothing is forwarded.
            unsafe { Delivered::new(std::ptr::null_mut(), message, wparam, lparam) }
        };
        assert!(deliver(WM_NCCALCSIZE, 1).calc_size_params().is_some());
        assert!(
            deliver(WM_NCCALCSIZE, 0).calc_size_params().is_none(),
            "without wparam the message carries a RECT, not the params"
        );
        assert!(deliver(WM_NCHITTEST, 1).calc_size_params().is_none());
        assert!(deliver(WM_NCCALCSIZE, 1).window_pos().is_none());
        assert!(deliver(WM_NCCALCSIZE, 1).min_max_info().is_none());
    }
}
