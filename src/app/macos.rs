//! macOS integration that winit does not provide.
//!
//! winit's default app menu quits with `terminate:`, and its app delegate does not implement
//! `applicationShouldTerminate:`, so ⌘Q, Quit in the Dock and logging out would end the process
//! without the unsaved-changes prompt. [`install_quit_handler`] adds that method to winit's
//! delegate: macOS quits at once when nothing is unsaved, and otherwise the quit is cancelled and
//! the app runs its own quit flow, which asks first.

use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};

use objc2::{
    class, ffi, msg_send,
    runtime::{AnyClass, AnyObject, Imp, Sel},
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
