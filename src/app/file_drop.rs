//! Files dragged onto the window on Wayland.
//!
//! winit delivers dropped files on X11, Windows and macOS but not on Wayland,
//! so there Xuan listens for drags itself: a data device on the window's own
//! display, on a queue of its own, as the tablet bridge does for pens. What it
//! hears goes into egui's raw input as the hovered and dropped files winit
//! reports elsewhere, so the rest of the editor does not tell them apart.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result};
use calloop::{EventLoop, channel};
use calloop_wayland_source::WaylandSource;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_client::backend::ObjectId;
use wayland_client::protocol::{
    wl_data_device, wl_data_device_manager, wl_data_offer, wl_registry, wl_seat,
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, delegate_noop, event_created_child,
};

/// The type file managers offer dragged files as: one `file://` URI a line.
const URI_LIST: &str = "text/uri-list";
/// More than any real drag's list of files; a source cannot make Xuan read without end.
const MAX_LIST_BYTES: u64 = 1024 * 1024;

#[derive(Debug, PartialEq)]
enum DropEvent {
    /// Files are being dragged over the window.
    Hover,
    /// The drag left the window or was cancelled.
    Leave,
    /// The files were dropped.
    Drop(Vec<PathBuf>),
}

pub(super) struct FileDrops {
    receiver: Receiver<DropEvent>,
    hovering: bool,
    stop: channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for FileDrops {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl FileDrops {
    /// Listens for drags on a Wayland window; `None` anywhere winit reports them itself.
    pub(super) fn new(cc: &eframe::CreationContext<'_>) -> Option<Self> {
        let display = cc.display_handle().ok()?.as_raw();
        let window = cc.window_handle().ok()?.as_raw();
        let (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(window)) =
            (display, window)
        else {
            return None;
        };
        // SAFETY: eframe keeps its window and display alive while EditorApp is
        // alive. on_exit drops this listener, and dropping it joins the thread
        // before the borrowed display is released, including on unwind.
        let backend = unsafe {
            wayland_backend::client::Backend::from_foreign_display(display.display.as_ptr().cast())
        };
        match start(
            Connection::from_backend(backend),
            window.surface.as_ptr() as usize,
            cc.egui_ctx.clone(),
        ) {
            Ok((receiver, stop, thread)) => Some(Self {
                receiver,
                hovering: false,
                stop,
                thread: Some(thread),
            }),
            Err(error) => {
                eprintln!("Could not listen for dropped files: {error}");
                None
            }
        }
    }

    /// Adds what was dragged or dropped since the last frame to egui's input.
    pub(super) fn update(&mut self, input: &mut egui::RawInput) {
        loop {
            match self.receiver.try_recv() {
                Ok(DropEvent::Hover) => self.hovering = true,
                Ok(DropEvent::Leave) => self.hovering = false,
                Ok(DropEvent::Drop(paths)) => {
                    self.hovering = false;
                    input
                        .dropped_files
                        .extend(paths.into_iter().map(|path| egui::DroppedFile {
                            path: Some(path),
                            ..Default::default()
                        }));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.hovering = false;
                    break;
                }
            }
        }
        if self.hovering && input.hovered_files.is_empty() {
            // The file names are not known until the drop: only that files hover.
            input.hovered_files.push(egui::HoveredFile {
                mime: URI_LIST.into(),
                ..Default::default()
            });
        }
    }
}

type Started = (Receiver<DropEvent>, channel::Sender<()>, JoinHandle<()>);

fn start(connection: Connection, surface: usize, context: egui::Context) -> Result<Started> {
    let (sender, receiver) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let thread = thread::Builder::new()
        .name("file-drops".into())
        .spawn(move || {
            let setup = || -> Result<_> {
                let event_loop = EventLoop::<DropState>::try_new()?;
                let signal = event_loop.get_signal();
                let (stop, stopped) = channel::channel();
                event_loop
                    .handle()
                    .insert_source(stopped, move |_, _, _| signal.stop())
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
                let queue = connection.new_event_queue();
                let handle = queue.handle();
                WaylandSource::new(connection.clone(), queue)
                    .insert(event_loop.handle())
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
                Ok((event_loop, handle, stop))
            };
            let (mut event_loop, handle, stop) = match setup() {
                Ok(setup) => setup,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            connection.display().get_registry(&handle, ());
            let mut state = DropState {
                surface,
                sender,
                context: context.clone(),
                connection: connection.clone(),
                manager: None,
                seats: HashMap::new(),
                offers: HashMap::new(),
                drag: None,
            };
            if ready_tx.send(Ok(stop)).is_ok()
                && let Err(error) = event_loop.run(None, &mut state, |_| {})
            {
                eprintln!("Listening for dropped files stopped: {error}");
            }
            state.end_drag();
            for (_, (offer, _)) in state.offers.drain() {
                offer.destroy();
            }
            for (_, seat) in state.seats.drain() {
                seat.destroy();
            }
            state.manager = None;
            let _ = connection.flush();
            drop(state);
            context.request_repaint();
        })?;
    match ready_rx
        .recv()
        .context("the file drop listener exited during setup")?
    {
        Ok(stop) => Ok((receiver, stop, thread)),
        Err(error) => {
            let _ = thread.join();
            Err(error)
        }
    }
}

struct Seat {
    proxy: wl_seat::WlSeat,
    device: Option<wl_data_device::WlDataDevice>,
}

impl Seat {
    fn destroy(self) {
        if let Some(device) = self.device
            && device.version() >= 2
        {
            device.release();
        }
        if self.proxy.version() >= 5 {
            self.proxy.release();
        }
    }
}

struct DropState {
    surface: usize,
    sender: Sender<DropEvent>,
    context: egui::Context,
    connection: Connection,
    manager: Option<(u32, wl_data_device_manager::WlDataDeviceManager)>,
    seats: HashMap<u32, Seat>,
    /// Every offer announced and not yet used up, with the types it comes in.
    offers: HashMap<ObjectId, (wl_data_offer::WlDataOffer, Vec<String>)>,
    /// The offer of files being dragged over the window.
    drag: Option<wl_data_offer::WlDataOffer>,
}

impl DropState {
    fn bind_seats(&mut self, handle: &QueueHandle<Self>) {
        if let Some((_, manager)) = &self.manager {
            for seat in self.seats.values_mut() {
                if seat.device.is_none() {
                    seat.device = Some(manager.get_data_device(&seat.proxy, handle, ()));
                }
            }
        }
    }

    fn send(&self, event: DropEvent) {
        let _ = self.sender.send(event);
        self.context.request_repaint();
    }

    fn forget(&mut self, offer: &wl_data_offer::WlDataOffer) {
        if let Some((offer, _)) = self.offers.remove(&offer.id()) {
            offer.destroy();
        }
    }

    /// Ends a drag over the window that did not drop.
    fn end_drag(&mut self) {
        if let Some(offer) = self.drag.take() {
            self.forget(&offer);
            self.send(DropEvent::Leave);
        }
    }

    /// Asks the source for the dropped files and reads them off the event loop:
    /// the source writes when it gets to it, and other events must not wait.
    fn take_drop(&mut self) {
        let Some(offer) = self.drag.take() else {
            return;
        };
        self.offers.remove(&offer.id());
        let pipe = std::io::pipe().and_then(|(reader, writer)| {
            offer.receive(URI_LIST.into(), std::os::fd::AsFd::as_fd(&writer));
            // The source must hold the only open write end, or the read never ends.
            self.connection
                .flush()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            Ok(reader)
        });
        let reader = match pipe {
            Ok(reader) => reader,
            Err(error) => {
                eprintln!("Could not read the dropped files: {error}");
                offer.destroy();
                self.send(DropEvent::Leave);
                return;
            }
        };
        let (sender, context, connection) = (
            self.sender.clone(),
            self.context.clone(),
            self.connection.clone(),
        );
        let read = move || {
            let mut list = Vec::new();
            let read = reader.take(MAX_LIST_BYTES).read_to_end(&mut list);
            if offer.version() >= 3 {
                offer.finish();
            }
            offer.destroy();
            let _ = connection.flush();
            let paths = match read {
                Ok(_) => file_paths(&String::from_utf8_lossy(&list)),
                Err(error) => {
                    eprintln!("Could not read the dropped files: {error}");
                    Vec::new()
                }
            };
            let _ = sender.send(if paths.is_empty() {
                DropEvent::Leave
            } else {
                DropEvent::Drop(paths)
            });
            context.request_repaint();
        };
        if let Err(error) = thread::Builder::new()
            .name("file-drop-read".into())
            .spawn(read)
        {
            eprintln!("Could not read the dropped files: {error}");
            self.send(DropEvent::Leave);
        }
    }
}

/// The local files in a `text/uri-list`: one URI a line, `#` lines are comments.
fn file_paths(list: &str) -> Vec<PathBuf> {
    list.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| url::Url::parse(line).ok())
        .filter(|url| url.scheme() == "file")
        .filter_map(|url| url.to_file_path().ok())
        .collect()
}

impl Dispatch<wl_registry::WlRegistry, ()> for DropState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        handle: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                match interface.as_str() {
                    "wl_data_device_manager" if state.manager.is_none() => {
                        state.manager =
                            Some((name, registry.bind(name, version.min(3), handle, ())));
                    }
                    "wl_seat" => {
                        state.seats.insert(
                            name,
                            Seat {
                                proxy: registry.bind(name, version.min(7), handle, ()),
                                device: None,
                            },
                        );
                    }
                    _ => {}
                }
                state.bind_seats(handle);
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(seat) = state.seats.remove(&name) {
                    state.end_drag();
                    seat.destroy();
                }
                if state.manager.as_ref().is_some_and(|(id, _)| *id == name) {
                    state.end_drag();
                    for seat in state.seats.values_mut() {
                        if let Some(device) = seat.device.take()
                            && device.version() >= 2
                        {
                            device.release();
                        }
                    }
                    state.manager = None;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for DropState {
    fn event(
        state: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::DataOffer { id } => {
                state.offers.insert(id.id(), (id, Vec::new()));
            }
            wl_data_device::Event::Enter {
                serial,
                surface,
                id,
                ..
            } => {
                state.end_drag();
                let Some(offer) = id else {
                    return;
                };
                let files = surface.id().as_ptr() as usize == state.surface
                    && state
                        .offers
                        .get(&offer.id())
                        .is_some_and(|(_, types)| types.iter().any(|t| t == URI_LIST));
                if !files {
                    // Another window's drag, or one that is not files: refuse it.
                    offer.accept(serial, None);
                    state.forget(&offer);
                    return;
                }
                offer.accept(serial, Some(URI_LIST.into()));
                if offer.version() >= 3 {
                    let copy = wl_data_device_manager::DndAction::Copy;
                    offer.set_actions(copy, copy);
                }
                state.drag = Some(offer);
                state.send(DropEvent::Hover);
            }
            wl_data_device::Event::Leave => state.end_drag(),
            wl_data_device::Event::Drop => state.take_drop(),
            wl_data_device::Event::Selection { id: Some(offer) } => {
                // The clipboard's offer: winit's clipboard has its own; this one is not used.
                state.forget(&offer);
            }
            _ => {}
        }
    }

    event_created_child!(DropState, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for DropState {
    fn event(
        state: &mut Self,
        offer: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event
            && let Some((_, types)) = state.offers.get_mut(&offer.id())
        {
            types.push(mime_type);
        }
    }
}

delegate_noop!(DropState: ignore wl_seat::WlSeat);
delegate_noop!(DropState: ignore wl_data_device_manager::WlDataDeviceManager);

#[cfg(test)]
#[path = "file_drop_tests.rs"]
mod tests;
