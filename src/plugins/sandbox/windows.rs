//! Starting a plugin in an AppContainer without network capabilities, and
//! granting the container access to the folders it needs. See the module
//! documentation of `sandbox` for what is granted and why.
//!
//! `std::process::Command` cannot pass the `SECURITY_CAPABILITIES` that put
//! a process in an AppContainer (`spawn_with_attributes` is unstable), so
//! the plugin is started with `CreateProcessW` here. Its standard input,
//! output and error are anonymous pipes, as with `Stdio::piped()`, and only
//! those three handles are inherited (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`).
//! The process starts suspended: the caller puts it in the plugin's Job
//! Object and then calls [`Contained::resume`].
use std::{
    ffi::{OsStr, c_void},
    fs::File,
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
    },
    path::Path,
    ptr,
};

use anyhow::{Context, Result, bail};
use windows_sys::{
    Win32::{
        Foundation::{
            ERROR_SUCCESS, HANDLE_FLAG_INHERIT, LocalFree, SetHandleInformation, WAIT_OBJECT_0,
            WAIT_TIMEOUT, WIN32_ERROR,
        },
        Security::{
            ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
            Authorization::{
                EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, NO_MULTIPLE_TRUSTEE,
                SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID,
                TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
            },
            CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, EqualSid, FreeSid, GetAce,
            INHERIT_ONLY_ACE,
            Isolation::{CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName},
            OBJECT_INHERIT_ACE, PSECURITY_DESCRIPTOR, PSID, SECURITY_CAPABILITIES,
            SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        },
        Storage::FileSystem::{
            DELETE, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
        },
        System::{
            Pipes::CreatePipe,
            Threading::{
                CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
                DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, INFINITE,
                InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, ResumeThread,
                STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW, TerminateProcess,
                UpdateProcThreadAttribute, WaitForSingleObject,
            },
        },
    },
    core::HRESULT,
};

use super::{
    container_name,
    plan::{command_line, environment, find_program, interpreter_dirs},
};
use crate::plugins::manifest::Manifest;

/// What the container may do in the plugin folder and the interpreter's.
const READ_EXECUTE: u32 = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;
/// What it may do in its data, scratch and work folders.
const MODIFY: u32 = READ_EXECUTE | FILE_GENERIC_WRITE | DELETE;
/// `HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)`.
const ALREADY_EXISTS: HRESULT = 0x8007_00B7_u32 as HRESULT;
/// `ACCESS_ALLOWED_ACE_TYPE`, in a module windows-sys gates behind a
/// feature Xuan does not otherwise need.
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
/// The SID `S-1-15-2-1`, ALL APPLICATION PACKAGES, as stored: revision 1,
/// two sub-authorities, authority 15, then 2 and 1 as little-endian u32s.
const ALL_APPLICATION_PACKAGES: [u8; 16] = [1, 2, 0, 0, 0, 0, 0, 15, 2, 0, 0, 0, 1, 0, 0, 0];

/// `text` as a NUL-terminated UTF-16 string.
fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain([0]).collect()
}

fn check(succeeded: windows_sys::core::BOOL) -> io::Result<()> {
    if succeeded == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn check_error(error: WIN32_ERROR) -> io::Result<()> {
    if error == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(error as i32))
    }
}

/// An AppContainer SID returned by userenv, freed with `FreeSid`.
struct Sid(PSID);

impl Drop for Sid {
    fn drop(&mut self) {
        // SAFETY: the SID came from CreateAppContainerProfile or
        // DeriveAppContainerSidFromAppContainerName, whose documentation
        // says to free it with FreeSid, and is freed once.
        unsafe {
            FreeSid(self.0);
        }
    }
}

/// Memory Windows allocated for us with `LocalAlloc`.
struct Local(*mut c_void);

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from a call documented to return
            // memory freed with LocalFree, and is freed once.
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}

/// The plugin's AppContainer, created with no capabilities the first time.
fn profile(plugin: &str) -> Result<Sid> {
    let name = container_name(plugin);
    let wide_name = wide(OsStr::new(&name));
    let description = wide(OsStr::new(&format!(
        "Xuan plugin {plugin}, without network access"
    )));
    let mut sid: PSID = ptr::null_mut();
    // SAFETY: the strings are NUL-terminated and outlive the call; no
    // capabilities are passed (a null array with count 0); `sid` receives a
    // SID that `Sid` frees.
    let mut result = unsafe {
        CreateAppContainerProfile(
            wide_name.as_ptr(),
            wide_name.as_ptr(),
            description.as_ptr(),
            ptr::null(),
            0,
            &mut sid,
        )
    };
    if result == ALREADY_EXISTS {
        // SAFETY: as above; the existing profile's SID is derived from its name.
        result = unsafe { DeriveAppContainerSidFromAppContainerName(wide_name.as_ptr(), &mut sid) };
    }
    if result < 0 || sid.is_null() {
        return Err(io::Error::from_raw_os_error(result))
            .with_context(|| format!("Cannot create the AppContainer {name}"));
    }
    Ok(Sid(sid))
}

/// Whether `dacl` already lets `sid`, or ALL APPLICATION PACKAGES, do
/// `access` on the object and, with `inherit`, on everything in it.
///
/// # Safety
///
/// `dacl` is null or a valid ACL, and `sid` a valid SID, for the call.
unsafe fn allows(dacl: *const ACL, sid: PSID, access: u32) -> bool {
    if dacl.is_null() {
        // A null DACL grants everyone everything.
        return true;
    }
    // SAFETY: the caller passes a valid ACL.
    let count = unsafe { (*dacl).AceCount };
    for index in 0..u32::from(count) {
        let mut ace: *mut c_void = ptr::null_mut();
        // SAFETY: `index` is below the ACL's count; `ace` receives a pointer
        // into the ACL.
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 {
            return false;
        }
        // SAFETY: every ACE starts with an ACE_HEADER, and ACEs are
        // DWORD-aligned inside the ACL.
        let header = unsafe { ace.cast::<ACE_HEADER>().read() };
        let flags = u32::from(header.AceFlags);
        let inherits = OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE;
        if header.AceType != ACCESS_ALLOWED_ACE_TYPE
            || flags & INHERIT_ONLY_ACE != 0
            || flags & inherits != inherits
            || usize::from(header.AceSize) < size_of::<ACCESS_ALLOWED_ACE>()
        {
            continue;
        }
        let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
        // SAFETY: an ACCESS_ALLOWED_ACE, of at least its size as checked,
        // with its SID in the rest of `AceSize` from `SidStart` on. Raw
        // pointers keep the provenance of the whole ACL.
        let (mask, ace_sid): (u32, PSID) =
            unsafe { ((*allowed).Mask, (&raw mut (*allowed).SidStart).cast()) };
        if mask & access != access {
            continue;
        }
        let sid_bytes =
            usize::from(header.AceSize) - std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart);
        // SAFETY: the ACE's last `sid_bytes` bytes hold its SID; 16 are read.
        let all_packages = sid_bytes >= ALL_APPLICATION_PACKAGES.len()
            && unsafe { ace_sid.cast::<[u8; 16]>().read_unaligned() } == ALL_APPLICATION_PACKAGES;
        // SAFETY: both are valid SIDs.
        if all_packages || unsafe { EqualSid(ace_sid, sid) } != 0 {
            return true;
        }
    }
    false
}

/// Let `sid` do `access` in the folder `dir` and everything in it, now and
/// created later, unless it already may. Inheritable entries are pushed
/// down to what the folder holds, which takes a moment for a large folder,
/// once.
fn grant(dir: &Path, sid: &Sid, access: u32) -> io::Result<()> {
    let path = wide(dir.as_os_str());
    let mut dacl: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `path` is NUL-terminated; only the DACL is asked for, and the
    // descriptor holding it is freed by `Local` after its last use below.
    let error = unsafe {
        GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    check_error(error)?;
    let _descriptor = Local(descriptor);
    // SAFETY: `dacl` points into the descriptor, alive until the end.
    if unsafe { allows(dacl, sid.0, access) } {
        return Ok(());
    }
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: access,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
            // With TRUSTEE_IS_SID the "name" is the SID pointer.
            ptstrName: sid.0.cast(),
        },
    };
    let mut new: *mut ACL = ptr::null_mut();
    // SAFETY: one entry, whose SID is alive; the old ACL is alive; the new
    // ACL is freed by `Local`.
    let error = unsafe { SetEntriesInAclW(1, &entry, dacl, &mut new) };
    check_error(error)?;
    let new = Local(new.cast());
    // SAFETY: `path` is NUL-terminated and the new ACL is alive.
    let error = unsafe {
        SetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            new.0.cast(),
            ptr::null(),
        )
    };
    check_error(error)
}

/// Let the plugin's container read, write and delete in `dir`.
pub fn share(plugin: &str, dir: &Path) -> Result<()> {
    let sid = profile(plugin)?;
    grant(dir, &sid, MODIFY)
        .with_context(|| format!("Cannot let the plugin's AppContainer use {}", dir.display()))
}

/// An anonymous pipe, as (read end, write end). Neither end is inheritable.
fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let (mut read, mut write) = (ptr::null_mut(), ptr::null_mut());
    // SAFETY: both pointers receive new handles, owned right after.
    check(unsafe { CreatePipe(&mut read, &mut write, ptr::null(), 0) })?;
    // SAFETY: the call succeeded, so both are new handles nothing else owns.
    Ok(unsafe {
        (
            OwnedHandle::from_raw_handle(read),
            OwnedHandle::from_raw_handle(write),
        )
    })
}

fn inheritable(handle: &OwnedHandle) -> io::Result<()> {
    // SAFETY: a valid handle; only its inherit flag changes.
    check(unsafe {
        SetHandleInformation(
            handle.as_raw_handle(),
            HANDLE_FLAG_INHERIT,
            HANDLE_FLAG_INHERIT,
        )
    })
}

/// A `PROC_THREAD_ATTRIBUTE_LIST` with room for two attributes. The values
/// it points to must outlive it.
struct Attributes {
    /// `usize` elements keep the opaque list pointer-aligned.
    buffer: Vec<usize>,
}

impl Attributes {
    const COUNT: u32 = 2;

    fn new() -> io::Result<Self> {
        let mut size = 0;
        // SAFETY: with a null list the call only reports the size needed,
        // and fails with ERROR_INSUFFICIENT_BUFFER, which is expected.
        unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), Self::COUNT, 0, &mut size) };
        let mut buffer = vec![0_usize; size.div_ceil(size_of::<usize>())];
        // SAFETY: the buffer has at least `size` bytes, as asked for.
        check(unsafe {
            InitializeProcThreadAttributeList(buffer.as_mut_ptr().cast(), Self::COUNT, 0, &mut size)
        })?;
        Ok(Self { buffer })
    }

    fn as_ptr(&mut self) -> *mut c_void {
        self.buffer.as_mut_ptr().cast()
    }

    /// Set `attribute` to the `size` bytes at `value`.
    ///
    /// # Safety
    ///
    /// `value` points to `size` readable bytes that outlive the list.
    unsafe fn set(&mut self, attribute: u32, value: *const c_void, size: usize) -> io::Result<()> {
        // SAFETY: an initialized list; the caller vouches for the value.
        check(unsafe {
            UpdateProcThreadAttribute(
                self.as_ptr(),
                0,
                attribute as usize,
                value,
                size,
                ptr::null_mut(),
                ptr::null(),
            )
        })
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: the list was initialized in `new` and is deleted once.
        unsafe { DeleteProcThreadAttributeList(self.as_ptr()) }
    }
}

/// A plugin process in its AppContainer, suspended until [`Self::resume`].
pub struct Contained {
    process: OwnedHandle,
    /// The main thread while it is suspended.
    thread: Option<OwnedHandle>,
    pub stdin: Option<File>,
    pub stdout: Option<File>,
    pub stderr: Option<File>,
    /// What the log should say about the start, such as a folder the
    /// container could not be given access to.
    pub notes: Vec<String>,
}

impl Contained {
    pub fn handle(&self) -> RawHandle {
        self.process.as_raw_handle()
    }

    /// Let the process run.
    pub fn resume(&mut self) -> io::Result<()> {
        if let Some(thread) = self.thread.take() {
            // SAFETY: a valid thread handle of the suspended process.
            if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
                self.thread = Some(thread);
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    /// Whether the process has exited.
    pub fn exited(&mut self) -> io::Result<bool> {
        // SAFETY: a valid process handle; a zero timeout only polls.
        match unsafe { WaitForSingleObject(self.handle(), 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(io::Error::last_os_error()),
        }
    }

    pub fn kill(&mut self) -> io::Result<()> {
        // SAFETY: a valid process handle.
        check(unsafe { TerminateProcess(self.handle(), 1) })
    }

    pub fn wait(&mut self) -> io::Result<()> {
        // SAFETY: a valid process handle.
        match unsafe { WaitForSingleObject(self.handle(), INFINITE) } {
            WAIT_OBJECT_0 => Ok(()),
            _ => Err(io::Error::last_os_error()),
        }
    }
}

impl Drop for Contained {
    fn drop(&mut self) {
        // Never resumed: it never ran, so it must not start later either.
        if self.thread.is_some() {
            let _ = self.kill();
        }
    }
}

/// Start `program` with `args` for the plugin of `manifest`, in its folder
/// and with Xuan's environment plus `env`, inside the plugin's AppContainer,
/// suspended. `program` is a path or a name looked up on `PATH`.
pub fn spawn(
    manifest: &Manifest,
    program: &Path,
    args: &[String],
    env: &[(String, String)],
) -> Result<Contained> {
    let environment = environment(std::env::vars_os(), env);
    let path_variable = (environment.iter())
        .find(|(name, _)| name.eq_ignore_ascii_case("PATH"))
        .map(|(_, value)| value.as_os_str());
    let program = find_program(program, path_variable)
        .with_context(|| format!("Cannot find `{}`", program.display()))?;
    let extension = program.extension().map(|e| e.to_ascii_lowercase());
    if extension.is_some_and(|e| e == "bat" || e == "cmd") {
        bail!(
            "`{}` is a batch file, which cannot run in an AppContainer; run a program instead",
            program.display()
        );
    }
    let sid = profile(&manifest.plugin.id)?;
    grant(&manifest.dir, &sid, READ_EXECUTE).with_context(|| {
        format!(
            "Cannot let the plugin's AppContainer read {}",
            manifest.dir.display()
        )
    })?;
    let mut notes = Vec::new();
    for dir in interpreter_dirs(&program, &manifest.dir) {
        if let Err(error) = grant(&dir, &sid, READ_EXECUTE) {
            notes.push(format!(
                "Could not let the AppContainer read {} ({error}). Unless it can already, {} cannot run; install it for all users, for example under C:\\Program Files",
                dir.display(),
                program.display()
            ));
        }
    }
    let program_text = (program.to_str())
        .with_context(|| format!("`{}` is not valid Unicode", program.display()))?;
    let mut command_line: Vec<u16> = command_line(program_text, args)?
        .encode_utf16()
        .chain([0])
        .collect();
    let mut block: Vec<u16> = Vec::new();
    for (name, value) in &environment {
        block.extend(name.encode_wide());
        block.push(u16::from(b'='));
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    let application = wide(program.as_os_str());
    let directory = wide(manifest.dir.as_os_str());

    let (stdin, stdin_parent) = pipe()?;
    let (stdout_parent, stdout) = pipe()?;
    let (stderr_parent, stderr) = pipe()?;
    for child_end in [&stdin, &stdout, &stderr] {
        inheritable(child_end)?;
    }
    let inherited = [
        stdin.as_raw_handle(),
        stdout.as_raw_handle(),
        stderr.as_raw_handle(),
    ];
    let capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: sid.0,
        Capabilities: ptr::null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    };
    let mut attributes = Attributes::new()?;
    // SAFETY: both values are locals that outlive `attributes`, which is
    // dropped at the end of this function, after CreateProcessW.
    unsafe {
        attributes.set(
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
            (&raw const capabilities).cast(),
            size_of::<SECURITY_CAPABILITIES>(),
        )?;
        attributes.set(
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            inherited.as_ptr().cast(),
            size_of_val(&inherited),
        )?;
    }
    let mut startup = STARTUPINFOEXW {
        StartupInfo: STARTUPINFOW {
            cb: size_of::<STARTUPINFOEXW>() as u32,
            dwFlags: STARTF_USESTDHANDLES,
            hStdInput: stdin.as_raw_handle(),
            hStdOutput: stdout.as_raw_handle(),
            hStdError: stderr.as_raw_handle(),
            ..STARTUPINFOW::default()
        },
        lpAttributeList: attributes.as_ptr(),
    };
    let mut information = PROCESS_INFORMATION::default();
    // SAFETY: every string is NUL-terminated and alive, the command line is
    // a mutable buffer as required, the environment block is UTF-16 (as
    // CREATE_UNICODE_ENVIRONMENT says) and ends with two NULs, the startup
    // information is a STARTUPINFOEXW (EXTENDED_STARTUPINFO_PRESENT) whose
    // attribute list and the values it points to are alive, and only the
    // three handles in the list are inherited.
    check(unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT
                | CREATE_UNICODE_ENVIRONMENT
                | CREATE_SUSPENDED
                | CREATE_NO_WINDOW,
            block.as_ptr().cast(),
            directory.as_ptr(),
            (&raw mut startup).cast::<STARTUPINFOW>(),
            &mut information,
        )
    })
    .with_context(|| format!("Cannot start `{}` in an AppContainer", program.display()))?;
    // SAFETY: CreateProcessW succeeded, so both are new handles we own.
    let (process, thread) = unsafe {
        (
            OwnedHandle::from_raw_handle(information.hProcess),
            OwnedHandle::from_raw_handle(information.hThread),
        )
    };
    // The child ends close here: the child holds its own copies.
    drop((stdin, stdout, stderr));
    Ok(Contained {
        process,
        thread: Some(thread),
        stdin: Some(File::from(stdin_parent)),
        stdout: Some(File::from(stdout_parent)),
        stderr: Some(File::from(stderr_parent)),
        notes,
    })
}
