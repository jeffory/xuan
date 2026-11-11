//! macOS integration that winit does not provide.
//!
//! winit's default app menu quits with `terminate:`, and its app delegate does not implement
//! `applicationShouldTerminate:`, so ⌘Q, Quit in the Dock and logging out would end the process
//! without the unsaved-changes prompt. [`install_quit_handler`] adds that method to winit's
//! delegate: macOS quits at once when nothing is unsaved, and otherwise the quit is cancelled and
//! the app runs its own quit flow, which asks first.
//!
//! Files opened from Finder (double-click, Open With) or dropped on the Dock icon arrive as an
//! `odoc` Apple Event, which AppKit passes to the delegate's `application:openURLs:`. winit's
//! delegate does not implement that either; [`install_open_handler`] adds it.
//!
//! In the Compact style the window's content runs under its transparent title bar, where macOS
//! moves the window on any drag unless the view under the pointer says otherwise. winit's view
//! does not, so dragging a tab there would move the window. [`keep_title_bar_drags`] makes the
//! view keep its drags; Xuan moves the window itself from the empty parts of the title row
//! (`chrome::title_row_gesture`).

use std::{
    ffi::{CStr, OsStr, c_char},
    os::unix::ffi::OsStrExt,
    path::PathBuf,
    sync::{
        Mutex, OnceLock, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};

use objc2::{
    class, ffi, msg_send,
    runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Imp, Sel},
    sel,
};

/// Whether quitting would ask first; the app updates it after every frame.
static NEEDS_PROMPT: AtomicBool = AtomicBool::new(false);
/// macOS asked to quit while quitting needed a prompt.
static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);
static CONTEXT: OnceLock<egui::Context> = OnceLock::new();

// NSApplicationTerminateReply values.
const TERMINATE_CANCEL: usize = 0;
const TERMINATE_NOW: usize = 1;

extern "C-unwind" fn should_terminate(
    _this: *mut AnyObject,
    _cmd: Sel,
    _app: *mut AnyObject,
) -> usize {
    if !NEEDS_PROMPT.load(Ordering::SeqCst) {
        // winit then reports the loop exiting, so eframe still saves its state.
        return TERMINATE_NOW;
    }
    QUIT_REQUESTED.store(true, Ordering::SeqCst);
    if let Some(context) = CONTEXT.get() {
        context.request_repaint();
    }
    TERMINATE_CANCEL
}

/// Makes macOS ask the app before quitting. Call on the main thread once the event loop exists:
/// winit sets its app delegate when it creates the loop.
pub(super) fn install_quit_handler(context: &egui::Context) {
    let _ = CONTEXT.set(context.clone());
    // SAFETY: called on the main thread. The method is added to the delegate's own class, which
    // does not implement it; its signature matches the encoding: NSUInteger return, self,
    // _cmd and the NSApplication.
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let delegate: *mut AnyObject = msg_send![app, delegate];
        let Some(delegate) = delegate.as_ref() else {
            eprintln!("No app delegate: macOS will quit without asking about unsaved changes");
            return;
        };
        let class = std::ptr::from_ref::<AnyClass>(delegate.class()).cast_mut();
        let imp = std::mem::transmute::<
            extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize,
            Imp,
        >(should_terminate);
        if !ffi::class_addMethod(
            class,
            sel!(applicationShouldTerminate:),
            imp,
            c"Q@:@".as_ptr(),
        )
        .as_bool()
        {
            eprintln!("The app delegate already answers applicationShouldTerminate:");
        }
    }
}

/// Records whether quitting now would need the unsaved-changes or Develop prompt.
pub(super) fn set_needs_prompt(needs_prompt: bool) {
    NEEDS_PROMPT.store(needs_prompt, Ordering::SeqCst);
}

/// Whether macOS asked to quit since the last call.
pub(super) fn take_quit_request() -> bool {
    QUIT_REQUESTED.swap(false, Ordering::SeqCst)
}

/// Files macOS asked to open (Finder's Open and Open With, drops on the Dock icon), waiting
/// for the app to take them.
static OPENED_FILES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

extern "C-unwind" fn open_urls(
    _this: *mut AnyObject,
    _cmd: Sel,
    _app: *mut AnyObject,
    urls: *mut AnyObject,
) {
    let mut paths = Vec::new();
    // SAFETY: AppKit passes an NSArray of NSURL, and this runs on the main thread.
    unsafe {
        let count: usize = msg_send![urls, count];
        for index in 0..count {
            let url: *mut AnyObject = msg_send![urls, objectAtIndex: index];
            let is_file: Bool = msg_send![url, isFileURL];
            if !is_file.as_bool() {
                continue;
            }
            let representation: *const c_char = msg_send![url, fileSystemRepresentation];
            if let Some(representation) = representation.as_ref() {
                let bytes = CStr::from_ptr(representation).to_bytes();
                paths.push(PathBuf::from(OsStr::from_bytes(bytes)));
            }
        }
    }
    if paths.is_empty() {
        return;
    }
    OPENED_FILES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .extend(paths);
    if let Some(context) = CONTEXT.get() {
        context.request_repaint();
    }
}

extern "C-unwind" fn will_finish_launching(
    _this: *mut AnyObject,
    _cmd: Sel,
    _notification: *mut AnyObject,
) {
    // SAFETY: AppKit posts the notification on the main thread. The method is added to the
    // delegate's own class, which does not implement it; its signature matches the encoding:
    // void return, self, _cmd, the NSApplication and an NSArray of NSURL.
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let delegate: *mut AnyObject = msg_send![app, delegate];
        let Some(delegate) = delegate.as_ref() else {
            eprintln!("No app delegate: files opened from Finder will not open in Xuan");
            return;
        };
        let class = std::ptr::from_ref::<AnyClass>(delegate.class()).cast_mut();
        let imp = std::mem::transmute::<
            extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject),
            Imp,
        >(open_urls);
        if !ffi::class_addMethod(class, sel!(application:openURLs:), imp, c"v@:@@".as_ptr())
            .as_bool()
        {
            eprintln!("The app delegate already answers application:openURLs:");
        }
    }
}

extern "C-unwind" fn cannot_move_window(_this: *mut AnyObject, _cmd: Sel) -> Bool {
    Bool::NO
}

/// The type encoding of `- (BOOL)mouseDownCanMoveWindow`: BOOL is `bool` on Apple silicon and
/// `signed char` on Intel.
#[cfg(target_arch = "aarch64")]
const CANNOT_MOVE_WINDOW_TYPES: &std::ffi::CStr = c"B@:";
#[cfg(not(target_arch = "aarch64"))]
const CANNOT_MOVE_WINDOW_TYPES: &std::ffi::CStr = c"c@:";

/// Makes the views of the app's windows keep the drags that start under the title bar, so a tab
/// drags rather than the window. Call on the main thread once the main window exists; floating
/// panels share the view's class, so they follow.
pub(super) fn keep_title_bar_drags() {
    // SAFETY: called on the main thread. `windows`, `count`, `objectAtIndex:` and `contentView`
    // are NSApplication's, NSArray's and NSWindow's, with these argument and return types. The
    // method is added to the view's own class, which does not implement it (NSView's default
    // returns YES for a view that is not opaque); its signature matches the encoding: BOOL
    // return, self and _cmd.
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let windows: *mut AnyObject = msg_send![app, windows];
        let Some(windows) = windows.as_ref() else {
            return;
        };
        let count: usize = msg_send![windows, count];
        let imp = std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel) -> Bool, Imp>(
            cannot_move_window,
        );
        for index in 0..count {
            let window: *mut AnyObject = msg_send![windows, objectAtIndex: index];
            let view: *mut AnyObject = msg_send![window, contentView];
            let Some(view) = view.as_ref() else {
                continue;
            };
            let class = std::ptr::from_ref::<AnyClass>(view.class()).cast_mut();
            // False when the class already has it, from an earlier window.
            let _ = ffi::class_addMethod(
                class,
                sel!(mouseDownCanMoveWindow),
                imp,
                CANNOT_MOVE_WINDOW_TYPES.as_ptr(),
            );
        }
    }
}

/// Makes files opened from Finder or dropped on the Dock icon reach [`take_opened_files`].
/// AppKit hands them to the app delegate's `application:openURLs:`, which winit's delegate does
/// not implement, so the files that start the app would arrive before anything else could add
/// it. Call on the main thread before the event loop runs: the method is added when the app
/// posts `NSApplicationWillFinishLaunchingNotification`, after winit sets its delegate and
/// before the launch files are delivered.
pub(crate) fn install_open_handler() {
    // SAFETY: called once on the main thread. The observer class adds one method whose
    // signature matches the selector's (void return, self, _cmd and the NSNotification); its
    // only instance is never released, so the notification center's unretained reference to
    // it stays valid.
    unsafe {
        let Some(mut builder) = ClassBuilder::new(c"XuanLaunchObserver", class!(NSObject)) else {
            return;
        };
        builder.add_method(
            sel!(xuanWillFinishLaunching:),
            will_finish_launching as extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject),
        );
        let observer_class = builder.register();
        let observer: *mut AnyObject = msg_send![observer_class, new];
        let name: *mut AnyObject = msg_send![
            class!(NSString),
            stringWithUTF8String: c"NSApplicationWillFinishLaunchingNotification".as_ptr()
        ];
        let center: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
        let _: () = msg_send![
            center,
            addObserver: observer,
            selector: sel!(xuanWillFinishLaunching:),
            name: name,
            object: std::ptr::null_mut::<AnyObject>()
        ];
    }
}

/// The files macOS asked to open since the last call, in order.
pub(super) fn take_opened_files() -> Vec<PathBuf> {
    std::mem::take(&mut *OPENED_FILES.lock().unwrap_or_else(PoisonError::into_inner))
}

/// Turns off macOS's own window tabs. Xuan has document tabs of its own, and AppKit would add
/// Show Tab Bar and Show All Tabs to the View menu and could draw its tab bar over Xuan's.
pub(super) fn disallow_window_tabs() {
    // NSWindowTabbingModeDisallowed.
    const TABBING_DISALLOWED: isize = 2;
    // SAFETY: called on the main thread. `setAllowsAutomaticWindowTabbing:` is NSWindow's class
    // setter (BOOL), and `windows`, `count`, `objectAtIndex:` and `setTabbingMode:`
    // (NSInteger) are NSApplication's, NSArray's and NSWindow's, with these types.
    unsafe {
        let _: () = msg_send![class!(NSWindow), setAllowsAutomaticWindowTabbing: Bool::NO];
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let windows: *mut AnyObject = msg_send![app, windows];
        let Some(windows) = windows.as_ref() else {
            return;
        };
        let count: usize = msg_send![windows, count];
        for index in 0..count {
            let window: *mut AnyObject = msg_send![windows, objectAtIndex: index];
            let _: () = msg_send![window, setTabbingMode: TABBING_DISALLOWED];
        }
    }
}
