//! Exercise the actual wire protocol and worker using an in-process compositor.
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

use super::*;
use wayland_server::protocol::{
    wl_compositor, wl_data_device as server_device, wl_data_device_manager as server_manager,
    wl_data_offer as server_offer, wl_seat as server_seat, wl_surface,
};
use wayland_server::{Client, DataInit, Display, DisplayHandle, GlobalDispatch, New, Resource};

const LIST: &str = "file:///tmp/a%20photo.png\r\n# a comment\r\nfile:///tmp/second.jpg\r\nhttps://example.com/remote.png\r\n";

enum ServerEvent {
    Surface(wl_surface::WlSurface),
    /// The client's data device and two offers to drag onto it.
    Device(server_device::WlDataDevice, [server_offer::WlDataOffer; 2]),
    Accepted(Option<String>),
    Finished,
}

struct ServerState {
    events: Sender<ServerEvent>,
}

macro_rules! global {
    ($interface:ty) => {
        impl GlobalDispatch<$interface, ()> for ServerState {
            fn bind(
                _: &mut Self,
                _: &DisplayHandle,
                _: &Client,
                resource: New<$interface>,
                _: &(),
                init: &mut DataInit<'_, Self>,
            ) {
                init.init(resource, ());
            }
        }
    };
}

global!(wl_compositor::WlCompositor);
global!(server_seat::WlSeat);
global!(server_manager::WlDataDeviceManager);

impl wayland_server::Dispatch<wl_compositor::WlCompositor, ()> for ServerState {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &wl_compositor::WlCompositor,
        request: wl_compositor::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_compositor::Request::CreateSurface { id } = request {
            state
                .events
                .send(ServerEvent::Surface(init.init(id, ())))
                .unwrap();
        }
    }
}

impl wayland_server::Dispatch<server_manager::WlDataDeviceManager, ()> for ServerState {
    fn request(
        state: &mut Self,
        client: &Client,
        _: &server_manager::WlDataDeviceManager,
        request: server_manager::Request,
        _: &(),
        display: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let server_manager::Request::GetDataDevice { id, .. } = request {
            let device = init.init(id, ());
            let offer = || {
                client
                    .create_resource::<server_offer::WlDataOffer, (), Self>(display, 3, ())
                    .unwrap()
            };
            state
                .events
                .send(ServerEvent::Device(device, [offer(), offer()]))
                .unwrap();
        }
    }
}

impl wayland_server::Dispatch<server_offer::WlDataOffer, ()> for ServerState {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &server_offer::WlDataOffer,
        request: server_offer::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            server_offer::Request::Accept { mime_type, .. } => {
                state.events.send(ServerEvent::Accepted(mime_type)).unwrap();
            }
            server_offer::Request::Receive { mime_type, fd } => {
                assert_eq!(mime_type, URI_LIST);
                // Dropping the file closes the source's end, which ends the read.
                std::fs::File::from(fd).write_all(LIST.as_bytes()).unwrap();
            }
            server_offer::Request::Finish => state.events.send(ServerEvent::Finished).unwrap(),
            _ => {}
        }
    }
}

macro_rules! ignore_requests {
    ($interface:ty) => {
        impl wayland_server::Dispatch<$interface, ()> for ServerState {
            fn request(
                _: &mut Self,
                _: &Client,
                _: &$interface,
                _: <$interface as Resource>::Request,
                _: &(),
                _: &DisplayHandle,
                _: &mut DataInit<'_, Self>,
            ) {
            }
        }
    };
}

ignore_requests!(server_seat::WlSeat);
ignore_requests!(wl_surface::WlSurface);
ignore_requests!(server_device::WlDataDevice);

struct ClientState;
impl
    Dispatch<
        wayland_client::protocol::wl_registry::WlRegistry,
        wayland_client::globals::GlobalListContents,
    > for ClientState
{
    fn event(
        _: &mut Self,
        _: &wayland_client::protocol::wl_registry::WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &wayland_client::globals::GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
delegate_noop!(ClientState: ignore wayland_client::protocol::wl_compositor::WlCompositor);
delegate_noop!(ClientState: ignore wayland_client::protocol::wl_surface::WlSurface);

#[test]
fn uri_lists_give_local_files_and_skip_comments_and_remote_urls() {
    assert_eq!(
        file_paths(LIST),
        [
            PathBuf::from("/tmp/a photo.png"),
            PathBuf::from("/tmp/second.jpg")
        ]
    );
    assert!(file_paths("not a uri\n\n").is_empty());
}

#[test]
fn files_dragged_over_the_window_hover_and_drop_and_other_drags_are_refused() {
    let (client_socket, server_socket) = UnixStream::pair().unwrap();
    let mut display = Display::<ServerState>::new().unwrap();
    let mut display_handle = display.handle();
    display_handle
        .insert_client(server_socket, Arc::new(()))
        .unwrap();
    display_handle.create_global::<ServerState, wl_compositor::WlCompositor, _>(1, ());
    display_handle.create_global::<ServerState, server_seat::WlSeat, _>(7, ());
    display_handle.create_global::<ServerState, server_manager::WlDataDeviceManager, _>(3, ());
    let (events_tx, events_rx) = mpsc::channel();
    let (server_stop, server_stopped) = mpsc::channel();
    let server = thread::spawn(move || {
        let mut state = ServerState { events: events_tx };
        loop {
            display.dispatch_clients(&mut state).unwrap();
            display.flush_clients().unwrap();
            if server_stopped
                .recv_timeout(Duration::from_millis(1))
                .is_ok()
            {
                break;
            }
        }
    });

    let connection = Connection::from_socket(client_socket).unwrap();
    let (globals, mut queue) =
        wayland_client::globals::registry_queue_init::<ClientState>(&connection).unwrap();
    let compositor: wayland_client::protocol::wl_compositor::WlCompositor =
        globals.bind(&queue.handle(), 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&queue.handle(), ());
    let other_surface = compositor.create_surface(&queue.handle(), ());
    queue.roundtrip(&mut ClientState).unwrap();
    let timeout = Duration::from_secs(3);
    let ServerEvent::Surface(server_surface) = events_rx.recv_timeout(timeout).unwrap() else {
        panic!()
    };
    let ServerEvent::Surface(server_other_surface) = events_rx.recv_timeout(timeout).unwrap()
    else {
        panic!()
    };

    // Match eframe: another library owns the live display and surface.
    // SAFETY: connection/surface outlive the joined worker below.
    let backend = unsafe {
        wayland_backend::client::Backend::from_foreign_display(
            connection.backend().display_ptr().cast(),
        )
    };
    let (drops, stop, worker) = start(
        Connection::from_backend(backend),
        surface.id().as_ptr() as usize,
        egui::Context::default(),
    )
    .unwrap();
    let ServerEvent::Device(device, [first, second]) = events_rx.recv_timeout(timeout).unwrap()
    else {
        panic!()
    };

    // A drag over another window on the same display is refused and not reported.
    device.data_offer(&first);
    first.offer(URI_LIST.into());
    device.enter(1, &server_other_surface, 10.0, 10.0, Some(&first));
    display_handle.flush_clients().unwrap();
    assert!(matches!(
        events_rx.recv_timeout(timeout).unwrap(),
        ServerEvent::Accepted(None)
    ));
    assert!(drops.recv_timeout(Duration::from_millis(30)).is_err());
    device.leave();

    // Files over the window hover, and dropping them gives their paths.
    device.data_offer(&second);
    second.offer("text/plain".into());
    second.offer(URI_LIST.into());
    device.enter(2, &server_surface, 10.0, 10.0, Some(&second));
    display_handle.flush_clients().unwrap();
    assert!(matches!(
        events_rx.recv_timeout(timeout).unwrap(),
        ServerEvent::Accepted(Some(kind)) if kind == URI_LIST
    ));
    assert_eq!(drops.recv_timeout(timeout).unwrap(), DropEvent::Hover);
    device.motion(3, 20.0, 20.0);
    device.drop();
    display_handle.flush_clients().unwrap();
    assert_eq!(
        drops.recv_timeout(timeout).unwrap(),
        DropEvent::Drop(vec![
            PathBuf::from("/tmp/a photo.png"),
            PathBuf::from("/tmp/second.jpg")
        ])
    );
    assert!(matches!(
        events_rx.recv_timeout(timeout).unwrap(),
        ServerEvent::Finished
    ));

    stop.send(()).unwrap();
    let (joined_tx, joined_rx) = mpsc::channel();
    let joiner = thread::spawn(move || {
        worker.join().unwrap();
        joined_tx.send(()).unwrap();
    });
    joined_rx
        .recv_timeout(timeout)
        .expect("the worker must stop without waiting for a new drag");
    joiner.join().unwrap();
    other_surface.destroy();
    surface.destroy();
    drop(connection);
    server_stop.send(()).unwrap();
    server.join().unwrap();
}

#[test]
fn hover_and_drops_reach_egui_as_the_files_winit_reports_elsewhere() {
    let (sender, receiver) = mpsc::channel();
    let (stop, _stopped) = channel::channel();
    let mut drops = FileDrops {
        receiver,
        hovering: false,
        stop,
        thread: None,
    };
    sender.send(DropEvent::Hover).unwrap();
    let mut input = egui::RawInput::default();
    drops.update(&mut input);
    assert_eq!(input.hovered_files.len(), 1);
    // Hovering lasts until the drag leaves or drops, not one frame.
    let mut input = egui::RawInput::default();
    drops.update(&mut input);
    assert_eq!(input.hovered_files.len(), 1);

    sender
        .send(DropEvent::Drop(vec![PathBuf::from("/tmp/photo.png")]))
        .unwrap();
    let mut input = egui::RawInput::default();
    drops.update(&mut input);
    assert!(input.hovered_files.is_empty());
    assert_eq!(
        input.dropped_files[0].path.as_deref(),
        Some(std::path::Path::new("/tmp/photo.png"))
    );

    sender.send(DropEvent::Hover).unwrap();
    sender.send(DropEvent::Leave).unwrap();
    let mut input = egui::RawInput::default();
    drops.update(&mut input);
    assert!(input.hovered_files.is_empty() && input.dropped_files.is_empty());
}
